use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
};

use futures::Stream;
use smoltcp::{
    iface::{Config as InterfaceConfig, Interface, SocketHandle, SocketSet},
    phy::Device,
    socket::tcp::{Socket as TcpSocket, SocketBuffer as TcpSocketBuffer, State as TcpState},
    storage::RingBuffer,
    time::{Duration, Instant},
    wire::{HardwareAddress, IpAddress, IpCidr, IpProtocol, Ipv4Address, Ipv6Address, TcpPacket},
};
use spin::Mutex as SpinMutex;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Notify, OwnedSemaphorePermit, Semaphore, TryAcquireError,
    },
};
use tokio_util::sync::PollSemaphore;
use tracing::{error, trace};

use crate::{
    device::VirtualDevice,
    packet::{AnyIpPktFrame, IpPacket},
    Runner,
};

// The smoltcp window must cover the measured local TUN/netstack BDP. 32 KiB
// and 128 KiB were insufficient in optimized same-host validation. 256 KiB is
// the next bounded window below the original 0x3fff * 20 allocation.
const SMOLTCP_TCP_SEND_BUFFER_SIZE: usize = 256 * 1024;
const SMOLTCP_TCP_RECV_BUFFER_SIZE: usize = 256 * 1024;

// Stream staging remains large for an unpressured active flow, but it is not
// allocated for an idle connection. Under global pressure every active
// direction retains an independent 32 KiB progress buffer; only the expansion
// from BASE to MAX consumes the shared budget.
const STREAM_BASE_BUFFER_SIZE: usize = 32 * 1024;
const STREAM_MAX_BUFFER_SIZE: usize = 0x3FFF * 20;
// Charge the entire expanded allocation, not merely MAX - BASE: migration
// briefly holds both rings, so delta accounting would violate the hard budget.
const STREAM_EXPANSION_ACCOUNTED_BYTES: usize = STREAM_MAX_BUFFER_SIZE;
#[cfg(target_os = "android")]
const STREAM_EXPANSION_BUDGET: usize = 32 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const STREAM_EXPANSION_BUDGET: usize = 64 * 1024 * 1024;

struct AdaptiveRing {
    ring: Option<RingBuffer<'static, u8>>,
    expansion_permit: Option<OwnedSemaphorePermit>,
}

impl AdaptiveRing {
    fn new() -> Self {
        Self {
            ring: None,
            expansion_permit: None,
        }
    }

    fn has_storage(&self) -> bool {
        self.ring.is_some()
    }

    fn is_expanded(&self) -> bool {
        self.expansion_permit.is_some()
    }

    fn install(&mut self, storage: Vec<u8>, permit: Option<OwnedSemaphorePermit>) {
        assert!(!self.has_storage());
        let expected = if permit.is_some() {
            STREAM_MAX_BUFFER_SIZE
        } else {
            STREAM_BASE_BUFFER_SIZE
        };
        assert_eq!(storage.len(), expected);
        self.ring = Some(RingBuffer::new(storage));
        self.expansion_permit = permit;
    }

    fn expand(&mut self, storage: Vec<u8>, permit: OwnedSemaphorePermit) {
        assert!(self.has_storage());
        assert!(!self.is_expanded());
        assert_eq!(storage.len(), STREAM_MAX_BUFFER_SIZE);

        let mut old = self.ring.take().expect("base stream ring");
        let mut expanded = RingBuffer::new(storage);
        while !old.is_empty() {
            let (copied, ()) = old.dequeue_many_with(|data| {
                let copied = expanded.enqueue_slice(data);
                (copied, ())
            });
            assert!(copied > 0);
        }
        self.ring = Some(expanded);
        self.expansion_permit = Some(permit);
    }

    fn release_expansion_if_empty(&mut self) -> bool {
        if !self.is_expanded() || !self.is_empty() {
            return false;
        }
        self.ring.take();
        self.expansion_permit.take();
        true
    }

    fn clear_and_release(&mut self) {
        self.ring.take();
        self.expansion_permit.take();
    }

    fn is_empty(&self) -> bool {
        self.ring.as_ref().map_or(true, RingBuffer::is_empty)
    }

    fn is_full(&self) -> bool {
        self.ring.as_ref().is_some_and(RingBuffer::is_full)
    }

    fn enqueue_slice(&mut self, data: &[u8]) -> usize {
        self.ring
            .as_mut()
            .expect("stream ring storage")
            .enqueue_slice(data)
    }

    fn dequeue_slice(&mut self, data: &mut [u8]) -> usize {
        self.ring
            .as_mut()
            .expect("stream ring storage")
            .dequeue_slice(data)
    }
}

struct ExpansionBudget {
    semaphore: Arc<Semaphore>,
    waiters: AtomicUsize,
    reclaim_requested: AtomicBool,
}

impl ExpansionBudget {
    fn new() -> Self {
        let expansion_slots = STREAM_EXPANSION_BUDGET / STREAM_EXPANSION_ACCOUNTED_BYTES;
        assert!(expansion_slots > 0);
        Self::with_slots(expansion_slots)
    }

    fn with_slots(expansion_slots: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(expansion_slots)),
            waiters: AtomicUsize::new(0),
            reclaim_requested: AtomicBool::new(false),
        }
    }

    fn waiters(&self) -> usize {
        self.waiters.load(Ordering::Acquire)
    }

    fn register_waiter(&self) {
        self.waiters.fetch_add(1, Ordering::AcqRel);
        self.reclaim_requested.store(true, Ordering::Release);
    }

    fn unregister_waiter(&self) {
        let previous = self.waiters.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0);
    }

    fn reclaim_requested(&self) -> bool {
        self.reclaim_requested.load(Ordering::Acquire)
    }

    fn complete_one_reclaim(&self) {
        // The waiter receiving this slot has not necessarily been polled yet,
        // so it remains in the count. More than one live waiter means another
        // single reclaim should be attempted on the next pressure pass.
        self.reclaim_requested.swap(false, Ordering::AcqRel);
        if self.waiters() > 1 {
            self.reclaim_requested.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TcpSocketState {
    Normal,
    Close,
    Closing,
    Closed,
}

struct TcpSocketControl {
    send_buffer: AdaptiveRing,
    send_waker: Option<Waker>,
    recv_buffer: AdaptiveRing,
    recv_waker: Option<Waker>,
    recv_expansion_waiting: bool,
    recv_state: TcpSocketState,
    send_state: TcpSocketState,
}

struct TcpSocketCreation {
    control: SharedControl,
    socket: TcpSocket<'static>,
}

type SharedNotify = Arc<Notify>;
type SharedControl = Arc<SpinMutex<TcpSocketControl>>;

fn mark_send_closing(control: &mut TcpSocketControl) {
    control.send_state = TcpSocketState::Closing;
    if let Some(waker) = control.send_waker.take() {
        waker.wake();
    }
}

fn set_recv_expansion_waiting(
    control: &mut TcpSocketControl,
    budget: &ExpansionBudget,
    waiting: bool,
) {
    if control.recv_expansion_waiting == waiting {
        return;
    }
    control.recv_expansion_waiting = waiting;
    if waiting {
        budget.register_waiter();
    } else {
        budget.unregister_waiter();
    }
}

fn reclaim_expanded_empty_buffers(
    sockets: &HashMap<SocketHandle, SharedControl>,
    budget: &ExpansionBudget,
) -> bool {
    if budget.waiters() == 0 || !budget.reclaim_requested() {
        return false;
    }

    for control in sockets.values() {
        let mut control = control.lock();
        if control.send_buffer.release_expansion_if_empty() {
            budget.complete_one_reclaim();
            return true;
        }
        if control.recv_buffer.release_expansion_if_empty() {
            budget.complete_one_reclaim();
            return true;
        }
    }
    false
}

struct TcpListenerRunner;

impl TcpListenerRunner {
    fn create(
        device: VirtualDevice,
        iface: Interface,
        iface_ingress_tx: Sender<Vec<u8>>,
        tcp_rx: Receiver<AnyIpPktFrame>,
        stream_tx: Sender<TcpStream>,
        sockets: HashMap<SocketHandle, SharedControl>,
        budget: Arc<ExpansionBudget>,
        creation_queue_capacity: usize,
    ) -> Runner {
        Runner::new(async move {
            let notify = Arc::new(Notify::new());
            let (socket_tx, socket_rx) = channel::<TcpSocketCreation>(creation_queue_capacity);
            let packet_budget = budget.clone();
            let res = tokio::select! {
                v = Self::handle_packet(notify.clone(), iface_ingress_tx, tcp_rx, stream_tx, socket_tx, packet_budget) => v,
                v = Self::handle_socket(notify, device, iface, sockets, socket_rx, budget) => v,
            };
            res?;
            trace!("VirtDevice::poll thread exited");
            Ok(())
        })
    }

    async fn handle_packet(
        notify: SharedNotify,
        iface_ingress_tx: Sender<Vec<u8>>,
        mut tcp_rx: Receiver<AnyIpPktFrame>,
        stream_tx: Sender<TcpStream>,
        socket_tx: Sender<TcpSocketCreation>,
        budget: Arc<ExpansionBudget>,
    ) -> std::io::Result<()> {
        while let Some(frame) = tcp_rx.recv().await {
            let packet = match IpPacket::new_checked(frame.as_slice()) {
                Ok(p) => p,
                Err(err) => {
                    error!("invalid TCP IP packet: {:?}", err,);
                    continue;
                }
            };

            // Specially handle icmp packet by TCP interface.
            if matches!(packet.protocol(), IpProtocol::Icmp | IpProtocol::Icmpv6) {
                iface_ingress_tx
                    .send(frame)
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
                notify.notify_one();
                continue;
            }

            let src_ip = packet.src_addr();
            let dst_ip = packet.dst_addr();
            let payload = packet.payload();

            let packet = match TcpPacket::new_checked(payload) {
                Ok(p) => p,
                Err(err) => {
                    error!(
                        "invalid TCP err: {err}, src_ip: {src_ip}, dst_ip: {dst_ip}, payload: {payload:?}"
                    );
                    continue;
                }
            };
            let src_port = packet.src_port();
            let dst_port = packet.dst_port();

            let src_addr = SocketAddr::new(src_ip, src_port);
            let dst_addr = SocketAddr::new(dst_ip, dst_port);

            // TCP first handshake packet, create a new Connection
            if packet.syn() && !packet.ack() {
                let mut socket = TcpSocket::new(
                    TcpSocketBuffer::new(vec![0u8; SMOLTCP_TCP_RECV_BUFFER_SIZE]),
                    TcpSocketBuffer::new(vec![0u8; SMOLTCP_TCP_SEND_BUFFER_SIZE]),
                );
                socket.set_keep_alive(Some(Duration::from_secs(28)));
                // FIXME: It should follow system's setting. 7200 is Linux's default.
                socket.set_timeout(Some(Duration::from_secs(7200)));
                // NO ACK delay
                // socket.set_ack_delay(None);

                if let Err(err) = socket.listen(dst_addr) {
                    error!("listen error: {:?}", err);
                    continue;
                }

                trace!("created TCP connection for {} <-> {}", src_addr, dst_addr);

                let control = Arc::new(SpinMutex::new(TcpSocketControl {
                    send_buffer: AdaptiveRing::new(),
                    send_waker: None,
                    recv_buffer: AdaptiveRing::new(),
                    recv_waker: None,
                    recv_expansion_waiting: false,
                    recv_state: TcpSocketState::Normal,
                    send_state: TcpSocketState::Normal,
                }));

                stream_tx
                    .send(TcpStream {
                        src_addr,
                        dst_addr,
                        notify: notify.clone(),
                        control: control.clone(),
                        expansion_budget: budget.clone(),
                        send_expansion_permit: PollSemaphore::new(budget.semaphore.clone()),
                        send_expansion_waiting: false,
                    })
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
                socket_tx
                    .send(TcpSocketCreation { control, socket })
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            }

            // Pipeline tcp stream packet
            iface_ingress_tx
                .send(frame)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            notify.notify_one();
        }
        Ok(())
    }

    async fn handle_socket(
        notify: SharedNotify,
        mut device: VirtualDevice,
        mut iface: Interface,
        mut sockets: HashMap<SocketHandle, SharedControl>,
        mut socket_rx: Receiver<TcpSocketCreation>,
        budget: Arc<ExpansionBudget>,
    ) -> std::io::Result<()> {
        let mut socket_set = SocketSet::new(vec![]);
        loop {
            if device.output_closed() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stack output channel is closed",
                ));
            }

            while let Ok(TcpSocketCreation { control, socket }) = socket_rx.try_recv() {
                let handle = socket_set.add(socket);
                sockets.insert(handle, control);
            }

            if reclaim_expanded_empty_buffers(&sockets, &budget) {
                // Reclamation is pressure-only. Let a queued writer claim the
                // released expansion slot before scanning for another victim.
                tokio::task::yield_now().await;
                continue;
            }

            let before_poll = Instant::now();
            device.begin_poll();
            let updated_sockets = iface.poll(before_poll, &mut device, &mut socket_set);
            device.release_unused_output_permit();
            if matches!(
                updated_sockets,
                smoltcp::iface::PollResult::SocketStateChanged
            ) {
                trace!("VirtDevice::poll costed {}", Instant::now() - before_poll);
            }

            // Check all the sockets' status
            let mut sockets_to_remove = Vec::new();

            for (socket_handle, control) in sockets.iter() {
                let socket_handle = *socket_handle;
                let socket = socket_set.get_mut::<TcpSocket>(socket_handle);
                let mut control = control.lock();

                // Remove the socket only when it is in the closed state.
                if socket.state() == TcpState::Closed {
                    sockets_to_remove.push(socket_handle);

                    control.send_buffer.clear_and_release();
                    control.recv_buffer.clear_and_release();
                    set_recv_expansion_waiting(&mut control, &budget, false);
                    control.send_state = TcpSocketState::Closed;
                    control.recv_state = TcpSocketState::Closed;

                    if let Some(waker) = control.send_waker.take() {
                        waker.wake();
                    }
                    if let Some(waker) = control.recv_waker.take() {
                        waker.wake();
                    }

                    trace!("closed TCP connection");
                    continue;
                }

                // SHUT_WR
                if matches!(control.send_state, TcpSocketState::Close)
                    && control.send_buffer.is_empty()
                {
                    trace!("closing TCP Write Half, {:?}", socket.state());

                    // Keep the smoltcp socket writable until every byte
                    // accepted by AsyncWrite has moved into its TCP buffer.
                    socket.close();
                    mark_send_closing(&mut control);
                }

                // Check if readable. Every active direction can allocate the
                // unbudgeted base ring; an available global slot upgrades it to
                // the original large steady-state ring.
                let mut wake_receiver = false;
                if socket.can_recv() && !control.recv_buffer.has_storage() {
                    match budget.semaphore.clone().try_acquire_owned() {
                        Ok(permit) => control
                            .recv_buffer
                            .install(vec![0u8; STREAM_MAX_BUFFER_SIZE], Some(permit)),
                        Err(TryAcquireError::NoPermits) => control
                            .recv_buffer
                            .install(vec![0u8; STREAM_BASE_BUFFER_SIZE], None),
                        Err(TryAcquireError::Closed) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "TCP stream expansion budget is closed",
                            ));
                        }
                    }
                }

                if socket.can_recv()
                    && control.recv_buffer.is_full()
                    && !control.recv_buffer.is_expanded()
                {
                    match budget.semaphore.clone().try_acquire_owned() {
                        Ok(permit) => {
                            set_recv_expansion_waiting(&mut control, &budget, false);
                            control
                                .recv_buffer
                                .expand(vec![0u8; STREAM_MAX_BUFFER_SIZE], permit);
                        }
                        Err(TryAcquireError::NoPermits) => {
                            set_recv_expansion_waiting(&mut control, &budget, true);
                        }
                        Err(TryAcquireError::Closed) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "TCP stream expansion budget is closed",
                            ));
                        }
                    }
                } else if !control.recv_buffer.is_full() {
                    set_recv_expansion_waiting(&mut control, &budget, false);
                }

                while socket.can_recv() && !control.recv_buffer.is_full() {
                    let result = socket.recv(|buffer| {
                        let n = control.recv_buffer.enqueue_slice(buffer);
                        (n, ())
                    });

                    match result {
                        Ok(..) => wake_receiver = true,
                        Err(err) => {
                            error!("socket recv error: {:?}, {:?}", err, socket.state());

                            // Don't know why. Abort the connection.
                            socket.abort();

                            if matches!(control.recv_state, TcpSocketState::Normal) {
                                control.recv_state = TcpSocketState::Closed;
                            }
                            set_recv_expansion_waiting(&mut control, &budget, false);
                            wake_receiver = true;

                            // The socket will be recycled in the next poll.
                            break;
                        }
                    }
                }

                // If socket is not in ESTABLISH, FIN-WAIT-1, FIN-WAIT-2,
                // the local client have closed our receiver.
                let states = [
                    TcpState::Listen,
                    TcpState::SynReceived,
                    TcpState::Established,
                    TcpState::FinWait1,
                    TcpState::FinWait2,
                ];
                if matches!(control.recv_state, TcpSocketState::Normal)
                    && !socket.may_recv()
                    && !states.contains(&socket.state())
                {
                    trace!("closed TCP Read Half, {:?}", socket.state());

                    // Let TcpStream::poll_read returns EOF.
                    control.recv_state = TcpSocketState::Closed;
                    set_recv_expansion_waiting(&mut control, &budget, false);
                    wake_receiver = true;
                }

                if wake_receiver && control.recv_waker.is_some() {
                    if let Some(waker) = control.recv_waker.take() {
                        waker.wake();
                    }
                }

                // Check if writable
                let mut wake_sender = false;
                while socket.can_send() && !control.send_buffer.is_empty() {
                    let result = socket.send(|buffer| {
                        let n = control.send_buffer.dequeue_slice(buffer);
                        (n, ())
                    });

                    match result {
                        Ok(..) => wake_sender = true,
                        Err(err) => {
                            error!("socket send error: {:?}, {:?}", err, socket.state());

                            // Don't know why. Abort the connection.
                            socket.abort();

                            if matches!(control.send_state, TcpSocketState::Normal) {
                                control.send_state = TcpSocketState::Closed;
                            }
                            control.send_buffer.clear_and_release();
                            wake_sender = true;

                            // The socket will be recycled in the next poll.
                            break;
                        }
                    }
                }

                if wake_sender && control.send_waker.is_some() {
                    if let Some(waker) = control.send_waker.take() {
                        waker.wake();
                    }
                }
            }

            for socket_handle in sockets_to_remove {
                sockets.remove(&socket_handle);
                socket_set.remove(socket_handle);
            }

            if reclaim_expanded_empty_buffers(&sockets, &budget) {
                tokio::task::yield_now().await;
                continue;
            }

            if device.output_closed() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stack output channel is closed",
                ));
            }

            if device.output_blocked_this_poll() {
                device.wait_output_capacity().await?;
                continue;
            }

            Self::wait_for_next_poll(
                notify.as_ref(),
                &device,
                iface.poll_delay(before_poll, &socket_set),
            )
            .await?;
        }
    }

    async fn wait_for_next_poll(
        notify: &Notify,
        device: &VirtualDevice,
        next_duration: Option<Duration>,
    ) -> std::io::Result<()> {
        let output_closed = || {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stack output channel is closed",
            )
        };

        match next_duration {
            Some(Duration::ZERO) => {
                // A genuine immediate smoltcp deadline may need several polls, but
                // it must still consume Tokio's cooperative budget so shutdown and
                // sibling futures cannot be starved by one runner.
                tokio::task::consume_budget().await;
                Ok(())
            }
            Some(duration) => {
                tokio::select! {
                    _ = notify.notified() => Ok(()),
                    _ = device.wait_output_closed() => Err(output_closed()),
                    _ = tokio::time::sleep(tokio::time::Duration::from(duration)) => Ok(()),
                }
            }
            None => {
                // No smoltcp timer exists. Socket/ingress producers notify this
                // runner, while dropping Stack closes output and terminates it.
                tokio::select! {
                    _ = notify.notified() => Ok(()),
                    _ = device.wait_output_closed() => Err(output_closed()),
                }
            }
        }
    }
}

pub struct TcpListener {
    stream_rx: Receiver<TcpStream>,
}

impl TcpListener {
    pub(super) fn new(
        tcp_rx: Receiver<AnyIpPktFrame>,
        stack_tx: Sender<AnyIpPktFrame>,
    ) -> std::io::Result<(Runner, Self)> {
        // Reuse the configured upstream TCP queue capacity for the internal staging
        // queue. This retains at most the same number of validated IP frames and
        // propagates backpressure instead of growing an unbounded Vec queue.
        let ingress_capacity = tcp_rx.max_capacity();
        let (mut device, iface_ingress_tx) = VirtualDevice::new(stack_tx, ingress_capacity);
        let iface = Self::create_interface(&mut device)?;

        let (stream_tx, stream_rx) = channel(ingress_capacity);
        let expansion_budget = Arc::new(ExpansionBudget::new());

        let runner = TcpListenerRunner::create(
            device,
            iface,
            iface_ingress_tx,
            tcp_rx,
            stream_tx,
            HashMap::new(),
            expansion_budget,
            ingress_capacity,
        );

        Ok((runner, Self { stream_rx }))
    }

    fn create_interface<D>(device: &mut D) -> std::io::Result<Interface>
    where
        D: Device + ?Sized,
    {
        let mut iface_config = InterfaceConfig::new(HardwareAddress::Ip);
        iface_config.random_seed = rand::random();
        let mut iface = Interface::new(iface_config, device, Instant::now());
        iface.update_ip_addrs(|ip_addrs| {
            ip_addrs
                .push(IpCidr::new(IpAddress::v4(0, 0, 0, 1), 0))
                .expect("iface IPv4");
            ip_addrs
                .push(IpCidr::new(IpAddress::v6(0, 0, 0, 0, 0, 0, 0, 1), 0))
                .expect("iface IPv6");
        });
        iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(0, 0, 0, 1))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, e))?;
        iface
            .routes_mut()
            .add_default_ipv6_route(Ipv6Address::new(0, 0, 0, 0, 0, 0, 0, 1))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, e))?;
        iface.set_any_ip(true);
        Ok(iface)
    }
}

impl Stream for TcpListener {
    type Item = (TcpStream, SocketAddr, SocketAddr);

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.stream_rx.poll_recv(cx).map(|stream| {
            stream.map(|stream| {
                let local_addr = *stream.local_addr();
                let remote_addr: SocketAddr = *stream.remote_addr();
                (stream, local_addr, remote_addr)
            })
        })
    }
}

pub struct TcpStream {
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
    notify: SharedNotify,
    control: SharedControl,
    expansion_budget: Arc<ExpansionBudget>,
    send_expansion_permit: PollSemaphore,
    send_expansion_waiting: bool,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        self.cancel_send_expansion_wait();
        let mut control = self.control.lock();

        control.recv_buffer.clear_and_release();
        set_recv_expansion_waiting(&mut control, &self.expansion_budget, false);

        if matches!(control.recv_state, TcpSocketState::Normal) {
            control.recv_state = TcpSocketState::Close;
        }

        if matches!(control.send_state, TcpSocketState::Normal) {
            control.send_state = TcpSocketState::Close;
        }

        self.notify.notify_one();
    }
}

impl TcpStream {
    fn cancel_send_expansion_wait(&mut self) {
        if self.send_expansion_waiting {
            self.expansion_budget.unregister_waiter();
            self.send_expansion_waiting = false;
        }
        let semaphore = self.send_expansion_permit.clone_inner();
        self.send_expansion_permit = PollSemaphore::new(semaphore);
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.src_addr
    }

    pub fn remote_addr(&self) -> &SocketAddr {
        &self.dst_addr
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut control = self.control.lock();

        // Read from buffer
        if control.recv_buffer.is_empty() {
            // If socket is already closed / half closed, just return EOF directly.
            if matches!(control.recv_state, TcpSocketState::Closed) {
                return Ok(()).into();
            }

            // Nothing could be read. Wait for notify.
            if let Some(old_waker) = control.recv_waker.replace(cx.waker().clone()) {
                if !old_waker.will_wake(cx.waker()) {
                    old_waker.wake();
                }
            }

            return Poll::Pending;
        }

        let recv_buf = buf.initialize_unfilled();
        let n = control.recv_buffer.dequeue_slice(recv_buf);
        buf.advance(n);

        if n > 0 {
            self.notify.notify_one();
        }

        Ok(()).into()
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.as_mut().get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        loop {
            let mut control = this.control.lock();
            if !matches!(control.send_state, TcpSocketState::Normal) {
                drop(control);
                this.cancel_send_expansion_wait();
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }

            if !control.send_buffer.has_storage() {
                drop(control);
                let permit = match this.expansion_budget.semaphore.clone().try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(TryAcquireError::NoPermits) => None,
                    Err(TryAcquireError::Closed) => {
                        return Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "TCP stream expansion budget is closed",
                        )));
                    }
                };
                let capacity = if permit.is_some() {
                    STREAM_MAX_BUFFER_SIZE
                } else {
                    STREAM_BASE_BUFFER_SIZE
                };
                let storage = vec![0u8; capacity];

                control = this.control.lock();
                if !matches!(control.send_state, TcpSocketState::Normal) {
                    drop(control);
                    this.cancel_send_expansion_wait();
                    return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                }
                if !control.send_buffer.has_storage() {
                    control.send_buffer.install(storage, permit);
                }
            }

            if !control.send_buffer.is_full() {
                if this.send_expansion_waiting {
                    drop(control);
                    this.cancel_send_expansion_wait();
                    continue;
                }
                let n = control.send_buffer.enqueue_slice(buf);
                drop(control);
                this.notify.notify_one();
                return Poll::Ready(Ok(n));
            }

            if control.send_buffer.is_expanded() {
                if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                    if !old_waker.will_wake(cx.waker()) {
                        old_waker.wake();
                    }
                }
                return Poll::Pending;
            }
            drop(control);

            match this.send_expansion_permit.poll_acquire(cx) {
                Poll::Ready(Some(permit)) => {
                    this.cancel_send_expansion_wait();
                    let storage = vec![0u8; STREAM_MAX_BUFFER_SIZE];
                    let mut control = this.control.lock();
                    if !matches!(control.send_state, TcpSocketState::Normal) {
                        drop(control);
                        drop(permit);
                        return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                    }
                    if !control.send_buffer.is_expanded() {
                        control.send_buffer.expand(storage, permit);
                    }
                    continue;
                }
                Poll::Ready(None) => {
                    this.cancel_send_expansion_wait();
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "TCP stream expansion budget is closed",
                    )));
                }
                Poll::Pending => {
                    if !this.send_expansion_waiting {
                        this.expansion_budget.register_waiter();
                        this.send_expansion_waiting = true;
                        this.notify.notify_one();
                    }
                    let mut control = this.control.lock();
                    if !matches!(control.send_state, TcpSocketState::Normal) {
                        drop(control);
                        this.cancel_send_expansion_wait();
                        return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                    }
                    if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                        if !old_waker.will_wake(cx.waker()) {
                            old_waker.wake();
                        }
                    }
                    return Poll::Pending;
                }
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Ok(()).into()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        this.cancel_send_expansion_wait();
        let mut control = this.control.lock();

        if matches!(
            control.send_state,
            TcpSocketState::Closing | TcpSocketState::Closed
        ) {
            return Ok(()).into();
        }

        // SHUT_WR
        if matches!(control.send_state, TcpSocketState::Normal) {
            control.send_state = TcpSocketState::Close;
        }

        if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
            if !old_waker.will_wake(cx.waker()) {
                old_waker.wake();
            }
        }

        this.notify.notify_one();

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;
    use std::time::Duration as StdDuration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn control() -> SharedControl {
        Arc::new(SpinMutex::new(TcpSocketControl {
            send_buffer: AdaptiveRing::new(),
            send_waker: None,
            recv_buffer: AdaptiveRing::new(),
            recv_waker: None,
            recv_expansion_waiting: false,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        }))
    }

    fn stream(control: SharedControl, budget: Arc<ExpansionBudget>) -> TcpStream {
        TcpStream {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
            notify: Arc::new(Notify::new()),
            control,
            send_expansion_permit: PollSemaphore::new(budget.semaphore.clone()),
            expansion_budget: budget,
            send_expansion_waiting: false,
        }
    }

    #[test]
    fn expansion_budget_is_hard_bounded_and_idle_streams_have_no_storage() {
        assert_eq!(SMOLTCP_TCP_SEND_BUFFER_SIZE, 256 * 1024);
        assert_eq!(SMOLTCP_TCP_RECV_BUFFER_SIZE, 256 * 1024);
        let budget = ExpansionBudget::new();
        let slots = budget.semaphore.available_permits();
        assert!(slots * STREAM_EXPANSION_ACCOUNTED_BYTES <= STREAM_EXPANSION_BUDGET);
        assert!((slots + 1) * STREAM_EXPANSION_ACCOUNTED_BYTES > STREAM_EXPANSION_BUDGET);

        let control = control();
        assert!(!control.lock().send_buffer.has_storage());
        assert!(!control.lock().recv_buffer.has_storage());
    }

    #[test]
    fn expanding_a_base_ring_preserves_byte_order() {
        let budget = Arc::new(ExpansionBudget::with_slots(1));
        let mut ring = AdaptiveRing::new();
        ring.install(vec![0u8; STREAM_BASE_BUFFER_SIZE], None);
        assert_eq!(ring.enqueue_slice(b"ordered payload"), 15);

        let permit = budget.semaphore.clone().try_acquire_owned().unwrap();
        ring.expand(vec![0u8; STREAM_MAX_BUFFER_SIZE], permit);
        assert!(ring.is_expanded());

        let mut output = [0u8; 15];
        assert_eq!(ring.dequeue_slice(&mut output), output.len());
        assert_eq!(&output, b"ordered payload");
    }

    #[test]
    fn one_wait_episode_reclaims_at_most_one_expanded_ring() {
        let budget = Arc::new(ExpansionBudget::with_slots(2));
        let mut socket_set = SocketSet::new(Vec::new());
        let mut sockets = HashMap::new();
        for _ in 0..2 {
            let handle = socket_set.add(TcpSocket::new(
                TcpSocketBuffer::new(vec![0u8; 8]),
                TcpSocketBuffer::new(vec![0u8; 8]),
            ));
            let control = control();
            control.lock().send_buffer.install(
                vec![0u8; STREAM_MAX_BUFFER_SIZE],
                Some(budget.semaphore.clone().try_acquire_owned().unwrap()),
            );
            sockets.insert(handle, control);
        }
        assert_eq!(budget.semaphore.available_permits(), 0);

        budget.register_waiter();
        assert!(reclaim_expanded_empty_buffers(&sockets, &budget));
        assert_eq!(budget.semaphore.available_permits(), 1);
        assert!(!reclaim_expanded_empty_buffers(&sockets, &budget));
        assert_eq!(budget.semaphore.available_permits(), 1);
        budget.unregister_waiter();
    }

    #[test]
    fn multiple_live_waiters_reclaim_one_ring_per_pressure_pass() {
        let budget = Arc::new(ExpansionBudget::with_slots(2));
        let mut socket_set = SocketSet::new(Vec::new());
        let mut sockets = HashMap::new();
        for _ in 0..2 {
            let handle = socket_set.add(TcpSocket::new(
                TcpSocketBuffer::new(vec![0u8; 8]),
                TcpSocketBuffer::new(vec![0u8; 8]),
            ));
            let control = control();
            control.lock().send_buffer.install(
                vec![0u8; STREAM_MAX_BUFFER_SIZE],
                Some(budget.semaphore.clone().try_acquire_owned().unwrap()),
            );
            sockets.insert(handle, control);
        }

        budget.register_waiter();
        budget.register_waiter();
        assert!(reclaim_expanded_empty_buffers(&sockets, &budget));
        assert_eq!(budget.semaphore.available_permits(), 1);
        assert!(reclaim_expanded_empty_buffers(&sockets, &budget));
        assert_eq!(budget.semaphore.available_permits(), 2);
        assert!(!reclaim_expanded_empty_buffers(&sockets, &budget));
        budget.unregister_waiter();
        budget.unregister_waiter();
    }

    #[tokio::test]
    async fn exhausted_expansion_budget_still_allows_base_ring_progress() {
        let budget = Arc::new(ExpansionBudget::with_slots(0));
        let control = control();
        let mut stream = stream(control.clone(), budget.clone());
        let payload = vec![7u8; STREAM_BASE_BUFFER_SIZE];

        assert_eq!(stream.write(&payload).await.unwrap(), payload.len());
        assert!(!control.lock().send_buffer.is_expanded());

        let mut blocked = Box::pin(stream.write(b"x"));
        assert!(poll!(blocked.as_mut()).is_pending());
        assert_eq!(budget.waiters(), 1);

        let mut drained = [0u8; 1];
        assert_eq!(control.lock().send_buffer.dequeue_slice(&mut drained), 1);
        assert!(matches!(poll!(blocked.as_mut()), Poll::Ready(Ok(1))));
        assert_eq!(budget.waiters(), 0);
    }

    #[tokio::test]
    async fn more_active_streams_than_expansion_slots_all_make_initial_progress() {
        let budget = Arc::new(ExpansionBudget::with_slots(1));
        let mut expanded = 0;
        let mut retained = Vec::new();
        for _ in 0..128 {
            let control = control();
            let mut stream = stream(control.clone(), budget.clone());
            assert_eq!(stream.write(b"x").await.unwrap(), 1);
            expanded += usize::from(control.lock().send_buffer.is_expanded());
            retained.push((stream, control));
        }
        assert_eq!(expanded, 1);
        assert_eq!(budget.waiters(), 0);
    }

    #[tokio::test]
    async fn released_expansion_slot_wakes_a_waiting_base_ring() {
        let budget = Arc::new(ExpansionBudget::with_slots(1));
        let mut blocker = AdaptiveRing::new();
        blocker.install(
            vec![0u8; STREAM_MAX_BUFFER_SIZE],
            Some(budget.semaphore.clone().try_acquire_owned().unwrap()),
        );

        let control = control();
        let mut stream = stream(control.clone(), budget.clone());
        let payload = vec![3u8; STREAM_BASE_BUFFER_SIZE];
        assert_eq!(stream.write(&payload).await.unwrap(), payload.len());

        let mut blocked = Box::pin(stream.write(b"x"));
        assert!(poll!(blocked.as_mut()).is_pending());
        assert_eq!(budget.waiters(), 1);
        assert!(blocker.release_expansion_if_empty());

        assert_eq!(blocked.await.unwrap(), 1);
        assert!(control.lock().send_buffer.is_expanded());
        assert_eq!(budget.waiters(), 0);
    }

    #[tokio::test]
    async fn shutdown_cancels_a_pending_send_expansion_wait() {
        let budget = Arc::new(ExpansionBudget::with_slots(0));
        let control = control();
        let mut stream = stream(control, budget.clone());
        let payload = vec![9u8; STREAM_BASE_BUFFER_SIZE];
        assert_eq!(stream.write(&payload).await.unwrap(), payload.len());

        let mut blocked = Box::pin(stream.write(b"x"));
        assert!(poll!(blocked.as_mut()).is_pending());
        drop(blocked);
        assert_eq!(budget.waiters(), 1);

        let mut shutdown = Box::pin(stream.shutdown());
        assert!(poll!(shutdown.as_mut()).is_pending());
        assert_eq!(budget.waiters(), 0);
    }

    #[tokio::test]
    async fn received_bytes_remain_readable_before_half_close_eof() {
        let budget = Arc::new(ExpansionBudget::with_slots(0));
        let control = control();
        {
            let mut control = control.lock();
            control
                .recv_buffer
                .install(vec![0u8; STREAM_BASE_BUFFER_SIZE], None);
            assert_eq!(control.recv_buffer.enqueue_slice(b"data"), 4);
            control.recv_state = TcpSocketState::Closed;
        }
        let mut stream = stream(control, budget);
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"data");
    }

    #[tokio::test]
    async fn listener_connection_queue_is_bounded_like_ingress() {
        let (_tcp_tx, tcp_rx) = tokio::sync::mpsc::channel(3);
        let (stack_tx, _stack_rx) = tokio::sync::mpsc::channel(1);
        let (_runner, listener) = TcpListener::new(tcp_rx, stack_tx).unwrap();
        assert_eq!(listener.stream_rx.max_capacity(), 3);
    }

    #[tokio::test]
    async fn shutdown_completes_when_fin_is_committed() {
        let control = control();
        let mut stream = stream(control.clone(), Arc::new(ExpansionBudget::with_slots(1)));

        let shutdown = tokio::spawn(async move { stream.shutdown().await });
        tokio::task::yield_now().await;
        assert_eq!(control.lock().send_state, TcpSocketState::Close);

        mark_send_closing(&mut control.lock());
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn runner_exits_when_output_receiver_is_dropped() {
        let (tcp_tx, tcp_rx) = tokio::sync::mpsc::channel(1);
        let (stack_tx, stack_rx) = tokio::sync::mpsc::channel(1);
        let (runner, _listener) = TcpListener::new(tcp_rx, stack_tx).unwrap();
        drop(stack_rx);

        let err = tokio::time::timeout(StdDuration::from_secs(1), runner)
            .await
            .expect("runner did not observe closed output")
            .expect_err("runner unexpectedly succeeded");
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        drop(tcp_tx);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn immediate_poll_path_keeps_runtime_cooperative() {
        let (out_tx, _out_rx) = tokio::sync::mpsc::channel(1);
        let (device, _in_tx) = VirtualDevice::new(out_tx, 1);
        let notify = Notify::new();

        let spinner = tokio::spawn(async move {
            loop {
                TcpListenerRunner::wait_for_next_poll(&notify, &device, Some(Duration::ZERO))
                    .await
                    .unwrap();
            }
        });

        tokio::time::timeout(StdDuration::from_secs(1), async {
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        })
        .await
        .expect("immediate poll loop starved the current-thread runtime");
        spinner.abort();
    }
}
