use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
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

// NOTE: Default buffer could contain 20 AEAD packets
const DEFAULT_TCP_SEND_BUFFER_SIZE: u32 = 0x3FFF * 20;
const DEFAULT_TCP_RECV_BUFFER_SIZE: u32 = 0x3FFF * 20;

#[cfg(target_os = "android")]
const STREAM_GLOBAL_BUFFER_BUDGET: usize = 32 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const STREAM_GLOBAL_BUFFER_BUDGET: usize = 64 * 1024 * 1024;
const STREAM_DIRECTION_BUFFER_BUDGET: usize = STREAM_GLOBAL_BUFFER_BUDGET / 2;

struct LazyBudgetedRing {
    ring: Option<RingBuffer<'static, u8>>,
    permit: Option<OwnedSemaphorePermit>,
    capacity: usize,
}

impl LazyBudgetedRing {
    fn new(capacity: usize) -> Self {
        Self {
            ring: None,
            permit: None,
            capacity,
        }
    }

    fn has_storage(&self) -> bool {
        self.ring.is_some()
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn install_storage(&mut self, permit: OwnedSemaphorePermit) {
        self.install_preallocated_storage(permit, vec![0u8; self.capacity]);
    }

    fn install_preallocated_storage(&mut self, permit: OwnedSemaphorePermit, storage: Vec<u8>) {
        assert!(!self.has_storage());
        assert_eq!(permit.num_permits(), 1);
        assert_eq!(storage.len(), self.capacity);
        self.ring = Some(RingBuffer::new(storage));
        self.permit = Some(permit);
    }

    fn release_storage(&mut self) -> bool {
        if !self.is_empty() {
            return false;
        }
        let released = self.ring.take().is_some();
        self.permit.take();
        released
    }

    fn clear_and_release(&mut self) {
        if let Some(ring) = self.ring.as_mut() {
            ring.clear();
        }
        self.release_storage();
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
            .expect("stream buffer storage")
            .enqueue_slice(data)
    }

    fn dequeue_slice(&mut self, data: &mut [u8]) -> usize {
        self.ring
            .as_mut()
            .expect("stream buffer storage")
            .dequeue_slice(data)
    }
}

struct BufferBudget {
    semaphore: Arc<Semaphore>,
    waiters: AtomicUsize,
    reclaim_requests: AtomicUsize,
}

impl BufferBudget {
    fn new(byte_budget: usize, block_size: usize) -> Self {
        let block_count = byte_budget / block_size;
        assert!(block_count > 0);
        Self {
            semaphore: Arc::new(Semaphore::new(block_count)),
            waiters: AtomicUsize::new(0),
            reclaim_requests: AtomicUsize::new(0),
        }
    }

    fn waiters(&self) -> usize {
        self.waiters.load(Ordering::Acquire)
    }

    fn register_waiter(&self) {
        self.waiters.fetch_add(1, Ordering::AcqRel);
    }

    fn unregister_waiter(&self) {
        let previous = self.waiters.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0);
    }

    fn request_reclaim(&self) {
        self.reclaim_requests.fetch_add(1, Ordering::Release);
    }

    fn has_reclaim_request(&self) -> bool {
        self.reclaim_requests.load(Ordering::Acquire) > 0
    }

    fn take_reclaim_request(&self) -> bool {
        self.reclaim_requests
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |requests| {
                requests.checked_sub(1)
            })
            .is_ok()
    }
}

#[derive(Clone)]
struct StreamBudgets {
    send: Arc<BufferBudget>,
    recv: Arc<BufferBudget>,
}

impl StreamBudgets {
    fn new() -> Self {
        Self {
            send: Arc::new(BufferBudget::new(
                STREAM_DIRECTION_BUFFER_BUDGET,
                DEFAULT_TCP_SEND_BUFFER_SIZE as usize,
            )),
            recv: Arc::new(BufferBudget::new(
                STREAM_DIRECTION_BUFFER_BUDGET,
                DEFAULT_TCP_RECV_BUFFER_SIZE as usize,
            )),
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
    send_buffer: LazyBudgetedRing,
    send_waker: Option<Waker>,
    recv_buffer: LazyBudgetedRing,
    recv_waker: Option<Waker>,
    recv_budget_waiting: bool,
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

fn set_recv_budget_waiting(control: &mut TcpSocketControl, budget: &BufferBudget, waiting: bool) {
    if control.recv_budget_waiting == waiting {
        return;
    }
    control.recv_budget_waiting = waiting;
    if waiting {
        budget.register_waiter();
    } else {
        budget.unregister_waiter();
    }
}

fn reclaim_empty_buffer(buffer: &mut LazyBudgetedRing, remaining: &mut usize) -> bool {
    if *remaining == 0 || !buffer.release_storage() {
        return false;
    }
    *remaining -= 1;
    true
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
        budgets: StreamBudgets,
        creation_queue_capacity: usize,
    ) -> Runner {
        Runner::new(async move {
            let notify = Arc::new(Notify::new());
            let (socket_tx, socket_rx) = channel::<TcpSocketCreation>(creation_queue_capacity);
            let packet_budgets = budgets.clone();
            let res = tokio::select! {
                v = Self::handle_packet(notify.clone(), iface_ingress_tx, tcp_rx, stream_tx, socket_tx, packet_budgets) => v,
                v = Self::handle_socket(notify, device, iface, sockets, socket_rx, budgets) => v,
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
        budgets: StreamBudgets,
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
                    TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE as usize]),
                    TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE as usize]),
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
                    send_buffer: LazyBudgetedRing::new(DEFAULT_TCP_SEND_BUFFER_SIZE as usize),
                    send_waker: None,
                    recv_buffer: LazyBudgetedRing::new(DEFAULT_TCP_RECV_BUFFER_SIZE as usize),
                    recv_waker: None,
                    recv_budget_waiting: false,
                    recv_state: TcpSocketState::Normal,
                    send_state: TcpSocketState::Normal,
                }));

                stream_tx
                    .send(TcpStream {
                        src_addr,
                        dst_addr,
                        notify: notify.clone(),
                        control: control.clone(),
                        send_budget: budgets.send.clone(),
                        recv_budget: budgets.recv.clone(),
                        send_budget_permit: PollSemaphore::new(budgets.send.semaphore.clone()),
                        send_budget_waiting: false,
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
        budgets: StreamBudgets,
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
            // Do not consume the request until an idle allocation is actually
            // released. Otherwise a scan with no reclaimable buffer can strand
            // the blocked writer forever.
            let mut send_reclaims_remaining = usize::from(budgets.send.has_reclaim_request());
            let mut recv_reclaims_remaining = budgets.recv.waiters();
            let mut budget_reclaimed = false;

            for (socket_handle, control) in sockets.iter() {
                let socket_handle = *socket_handle;
                let socket = socket_set.get_mut::<TcpSocket>(socket_handle);
                let mut control = control.lock();

                // Remove the socket only when it is in the closed state.
                if socket.state() == TcpState::Closed {
                    sockets_to_remove.push(socket_handle);

                    control.send_buffer.clear_and_release();
                    control.recv_buffer.clear_and_release();
                    set_recv_budget_waiting(&mut control, &budgets.recv, false);
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

                if reclaim_empty_buffer(&mut control.send_buffer, &mut send_reclaims_remaining) {
                    assert!(budgets.send.take_reclaim_request());
                    budget_reclaimed = true;
                }

                let released_recv_for_pressure =
                    reclaim_empty_buffer(&mut control.recv_buffer, &mut recv_reclaims_remaining);
                if released_recv_for_pressure {
                    budget_reclaimed = true;
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

                // Check if readable
                let mut wake_receiver = false;
                if socket.can_recv()
                    && !released_recv_for_pressure
                    && !control.recv_buffer.has_storage()
                {
                    match budgets.recv.semaphore.clone().try_acquire_owned() {
                        Ok(permit) => {
                            set_recv_budget_waiting(&mut control, &budgets.recv, false);
                            control.recv_buffer.install_storage(permit);
                        }
                        Err(TryAcquireError::NoPermits) => {
                            set_recv_budget_waiting(&mut control, &budgets.recv, true);
                        }
                        Err(TryAcquireError::Closed) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "TCP receive buffer budget is closed",
                            ));
                        }
                    }
                }
                while socket.can_recv()
                    && control.recv_buffer.has_storage()
                    && !control.recv_buffer.is_full()
                {
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
                            set_recv_budget_waiting(&mut control, &budgets.recv, false);
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
                    set_recv_budget_waiting(&mut control, &budgets.recv, false);
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

                if reclaim_empty_buffer(&mut control.send_buffer, &mut send_reclaims_remaining) {
                    assert!(budgets.send.take_reclaim_request());
                    budget_reclaimed = true;
                }
            }

            for socket_handle in sockets_to_remove {
                sockets.remove(&socket_handle);
                socket_set.remove(socket_handle);
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

            if budget_reclaimed {
                // Reclamation is a pressure-only path. Yield once so the woken
                // writer, or a receiver blocked later in the socket scan, can
                // claim the released block before another idle block is evicted.
                tokio::task::yield_now().await;
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
        let budgets = StreamBudgets::new();

        let runner = TcpListenerRunner::create(
            device,
            iface,
            iface_ingress_tx,
            tcp_rx,
            stream_tx,
            HashMap::new(),
            budgets,
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
    send_budget: Arc<BufferBudget>,
    recv_budget: Arc<BufferBudget>,
    send_budget_permit: PollSemaphore,
    send_budget_waiting: bool,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        self.cancel_send_budget_wait();
        let mut control = self.control.lock();

        // No reader remains. Reclaim receive staging immediately instead of
        // retaining one global buffer block until the TCP state machine closes.
        control.recv_buffer.clear_and_release();
        set_recv_budget_waiting(&mut control, &self.recv_budget, false);

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
    fn cancel_send_budget_wait(&mut self) {
        self.send_budget_waiting = false;
        let semaphore = self.send_budget_permit.clone_inner();
        self.send_budget_permit = PollSemaphore::new(semaphore);
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

        let send_buffer_capacity;
        {
            let mut control = this.control.lock();

            // If state == Close | Closing | Closed, the TCP stream WR half is closed.
            if !matches!(control.send_state, TcpSocketState::Normal) {
                drop(control);
                this.cancel_send_budget_wait();
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }

            if control.send_buffer.has_storage() {
                if control.send_buffer.is_full() {
                    if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                        if !old_waker.will_wake(cx.waker()) {
                            old_waker.wake();
                        }
                    }
                    return Poll::Pending;
                }

                let n = control.send_buffer.enqueue_slice(buf);
                drop(control);
                this.notify.notify_one();
                return Poll::Ready(Ok(n));
            }
            send_buffer_capacity = control.send_buffer.capacity();
        }

        let permit = match this.send_budget_permit.poll_acquire(cx) {
            Poll::Ready(Some(permit)) => {
                this.send_budget_waiting = false;
                permit
            }
            Poll::Ready(None) => {
                this.send_budget_waiting = false;
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "TCP send buffer budget is closed",
                )));
            }
            Poll::Pending => {
                if !this.send_budget_waiting {
                    // One pending episode requests at most one idle-buffer
                    // reclaim. A cancelled write can therefore cause at most
                    // one harmless extra eviction instead of a reclaim cascade.
                    this.send_budget.request_reclaim();
                    this.send_budget_waiting = true;
                }
                let mut control = this.control.lock();
                if !matches!(control.send_state, TcpSocketState::Normal) {
                    drop(control);
                    this.cancel_send_budget_wait();
                    return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
                }
                if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                    if !old_waker.will_wake(cx.waker()) {
                        old_waker.wake();
                    }
                }
                return Poll::Pending;
            }
        };

        // Allocation and zeroing can fault hundreds of KiB of pages. Keep that
        // work outside the spin mutex so the netstack runner never burns CPU
        // waiting for a first-write allocation to finish.
        let storage = vec![0u8; send_buffer_capacity];
        let mut control = this.control.lock();
        if !matches!(control.send_state, TcpSocketState::Normal) {
            drop(permit);
            drop(control);
            this.cancel_send_budget_wait();
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        control
            .send_buffer
            .install_preallocated_storage(permit, storage);
        let n = control.send_buffer.enqueue_slice(buf);
        drop(control);
        this.notify.notify_one();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Ok(()).into()
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.cancel_send_budget_wait();
        let mut control = self.control.lock();

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

        self.notify.notify_one();

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;
    use std::time::Duration as StdDuration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn budget(blocks: usize, block_size: usize) -> Arc<BufferBudget> {
        Arc::new(BufferBudget::new(blocks * block_size, block_size))
    }

    fn control(capacity: usize) -> SharedControl {
        Arc::new(SpinMutex::new(TcpSocketControl {
            send_buffer: LazyBudgetedRing::new(capacity),
            send_waker: None,
            recv_buffer: LazyBudgetedRing::new(capacity),
            recv_waker: None,
            recv_budget_waiting: false,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        }))
    }

    fn stream(
        control: SharedControl,
        send_budget: Arc<BufferBudget>,
        recv_budget: Arc<BufferBudget>,
    ) -> TcpStream {
        TcpStream {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
            notify: Arc::new(Notify::new()),
            control,
            send_budget: send_budget.clone(),
            recv_budget,
            send_budget_permit: PollSemaphore::new(send_budget.semaphore.clone()),
            send_budget_waiting: false,
        }
    }

    fn install(buffer: &mut LazyBudgetedRing, budget: &Arc<BufferBudget>) {
        let permit = budget.semaphore.clone().try_acquire_owned().unwrap();
        buffer.install_storage(permit);
    }

    #[test]
    fn empty_connections_allocate_no_stream_payload_and_budget_is_hard_bounded() {
        let budgets = StreamBudgets::new();
        let control = control(DEFAULT_TCP_SEND_BUFFER_SIZE as usize);

        assert!(!control.lock().send_buffer.has_storage());
        assert!(!control.lock().recv_buffer.has_storage());
        assert!(
            budgets.send.semaphore.available_permits() * DEFAULT_TCP_SEND_BUFFER_SIZE as usize
                <= STREAM_DIRECTION_BUFFER_BUDGET
        );
        assert!(
            budgets.recv.semaphore.available_permits() * DEFAULT_TCP_RECV_BUFFER_SIZE as usize
                <= STREAM_DIRECTION_BUFFER_BUDGET
        );
    }

    #[test]
    fn empty_ring_is_retained_without_pressure_and_reclaimed_for_waiter() {
        let budget = budget(1, 8);
        let mut ring = LazyBudgetedRing::new(8);
        install(&mut ring, &budget);
        assert_eq!(ring.enqueue_slice(b"payload"), 7);
        let mut reclaims = 1;
        assert!(!reclaim_empty_buffer(&mut ring, &mut reclaims));
        assert_eq!(reclaims, 1);
        assert!(ring.has_storage());

        let mut output = [0u8; 7];
        assert_eq!(ring.dequeue_slice(&mut output), 7);
        assert_eq!(&output, b"payload");
        assert!(ring.has_storage());
        assert_eq!(budget.semaphore.available_permits(), 0);

        let mut reclaims = 0;
        assert!(!reclaim_empty_buffer(&mut ring, &mut reclaims));
        let mut reclaims = 1;
        assert!(reclaim_empty_buffer(&mut ring, &mut reclaims));
        assert!(!ring.has_storage());
        assert_eq!(budget.semaphore.available_permits(), 1);
    }

    #[test]
    fn send_reclaim_request_survives_a_scan_without_an_idle_buffer() {
        let budget = budget(1, 8);
        let mut ring = LazyBudgetedRing::new(8);
        budget.request_reclaim();

        let mut reclaims = usize::from(budget.has_reclaim_request());
        assert!(!reclaim_empty_buffer(&mut ring, &mut reclaims));
        assert!(budget.has_reclaim_request());

        install(&mut ring, &budget);
        assert!(reclaim_empty_buffer(&mut ring, &mut reclaims));
        assert!(budget.take_reclaim_request());
        assert!(!budget.has_reclaim_request());
    }

    #[test]
    fn receive_pressure_is_counted_once_and_cleared_on_progress() {
        let budget = budget(1, 8);
        let control = control(8);
        let mut control = control.lock();

        set_recv_budget_waiting(&mut control, &budget, true);
        set_recv_budget_waiting(&mut control, &budget, true);
        assert_eq!(budget.waiters(), 1);

        set_recv_budget_waiting(&mut control, &budget, false);
        assert_eq!(budget.waiters(), 0);
    }

    #[tokio::test]
    async fn stream_write_waits_for_global_block_and_wakes_on_reclaim() {
        let send_budget = budget(1, 8);
        let recv_budget = budget(1, 8);
        let mut blocker = LazyBudgetedRing::new(8);
        install(&mut blocker, &send_budget);
        let control = control(8);
        let mut stream = stream(control.clone(), send_budget.clone(), recv_budget);

        let mut write = Box::pin(stream.write(b"x"));
        assert!(poll!(write.as_mut()).is_pending());
        assert_eq!(send_budget.reclaim_requests.load(Ordering::Acquire), 1);

        assert!(blocker.release_storage());
        assert_eq!(write.await.unwrap(), 1);
        assert!(send_budget.take_reclaim_request());
        assert!(!send_budget.take_reclaim_request());
        assert!(control.lock().send_buffer.has_storage());
    }

    #[tokio::test]
    async fn cancelled_write_releases_granted_block_when_stream_is_reset() {
        let send_budget = budget(1, 8);
        let recv_budget = budget(1, 8);
        let mut blocker = LazyBudgetedRing::new(8);
        install(&mut blocker, &send_budget);
        let control = control(8);
        let mut stream = stream(control, send_budget.clone(), recv_budget);

        let mut write = Box::pin(stream.write(b"x"));
        assert!(poll!(write.as_mut()).is_pending());
        drop(write);
        assert_eq!(send_budget.reclaim_requests.load(Ordering::Acquire), 1);

        assert!(blocker.release_storage());
        assert_eq!(send_budget.semaphore.available_permits(), 0);

        let mut shutdown = Box::pin(stream.shutdown());
        assert!(poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        assert!(send_budget.take_reclaim_request());
        assert!(!send_budget.take_reclaim_request());
        assert_eq!(send_budget.semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn received_bytes_remain_readable_before_half_close_eof() {
        let send_budget = budget(1, 8);
        let recv_budget = budget(1, 8);
        let control = control(8);
        {
            let mut control = control.lock();
            install(&mut control.recv_buffer, &recv_budget);
            assert_eq!(control.recv_buffer.enqueue_slice(b"data"), 4);
            control.recv_state = TcpSocketState::Closed;
        }
        let mut stream = stream(control, send_budget, recv_budget.clone());

        let mut output = [0u8; 4];
        stream.read_exact(&mut output).await.unwrap();
        assert_eq!(&output, b"data");
        let mut eof = [0u8; 1];
        assert_eq!(stream.read(&mut eof).await.unwrap(), 0);
        drop(stream);
        assert_eq!(recv_budget.semaphore.available_permits(), 1);
    }

    #[test]
    fn listener_connection_queue_is_bounded_like_ingress() {
        let (_tcp_tx, tcp_rx) = tokio::sync::mpsc::channel(3);
        let (stack_tx, _stack_rx) = tokio::sync::mpsc::channel(1);
        let (runner, listener) = TcpListener::new(tcp_rx, stack_tx).unwrap();
        assert_eq!(listener.stream_rx.max_capacity(), 3);
        drop(runner);
    }

    #[tokio::test]
    async fn shutdown_completes_when_fin_is_committed() {
        let control = control(16);
        let mut stream = stream(control.clone(), budget(1, 16), budget(1, 16));

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
