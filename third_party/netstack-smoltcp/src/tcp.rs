use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use bytes::Bytes;
use futures::Stream;
use smoltcp::{
    iface::{Config as InterfaceConfig, Interface, SocketHandle, SocketSet},
    phy::Device,
    socket::tcp::{Socket as TcpSocket, SocketBuffer as TcpSocketBuffer, State as TcpState},
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
const STREAM_QUEUE_LIMIT: usize = 64 * 1024;
const STREAM_QUEUE_CHUNK_SIZE: usize = 32 * 1024;

#[cfg(target_os = "android")]
const STREAM_GLOBAL_BUDGET: usize = 32 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const STREAM_GLOBAL_BUDGET: usize = 64 * 1024 * 1024;
const STREAM_DIRECTION_BUDGET: usize = STREAM_GLOBAL_BUDGET / 2;

struct BudgetedChunk {
    bytes: Bytes,
    permits: OwnedSemaphorePermit,
}

struct BudgetedQueue {
    chunks: VecDeque<BudgetedChunk>,
    len: usize,
    limit: usize,
}

impl BudgetedQueue {
    fn new(limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            len: 0,
            limit,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn is_full(&self) -> bool {
        self.len == self.limit
    }

    fn remaining_capacity(&self) -> usize {
        self.limit - self.len
    }

    fn enqueue(&mut self, bytes: &[u8], permits: OwnedSemaphorePermit) {
        assert!(!bytes.is_empty());
        assert!(bytes.len() <= self.remaining_capacity());
        assert_eq!(permits.num_permits(), bytes.len());
        self.len += bytes.len();
        self.chunks.push_back(BudgetedChunk {
            bytes: Bytes::copy_from_slice(bytes),
            permits,
        });
    }

    fn dequeue_slice(&mut self, output: &mut [u8]) -> usize {
        let mut written = 0;
        while written < output.len() {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            let count = (output.len() - written).min(front.bytes.len());
            output[written..written + count].copy_from_slice(&front.bytes[..count]);
            drop(front.permits.split(count).expect("queue permit count"));
            drop(front.bytes.split_to(count));
            self.len -= count;
            written += count;
            if front.bytes.is_empty() {
                self.chunks.pop_front();
            }
        }
        written
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.len = 0;
    }
}

#[derive(Clone)]
struct StreamBudgets {
    send: Arc<Semaphore>,
    recv: Arc<Semaphore>,
}

impl StreamBudgets {
    fn new(direction_budget: usize) -> Self {
        Self {
            send: Arc::new(Semaphore::new(direction_budget)),
            recv: Arc::new(Semaphore::new(direction_budget)),
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
    send_buffer: BudgetedQueue,
    send_waker: Option<Waker>,
    recv_buffer: BudgetedQueue,
    recv_waker: Option<Waker>,
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
            let res = tokio::select! {
                v = Self::handle_packet(notify.clone(), iface_ingress_tx, tcp_rx, stream_tx, socket_tx, budgets.send) => v,
                v = Self::handle_socket(notify, device, iface, sockets, socket_rx, budgets.recv) => v,
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
        send_budget: Arc<Semaphore>,
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
                    send_buffer: BudgetedQueue::new(STREAM_QUEUE_LIMIT),
                    send_waker: None,
                    recv_buffer: BudgetedQueue::new(STREAM_QUEUE_LIMIT),
                    recv_waker: None,
                    recv_state: TcpSocketState::Normal,
                    send_state: TcpSocketState::Normal,
                }));

                stream_tx
                    .send(TcpStream {
                        src_addr,
                        dst_addr,
                        notify: notify.clone(),
                        control: control.clone(),
                        send_budget: PollSemaphore::new(send_budget.clone()),
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
        recv_budget: Arc<Semaphore>,
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

            for (socket_handle, control) in sockets.iter() {
                let socket_handle = *socket_handle;
                let socket = socket_set.get_mut::<TcpSocket>(socket_handle);
                let mut control = control.lock();

                // Remove the socket only when it is in the closed state.
                if socket.state() == TcpState::Closed {
                    sockets_to_remove.push(socket_handle);

                    // Bytes that were never accepted by smoltcp cannot be
                    // delivered after close. Release their global budget now;
                    // received bytes stay readable before AsyncRead reports EOF.
                    control.send_buffer.clear();
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

                // Check if readable
                let mut wake_receiver = false;
                while socket.can_recv() && !control.recv_buffer.is_full() {
                    let reserved = STREAM_QUEUE_CHUNK_SIZE
                        .min(control.recv_buffer.remaining_capacity())
                        .min(recv_budget.available_permits());
                    if reserved == 0 {
                        break;
                    }
                    let Ok(mut permits) =
                        recv_budget.clone().try_acquire_many_owned(reserved as u32)
                    else {
                        break;
                    };
                    let result = socket.recv(|buffer| {
                        let n = reserved.min(buffer.len());
                        let used_permits = permits.split(n).expect("reserved receive budget");
                        drop(permits);
                        control.recv_buffer.enqueue(&buffer[..n], used_permits);
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
                            control.send_buffer.clear();
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
        let budgets = StreamBudgets::new(STREAM_DIRECTION_BUDGET);

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
    send_budget: PollSemaphore,
    send_budget_waiting: bool,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let mut control = self.control.lock();
        // No reader remains, so received payload must not retain global budget
        // until the TCP state machine finishes closing.
        control.recv_buffer.clear();

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
        let semaphore = self.send_budget.clone_inner();
        self.send_budget = PollSemaphore::new(semaphore);
        self.send_budget_waiting = false;
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

        let write_len = {
            let mut control = this.control.lock();

            // If state == Close | Closing | Closed, the TCP stream WR half is closed.
            if !matches!(control.send_state, TcpSocketState::Normal) {
                drop(control);
                this.cancel_send_budget_wait();
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }

            if control.send_buffer.is_full() {
                if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                    if !old_waker.will_wake(cx.waker()) {
                        old_waker.wake();
                    }
                }
                return Poll::Pending;
            }

            STREAM_QUEUE_CHUNK_SIZE
                .min(control.send_buffer.remaining_capacity())
                .min(buf.len())
        };

        let budget = this.send_budget.clone_inner();
        let mut permits = if this.send_budget_waiting {
            match this.send_budget.poll_acquire(cx) {
                Poll::Ready(Some(permits)) => {
                    this.send_budget_waiting = false;
                    permits
                }
                Poll::Ready(None) => {
                    this.send_budget_waiting = false;
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "TCP stream byte budget is closed",
                    )));
                }
                Poll::Pending => return pending_write(this, cx),
            }
        } else {
            match budget.clone().try_acquire_many_owned(write_len as u32) {
                Ok(permits) => permits,
                Err(TryAcquireError::Closed) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "TCP stream byte budget is closed",
                    )));
                }
                Err(TryAcquireError::NoPermits) => {
                    this.send_budget_waiting = true;
                    match this.send_budget.poll_acquire(cx) {
                        Poll::Ready(Some(permits)) => {
                            this.send_budget_waiting = false;
                            permits
                        }
                        Poll::Ready(None) => {
                            this.send_budget_waiting = false;
                            return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "TCP stream byte budget is closed",
                            )));
                        }
                        Poll::Pending => return pending_write(this, cx),
                    }
                }
            }
        };

        // A saturated stream waits fairly for one byte rather than placing a
        // 32 KiB request at the semaphore head. Once granted, opportunistically
        // coalesce currently available capacity so normal writes stay chunky.
        let extra = (write_len - permits.num_permits()).min(budget.available_permits());
        if extra > 0 {
            if let Ok(extra_permits) = budget.clone().try_acquire_many_owned(extra as u32) {
                permits.merge(extra_permits);
            }
        }
        let write_len = permits.num_permits();

        let mut control = this.control.lock();
        if !matches!(control.send_state, TcpSocketState::Normal) {
            drop(permits);
            drop(control);
            this.cancel_send_budget_wait();
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        control.send_buffer.enqueue(&buf[..write_len], permits);
        this.notify.notify_one();
        Poll::Ready(Ok(write_len))
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

fn pending_write(stream: &mut TcpStream, cx: &mut Context<'_>) -> Poll<std::io::Result<usize>> {
    // PollSemaphore wakes for global capacity; the socket waker is
    // also retained so close/reset cancels this pending write.
    let mut control = stream.control.lock();
    if !matches!(control.send_state, TcpSocketState::Normal) {
        drop(control);
        stream.cancel_send_budget_wait();
        return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
    }
    if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
        if !old_waker.will_wake(cx.waker()) {
            old_waker.wake();
        }
    }
    Poll::Pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;
    use std::time::Duration as StdDuration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn control(queue_limit: usize) -> SharedControl {
        Arc::new(SpinMutex::new(TcpSocketControl {
            send_buffer: BudgetedQueue::new(queue_limit),
            send_waker: None,
            recv_buffer: BudgetedQueue::new(queue_limit),
            recv_waker: None,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        }))
    }

    fn stream(control: SharedControl, send_budget: Arc<Semaphore>) -> TcpStream {
        TcpStream {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
            notify: Arc::new(Notify::new()),
            control,
            send_budget: PollSemaphore::new(send_budget),
            send_budget_waiting: false,
        }
    }

    fn enqueue(queue: &mut BudgetedQueue, budget: &Arc<Semaphore>, bytes: &[u8]) {
        let permits = budget
            .clone()
            .try_acquire_many_owned(bytes.len() as u32)
            .unwrap();
        queue.enqueue(bytes, permits);
    }

    #[test]
    fn smoltcp_window_and_platform_budget_are_explicit() {
        assert_eq!(DEFAULT_TCP_SEND_BUFFER_SIZE, 0x3fff * 20);
        assert_eq!(DEFAULT_TCP_RECV_BUFFER_SIZE, 0x3fff * 20);
        assert_eq!(STREAM_QUEUE_LIMIT, 64 * 1024);
        assert_eq!(STREAM_QUEUE_CHUNK_SIZE, 32 * 1024);
        assert_eq!(STREAM_DIRECTION_BUDGET * 2, STREAM_GLOBAL_BUDGET);
    }

    #[test]
    fn empty_queue_allocates_no_payload_and_releases_partial_budget() {
        let budget = Arc::new(Semaphore::new(8));
        let mut queue = BudgetedQueue::new(8);
        assert_eq!(queue.chunks.capacity(), 0);
        assert_eq!(budget.available_permits(), 8);

        enqueue(&mut queue, &budget, b"12345678");
        assert_eq!(queue.len(), 8);
        assert_eq!(budget.available_permits(), 0);

        let mut first = [0u8; 3];
        assert_eq!(queue.dequeue_slice(&mut first), 3);
        assert_eq!(&first, b"123");
        assert_eq!(budget.available_permits(), 3);

        queue.clear();
        assert_eq!(budget.available_permits(), 8);
        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn stream_write_waits_for_global_budget_and_wakes_on_release() {
        let budget = Arc::new(Semaphore::new(4));
        let mut blocker = BudgetedQueue::new(4);
        enqueue(&mut blocker, &budget, b"full");
        let control = control(4);
        let mut stream = stream(control.clone(), budget.clone());

        let mut write = Box::pin(stream.write(b"x"));
        assert!(poll!(write.as_mut()).is_pending());
        assert!(control.lock().send_waker.is_some());

        let mut released = [0u8; 1];
        assert_eq!(blocker.dequeue_slice(&mut released), 1);
        assert_eq!(write.await.unwrap(), 1);
        assert_eq!(control.lock().send_buffer.len(), 1);
        assert_eq!(budget.available_permits(), 0);
    }

    #[tokio::test]
    async fn cancelled_write_retains_at_most_one_permit_until_stream_reset() {
        let budget = Arc::new(Semaphore::new(4));
        let mut blocker = BudgetedQueue::new(4);
        enqueue(&mut blocker, &budget, b"full");
        let control = control(4);
        let mut stream = stream(control, budget.clone());

        let mut write = Box::pin(stream.write(b"data"));
        assert!(poll!(write.as_mut()).is_pending());
        drop(write);

        blocker.clear();
        assert_eq!(budget.available_permits(), 3);

        // Starting shutdown resets PollSemaphore and cancels the abandoned
        // write waiter even though the TcpStream itself remains alive.
        let mut shutdown = Box::pin(stream.shutdown());
        assert!(poll!(shutdown.as_mut()).is_pending());
        drop(shutdown);
        assert_eq!(budget.available_permits(), 4);
    }

    #[tokio::test]
    async fn saturated_write_uses_partial_capacity_without_large_head_waiter() {
        let budget = Arc::new(Semaphore::new(4));
        let mut blocker = BudgetedQueue::new(3);
        enqueue(&mut blocker, &budget, b"use");
        let control = control(4);
        let mut stream = stream(control.clone(), budget.clone());

        assert_eq!(stream.write(b"data").await.unwrap(), 1);
        assert_eq!(control.lock().send_buffer.len(), 1);
        assert_eq!(budget.available_permits(), 0);
    }

    #[tokio::test]
    async fn socket_close_cancels_pending_global_budget_wait() {
        let budget = Arc::new(Semaphore::new(1));
        let mut blocker = BudgetedQueue::new(1);
        enqueue(&mut blocker, &budget, b"x");
        let control = control(1);
        let mut stream = stream(control.clone(), budget.clone());

        let mut write = Box::pin(stream.write(b"y"));
        assert!(poll!(write.as_mut()).is_pending());
        {
            let mut control = control.lock();
            control.send_state = TcpSocketState::Closed;
            control.send_waker.take().unwrap().wake();
        }
        assert_eq!(
            write.await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );

        blocker.clear();
        assert_eq!(budget.available_permits(), 1);
    }

    #[tokio::test]
    async fn per_connection_limit_backpressures_and_wakes_writer() {
        let budget = Arc::new(Semaphore::new(2));
        let control = control(1);
        enqueue(&mut control.lock().send_buffer, &budget, b"a");
        let mut stream = stream(control.clone(), budget.clone());

        let mut write = Box::pin(stream.write(b"b"));
        assert!(poll!(write.as_mut()).is_pending());
        let waker = {
            let mut control = control.lock();
            let mut byte = [0u8; 1];
            assert_eq!(control.send_buffer.dequeue_slice(&mut byte), 1);
            control.send_waker.take().unwrap()
        };
        waker.wake();
        assert_eq!(write.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn received_bytes_remain_readable_before_half_close_eof() {
        let recv_budget = Arc::new(Semaphore::new(4));
        let control = control(4);
        enqueue(&mut control.lock().recv_buffer, &recv_budget, b"data");
        control.lock().recv_state = TcpSocketState::Closed;
        let mut stream = stream(control, Arc::new(Semaphore::new(4)));

        let mut output = [0u8; 4];
        stream.read_exact(&mut output).await.unwrap();
        assert_eq!(&output, b"data");
        assert_eq!(recv_budget.available_permits(), 4);
        let mut eof = [0u8; 1];
        assert_eq!(stream.read(&mut eof).await.unwrap(), 0);
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
        let mut stream = stream(control.clone(), Arc::new(Semaphore::new(16)));

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
