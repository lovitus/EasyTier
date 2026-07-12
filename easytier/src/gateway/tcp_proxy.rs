use anyhow::Context;
use cidr::Ipv4Inet;
use core::panic;
use crossbeam::atomic::AtomicCell;
use dashmap::DashMap;
use hotpath::instant::Instant;
use pnet::packet::MutablePacket;
use pnet::packet::Packet;
use pnet::packet::ip::IpNextHeaderProtocols;
use pnet::packet::ipv4::{Ipv4Packet, MutableIpv4Packet};
use pnet::packet::tcp::{MutableTcpPacket, TcpPacket, ipv4_checksum};
use socket2::{SockRef, TcpKeepalive};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::Instrument;

use crate::common::error::Result;
use crate::common::global_ctx::{ArcGlobalCtx, GlobalCtx};
use crate::common::join_joinset_background;
use crate::common::log;
use crate::common::stats_manager::{LabelSet, LabelType, MetricName};
use crate::peers::peer_manager::PeerManager;
use crate::peers::{NicPacketFilter, PeerPacketFilter};
use crate::proto::api::instance::{
    ListTcpProxyEntryRequest, ListTcpProxyEntryResponse, TcpProxyEntry, TcpProxyEntryState,
    TcpProxyEntryTransportType, TcpProxyRpc,
};
use crate::proto::rpc_types;
use crate::proto::rpc_types::controller::BaseController;
use crate::tunnel::packet_def::{PacketType, PeerManagerHeader, ZCPacket};

use super::CidrSet;

#[cfg(feature = "smoltcp")]
use super::tokio_smoltcp::{self, Net, NetConfig, channel_device};

pub(crate) struct ClaimedNatDstStream(pub Box<dyn std::any::Any + Send>);

impl std::fmt::Debug for ClaimedNatDstStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClaimedNatDstStream(..)")
    }
}

#[async_trait::async_trait]
pub(crate) trait NatDstConnector: Send + Sync + Clone + 'static {
    type DstStream: AsyncRead + AsyncWrite + Unpin + Send;

    async fn connect(
        &self,
        src: SocketAddr,
        dst: SocketAddr,
        claimed: Option<ClaimedNatDstStream>,
    ) -> anyhow::Result<Self::DstStream>;
    async fn shutdown(&self, stream: &mut Self::DstStream) -> std::io::Result<()> {
        stream.shutdown().await
    }
    fn claim_deferred_stream(
        &self,
        _src: SocketAddr,
        _dst: SocketAddr,
        _initial_sequence: u32,
    ) -> Option<ClaimedNatDstStream> {
        None
    }
    fn check_packet_from_peer_fast(&self, cidr_set: &CidrSet, global_ctx: &GlobalCtx) -> bool;
    fn check_packet_from_peer(
        &self,
        cidr_set: &CidrSet,
        global_ctx: &GlobalCtx,
        hdr: &PeerManagerHeader,
        ipv4: &Ipv4Addr,
        real_dst_ip: &mut Ipv4Addr,
    ) -> bool;
    fn transport_type(&self) -> TcpProxyEntryTransportType;
}

#[derive(Debug, Clone)]
pub struct NatDstTcpConnector;

#[async_trait::async_trait]
impl NatDstConnector for NatDstTcpConnector {
    type DstStream = TcpStream;
    async fn connect(
        &self,
        _src: SocketAddr,
        nat_dst: SocketAddr,
        claimed: Option<ClaimedNatDstStream>,
    ) -> anyhow::Result<Self::DstStream> {
        debug_assert!(claimed.is_none());
        let socket = TcpSocket::new_v4()
            .inspect_err(|error| log::error!(?error, "create v4 socket failed"))?;

        let stream = timeout(Duration::from_secs(10), socket.connect(nat_dst))
            .await?
            .with_context(|| format!("connect to nat dst failed: {:?}", nat_dst))?;

        prepare_kernel_tcp_socket(&stream)?;

        Ok(stream)
    }

    fn check_packet_from_peer_fast(&self, cidr_set: &CidrSet, global_ctx: &GlobalCtx) -> bool {
        !cidr_set.is_empty() || global_ctx.enable_exit_node() || global_ctx.no_tun()
    }

    fn check_packet_from_peer(
        &self,
        cidr_set: &CidrSet,
        global_ctx: &GlobalCtx,
        hdr: &PeerManagerHeader,
        ipv4: &Ipv4Addr,
        real_dst_ip: &mut Ipv4Addr,
    ) -> bool {
        let is_exit_node = hdr.is_exit_node();

        if !(cidr_set.contains_v4(*ipv4, real_dst_ip)
            || is_exit_node
            || global_ctx.no_tun()
                && Some(*ipv4) == global_ctx.get_ipv4().as_ref().map(Ipv4Inet::address))
        {
            return false;
        }

        true
    }

    fn transport_type(&self) -> TcpProxyEntryTransportType {
        TcpProxyEntryTransportType::Tcp
    }
}

type NatDstEntryState = TcpProxyEntryState;

#[derive(Debug)]
pub struct NatDstEntry {
    id: uuid::Uuid,
    src: SocketAddr,
    real_dst: SocketAddr,
    mapped_dst: SocketAddr,
    start_time: Instant,
    start_time_local: chrono::DateTime<chrono::Local>,
    tasks: Mutex<JoinSet<()>>,
    state: AtomicCell<NatDstEntryState>,
    initial_sequence: Option<u32>,
    claimed_stream: Mutex<Option<ClaimedNatDstStream>>,
}

impl NatDstEntry {
    pub fn new(
        src: SocketAddr,
        real_dst: SocketAddr,
        mapped_dst: SocketAddr,
        initial_sequence: Option<u32>,
        claimed_stream: Option<ClaimedNatDstStream>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            src,
            real_dst,
            mapped_dst,
            start_time: Instant::now(),
            start_time_local: chrono::Local::now(),
            tasks: Mutex::new(JoinSet::new()),
            state: AtomicCell::new(NatDstEntryState::SynReceived),
            initial_sequence,
            claimed_stream: Mutex::new(claimed_stream),
        }
    }

    fn parse_as_pb(&self, transport_type: TcpProxyEntryTransportType) -> TcpProxyEntry {
        TcpProxyEntry {
            src: Some(self.src.into()),
            dst: Some(self.real_dst.into()),
            start_time: self.start_time_local.timestamp() as u64,
            state: self.state.load().into(),
            transport_type: transport_type.into(),
            ..Default::default()
        }
    }
}

enum ProxyTcpStream {
    KernelTcpStream(TcpStream),
    #[cfg(feature = "smoltcp")]
    SmolTcpStream(tokio_smoltcp::TcpStream),
}

impl ProxyTcpStream {
    pub fn set_nodelay(&self, nodelay: bool) -> Result<()> {
        match self {
            Self::KernelTcpStream(stream) => stream.set_nodelay(nodelay).map_err(Into::into),
            #[cfg(feature = "smoltcp")]
            Self::SmolTcpStream(_stream) => {
                tracing::warn!("smol tcp stream set_nodelay not implemented");
                Ok(())
            }
        }
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        match self {
            Self::KernelTcpStream(stream) => {
                stream.shutdown().await?;
                Ok(())
            }
            #[cfg(feature = "smoltcp")]
            Self::SmolTcpStream(stream) => {
                stream.shutdown().await?;
                Ok(())
            }
        }
    }

    pub async fn copy_bidirectional<D: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        dst: &mut D,
    ) -> Result<()> {
        match self {
            Self::KernelTcpStream(stream) => {
                copy_bidirectional(stream, dst).await?;
                Ok(())
            }
            #[cfg(feature = "smoltcp")]
            Self::SmolTcpStream(stream) => {
                copy_bidirectional(stream, dst).await?;
                Ok(())
            }
        }
    }
}

#[cfg(feature = "smoltcp")]
type SmolTcpAcceptResult = Result<(tokio_smoltcp::TcpStream, SocketAddr)>;
#[cfg(feature = "smoltcp")]
struct SmolTcpListener {
    stream_tx: mpsc::UnboundedSender<SmolTcpAcceptResult>,
    stream_rx: mpsc::UnboundedReceiver<SmolTcpAcceptResult>,

    tasks: Arc<std::sync::Mutex<JoinSet<()>>>,
}

#[cfg(feature = "smoltcp")]
impl SmolTcpListener {
    pub async fn new() -> Self {
        let tasks = Arc::new(std::sync::Mutex::new(JoinSet::new()));
        join_joinset_background(tasks.clone(), "smoltcp listener".to_owned());

        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            stream_tx: tx,
            stream_rx: rx,
            tasks,
        }
    }

    pub async fn accept(&mut self) -> SmolTcpAcceptResult {
        self.stream_rx.recv().await.unwrap()
    }

    pub fn stream_tx(&self) -> mpsc::UnboundedSender<SmolTcpAcceptResult> {
        self.stream_tx.clone()
    }

    pub async fn add_listener(
        tx: mpsc::UnboundedSender<SmolTcpAcceptResult>,
        net: Arc<Mutex<Option<Net>>>,
        tasks: Arc<std::sync::Mutex<JoinSet<()>>>,
    ) {
        let locked_net = net.lock().await;
        let mut tcp = locked_net
            .as_ref()
            .unwrap()
            .tcp_bind("0.0.0.0:8899".parse().unwrap())
            .await
            .unwrap();
        tasks.lock().unwrap().spawn(async move {
            let ret = timeout(Duration::from_secs(10), tcp.accept()).await;
            if let Ok(accept_ret) = ret {
                tx.send(accept_ret.map_err(|e| {
                    anyhow::anyhow!("smol tcp listener accept failed: {:?}", e).into()
                }))
                .unwrap();
            } else {
                tracing::error!("smol tcp listener accept timeout");
            }
        });
    }
}

enum ProxyTcpListener {
    KernelTcpListener(TcpListener),
    #[cfg(feature = "smoltcp")]
    SmolTcpListener(SmolTcpListener),
}

fn prepare_kernel_tcp_socket(stream: &TcpStream) -> Result<()> {
    const TCP_KEEPALIVE_TIME: Duration = Duration::from_secs(5);
    const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
    const TCP_KEEPALIVE_RETRIES: u32 = 2;

    let ka = TcpKeepalive::new()
        .with_time(TCP_KEEPALIVE_TIME)
        .with_interval(TCP_KEEPALIVE_INTERVAL);

    #[cfg(not(target_os = "windows"))]
    let ka = ka.with_retries(TCP_KEEPALIVE_RETRIES);

    let sf = SockRef::from(&stream);
    sf.set_tcp_keepalive(&ka)?;
    if let Err(e) = sf.set_nodelay(true) {
        tracing::warn!("set_nodelay failed, ignore it: {:?}", e);
    }

    Ok(())
}

impl ProxyTcpListener {
    pub async fn accept(&mut self) -> Result<(ProxyTcpStream, SocketAddr)> {
        match self {
            Self::KernelTcpListener(listener) => {
                let (stream, addr) = listener.accept().await?;
                prepare_kernel_tcp_socket(&stream)?;
                Ok((ProxyTcpStream::KernelTcpStream(stream), addr))
            }
            #[cfg(feature = "smoltcp")]
            Self::SmolTcpListener(listener) => {
                let Ok((stream, src)) = listener.accept().await else {
                    return Err(anyhow::anyhow!("smol tcp listener closed").into());
                };
                tracing::info!(?src, "smol tcp listener accepted");
                Ok((ProxyTcpStream::SmolTcpStream(stream), src))
            }
        }
    }
}

type ArcNatDstEntry = Arc<NatDstEntry>;

type SynSockMap = Arc<DashMap<SocketAddr, ArcNatDstEntry>>;
type ConnSockMap = Arc<DashMap<uuid::Uuid, ArcNatDstEntry>>;
// peer src addr to nat entry, when respond tcp packet, should modify the tcp src addr to the nat entry's dst addr
type AddrConnSockMap = Arc<DashMap<SocketAddr, ArcNatDstEntry>>;

#[derive(Debug)]
pub struct TcpProxy<C: NatDstConnector> {
    global_ctx: Arc<GlobalCtx>,
    peer_manager: Weak<PeerManager>,
    local_port: AtomicU16,

    tasks: Arc<std::sync::Mutex<JoinSet<()>>>,

    syn_map: SynSockMap,
    conn_map: ConnSockMap,
    addr_conn_map: AddrConnSockMap,

    cidr_set: CidrSet,

    smoltcp_stack_sender: Option<mpsc::Sender<ZCPacket>>,
    smoltcp_stack_receiver: Arc<Mutex<Option<mpsc::Receiver<ZCPacket>>>>,
    #[cfg(feature = "smoltcp")]
    smoltcp_net: Arc<Mutex<Option<Net>>>,
    #[cfg(feature = "smoltcp")]
    smoltcp_listener_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<SmolTcpAcceptResult>>>,
    enable_smoltcp: Arc<AtomicBool>,

    connector: C,
}

#[async_trait::async_trait]
impl<C: NatDstConnector> PeerPacketFilter for TcpProxy<C> {
    async fn try_process_packet_from_peer(&self, mut packet: ZCPacket) -> Option<ZCPacket> {
        match self.try_handle_peer_packet(&mut packet).await {
            Some(true) => {
                if self.is_smoltcp_enabled() {
                    let smoltcp_stack_sender = self.smoltcp_stack_sender.as_ref().unwrap();
                    if let Err(e) = smoltcp_stack_sender.try_send(packet) {
                        tracing::error!("send to smoltcp stack failed: {:?}", e);
                    }
                } else if let Some(peer_manager) = self.get_peer_manager()
                    && let Err(e) = peer_manager.get_nic_channel().send(packet).await
                {
                    tracing::error!("send to nic failed: {:?}", e);
                }
                None
            }
            Some(false) => None,
            None => Some(packet),
        }
    }
}

#[async_trait::async_trait]
impl<C: NatDstConnector> NicPacketFilter for TcpProxy<C> {
    async fn try_process_packet_from_nic(
        &self,
        zc_packet: &mut ZCPacket,
        _context: &crate::peers::NicPacketContext,
    ) -> crate::peers::NicPacketFilterAction {
        if zc_packet.bypass_proxy_interception() {
            return crate::peers::NicPacketFilterAction::Continue;
        }
        let Some(my_ipv4_inet) = self.get_local_inet() else {
            return crate::peers::NicPacketFilterAction::Continue;
        };
        let my_ipv4 = my_ipv4_inet.address();

        let data = zc_packet.payload();
        let ip_packet = Ipv4Packet::new(data).unwrap();
        if ip_packet.get_version() != 4
            || ip_packet.get_source() != my_ipv4
            || ip_packet.get_next_level_protocol() != IpNextHeaderProtocols::Tcp
        {
            return crate::peers::NicPacketFilterAction::Continue;
        }

        let tcp_packet = TcpPacket::new(ip_packet.payload()).unwrap();
        if tcp_packet.get_source() != self.get_local_port() {
            return crate::peers::NicPacketFilterAction::Continue;
        }

        let observed_dst = SocketAddr::V4(SocketAddrV4::new(
            ip_packet.get_destination(),
            tcp_packet.get_destination(),
        ));
        let Some((nat_entry, dst_addr, need_transform_dst)) = Self::find_nat_entry_for_nic(
            &self.addr_conn_map,
            &self.syn_map,
            observed_dst,
            &my_ipv4_inet,
            !self.is_smoltcp_enabled(),
        ) else {
            return crate::peers::NicPacketFilterAction::Continue;
        };
        tracing::trace!(?observed_dst, ?dst_addr, "tcp packet found nat entry");
        assert_eq!(nat_entry.src, dst_addr);

        let IpAddr::V4(ip) = nat_entry.mapped_dst.ip() else {
            panic!("v4 nat entry src ip is not v4");
        };

        zc_packet
            .mut_peer_manager_header()
            .unwrap()
            .set_no_proxy(true);
        if need_transform_dst {
            zc_packet.mut_peer_manager_header().unwrap().to_peer_id = self.get_my_peer_id().into();
        }

        let mut ip_packet = MutableIpv4Packet::new(zc_packet.mut_payload()).unwrap();
        ip_packet.set_source(ip);
        if need_transform_dst {
            ip_packet.set_destination(my_ipv4);
        }
        let dst = ip_packet.get_destination();

        let mut tcp_packet = MutableTcpPacket::new(ip_packet.payload_mut()).unwrap();
        tcp_packet.set_source(nat_entry.real_dst.port());

        Self::update_tcp_packet_checksum(&mut tcp_packet, &ip, &dst);
        drop(tcp_packet);
        Self::update_ip_packet_checksum(&mut ip_packet);

        tracing::trace!(dst_addr = ?dst_addr, nat_entry = ?nat_entry, packet = ?ip_packet, "tcp packet after modified");

        crate::peers::NicPacketFilterAction::StopAndSend
    }
}

impl<C: NatDstConnector> TcpProxy<C> {
    fn find_syn_entry_for_accept(
        syn_map: &SynSockMap,
        observed_src: SocketAddr,
        my_ipv4_inet: Option<&Ipv4Inet>,
    ) -> Option<(ArcNatDstEntry, SocketAddr)> {
        if let Some(entry) = syn_map.get(&observed_src) {
            return Some((entry.clone(), observed_src));
        }

        let my_ipv4_inet = my_ipv4_inet?;
        if observed_src.ip() != Self::get_fake_local_ipv4(my_ipv4_inet) {
            return None;
        }

        let mut translated_src = observed_src;
        translated_src.set_ip(IpAddr::V4(my_ipv4_inet.address()));
        syn_map
            .get(&translated_src)
            .map(|entry| (entry.clone(), translated_src))
    }

    fn find_nat_entry_for_nic(
        addr_conn_map: &AddrConnSockMap,
        syn_map: &SynSockMap,
        observed_dst: SocketAddr,
        my_ipv4_inet: &Ipv4Inet,
        allow_fake_local_fallback: bool,
    ) -> Option<(ArcNatDstEntry, SocketAddr, bool)> {
        let find_exact = |addr: &SocketAddr| {
            addr_conn_map
                .get(addr)
                .map(|entry| entry.clone())
                .or_else(|| syn_map.get(addr).map(|entry| entry.clone()))
        };

        if let Some(entry) = find_exact(&observed_dst) {
            return Some((entry, observed_dst, false));
        }

        if !allow_fake_local_fallback
            || observed_dst.ip() != Self::get_fake_local_ipv4(my_ipv4_inet)
        {
            return None;
        }

        let mut translated_dst = observed_dst;
        translated_dst.set_ip(IpAddr::V4(my_ipv4_inet.address()));
        find_exact(&translated_dst).map(|entry| (entry, translated_dst, true))
    }

    pub fn new(peer_manager: Arc<PeerManager>, connector: C) -> Arc<Self> {
        let (smoltcp_stack_sender, smoltcp_stack_receiver) = mpsc::channel::<ZCPacket>(1000);
        let global_ctx = peer_manager.get_global_ctx();

        Arc::new(Self {
            global_ctx: global_ctx.clone(),
            peer_manager: Arc::downgrade(&peer_manager),

            local_port: AtomicU16::new(0),
            tasks: Arc::new(std::sync::Mutex::new(JoinSet::new())),

            syn_map: Arc::new(DashMap::new()),
            conn_map: Arc::new(DashMap::new()),
            addr_conn_map: Arc::new(DashMap::new()),

            cidr_set: CidrSet::new(global_ctx),

            smoltcp_stack_sender: Some(smoltcp_stack_sender),
            smoltcp_stack_receiver: Arc::new(Mutex::new(Some(smoltcp_stack_receiver))),

            #[cfg(feature = "smoltcp")]
            smoltcp_net: Arc::new(Mutex::new(None)),
            #[cfg(feature = "smoltcp")]
            smoltcp_listener_tx: std::sync::Mutex::new(None),

            enable_smoltcp: Arc::new(AtomicBool::new(true)),

            connector,
        })
    }

    pub fn get_peer_manager(&self) -> Option<Arc<PeerManager>> {
        self.peer_manager.upgrade()
    }

    fn update_tcp_packet_checksum(
        tcp_packet: &mut MutableTcpPacket,
        ipv4_src: &Ipv4Addr,
        ipv4_dst: &Ipv4Addr,
    ) {
        tcp_packet.set_checksum(ipv4_checksum(
            &tcp_packet.to_immutable(),
            ipv4_src,
            ipv4_dst,
        ));
    }

    fn update_ip_packet_checksum(ip_packet: &mut MutableIpv4Packet) {
        ip_packet.set_checksum(pnet::packet::ipv4::checksum(&ip_packet.to_immutable()));
    }

    pub async fn start(self: &Arc<Self>, add_pipeline: bool) -> Result<()> {
        self.run_syn_map_cleaner().await?;
        self.run_listener().await?;
        if add_pipeline {
            let peer_manager = self
                .get_peer_manager()
                .ok_or_else(|| anyhow::anyhow!("peer manager is gone"))?;
            peer_manager
                .add_packet_process_pipeline(Box::new(self.clone()))
                .await;
            peer_manager
                .add_nic_packet_process_pipeline(Box::new(self.clone()))
                .await;
        }
        join_joinset_background(self.tasks.clone(), "TcpProxy".to_owned());

        Ok(())
    }

    async fn run_syn_map_cleaner(&self) -> Result<()> {
        let syn_map = self.syn_map.clone();
        let tasks = self.tasks.clone();
        let syn_map_cleaner_task = async move {
            loop {
                syn_map.retain(|_, entry| {
                    if entry.start_time.elapsed() > Duration::from_secs(30) {
                        tracing::warn!(entry = ?entry, "syn nat entry expired");
                        entry.state.store(NatDstEntryState::Closed);
                        false
                    } else {
                        true
                    }
                });
                syn_map.shrink_to_fit();
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        };
        tasks.lock().unwrap().spawn(syn_map_cleaner_task);

        Ok(())
    }

    async fn get_proxy_listener(&self) -> Result<ProxyTcpListener> {
        #[cfg(feature = "smoltcp")]
        if self.global_ctx.get_flags().use_smoltcp
            || self.global_ctx.no_tun()
            || cfg!(any(
                target_os = "android",
                any(
                    target_os = "ios",
                    all(target_os = "macos", feature = "macos-ne")
                ),
                target_env = "ohos"
            ))
        {
            // use smoltcp network stack

            use crate::gateway::tokio_smoltcp::BufferSize;
            self.local_port
                .store(8899, std::sync::atomic::Ordering::Relaxed);

            let mut cap = smoltcp::phy::DeviceCapabilities::default();
            cap.max_transmission_unit = 1280;
            cap.medium = smoltcp::phy::Medium::Ip;
            let (dev, stack_sink, mut stack_stream) = channel_device::ChannelDevice::new(cap);

            let mut smoltcp_stack_receiver =
                self.smoltcp_stack_receiver.lock().await.take().unwrap();
            self.tasks.lock().unwrap().spawn(async move {
                while let Some(packet) = smoltcp_stack_receiver.recv().await {
                    tracing::trace!(?packet, "receive from peer send to smoltcp packet");
                    if let Err(e) = stack_sink.send(Ok(packet.payload().to_vec())).await {
                        tracing::error!("send to smoltcp stack failed: {:?}", e);
                    }
                }
                tracing::error!("smoltcp stack sink exited");
            });

            let peer_mgr = self.peer_manager.clone();
            self.tasks.lock().unwrap().spawn(async move {
                while let Some(data) = stack_stream.recv().await {
                    tracing::trace!(
                        ?data,
                        "receive from smoltcp stack and send to peer mgr packet"
                    );
                    let Some(ipv4) = Ipv4Packet::new(&data) else {
                        tracing::error!(?data, "smoltcp stack stream get non ipv4 packet");
                        continue;
                    };

                    let dst = ipv4.get_destination();
                    let packet = ZCPacket::new_with_payload(&data);
                    let Some(peer_mgr) = peer_mgr.upgrade() else {
                        tracing::warn!("peer manager is gone, smoltcp sender exited");
                        return;
                    };
                    if let Err(e) = peer_mgr
                        .send_msg_by_ip(packet, IpAddr::V4(dst), false)
                        .await
                    {
                        tracing::error!("send to peer failed in smoltcp sender: {:?}", e);
                    }
                }
                tracing::error!("smoltcp stack stream exited");
            });

            let interface_config = smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ip);
            let net = Net::new(
                dev,
                NetConfig::new(
                    interface_config,
                    format!("{}/24", self.get_local_ip().unwrap())
                        .parse()
                        .unwrap(),
                    vec![format!("{}", self.get_local_ip().unwrap()).parse().unwrap()],
                    Some(BufferSize {
                        tcp_rx_size: 1024 * 16,
                        tcp_tx_size: 1024 * 16,
                        ..Default::default()
                    }),
                ),
            );
            net.set_any_ip(true);
            self.smoltcp_net.lock().await.replace(net);
            let tcp = SmolTcpListener::new().await;
            self.smoltcp_listener_tx
                .lock()
                .unwrap()
                .replace(tcp.stream_tx());

            self.enable_smoltcp
                .store(true, std::sync::atomic::Ordering::Relaxed);

            return Ok(ProxyTcpListener::SmolTcpListener(tcp));
        }

        {
            // use kernel network stack
            let listen_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
            let net_ns = self.global_ctx.net_ns.clone();
            let tcp_listener = net_ns
                .run_async(|| async { TcpListener::bind(&listen_addr).await })
                .await?;
            self.local_port.store(
                tcp_listener.local_addr()?.port(),
                std::sync::atomic::Ordering::Relaxed,
            );

            self.enable_smoltcp
                .store(false, std::sync::atomic::Ordering::Relaxed);

            Ok(ProxyTcpListener::KernelTcpListener(tcp_listener))
        }
    }

    async fn run_listener(&self) -> Result<()> {
        // bind on both v4 & v6
        let mut tcp_listener = self.get_proxy_listener().await?;

        let global_ctx = self.global_ctx.clone();
        let tasks = Arc::downgrade(&self.tasks);
        let syn_map = self.syn_map.clone();
        let conn_map = self.conn_map.clone();
        let addr_conn_map = self.addr_conn_map.clone();
        let connector = self.connector.clone();
        let accept_task = async move {
            let conn_map = conn_map.clone();
            loop {
                let accept_ret = tcp_listener.accept().await;
                let Ok((tcp_stream, mut socket_addr)) = accept_ret else {
                    tracing::error!("nat tcp listener accept failed: {:?}", accept_ret.err());
                    continue;
                };

                let my_ip_inet = global_ctx.get_ipv4();
                let my_ip = my_ip_inet
                    .as_ref()
                    .map(Ipv4Inet::address)
                    .unwrap_or(Ipv4Addr::UNSPECIFIED);

                let observed_src = socket_addr;
                let Some((entry, lookup_src)) =
                    Self::find_syn_entry_for_accept(&syn_map, observed_src, my_ip_inet.as_ref())
                else {
                    tracing::error!(
                        ?my_ip,
                        ?observed_src,
                        "tcp connection from unknown source, ignore it"
                    );
                    continue;
                };
                socket_addr = lookup_src;
                tracing::info!(
                    ?observed_src,
                    ?socket_addr,
                    "tcp connection accepted for proxy, nat dst: {:?}",
                    entry.real_dst
                );
                assert_eq!(entry.state.load(), NatDstEntryState::SynReceived);

                let entry_clone = entry.clone();
                drop(entry);
                entry_clone.state.store(NatDstEntryState::ConnectingDst);

                let _ = addr_conn_map.insert(entry_clone.src, entry_clone.clone());
                let old_nat_val = conn_map.insert(entry_clone.id, entry_clone.clone());
                assert!(old_nat_val.is_none());
                syn_map.remove_if(&socket_addr, |_, entry| entry.id == entry_clone.id);

                let Some(tasks) = tasks.upgrade() else {
                    tracing::error!("tcp proxy tasks is dropped, exit accept loop");
                    break;
                };

                tasks.lock().unwrap().spawn(Self::connect_to_nat_dst(
                    connector.clone(),
                    global_ctx.clone(),
                    tcp_stream,
                    conn_map.clone(),
                    addr_conn_map.clone(),
                    entry_clone,
                ));
            }
        };
        self.tasks
            .lock()
            .unwrap()
            .spawn(accept_task.instrument(tracing::info_span!("tcp_proxy_listener")));

        Ok(())
    }

    fn remove_entry_from_all_conn_map(
        conn_map: ConnSockMap,
        addr_conn_map: AddrConnSockMap,
        nat_entry: ArcNatDstEntry,
    ) {
        conn_map.remove(&nat_entry.id);
        addr_conn_map.remove_if(&nat_entry.src, |_, entry| entry.id == nat_entry.id);
        if conn_map.capacity() - conn_map.len() > 16 {
            conn_map.shrink_to_fit();
        }
        if addr_conn_map.capacity() - addr_conn_map.len() > 16 {
            addr_conn_map.shrink_to_fit();
        }
    }

    async fn connect_to_nat_dst(
        connector: C,
        global_ctx: ArcGlobalCtx,
        src_tcp_stream: ProxyTcpStream,
        conn_map: ConnSockMap,
        addr_conn_map: AddrConnSockMap,
        nat_entry: ArcNatDstEntry,
    ) {
        if let Err(e) = src_tcp_stream.set_nodelay(true) {
            tracing::warn!("set_nodelay failed, ignore it: {:?}", e);
        }

        if global_ctx.should_deny_proxy(&nat_entry.real_dst, false) {
            tracing::error!(
                ?nat_entry,
                "nat dst port {} is in running listeners, ignore it",
                nat_entry.real_dst.port()
            );
            nat_entry.state.store(NatDstEntryState::Closed);
            Self::remove_entry_from_all_conn_map(conn_map, addr_conn_map, nat_entry);
            return;
        }

        let nat_dst = if global_ctx.is_ip_local_virtual_ip(&nat_entry.real_dst.ip()) {
            format!("127.0.0.1:{}", nat_entry.real_dst.port())
                .parse()
                .unwrap()
        } else {
            nat_entry.real_dst
        };

        global_ctx
            .stats_manager()
            .get_counter(
                MetricName::TcpProxyConnect,
                LabelSet::new()
                    .with_label_type(LabelType::Protocol(
                        connector.transport_type().as_str_name().to_string(),
                    ))
                    .with_label_type(LabelType::DstIp(nat_dst.ip().to_string()))
                    .with_label_type(LabelType::MappedDstIp(
                        nat_entry.mapped_dst.ip().to_string(),
                    )),
            )
            .inc();

        let claimed_stream = nat_entry.claimed_stream.lock().await.take();
        let _guard = global_ctx.net_ns.guard();
        let Ok(dst_tcp_stream) = connector
            .connect(nat_entry.src, nat_dst, claimed_stream)
            .await
        else {
            tracing::error!("connect to dst failed: {:?}", nat_entry);
            nat_entry.state.store(NatDstEntryState::Closed);
            Self::remove_entry_from_all_conn_map(conn_map, addr_conn_map, nat_entry);
            return;
        };
        drop(_guard);

        tracing::info!(?nat_entry, ?nat_dst, "tcp connection to dst established");

        assert_eq!(nat_entry.state.load(), NatDstEntryState::ConnectingDst);
        nat_entry.state.store(NatDstEntryState::Connected);

        Self::handle_nat_connection(
            connector,
            src_tcp_stream,
            dst_tcp_stream,
            conn_map,
            addr_conn_map,
            nat_entry,
        )
        .await;
    }

    async fn handle_nat_connection(
        connector: C,
        mut src_tcp_stream: ProxyTcpStream,
        mut dst_tcp_stream: C::DstStream,
        conn_map: ConnSockMap,
        addr_conn_map: AddrConnSockMap,
        nat_entry: ArcNatDstEntry,
    ) {
        let nat_entry_clone = nat_entry.clone();
        nat_entry.tasks.lock().await.spawn(async move {
            let ret = src_tcp_stream.copy_bidirectional(&mut dst_tcp_stream).await;
            tracing::info!(nat_entry = ?nat_entry_clone, ret = ?ret, "nat tcp connection closed");

            nat_entry_clone.state.store(NatDstEntryState::ClosingSrc);
            let ret = timeout(Duration::from_secs(10), src_tcp_stream.shutdown()).await;
            tracing::info!(nat_entry = ?nat_entry_clone, ret = ?ret, "src tcp stream shutdown");

            nat_entry_clone.state.store(NatDstEntryState::ClosingDst);
            let ret = timeout(
                Duration::from_secs(10),
                connector.shutdown(&mut dst_tcp_stream),
            )
            .await;
            tracing::info!(nat_entry = ?nat_entry_clone, ret = ?ret, "dst tcp stream shutdown");

            drop(src_tcp_stream);
            drop(dst_tcp_stream);

            nat_entry_clone.state.store(NatDstEntryState::Closed);
            // sleep later so the fin packet can be processed
            tokio::time::sleep(Duration::from_secs(10)).await;

            Self::remove_entry_from_all_conn_map(conn_map, addr_conn_map, nat_entry_clone);
        });
    }

    pub fn get_local_port(&self) -> u16 {
        self.local_port.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_my_peer_id(&self) -> u32 {
        self.peer_manager
            .upgrade()
            .map(|pm| pm.my_peer_id())
            .unwrap_or_default()
    }

    pub fn get_local_ip(&self) -> Option<Ipv4Addr> {
        self.get_local_inet().map(|inet| inet.address())
    }

    pub fn get_local_inet(&self) -> Option<Ipv4Inet> {
        if self.is_smoltcp_enabled() {
            Some(Ipv4Inet::new(Ipv4Addr::new(192, 88, 99, 254), 24).unwrap())
        } else {
            self.global_ctx.get_ipv4().as_ref().cloned()
        }
    }

    pub fn get_global_ctx(&self) -> &ArcGlobalCtx {
        &self.global_ctx
    }

    pub fn is_smoltcp_enabled(&self) -> bool {
        self.enable_smoltcp
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_fake_local_ipv4(local_ip: &Ipv4Inet) -> Ipv4Addr {
        let first = u32::from(local_ip.first_address());
        let last = u32::from(local_ip.last_address());
        let local = u32::from(local_ip.address());

        match last.saturating_sub(first) {
            0 => local_ip.address(),
            1 => Ipv4Addr::from(if local == first { last } else { first }),
            _ => {
                let first_host = first + 1;
                let fake = if first_host == local {
                    first_host + 1
                } else {
                    first_host
                };
                Ipv4Addr::from(fake)
            }
        }
    }

    async fn try_handle_peer_packet(&self, packet: &mut ZCPacket) -> Option<bool> {
        if !self
            .connector
            .check_packet_from_peer_fast(&self.cidr_set, &self.global_ctx)
        {
            return None;
        }

        let ipv4_inet = self.get_local_inet()?;
        let ipv4_addr = ipv4_inet.address();
        let deferred_proxy = packet
            .peer_manager_header()
            .is_some_and(PeerManagerHeader::is_deferred_proxy);
        {
            let hdr = packet.peer_manager_header().unwrap();
            if (hdr.packet_type != PacketType::Data as u8
                && hdr.packet_type != PacketType::DataWithKcpSrcModified as u8
                && hdr.packet_type != PacketType::DataWithQuicSrcModified as u8)
                || hdr.is_no_proxy()
            {
                return None;
            };
        }

        let origin_ip = {
            let payload_bytes = packet.mut_payload();
            let ipv4 = Ipv4Packet::new(payload_bytes)?;
            if ipv4.get_version() != 4
                || ipv4.get_next_level_protocol() != IpNextHeaderProtocols::Tcp
            {
                return None;
            }

            ipv4.get_destination()
        };
        let mut real_dst_ip = origin_ip;
        let hdr = packet.mut_peer_manager_header().unwrap();

        if !self.connector.check_packet_from_peer(
            &self.cidr_set,
            &self.global_ctx,
            hdr,
            &origin_ip,
            &mut real_dst_ip,
        ) {
            return None;
        }

        tracing::trace!(ipv4 = ?origin_ip, cidr_set = ?self.cidr_set, "proxy tcp packet received");

        let payload_bytes = packet.mut_payload();
        let ip_packet = Ipv4Packet::new(payload_bytes).unwrap();
        let tcp_packet = TcpPacket::new(ip_packet.payload()).unwrap();

        let source_ip = ip_packet.get_source();
        let source_port = tcp_packet.get_source();
        let src = SocketAddr::V4(SocketAddrV4::new(source_ip, source_port));

        let is_tcp_syn = tcp_packet.get_flags() & pnet::packet::tcp::TcpFlags::SYN != 0;
        let is_tcp_ack = tcp_packet.get_flags() & pnet::packet::tcp::TcpFlags::ACK != 0;
        if is_tcp_syn && !is_tcp_ack {
            let initial_sequence = tcp_packet.get_sequence();
            let dest_ip = ip_packet.get_destination();
            let dest_port = tcp_packet.get_destination();
            let mapped_dst = SocketAddr::V4(SocketAddrV4::new(dest_ip, dest_port));
            let real_dst = SocketAddr::V4(SocketAddrV4::new(real_dst_ip, dest_port));

            let is_existing_deferred_flow = deferred_proxy
                && (self.addr_conn_map.contains_key(&src)
                    || self.syn_map.get(&src).is_some_and(|entry| {
                        entry.initial_sequence == Some(initial_sequence)
                            && entry.mapped_dst == mapped_dst
                    }));
            if !is_existing_deferred_flow {
                let claimed_stream = if deferred_proxy {
                    let Some(claimed) =
                        self.connector
                            .claim_deferred_stream(src, mapped_dst, initial_sequence)
                    else {
                        tracing::warn!(
                            ?src,
                            ?mapped_dst,
                            initial_sequence,
                            "drop stale deferred proxy SYN"
                        );
                        return Some(false);
                    };
                    Some(claimed)
                } else {
                    None
                };
                let old_val = self.syn_map.insert(
                    src,
                    Arc::new(NatDstEntry::new(
                        src,
                        real_dst,
                        mapped_dst,
                        deferred_proxy.then_some(initial_sequence),
                        claimed_stream,
                    )),
                );
                tracing::info!(src = ?src, ?real_dst, ?mapped_dst, old_entry = ?old_val, "tcp syn received");
            }

            // if smoltcp is enabled, add the listener to the net
            #[cfg(feature = "smoltcp")]
            if self.is_smoltcp_enabled() {
                let smoltcp_listener_tx = self.smoltcp_listener_tx.lock().unwrap().clone().unwrap();
                SmolTcpListener::add_listener(
                    smoltcp_listener_tx,
                    self.smoltcp_net.clone(),
                    self.tasks.clone(),
                )
                .await;
                tracing::info!("smol tcp listener added for src: {:?}", src);
            }
        } else if !self.addr_conn_map.contains_key(&src) && !self.syn_map.contains_key(&src) {
            // if not in syn map and addr conn map, may forwarding n2n packet
            return None;
        }

        drop(tcp_packet);
        drop(ip_packet);
        let _ = payload_bytes;
        let hdr = packet.mut_peer_manager_header().unwrap();
        hdr.packet_type = PacketType::Data as u8;
        hdr.set_deferred_proxy(false);

        let payload_bytes = packet.mut_payload();
        let mut ip_packet = MutableIpv4Packet::new(payload_bytes).unwrap();
        if !self.is_smoltcp_enabled() && source_ip == ipv4_addr {
            // modify the source so the response packet can be handled by tun device
            ip_packet.set_source(Self::get_fake_local_ipv4(&ipv4_inet));
        }
        ip_packet.set_destination(ipv4_addr);
        let source = ip_packet.get_source();

        let mut tcp_packet = MutableTcpPacket::new(ip_packet.payload_mut()).unwrap();
        tcp_packet.set_destination(self.get_local_port());

        Self::update_tcp_packet_checksum(&mut tcp_packet, &source, &ipv4_addr);
        drop(tcp_packet);
        Self::update_ip_packet_checksum(&mut ip_packet);

        tracing::trace!(?source, ?ipv4_addr, ?packet, "tcp packet after modified");

        Some(true)
    }

    pub fn is_tcp_proxy_connection(&self, src: SocketAddr) -> bool {
        self.syn_map.contains_key(&src) || self.addr_conn_map.contains_key(&src)
    }

    pub fn list_proxy_entries(&self) -> Vec<TcpProxyEntry> {
        let mut entries: Vec<TcpProxyEntry> = Vec::new();
        let transport_type = self.connector.transport_type();
        for entry in self.syn_map.iter() {
            entries.push(entry.value().as_ref().parse_as_pb(transport_type));
        }
        for entry in self.conn_map.iter() {
            entries.push(entry.value().as_ref().parse_as_pb(transport_type));
        }
        entries
    }

    pub fn get_transport_type(&self) -> TcpProxyEntryTransportType {
        self.connector.transport_type()
    }

    pub(crate) fn get_connector(&self) -> C {
        self.connector.clone()
    }
}

#[derive(Clone)]
pub struct TcpProxyRpcService<C: NatDstConnector> {
    tcp_proxy: Weak<TcpProxy<C>>,
}

#[async_trait::async_trait]
impl<C: NatDstConnector> TcpProxyRpc for TcpProxyRpcService<C> {
    type Controller = BaseController;
    async fn list_tcp_proxy_entry(
        &self,
        _: BaseController,
        _request: ListTcpProxyEntryRequest, // Accept request of type HelloRequest
    ) -> std::result::Result<ListTcpProxyEntryResponse, rpc_types::error::Error> {
        let mut reply = ListTcpProxyEntryResponse::default();
        if let Some(tcp_proxy) = self.tcp_proxy.upgrade() {
            reply.entries = tcp_proxy.list_proxy_entries();
        }
        Ok(reply)
    }
}

impl<C: NatDstConnector> TcpProxyRpcService<C> {
    pub fn new(tcp_proxy: Arc<TcpProxy<C>>) -> Self {
        Self {
            tcp_proxy: Arc::downgrade(&tcp_proxy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_local_ipv4_uses_unicast_host_address() {
        let local: Ipv4Inet = "10.144.144.1/24".parse().unwrap();
        let fake = TcpProxy::<NatDstTcpConnector>::get_fake_local_ipv4(&local);

        assert_eq!(fake, Ipv4Addr::new(10, 144, 144, 2));
        assert_ne!(fake, local.first_address());
        assert_ne!(fake, local.last_address());
        assert_ne!(fake, local.address());
    }

    #[test]
    fn fake_local_ipv4_handles_small_subnets() {
        let slash_30: Ipv4Inet = "10.0.0.1/30".parse().unwrap();
        assert_eq!(
            TcpProxy::<NatDstTcpConnector>::get_fake_local_ipv4(&slash_30),
            Ipv4Addr::new(10, 0, 0, 2)
        );

        let slash_31: Ipv4Inet = "10.0.0.0/31".parse().unwrap();
        assert_eq!(
            TcpProxy::<NatDstTcpConnector>::get_fake_local_ipv4(&slash_31),
            Ipv4Addr::new(10, 0, 0, 1)
        );

        let slash_32: Ipv4Inet = "10.0.0.1/32".parse().unwrap();
        assert_eq!(
            TcpProxy::<NatDstTcpConnector>::get_fake_local_ipv4(&slash_32),
            slash_32.address()
        );
    }

    #[test]
    fn native_nat_entry_wins_over_fake_local_fallback() {
        let local: Ipv4Inet = "10.144.144.3/24".parse().unwrap();
        let observed_dst: SocketAddr = "10.144.144.1:28761".parse().unwrap();
        assert_eq!(
            observed_dst.ip(),
            TcpProxy::<NatDstTcpConnector>::get_fake_local_ipv4(&local)
        );

        let syn_map = Arc::new(DashMap::new());
        let addr_conn_map = Arc::new(DashMap::new());
        let native_entry = Arc::new(NatDstEntry::new(
            observed_dst,
            "10.1.2.4:23457".parse().unwrap(),
            "10.1.2.4:23457".parse().unwrap(),
            None,
            None,
        ));
        let wrapped_src = SocketAddr::new(IpAddr::V4(local.address()), observed_dst.port());
        let wrapped_entry = Arc::new(NatDstEntry::new(
            wrapped_src,
            "10.1.2.4:23457".parse().unwrap(),
            "10.1.2.4:23457".parse().unwrap(),
            None,
            None,
        ));
        syn_map.insert(observed_dst, native_entry.clone());
        syn_map.insert(wrapped_src, wrapped_entry.clone());

        let (entry, key, transformed) = TcpProxy::<NatDstTcpConnector>::find_nat_entry_for_nic(
            &addr_conn_map,
            &syn_map,
            observed_dst,
            &local,
            true,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&entry, &native_entry));
        assert_eq!(key, observed_dst);
        assert!(!transformed);
        let (entry, key) = TcpProxy::<NatDstTcpConnector>::find_syn_entry_for_accept(
            &syn_map,
            observed_dst,
            Some(&local),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&entry, &native_entry));
        assert_eq!(key, observed_dst);

        syn_map.remove(&observed_dst);
        let (entry, key, transformed) = TcpProxy::<NatDstTcpConnector>::find_nat_entry_for_nic(
            &addr_conn_map,
            &syn_map,
            observed_dst,
            &local,
            true,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&entry, &wrapped_entry));
        assert_eq!(key, wrapped_src);
        assert!(transformed);
        let (entry, key) = TcpProxy::<NatDstTcpConnector>::find_syn_entry_for_accept(
            &syn_map,
            observed_dst,
            Some(&local),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&entry, &wrapped_entry));
        assert_eq!(key, wrapped_src);
    }
}
