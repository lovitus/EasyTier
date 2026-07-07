use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    Arc,
};

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::{
    select,
    sync::{watch, Notify, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tracing::Instrument;

use crate::{
    error::Error,
    ffi_safe::{Kcp, KcpConfig},
    packet_def::KcpPacket,
    state::{KcpConnectionFSM, PacketHeaderFlagManipulator},
};

pub type Sender<T> = tokio::sync::mpsc::Sender<T>;
pub type Receiver<T> = tokio::sync::mpsc::Receiver<T>;

pub type KcpPakcetSender = Sender<KcpPacket>;
pub type KcpPacketReceiver = Receiver<KcpPacket>;

pub type KcpStreamSender = Sender<BytesMut>;
pub type KcpStreamReceiver = Receiver<BytesMut>;

const DEFAULT_MAX_CONNECTIONS: usize = 4096;
const DEFAULT_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const DEFAULT_CLOSE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KcpCloseStatus {
    Open,
    Graceful,
    Forced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId {
    conv: u32,
    src_session_id: u32,
    dst_session_id: u32,
}

impl From<&KcpPacket> for ConnId {
    fn from(packet: &KcpPacket) -> Self {
        Self {
            conv: packet.header().conv(),
            src_session_id: packet.header().src_session_id(),
            dst_session_id: packet.header().dst_session_id(),
        }
    }
}

impl ConnId {
    fn fill_packet_header(&self, packet: &mut KcpPacket) {
        packet
            .mut_header()
            .set_conv(self.conv)
            .set_src_session_id(self.src_session_id)
            .set_dst_session_id(self.dst_session_id);
    }
}

struct KcpConnectionInner {
    update_notifier: Notify,
    recv_notifier: Notify,
    send_notifier: Notify,

    has_new_input: AtomicBool,
    waiting_new_send_window: AtomicBool,
}

struct KcpConnection {
    conn_id: ConnId,
    kcp: Arc<Mutex<Box<Kcp>>>,

    inner: Arc<KcpConnectionInner>,

    send_sender: Option<Sender<BytesMut>>,
    send_receiver: Option<Receiver<BytesMut>>,

    recv_sender: Option<Sender<BytesMut>>,
    recv_receiver: Option<Receiver<BytesMut>>,

    send_close_notifier: Arc<Notify>,
    send_drained: watch::Sender<bool>,
    close_status: watch::Sender<KcpCloseStatus>,
    recv_closed: Arc<AtomicBool>,

    tasks: JoinSet<()>,
}

impl KcpConnection {
    fn new_with_config(conn_id: ConnId, config: KcpConfig) -> Result<Self, Error> {
        let kcp = Kcp::new(config)?;

        let (send_sender, send_receiver) = tokio::sync::mpsc::channel(128);
        let (recv_sender, recv_receiver) = tokio::sync::mpsc::channel(128);

        Ok(Self {
            conn_id,
            kcp: Arc::new(Mutex::new(kcp)),

            inner: Arc::new(KcpConnectionInner {
                update_notifier: Notify::new(),
                recv_notifier: Notify::new(),
                send_notifier: Notify::new(),

                has_new_input: AtomicBool::new(false),
                waiting_new_send_window: AtomicBool::new(false),
            }),

            send_sender: Some(send_sender),
            send_receiver: Some(send_receiver),

            recv_sender: Some(recv_sender),
            recv_receiver: Some(recv_receiver),

            send_close_notifier: Arc::new(Notify::new()),
            send_drained: watch::channel(false).0,
            close_status: watch::channel(KcpCloseStatus::Open).0,
            recv_closed: Arc::new(AtomicBool::new(false)),

            tasks: JoinSet::new(),
        })
    }

    pub fn run(&mut self, output_sender: KcpPakcetSender) {
        let conn_id = self.conn_id;
        self.kcp
            .lock()
            .set_output_cb(Box::new(move |conv, data: BytesMut| {
                let mut kcp_packet = KcpPacket::new_with_payload(&data);
                conn_id.fill_packet_header(&mut kcp_packet);
                kcp_packet.mut_header().set_data(true).set_ack(true);
                tracing::trace!(?conv, "sending output data: {:?}", kcp_packet);
                if let Err(e) = output_sender.try_send(kcp_packet) {
                    tracing::debug!(?e, ?conn_id, "send output data failed");
                }
                Ok(())
            }));

        // kcp updater
        let inner = self.inner.clone();
        let kcp = self.kcp.clone();
        let recv_closed = self.recv_closed.clone();
        self.tasks.spawn(async move {
            loop {
                let next_update_ms = kcp.lock().next_update_delay_ms();
                select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(next_update_ms as u64)) => {}
                    _ = inner.update_notifier.notified() => {}
                }

                kcp.lock().update();

                if inner.has_new_input.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    inner.recv_notifier.notify_one();
                }

                if inner.waiting_new_send_window.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    inner.send_notifier.notify_one();
                }

                if recv_closed.load(std::sync::atomic::Ordering::Relaxed) {
                    inner.recv_notifier.notify_one();
                }
            }
        });

        // handle packet send
        let kcp = self.kcp.clone();
        let inner = self.inner.clone();
        let mut send_receiver = self.send_receiver.take().unwrap();
        let send_close_notifier = self.send_close_notifier.clone();
        let send_drained = self.send_drained.clone();
        self.tasks.spawn(
            async move {
                while let Some(data) = send_receiver.recv().await {
                    loop {
                        let (waitsnd, sndwnd) = {
                            let kcp = kcp.lock();
                            (kcp.waitsnd(), kcp.sendwnd())
                        };
                        if waitsnd > 2 * sndwnd {
                            inner
                                .waiting_new_send_window
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            inner.send_notifier.notified().await;
                        } else {
                            break;
                        }
                    }
                    kcp.lock().send(data.freeze()).unwrap();
                    kcp.lock().flush();
                    inner.update_notifier.notify_one();
                }

                tracing::debug!(
                    ?conn_id,
                    "connection packet sender close, waiting for waitsnd to be 0"
                );
                send_close_notifier.notify_one();

                // waiting for waitsnd to be 0
                while kcp.lock().waitsnd() > 0 {
                    inner
                        .waiting_new_send_window
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    inner.send_notifier.notified().await;
                }

                send_drained.send_replace(true);
                tracing::debug!(?conn_id, "connection packet send task done");
            }
            .instrument(tracing::trace_span!("send_task", conn = ?conn_id)),
        );

        // handle packet recv
        let kcp = self.kcp.clone();
        let inner = self.inner.clone();
        let conn_id = self.conn_id;
        let recv_sender = self.recv_sender.take().unwrap();
        let recv_closed = self.recv_closed.clone();
        self.tasks.spawn(
            async move {
                let mut buf = BytesMut::new();
                loop {
                    let peeksize = kcp.lock().peeksize();
                    if peeksize <= 0 {
                        if recv_closed.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        tracing::trace!("recv nothing, wait for next update");
                        inner.recv_notifier.notified().await;
                        continue;
                    };

                    if buf.capacity() < peeksize as usize {
                        buf.reserve(std::cmp::max(peeksize as usize, 4096));
                    }
                    kcp.lock().recv(&mut buf).unwrap();
                    tracing::trace!("recv data ({}): {:?}", buf.len(), buf);
                    assert_ne!(0, buf.len());
                    let send_ret = recv_sender.send(buf.split()).await;
                    if send_ret.is_err() {
                        break;
                    }
                }

                tracing::debug!(?conn_id, "connection packet recv task done");
            }
            .instrument(tracing::trace_span!("recv_task", conn = ?conn_id)),
        );
    }

    fn handle_input(&mut self, packet: &KcpPacket) -> Result<(), Error> {
        self.kcp.lock().handle_input(packet.payload())?;
        self.inner
            .has_new_input
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.update_notifier.notify_one();
        Ok(())
    }

    fn send_sender(&mut self) -> KcpStreamSender {
        self.send_sender.take().unwrap()
    }

    fn recv_receiver(&mut self) -> KcpStreamReceiver {
        self.recv_receiver.take().unwrap()
    }

    fn send_close_notifier(&self) -> Arc<Notify> {
        self.send_close_notifier.clone()
    }

    fn close_status_receiver(&self) -> watch::Receiver<KcpCloseStatus> {
        self.close_status.subscribe()
    }

    fn send_drained_receiver(&self) -> watch::Receiver<bool> {
        self.send_drained.subscribe()
    }

    fn close_status_sender(&self) -> watch::Sender<KcpCloseStatus> {
        self.close_status.clone()
    }

    fn close_recv(&self) {
        self.recv_closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.recv_notifier.notify_one();
    }
}

impl Drop for KcpConnection {
    fn drop(&mut self) {
        self.send_close_notifier.notify_one();
    }
}

impl PacketHeaderFlagManipulator for KcpPacket {
    fn has_syn(&self) -> bool {
        self.header().is_syn()
    }

    fn has_ack(&self) -> bool {
        self.header().is_ack()
    }

    fn has_fin(&self) -> bool {
        self.header().is_fin()
    }

    fn has_rst(&self) -> bool {
        self.header().is_rst()
    }

    fn has_data(&self) -> bool {
        self.header().is_data()
    }

    fn set_syn(&mut self, value: bool) {
        self.mut_header().set_syn(value);
    }

    fn set_ack(&mut self, value: bool) {
        self.mut_header().set_ack(value);
    }

    fn set_fin(&mut self, value: bool) {
        self.mut_header().set_fin(value);
    }

    fn set_rst(&mut self, value: bool) {
        self.mut_header().set_rst(value);
    }

    fn set_data(&mut self, value: bool) {
        self.mut_header().set_data(value);
    }
}

struct KcpConnectionState {
    fsm: KcpConnectionFSM,
    notify: Arc<Notify>,
    conn_data: Bytes,
    last_pong: std::time::Instant,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl std::fmt::Debug for KcpConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcpConnectionState")
            .field("fsm", &self.fsm)
            .finish()
    }
}

impl KcpConnectionState {
    fn new(fsm: KcpConnectionFSM, slot: tokio::sync::OwnedSemaphorePermit) -> Self {
        Self {
            fsm,
            notify: Arc::new(Notify::new()),
            conn_data: Bytes::new(),
            last_pong: std::time::Instant::now(),
            _slot: slot,
        }
    }

    fn handle_packet(&mut self, packet: &KcpPacket) -> Result<Option<KcpPacket>, Error> {
        self.notify_pong();
        let mut out_packet = None;
        let old_state = self.fsm;
        let _ = self.fsm.handle_packet(packet, &mut out_packet);
        if old_state != self.fsm {
            self.notify.notify_one();
            return Ok(out_packet);
        }
        Ok(None)
    }

    fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    fn is_established(&self) -> bool {
        matches!(self.fsm, KcpConnectionFSM::Established)
    }

    fn is_peer_closed(&self) -> bool {
        matches!(
            self.fsm,
            KcpConnectionFSM::PeerClosed | KcpConnectionFSM::LastAck | KcpConnectionFSM::Closed
        )
    }

    fn is_local_closed(&self) -> bool {
        matches!(
            self.fsm,
            KcpConnectionFSM::LocalClosed | KcpConnectionFSM::LastAck | KcpConnectionFSM::Closed
        )
    }

    fn is_closed(&self) -> bool {
        matches!(self.fsm, KcpConnectionFSM::Closed)
    }

    fn set_data(&mut self, data: Bytes) {
        self.conn_data = data;
    }

    fn notify_pong(&mut self) {
        self.last_pong = std::time::Instant::now();
    }

    fn is_pong_timeout(&self) -> bool {
        self.last_pong.elapsed() > std::time::Duration::from_secs(60)
    }
}

struct KcpEndpointData {
    cur_conv: AtomicU32,
    conn_map: DashMap<ConnId, KcpConnection>,
    state_map: DashMap<ConnId, KcpConnectionState>,
    conn_slots: Arc<Semaphore>,
    connect_cancel_cleanup_total: AtomicUsize,
    forced_cleanup_total: AtomicUsize,
    orphan_timeout_cleanup_total: AtomicUsize,
}

impl KcpEndpointData {
    fn new(max_connections: usize) -> Self {
        Self {
            cur_conv: AtomicU32::new(rand::random()),
            conn_map: DashMap::new(),
            state_map: DashMap::new(),
            conn_slots: Arc::new(Semaphore::new(max_connections)),
            connect_cancel_cleanup_total: AtomicUsize::new(0),
            forced_cleanup_total: AtomicUsize::new(0),
            orphan_timeout_cleanup_total: AtomicUsize::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct KcpEndpointStats {
    pub(crate) state_map_len: usize,
    pub(crate) conn_map_len: usize,
    pub(crate) connect_cancel_cleanup_total: usize,
    pub(crate) forced_cleanup_total: usize,
    pub(crate) orphan_timeout_cleanup_total: usize,
}

fn make_rst_packet(conn_id: ConnId) -> KcpPacket {
    let mut rst = KcpPacket::new(0);
    conn_id.fill_packet_header(&mut rst);
    rst.mut_header().set_rst(true);
    rst
}

fn cleanup_connection_state(data: &KcpEndpointData, conn_id: ConnId) {
    data.conn_map.remove(&conn_id);
    data.state_map.remove(&conn_id);
}

fn cleanup_stale_connections(data: &KcpEndpointData) {
    let mut timed_out_count = 0usize;
    data.state_map.retain(|_, state| {
        let timed_out = state.is_pong_timeout();
        if timed_out {
            timed_out_count += 1;
        }
        !matches!(state.fsm, KcpConnectionFSM::Closed) && !timed_out
    });
    if timed_out_count > 0 {
        data.orphan_timeout_cleanup_total
            .fetch_add(timed_out_count, Ordering::Relaxed);
    }
    data.conn_map
        .retain(|conn_id, _| data.state_map.contains_key(conn_id));
    data.state_map.shrink_to_fit();
    data.conn_map.shrink_to_fit();
}

struct ConnectCleanupGuard {
    data: Arc<KcpEndpointData>,
    output_sender: KcpPakcetSender,
    conn_id: ConnId,
    active: bool,
}

impl ConnectCleanupGuard {
    fn new(data: Arc<KcpEndpointData>, output_sender: KcpPakcetSender, conn_id: ConnId) -> Self {
        Self {
            data,
            output_sender,
            conn_id,
            active: true,
        }
    }

    fn cleanup(mut self, count_as_cancel: bool) {
        self.cleanup_inner(count_as_cancel);
    }

    fn disarm(mut self) {
        self.active = false;
    }

    fn cleanup_inner(&mut self, count_as_cancel: bool) {
        if !self.active {
            return;
        }
        self.active = false;
        cleanup_connection_state(&self.data, self.conn_id);
        if count_as_cancel {
            self.data
                .connect_cancel_cleanup_total
                .fetch_add(1, Ordering::Relaxed);
        }
        if let Err(error) = self.output_sender.try_send(make_rst_packet(self.conn_id)) {
            tracing::debug!(
                ?error,
                conn_id = ?self.conn_id,
                "failed to enqueue KCP reset during connect cleanup"
            );
        }
    }
}

impl Drop for ConnectCleanupGuard {
    fn drop(&mut self) {
        self.cleanup_inner(true);
    }
}

async fn force_close_connection(
    data: &KcpEndpointData,
    output_sender: &KcpPakcetSender,
    conn_id: ConnId,
    send_timeout: std::time::Duration,
) {
    let rst = make_rst_packet(conn_id);
    let _ = timeout(send_timeout, output_sender.send(rst)).await;
    cleanup_connection_state(data, conn_id);
    data.forced_cleanup_total.fetch_add(1, Ordering::Relaxed);
    tracing::warn!(?conn_id, "KCP graceful close timed out; forcing cleanup");
}

pub type KcpConfigFactory = Box<dyn Fn(u32) -> KcpConfig + Send + Sync>;

pub struct KcpEndpoint {
    id: u64,
    data: Arc<KcpEndpointData>,

    input_sender: KcpPakcetSender,
    input_receiver: Option<KcpPacketReceiver>,

    output_sender: KcpPakcetSender,
    output_receiver: Option<KcpPacketReceiver>,

    new_conn_sender: tokio::sync::mpsc::Sender<ConnId>,
    new_conn_receiver: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<ConnId>>>,

    kcp_config_factory: KcpConfigFactory,
    close_timeout: std::time::Duration,
    close_retry_interval: std::time::Duration,

    tasks: JoinSet<()>,
}

impl std::fmt::Debug for KcpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcpEndpoint").field("id", &self.id).finish()
    }
}

impl Default for KcpEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KcpEndpoint {
    pub fn new() -> Self {
        Self::new_with_max_connections(DEFAULT_MAX_CONNECTIONS)
    }

    pub fn new_with_max_connections(max_connections: usize) -> Self {
        assert!(max_connections > 0, "max_connections must be positive");
        let (input_sender, input_receiver) = tokio::sync::mpsc::channel(1024);
        let (output_sender, output_receiver) = tokio::sync::mpsc::channel(1024);
        let (new_conn_sender, new_conn_receiver) = tokio::sync::mpsc::channel(max_connections);

        Self {
            id: rand::random(),
            data: Arc::new(KcpEndpointData::new(max_connections)),

            input_sender,
            input_receiver: Some(input_receiver),

            output_sender,
            output_receiver: Some(output_receiver),

            new_conn_sender,
            new_conn_receiver: Arc::new(tokio::sync::Mutex::new(new_conn_receiver)),

            kcp_config_factory: Box::new(KcpConfig::new_turbo),
            close_timeout: DEFAULT_CLOSE_TIMEOUT,
            close_retry_interval: DEFAULT_CLOSE_RETRY_INTERVAL,

            tasks: JoinSet::new(),
        }
    }

    pub fn set_kcp_config_factory(&mut self, factory: KcpConfigFactory) {
        self.kcp_config_factory = factory;
    }

    pub fn set_close_config(
        &mut self,
        timeout: std::time::Duration,
        retry_interval: std::time::Duration,
    ) {
        assert!(!timeout.is_zero(), "close timeout must be positive");
        assert!(
            !retry_interval.is_zero(),
            "close retry interval must be positive"
        );
        self.close_timeout = timeout;
        self.close_retry_interval = retry_interval;
    }

    async fn try_handle_pingpong(
        data: &KcpEndpointData,
        packet: &KcpPacket,
        output_sender: &KcpPakcetSender,
    ) -> bool {
        let hdr = packet.header();

        if hdr.is_ping() && !hdr.is_pong() {
            let conn_id = ConnId::from(packet);
            let need_send_pong = data
                .state_map
                .get_mut(&conn_id)
                .map(|x| !x.is_local_closed())
                .unwrap_or(false);

            let mut out_packet = packet.clone();
            if need_send_pong {
                out_packet.mut_header().set_pong(true);
            } else {
                out_packet.mut_header().set_ping(false);
                out_packet.mut_header().set_rst(true);
            };

            tracing::trace!("sending pong packet: {:?}", out_packet);
            let ret = output_sender.send(out_packet).await;
            if let Err(e) = ret {
                tracing::error!(?e, "send pong packet failed");
            }
        }

        // all incoming packet should update pong time
        let conv = ConnId::from(packet);
        if let Some(mut state) = data.state_map.get_mut(&conv) {
            state.notify_pong();
        }

        packet.header().is_ping()
    }

    pub async fn run(&mut self) {
        let mut input_receiver = self.input_receiver.take().unwrap();
        let data = self.data.clone();
        let output_sender = self.output_sender.clone();
        let new_conn_sender = self.new_conn_sender.clone();

        self.tasks.spawn(
            async move {
                while let Some(packet) = input_receiver.recv().await {
                    tracing::trace!("recv packet: {:?}", packet);
                    if Self::try_handle_pingpong(&data, &packet, &output_sender).await {
                        continue;
                    }

                    let conv = ConnId::from(&packet);
                    if packet.header().is_data() && !packet.payload().is_empty() {
                        if let Some(mut conn) = data.conn_map.get_mut(&conv) {
                            if let Err(e) = conn.handle_input(&packet) {
                                tracing::error!(?e, ?conv, "handle input on connection failed");
                            } else {
                                tracing::trace!(?conv, "handle input on connection done");
                            }
                        } else {
                            tracing::debug!(
                                ?conv,
                                ?packet,
                                "no conn for conv when handling data packet"
                            );
                        }
                    }

                    let mut state_ref = data.state_map.get_mut(&conv);
                    let state = state_ref.as_deref_mut();
                    let mut out_packet: Option<KcpPacket> = None;
                    let mut remove_closed = false;
                    if let Some(state) = state {
                        let prev_established = state.is_established();
                        let ret = state.handle_packet(&packet);
                        tracing::trace!(?conv, ?state, "handle packet for conn, ret: {:?}", ret);
                        if let Ok(pkt) = ret {
                            out_packet = pkt;
                        }

                        if !prev_established && state.is_established() {
                            let _ = new_conn_sender.try_send(conv);
                        }

                        if state.is_peer_closed() {
                            tracing::debug!(?conv, "peer half closed, close recv");
                            if let Some(conn) = data.conn_map.get_mut(&conv) {
                                conn.close_recv()
                            }
                        }

                        if state.is_closed() {
                            tracing::debug!(?conv, "connection closed, remove state");
                            data.conn_map.remove(&conv);
                            remove_closed = true;
                        }
                    } else {
                        if packet.header().is_rst() {
                            tracing::debug!(?conv, "reset packet for conn, but no state");
                            continue;
                        }
                        let mut tmp_fsm = KcpConnectionFSM::listen();
                        let res = tmp_fsm.handle_packet(&packet, &mut out_packet);
                        tracing::trace!(
                            ?conv,
                            ?out_packet,
                            "handle first packet for conn, ret: {:?}",
                            res
                        );
                        if res.is_ok() {
                            if let Ok(slot) = data.conn_slots.clone().try_acquire_owned() {
                                let mut conn_state = KcpConnectionState::new(tmp_fsm, slot);
                                conn_state.set_data(packet.payload().to_vec().into());
                                data.state_map.insert(conv, conn_state);
                            } else {
                                let mut rst = KcpPacket::new(0);
                                conv.fill_packet_header(&mut rst);
                                rst.mut_header().set_rst(true);
                                out_packet = Some(rst);
                                tracing::warn!(?conv, "KCP connection capacity reached");
                            }
                        }
                    }

                    drop(state_ref);
                    if remove_closed {
                        data.state_map.remove(&conv);
                    }
                    if let Some(mut out_packet) = out_packet {
                        conv.fill_packet_header(&mut out_packet);
                        tracing::trace!(?conv, ?out_packet, "sending output packet");
                        let ret = output_sender.send(out_packet).await;
                        if let Err(e) = ret {
                            tracing::error!(?e, "send output packet failed");
                        }
                    }
                }
            }
            .instrument(tracing::trace_span!("recv_task", id = self.id)),
        );

        // conn clean task
        let data = self.data.clone();
        self.tasks.spawn(async move {
            loop {
                cleanup_stale_connections(&data);
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });

        // conn ping task
        let data = self.data.clone();
        let output_sender = self.output_sender.clone();
        self.tasks.spawn(async move {
            loop {
                let packets = data
                    .state_map
                    .iter()
                    .filter_map(|item| {
                        let (conn_id, state) = item.pair();
                        if state.is_closed() {
                            return None;
                        }
                        let mut out_packet = KcpPacket::new(0);
                        conn_id.fill_packet_header(&mut out_packet);
                        out_packet.mut_header().set_ping(true);
                        Some(out_packet)
                    })
                    .collect::<Vec<_>>();

                for packet in packets {
                    let ret = output_sender.send(packet).await;
                    if let Err(e) = ret {
                        tracing::error!(?e, "send ping packet failed");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }

                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });
    }

    fn add_conn(&self, conn_id: ConnId) -> Result<(), Error> {
        let mut conn =
            KcpConnection::new_with_config(conn_id, (self.kcp_config_factory)(conn_id.conv))?;
        conn.run(self.output_sender.clone());

        let data = self.data.clone();
        let close_notifier = conn.send_close_notifier();
        let mut send_drained = conn.send_drained_receiver();
        let close_status = conn.close_status_sender();

        data.conn_map.insert(conn_id, conn);

        let output_sender = self.output_sender.clone();
        let data = Arc::downgrade(&data);
        let close_timeout = self.close_timeout;
        let close_retry_interval = self.close_retry_interval;
        tokio::spawn(async move {
            close_notifier.notified().await;
            let Some(data) = data.upgrade() else {
                close_status.send_replace(KcpCloseStatus::Forced);
                return;
            };
            let deadline = tokio::time::Instant::now() + close_timeout;
            if !*send_drained.borrow() {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero()
                    || !matches!(
                        timeout(remaining, send_drained.wait_for(|drained| *drained)).await,
                        Ok(Ok(_))
                    )
                {
                    force_close_connection(&data, &output_sender, conn_id, close_retry_interval)
                        .await;
                    close_status.send_replace(KcpCloseStatus::Forced);
                    return;
                }
            }

            let mut fin_packet = KcpPacket::new(0);
            let Some(mut state) = data.state_map.get_mut(&conn_id) else {
                close_status.send_replace(KcpCloseStatus::Graceful);
                return;
            };

            let close_ret = state.fsm.close(&mut fin_packet);
            let state_notify = state.notify();
            drop(state);
            if let Err(e) = close_ret {
                tracing::error!(?e, ?conn_id, "close connection failed");
            }
            conn_id.fill_packet_header(&mut fin_packet);

            let mut timed_out = !matches!(
                timeout(close_retry_interval, output_sender.send(fin_packet.clone())).await,
                Ok(Ok(()))
            );
            while !timed_out {
                let closed = data
                    .state_map
                    .get(&conn_id)
                    .is_none_or(|state| state.is_closed());
                if closed {
                    break;
                }

                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    timed_out = true;
                    break;
                }
                let wait = std::cmp::min(remaining, close_retry_interval);
                if timeout(wait, state_notify.notified()).await.is_err()
                    && !matches!(
                        timeout(close_retry_interval, output_sender.send(fin_packet.clone())).await,
                        Ok(Ok(()))
                    )
                {
                    timed_out = true;
                }
            }

            if timed_out {
                force_close_connection(&data, &output_sender, conn_id, close_retry_interval).await;
            } else {
                data.conn_map.remove(&conn_id);
                data.state_map.remove(&conn_id);
            }

            close_status.send_replace(if timed_out {
                KcpCloseStatus::Forced
            } else {
                KcpCloseStatus::Graceful
            });
            tracing::debug!(?conn_id, timed_out, "connection close watcher done");
        });

        Ok(())
    }

    pub fn output_receiver(&mut self) -> Option<KcpPacketReceiver> {
        self.output_receiver.take()
    }

    pub fn input_sender(&self) -> KcpPakcetSender {
        self.input_sender.clone()
    }

    pub fn input_sender_ref(&self) -> &KcpPakcetSender {
        &self.input_sender
    }

    pub fn conn_sender_receiver(
        &self,
        conn_id: ConnId,
    ) -> Option<(KcpStreamSender, KcpStreamReceiver)> {
        let mut conn = self.data.conn_map.get_mut(&conn_id)?;
        Some((conn.send_sender(), conn.recv_receiver()))
    }

    pub(crate) fn conn_stream_parts(
        &self,
        conn_id: ConnId,
    ) -> Option<(
        KcpStreamSender,
        KcpStreamReceiver,
        watch::Receiver<KcpCloseStatus>,
    )> {
        let mut conn = self.data.conn_map.get_mut(&conn_id)?;
        Some((
            conn.send_sender(),
            conn.recv_receiver(),
            conn.close_status_receiver(),
        ))
    }

    pub fn conn_data(&self, conn_id: &ConnId) -> Option<Bytes> {
        let state = self.data.state_map.get(conn_id)?;
        Some(state.conn_data.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn stats(&self) -> KcpEndpointStats {
        KcpEndpointStats {
            state_map_len: self.data.state_map.len(),
            conn_map_len: self.data.conn_map.len(),
            connect_cancel_cleanup_total: self
                .data
                .connect_cancel_cleanup_total
                .load(Ordering::Relaxed),
            forced_cleanup_total: self.data.forced_cleanup_total.load(Ordering::Relaxed),
            orphan_timeout_cleanup_total: self
                .data
                .orphan_timeout_cleanup_total
                .load(Ordering::Relaxed),
        }
    }

    #[tracing::instrument(ret)]
    pub async fn connect(
        &self,
        timeout_dur: std::time::Duration,
        src_session_id: u32,
        dst_session_id: u32,
        conn_data: Bytes,
    ) -> Result<ConnId, Error> {
        let mut out_packet = KcpPacket::new_with_payload(&conn_data);
        let conn_id = loop {
            let conv_cand = self
                .data
                .cur_conv
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let conn_id = ConnId {
                conv: conv_cand,
                src_session_id,
                dst_session_id,
            };
            if !self.data.state_map.contains_key(&conn_id) {
                break conn_id;
            }
        };

        let fsm = KcpConnectionFSM::connect(&mut out_packet);
        let slot = self
            .data
            .conn_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::CreateConnectionFailed)?;
        let mut state = KcpConnectionState::new(fsm, slot);
        state.set_data(conn_data);
        let notify = state.notify();
        self.data.state_map.insert(conn_id, state);
        let cleanup_guard =
            ConnectCleanupGuard::new(self.data.clone(), self.output_sender.clone(), conn_id);

        conn_id.fill_packet_header(&mut out_packet);

        tracing::trace!(?conn_id, "connect packet: {:?}", out_packet);
        if let Err(error) = self.output_sender.send(out_packet).await {
            cleanup_guard.cleanup(false);
            return Err(anyhow::Error::from(error)
                .context("send connect packet failed")
                .into());
        }

        if timeout(timeout_dur, notify.notified()).await.is_err() {
            cleanup_guard.cleanup(false);
            return Err(Error::ConnectTimeout);
        }

        if let Some(state) = self.data.state_map.get(&conn_id) {
            tracing::debug!(?conn_id, ?state, "connect done, checkin state");
            if matches!(state.fsm, KcpConnectionFSM::Established) {
                if let Err(error) = self.add_conn(conn_id) {
                    drop(state);
                    cleanup_guard.cleanup(false);
                    return Err(error);
                }
                drop(state);
                cleanup_guard.disarm();
                return Ok(conn_id);
            } else {
                drop(state);
                cleanup_guard.cleanup(false);
                return Err(anyhow::anyhow!("connect failed").into());
            }
        }

        cleanup_guard.cleanup(false);
        return Err(anyhow::anyhow!("connect failed").into());
    }

    pub async fn accept(&self) -> Result<ConnId, Error> {
        let conn_receiver = self.new_conn_receiver.clone();

        loop {
            let Some(conn_id) = conn_receiver.lock().await.recv().await else {
                return Err(Error::Shutdown);
            };

            let Some(state) = self.data.state_map.get(&conn_id) else {
                tracing::debug!(?conn_id, "no state for conn, ignore");
                continue;
            };

            if matches!(state.fsm, KcpConnectionFSM::Established) {
                if let Err(error) = self.add_conn(conn_id) {
                    drop(state);
                    cleanup_connection_state(&self.data, conn_id);
                    let _ = self.output_sender.try_send(make_rst_packet(conn_id));
                    return Err(error);
                }
                return Ok(conn_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer as _};

    use super::*;
    use crate::stream::KcpStream;

    fn _enable_log() {
        let console_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stderr)
            .with_filter(LevelFilter::TRACE);

        tracing_subscriber::Registry::default()
            .with(console_layer)
            .init();
    }

    async fn prepare_test() -> (KcpEndpoint, KcpEndpoint, JoinSet<()>) {
        let mut client_endpoint = KcpEndpoint::new();
        let mut server_endpoint = KcpEndpoint::new();
        let mut t = JoinSet::new();

        client_endpoint.run().await;
        server_endpoint.run().await;

        let client_input_sender = client_endpoint.input_sender();
        let mut server_output_receiver = server_endpoint.output_receiver().unwrap();
        t.spawn(async move {
            while let Some(packet) = server_output_receiver.recv().await {
                let _ = client_input_sender.send(packet).await;
            }
        });

        let server_input_sender = server_endpoint.input_sender();
        let mut client_output_receiver = client_endpoint.output_receiver().unwrap();
        t.spawn(async move {
            while let Some(packet) = client_output_receiver.recv().await {
                let _ = server_input_sender.send(packet).await;
            }
        });

        (client_endpoint, server_endpoint, t)
    }

    async fn prepare_lossy_stream_test(
        delay: std::time::Duration,
    ) -> (
        KcpEndpoint,
        KcpEndpoint,
        JoinSet<()>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let mut client_endpoint = KcpEndpoint::new_with_max_connections(8);
        let mut server_endpoint = KcpEndpoint::new_with_max_connections(8);
        client_endpoint.set_close_config(
            std::time::Duration::from_secs(2),
            std::time::Duration::from_millis(30),
        );
        server_endpoint.set_close_config(
            std::time::Duration::from_secs(2),
            std::time::Duration::from_millis(30),
        );
        client_endpoint.run().await;
        server_endpoint.run().await;

        let client_fin_count = Arc::new(AtomicUsize::new(0));
        let server_fin_count = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();

        let client_input = client_endpoint.input_sender();
        let mut server_output = server_endpoint.output_receiver().unwrap();
        let server_fin_count_task = server_fin_count.clone();
        tasks.spawn(async move {
            while let Some(packet) = server_output.recv().await {
                if packet.header().is_fin()
                    && server_fin_count_task.fetch_add(1, Ordering::SeqCst) == 0
                {
                    continue;
                }
                tokio::time::sleep(delay).await;
                let _ = client_input.send(packet).await;
            }
        });

        let server_input = server_endpoint.input_sender();
        let mut client_output = client_endpoint.output_receiver().unwrap();
        let client_fin_count_task = client_fin_count.clone();
        tasks.spawn(async move {
            while let Some(packet) = client_output.recv().await {
                if packet.header().is_fin()
                    && client_fin_count_task.fetch_add(1, Ordering::SeqCst) == 0
                {
                    continue;
                }
                tokio::time::sleep(delay).await;
                let _ = server_input.send(packet).await;
            }
        });

        (
            client_endpoint,
            server_endpoint,
            tasks,
            client_fin_count,
            server_fin_count,
        )
    }

    async fn wait_until_empty(endpoint: &KcpEndpoint) {
        timeout(std::time::Duration::from_secs(2), async {
            loop {
                let stats = endpoint.stats();
                if stats.conn_map_len == 0 && stats.state_map_len == 0 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("KCP connection state was not reclaimed");
    }

    async fn wait_until_state_count(endpoint: &KcpEndpoint, count: usize) {
        timeout(std::time::Duration::from_secs(2), async {
            loop {
                if endpoint.stats().state_map_len == count {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("KCP connection state count did not reach expected value");
    }

    #[tokio::test]
    async fn dropped_connect_future_reclaims_state_and_enqueues_rst() {
        let mut endpoint = KcpEndpoint::new();
        let mut output_receiver = endpoint.output_receiver().unwrap();
        let endpoint = Arc::new(endpoint);
        let connect_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            connect_endpoint
                .connect(
                    std::time::Duration::from_secs(60),
                    1,
                    3,
                    Bytes::from("conn"),
                )
                .await
        });

        wait_until_state_count(&endpoint, 1).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        wait_until_empty(&endpoint).await;
        assert_eq!(endpoint.stats().connect_cancel_cleanup_total, 1);

        let mut saw_rst = false;
        while let Ok(packet) = output_receiver.try_recv() {
            saw_rst |= packet.header().is_rst();
        }
        assert!(
            saw_rst,
            "connect cancellation should enqueue a reset packet"
        );
    }

    #[tokio::test]
    async fn multiple_cancelled_connects_do_not_leave_hedge_loser_state() {
        let mut endpoint = KcpEndpoint::new();
        let _output_receiver = endpoint.output_receiver().unwrap();
        let endpoint = Arc::new(endpoint);
        let mut tasks = JoinSet::new();
        for index in 0..5u32 {
            let connect_endpoint = endpoint.clone();
            tasks.spawn(async move {
                connect_endpoint
                    .connect(
                        std::time::Duration::from_secs(60),
                        index + 1,
                        index + 100,
                        Bytes::from("conn"),
                    )
                    .await
            });
        }

        wait_until_state_count(&endpoint, 5).await;
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            assert!(result.unwrap_err().is_cancelled());
        }
        wait_until_empty(&endpoint).await;
        assert_eq!(endpoint.stats().connect_cancel_cleanup_total, 5);
    }

    #[tokio::test]
    async fn connect_timeout_and_send_failure_reclaim_state() {
        let endpoint = KcpEndpoint::new();
        let timeout_error = endpoint
            .connect(
                std::time::Duration::from_millis(20),
                1,
                3,
                Bytes::from("conn"),
            )
            .await
            .unwrap_err();
        assert!(matches!(timeout_error, Error::ConnectTimeout));
        wait_until_empty(&endpoint).await;
        assert_eq!(endpoint.stats().connect_cancel_cleanup_total, 0);

        let mut endpoint = KcpEndpoint::new();
        drop(endpoint.output_receiver().unwrap());
        let send_error = endpoint
            .connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn"))
            .await
            .unwrap_err();
        assert!(matches!(send_error, Error::AnyhowError(_)));
        wait_until_empty(&endpoint).await;
        assert_eq!(endpoint.stats().connect_cancel_cleanup_total, 0);
    }

    #[tokio::test]
    async fn late_syn_ack_after_connect_cancel_does_not_recreate_state() {
        let mut endpoint = KcpEndpoint::new();
        endpoint.run().await;
        let endpoint = Arc::new(endpoint);
        let connect_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            connect_endpoint
                .connect(
                    std::time::Duration::from_secs(60),
                    1,
                    3,
                    Bytes::from("conn"),
                )
                .await
        });

        wait_until_state_count(&endpoint, 1).await;
        let conn_id = *endpoint.data.state_map.iter().next().unwrap().key();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        wait_until_empty(&endpoint).await;

        let mut syn_ack = KcpPacket::new(0);
        conn_id.fill_packet_header(&mut syn_ack);
        syn_ack.mut_header().set_syn(true).set_ack(true);
        endpoint.input_sender().send(syn_ack).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        wait_until_empty(&endpoint).await;
    }

    #[tokio::test]
    async fn dropped_stream_reclaims_connection_state() {
        let (client_endpoint, server_endpoint, tasks) = prepare_test().await;
        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        let client = KcpStream::new(&client_endpoint, conn_id).unwrap();
        let server = KcpStream::new(&server_endpoint, conn_id).unwrap();
        drop(client);
        drop(server);

        wait_until_empty(&client_endpoint).await;
        wait_until_empty(&server_endpoint).await;
        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn stream_new_failure_after_concurrent_cleanup_has_no_residual_state() {
        let (client_endpoint, server_endpoint, tasks) = prepare_test().await;
        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        cleanup_connection_state(&client_endpoint.data, conn_id);
        assert!(KcpStream::new(&client_endpoint, conn_id).is_none());
        wait_until_empty(&client_endpoint).await;

        cleanup_connection_state(&server_endpoint.data, conn_id);
        wait_until_empty(&server_endpoint).await;
        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn stale_cleanup_reclaims_timed_out_orphan_state_and_conn() {
        let (client_endpoint, server_endpoint, tasks) = prepare_test().await;
        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        {
            let mut state = server_endpoint.data.state_map.get_mut(&conn_id).unwrap();
            state.last_pong = std::time::Instant::now() - std::time::Duration::from_secs(61);
        }
        cleanup_stale_connections(&server_endpoint.data);
        wait_until_empty(&server_endpoint).await;
        assert_eq!(server_endpoint.stats().orphan_timeout_cleanup_total, 1);

        cleanup_connection_state(&client_endpoint.data, conn_id);
        wait_until_empty(&client_endpoint).await;
        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn endpoint_uses_config_factory_for_new_connections() {
        let mut endpoint = KcpEndpoint::new();
        endpoint.set_kcp_config_factory(Box::new(|conv| {
            let mut config = KcpConfig::new_turbo(conv);
            config.interval = Some(37);
            config
        }));
        let conn_id = ConnId {
            conv: 7,
            src_session_id: 1,
            dst_session_id: 3,
        };

        endpoint.add_conn(conn_id).unwrap();
        let conn = endpoint.data.conn_map.get(&conn_id).unwrap();
        assert_eq!(conn.kcp.lock().config().interval, Some(37));
        drop(conn);
        cleanup_connection_state(&endpoint.data, conn_id);
        wait_until_empty(&endpoint).await;
    }

    #[tokio::test]
    async fn test_kcp_connect_and_close() {
        let mut p = KcpPacket::new(0);
        let _ = p.mut_header().conv();

        let (client_endpoint, server_endpoint, t) = prepare_test().await;

        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );

        assert_eq!(*connect_ret.as_ref().unwrap(), accept_ret.unwrap());

        let conv = connect_ret.unwrap();

        let client_conn_data = client_endpoint.conn_data(&conv).unwrap();
        assert_eq!("conn", String::from_utf8_lossy(&client_conn_data));

        let server_conn_data = server_endpoint.conn_data(&conv).unwrap();
        assert_eq!("conn", String::from_utf8_lossy(&server_conn_data));

        let (client_sender, mut client_receiver) =
            client_endpoint.conn_sender_receiver(conv).unwrap();
        let (server_sender, mut server_receiver) =
            server_endpoint.conn_sender_receiver(conv).unwrap();

        client_sender.send(BytesMut::from("hello")).await.unwrap();
        let data = server_receiver.recv().await.unwrap();
        assert_eq!("hello", String::from_utf8_lossy(&data));

        server_sender.send(BytesMut::from("world")).await.unwrap();
        let data = client_receiver.recv().await.unwrap();
        assert_eq!("world", String::from_utf8_lossy(&data));

        // test half close
        drop(client_sender);
        assert!(server_receiver.recv().await.is_none());
        // server can still send data
        server_sender.send(BytesMut::from("world")).await.unwrap();
        let data = client_receiver.recv().await.unwrap();
        assert_eq!("world", String::from_utf8_lossy(&data));

        // full close
        drop(server_sender);
        assert!(client_receiver.recv().await.is_none());

        drop(client_endpoint);
        drop(server_endpoint);

        t.join_all().await;
    }

    #[tokio::test]
    async fn graceful_shutdown_retransmits_fin_and_reclaims_state() {
        let (client_endpoint, server_endpoint, tasks, client_fin_count, server_fin_count) =
            prepare_lossy_stream_test(std::time::Duration::from_millis(40)).await;

        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(2), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();
        let mut server = KcpStream::new(&server_endpoint, conn_id).unwrap();
        client.write_all(b"request").await.unwrap();
        let mut request = [0u8; 7];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        server.write_all(b"response").await.unwrap();
        let mut response = [0u8; 8];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"response");

        let (client_close, server_close) = tokio::join!(
            client.shutdown_gracefully(std::time::Duration::from_secs(3)),
            server.shutdown_gracefully(std::time::Duration::from_secs(3))
        );
        client_close.unwrap();
        server_close.unwrap();

        wait_until_empty(&client_endpoint).await;
        wait_until_empty(&server_endpoint).await;
        assert!(client_fin_count.load(Ordering::SeqCst) >= 2);
        assert!(server_fin_count.load(Ordering::SeqCst) >= 2);

        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn passive_shutdown_retransmits_fin_until_final_ack() {
        let (client_endpoint, server_endpoint, tasks, client_fin_count, server_fin_count) =
            prepare_lossy_stream_test(std::time::Duration::from_millis(20)).await;

        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(2), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();
        let mut server = KcpStream::new(&server_endpoint, conn_id).unwrap();
        server.write_all(b"response").await.unwrap();
        let server_close = tokio::spawn(async move {
            server
                .shutdown_gracefully(std::time::Duration::from_secs(3))
                .await
        });

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"response");
        client
            .shutdown_gracefully(std::time::Duration::from_secs(3))
            .await
            .unwrap();
        server_close.await.unwrap().unwrap();

        wait_until_empty(&client_endpoint).await;
        wait_until_empty(&server_endpoint).await;
        assert!(client_fin_count.load(Ordering::SeqCst) >= 2);
        assert!(server_fin_count.load(Ordering::SeqCst) >= 2);

        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn peer_fin_is_reported_as_eof_after_final_payload() {
        let (client_endpoint, server_endpoint, tasks) = prepare_test().await;
        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();
        let mut server = KcpStream::new(&server_endpoint, conn_id).unwrap();
        server.write_all(b"response").await.unwrap();
        let server_close = tokio::spawn(async move {
            server
                .shutdown_gracefully(std::time::Duration::from_secs(2))
                .await
        });

        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"response");
        client
            .shutdown_gracefully(std::time::Duration::from_secs(2))
            .await
            .unwrap();
        server_close.await.unwrap().unwrap();

        wait_until_empty(&client_endpoint).await;
        wait_until_empty(&server_endpoint).await;
        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn peer_fin_drains_data_buffered_behind_full_stream_channel() {
        let (client_endpoint, server_endpoint, tasks) = prepare_test().await;
        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());

        let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();
        let mut server = KcpStream::new(&server_endpoint, conn_id).unwrap();
        let expected = (0..256u16).flat_map(u16::to_be_bytes).collect::<Vec<_>>();
        let response = expected.clone();
        let server_close = tokio::spawn(async move {
            for chunk in response.chunks_exact(2) {
                server.write_all(chunk).await.unwrap();
            }
            server
                .shutdown_gracefully(std::time::Duration::from_secs(3))
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut received = Vec::new();
        client.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, expected);
        client
            .shutdown_gracefully(std::time::Duration::from_secs(3))
            .await
            .unwrap();
        server_close.await.unwrap().unwrap();

        wait_until_empty(&client_endpoint).await;
        wait_until_empty(&server_endpoint).await;
        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn connection_slots_are_reused_without_residual_state() {
        let (mut client_endpoint, mut server_endpoint, tasks) = prepare_test().await;
        client_endpoint.set_close_config(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(20),
        );
        server_endpoint.set_close_config(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(20),
        );

        for index in 0..1000u32 {
            let (connect_ret, accept_ret) = tokio::join!(
                client_endpoint.connect(
                    std::time::Duration::from_secs(1),
                    index + 1,
                    index + 1001,
                    Bytes::from("conn")
                ),
                server_endpoint.accept()
            );
            let conn_id = connect_ret.unwrap();
            assert_eq!(conn_id, accept_ret.unwrap());

            let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();
            let mut server = KcpStream::new(&server_endpoint, conn_id).unwrap();
            server.write_all(b"x").await.unwrap();
            let mut byte = [0u8; 1];
            client.read_exact(&mut byte).await.unwrap();
            assert_eq!(byte, [b'x']);

            let (client_close, server_close) = tokio::join!(
                client.shutdown_gracefully(std::time::Duration::from_secs(2)),
                server.shutdown_gracefully(std::time::Duration::from_secs(2))
            );
            client_close.unwrap();
            server_close.unwrap();
            wait_until_empty(&client_endpoint).await;
            wait_until_empty(&server_endpoint).await;
        }
        assert_eq!(
            client_endpoint.data.conn_slots.available_permits(),
            DEFAULT_MAX_CONNECTIONS
        );
        assert_eq!(
            server_endpoint.data.conn_slots.available_permits(),
            DEFAULT_MAX_CONNECTIONS
        );

        drop(client_endpoint);
        drop(server_endpoint);
        tasks.join_all().await;
    }

    #[tokio::test]
    async fn graceful_shutdown_forces_local_cleanup_when_output_is_gone() {
        let (mut client_endpoint, server_endpoint, mut tasks) = prepare_test().await;
        client_endpoint.set_close_config(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(20),
        );

        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );
        let conn_id = connect_ret.unwrap();
        assert_eq!(conn_id, accept_ret.unwrap());
        let mut client = KcpStream::new(&client_endpoint, conn_id).unwrap();

        tasks.abort_all();
        drop(tasks);
        tokio::task::yield_now().await;
        client.write_all(b"unacked").await.unwrap();
        let error = client
            .shutdown_gracefully(std::time::Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        wait_until_empty(&client_endpoint).await;
        assert_eq!(
            client_endpoint.data.conn_slots.available_permits(),
            DEFAULT_MAX_CONNECTIONS
        );
    }
}
