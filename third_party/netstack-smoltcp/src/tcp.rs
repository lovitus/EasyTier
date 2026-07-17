use std::{
    collections::HashMap,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
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
        mpsc::{unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender},
        Notify,
    },
};
use tracing::{error, trace};

use crate::{
    device::VirtualDevice,
    packet::{AnyIpPktFrame, IpPacket},
    Runner,
};

// This userspace TCP connection terminates at the local TUN client. Keep two
// complete TLS/AEAD records per direction without scaling fixed memory with WAN RTT.
const DEFAULT_TCP_SEND_BUFFER_SIZE: usize = 32 * 1024;
const DEFAULT_TCP_RECV_BUFFER_SIZE: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TcpSocketState {
    Normal,
    Close,
    Closing,
    Closed,
}

struct TcpSocketControl {
    send_buffer: RingBuffer<'static, u8>,
    send_waker: Option<Waker>,
    recv_buffer: RingBuffer<'static, u8>,
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
        stream_tx: UnboundedSender<TcpStream>,
        sockets: HashMap<SocketHandle, SharedControl>,
    ) -> Runner {
        Runner::new(async move {
            let notify = Arc::new(Notify::new());
            let (socket_tx, socket_rx) = unbounded_channel::<TcpSocketCreation>();
            let res = tokio::select! {
                v = Self::handle_packet(notify.clone(), iface_ingress_tx, tcp_rx, stream_tx, socket_tx) => v,
                v = Self::handle_socket(notify, device, iface, sockets, socket_rx) => v,
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
        stream_tx: UnboundedSender<TcpStream>,
        socket_tx: UnboundedSender<TcpSocketCreation>,
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
                    TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE]),
                    TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE]),
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
                    send_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE]),
                    send_waker: None,
                    recv_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE]),
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
                    })
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
                socket_tx
                    .send(TcpSocketCreation { control, socket })
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
        mut socket_rx: UnboundedReceiver<TcpSocketCreation>,
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
    stream_rx: UnboundedReceiver<TcpStream>,
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

        let (stream_tx, stream_rx) = unbounded_channel();

        let runner = TcpListenerRunner::create(
            device,
            iface,
            iface_ingress_tx,
            tcp_rx,
            stream_tx,
            HashMap::new(),
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
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let mut control = self.control.lock();

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
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut control = self.control.lock();

        // If state == Close | Closing | Closed, the TCP stream WR half is closed.
        if !matches!(control.send_state, TcpSocketState::Normal) {
            return Err(std::io::ErrorKind::BrokenPipe.into()).into();
        }

        // Write to buffer

        if control.send_buffer.is_full() {
            if let Some(old_waker) = control.send_waker.replace(cx.waker().clone()) {
                if !old_waker.will_wake(cx.waker()) {
                    old_waker.wake();
                }
            }

            return Poll::Pending;
        }

        let n = control.send_buffer.enqueue_slice(buf);

        if n > 0 {
            self.notify.notify_one();
        }

        Ok(n).into()
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Ok(()).into()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
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
    use tokio::io::AsyncWriteExt as _;

    #[test]
    fn default_tcp_buffers_are_32_kib_per_layer() {
        assert_eq!(DEFAULT_TCP_SEND_BUFFER_SIZE, 32 * 1024);
        assert_eq!(DEFAULT_TCP_RECV_BUFFER_SIZE, 32 * 1024);

        let socket = TcpSocket::new(
            TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE]),
            TcpSocketBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE]),
        );
        assert_eq!(socket.recv_capacity(), 32 * 1024);
        assert_eq!(socket.send_capacity(), 32 * 1024);

        let control = TcpSocketControl {
            send_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE]),
            send_waker: None,
            recv_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE]),
            recv_waker: None,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        };
        assert_eq!(control.send_buffer.capacity(), 32 * 1024);
        assert_eq!(control.recv_buffer.capacity(), 32 * 1024);
    }

    #[tokio::test]
    async fn default_stream_send_buffer_backpressures_and_wakes_at_32_kib() {
        let control = Arc::new(SpinMutex::new(TcpSocketControl {
            send_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_SEND_BUFFER_SIZE]),
            send_waker: None,
            recv_buffer: RingBuffer::new(vec![0u8; DEFAULT_TCP_RECV_BUFFER_SIZE]),
            recv_waker: None,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        }));
        let mut stream = TcpStream {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
            notify: Arc::new(Notify::new()),
            control: control.clone(),
        };

        let full_buffer = vec![0xaa; DEFAULT_TCP_SEND_BUFFER_SIZE];
        assert_eq!(stream.write(&full_buffer).await.unwrap(), full_buffer.len());

        let mut blocked_write = Box::pin(stream.write(&[0xbb]));
        assert!(poll!(blocked_write.as_mut()).is_pending());
        assert!(control.lock().send_waker.is_some());

        let mut drained = [0u8; 1];
        let waker = {
            let mut control = control.lock();
            assert_eq!(control.send_buffer.dequeue_slice(&mut drained), 1);
            control.send_waker.take().unwrap()
        };
        waker.wake();

        assert_eq!(blocked_write.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn shutdown_completes_when_fin_is_committed() {
        let control = Arc::new(SpinMutex::new(TcpSocketControl {
            send_buffer: RingBuffer::new(vec![0u8; 16]),
            send_waker: None,
            recv_buffer: RingBuffer::new(vec![0u8; 16]),
            recv_waker: None,
            recv_state: TcpSocketState::Normal,
            send_state: TcpSocketState::Normal,
        }));
        let mut stream = TcpStream {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
            notify: Arc::new(Notify::new()),
            control: control.clone(),
        };

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
