//! Private Linux UDP submission. Protocol framing and connection ownership stay
//! in the existing UDP tunnel; rejected GSO only disables this writer's batching.
use super::{RingSink, RingStream, RingTunnel, TunnelError, UdpPacketType, ZCPacket, ZCPacketType};
use bytes::Bytes;
#[cfg(test)]
use bytes::BytesMut;
use futures::{FutureExt, Sink, StreamExt, ready};
use nix::libc;
use std::{
    collections::VecDeque,
    io, mem,
    net::SocketAddr,
    os::fd::AsRawFd,
    pin::Pin,
    ptr,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{io::Interest, net::UdpSocket};

const STAGE: usize = 32;
pub(super) const RING: usize = 64;
const WIRE: usize = 32;
const MAX_BYTES: usize = 60_000;

// Do not retain a 128-slot ring in addition to staging and writer batches.
// The existing shared MPSC budget is unchanged: STAGE + RING + WIRE = 128.
struct StagedSink {
    inner: RingSink,
    pending: VecDeque<ZCPacket>,
    closed: bool,
}
impl StagedSink {
    fn new(inner: RingSink) -> Self {
        Self {
            inner,
            pending: VecDeque::with_capacity(STAGE),
            closed: false,
        }
    }
}
impl Sink<ZCPacket> for StagedSink {
    type Error = TunnelError;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.closed {
            return Poll::Ready(Err(TunnelError::Shutdown));
        }
        if self.pending.len() == STAGE {
            ready!(self.as_mut().poll_flush(cx))?;
        }
        // RingSink readiness does not reserve an item. Detect receiver closure
        // now, while allowing the remaining bounded staging slots under pressure.
        if let Poll::Ready(Err(error)) = Pin::new(&mut self.inner).poll_ready(cx) {
            return Poll::Ready(Err(error));
        }
        Poll::Ready(Ok(()))
    }
    fn start_send(mut self: Pin<&mut Self>, packet: ZCPacket) -> Result<(), Self::Error> {
        if self.closed {
            return Err(TunnelError::Shutdown);
        }
        if self.pending.len() >= STAGE {
            return Err(TunnelError::BufferFull);
        }
        self.pending.push_back(packet);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if this.closed {
            return Poll::Ready(Ok(()));
        }
        while !this.pending.is_empty() {
            ready!(Pin::new(&mut this.inner).poll_ready(cx))?;
            let packet = this.pending.pop_front().unwrap();
            Pin::new(&mut this.inner).start_send(packet)?;
        }
        ready!(Pin::new(&mut this.inner).poll_flush(cx))?;
        Poll::Ready(Ok(()))
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        ready!(self.as_mut().poll_flush(cx))?;
        ready!(Pin::new(&mut self.inner).poll_close(cx))?;
        self.closed = true;
        Poll::Ready(Ok(()))
    }
}
pub(super) fn make(ring: Arc<RingTunnel>) -> Box<dyn crate::tunnel::ZCPacketSink + Unpin> {
    Box::new(StagedSink::new(RingSink::new(ring)))
}

// Only the disable bit exists in production. Counters are contract-test evidence,
// not per-connection telemetry or a new global metrics/lifecycle facility.
#[derive(Default)]
struct WriterStats {
    gso_disabled: bool,
    #[cfg(test)]
    calls: u64,
    #[cfg(test)]
    gso_calls: u64,
    #[cfg(test)]
    eagain: u64,
    #[cfg(test)]
    capability_fallbacks: u64,
    #[cfg(test)]
    packets: u64,
}

struct Frame {
    bytes: Bytes,
    batchable: bool,
}
fn frame(
    packet: ZCPacket,
    conn_id: u32,
    stealth: &crate::tunnel::stealth::OuterSessionState,
) -> Frame {
    // Handshake/control frames and the gate-key phase stay on single sends.
    let batchable =
        packet.is_lossy() && (!stealth.is_enabled() || stealth.outer_key_elapsed().is_some());
    let mut packet = packet.convert_type(ZCPacketType::UDP);
    let len = packet.udp_payload().len();
    let header = packet.mut_udp_tunnel_header().unwrap();
    header.conn_id.set(conn_id);
    header.len.set(len as u16);
    header.msg_type = UdpPacketType::Data as u8;
    let raw = packet.into_bytes();
    let bytes = match stealth.seal_datagram(&raw) {
        Some(ciphertext) => Bytes::from(ciphertext),
        None => raw,
    };
    Frame { bytes, batchable }
}
fn group_end(frames: &[Frame], begin: usize) -> usize {
    let size = frames[begin].bytes.len();
    if !frames[begin].batchable || size == 0 || size > u16::MAX as usize {
        return begin + 1;
    }
    let mut end = begin + 1;
    let mut bytes = size;
    while end < frames.len() && end - begin < WIRE {
        let next = &frames[end];
        if !next.batchable || next.bytes.len() > size || bytes + next.bytes.len() > MAX_BYTES {
            break;
        }
        bytes += next.bytes.len();
        end += 1;
        if next.bytes.len() < size {
            break;
        }
    }
    end
}

async fn send_group(
    socket: &UdpSocket,
    address: SocketAddr,
    frames: &[Frame],
    _stats: &mut WriterStats,
) -> io::Result<usize> {
    let expected: usize = frames.iter().map(|p| p.bytes.len()).sum();
    let segment = frames[0].bytes.len() as u16;
    loop {
        // All raw C pointers are constructed inside the synchronous closure.
        // The async future retains only Send-safe owned/borrowed Rust values.
        let result = socket
            .async_io(Interest::WRITABLE, || {
                #[cfg(test)]
                {
                    _stats.calls += 1;
                    _stats.gso_calls += 1;
                }
                let destination = socket2::SockAddr::from(address);
                let mut vectors = [libc::iovec {
                    iov_base: ptr::null_mut(),
                    iov_len: 0,
                }; WIRE];
                for (item, packet) in vectors.iter_mut().zip(frames) {
                    item.iov_base = packet.bytes.as_ptr() as *mut libc::c_void;
                    item.iov_len = packet.bytes.len();
                }
                let mut control = [0usize; 4];
                // SAFETY: initialized C records and live byte slices; no pointer
                // is retained beyond sendmsg. Control storage is cmsghdr-aligned.
                let mut message: libc::msghdr = unsafe { mem::zeroed() };
                message.msg_name = destination.as_ptr() as *mut libc::c_void;
                message.msg_namelen = destination.len();
                message.msg_iov = vectors.as_mut_ptr();
                message.msg_iovlen = frames.len() as _;
                message.msg_control = control.as_mut_ptr().cast();
                message.msg_controllen =
                    unsafe { libc::CMSG_SPACE(mem::size_of::<u16>() as _) } as _;
                assert!(message.msg_controllen as usize <= mem::size_of_val(&control));
                unsafe {
                    let header = libc::CMSG_FIRSTHDR(&message);
                    assert!(!header.is_null());
                    (*header).cmsg_level = libc::IPPROTO_UDP;
                    (*header).cmsg_type = libc::UDP_SEGMENT;
                    (*header).cmsg_len = libc::CMSG_LEN(mem::size_of::<u16>() as _) as _;
                    ptr::write_unaligned(libc::CMSG_DATA(header).cast::<u16>(), segment);
                }
                let n = unsafe {
                    libc::sendmsg(
                        socket.as_raw_fd(),
                        &message,
                        libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                    )
                };
                if n < 0 {
                    let error = io::Error::last_os_error();
                    #[cfg(test)]
                    {
                        _stats.eagain += u64::from(error.kind() == io::ErrorKind::WouldBlock);
                    }
                    Err(error)
                } else {
                    Ok(n as usize)
                }
            })
            .await;
        match result {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Ok(n) if n != expected => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short GSO submission; not replayed",
                ));
            }
            other => return other,
        }
    }
}
async fn send_frames(
    socket: &UdpSocket,
    address: SocketAddr,
    frames: &[Frame],
    stats: &mut WriterStats,
) -> io::Result<bool> {
    let mut begin = 0;
    while begin < frames.len() {
        let mut end = if !stats.gso_disabled {
            group_end(frames, begin)
        } else {
            begin + 1
        };
        let written = if end == begin + 1 {
            #[cfg(test)]
            {
                stats.calls += 1;
            }
            socket.send_to(&frames[begin].bytes, address).await?
        } else {
            match send_group(socket, address, &frames[begin..end], stats).await {
                Ok(written) => written,
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::EINVAL | libc::EIO | libc::ENOPROTOOPT | libc::EOPNOTSUPP)
                    ) =>
                {
                    // The rejected sendmsg accepted no group bytes. Keep the
                    // already-sealed frames and resume at this group's first
                    // item, not at the start of the vector. Subsequent groups
                    // and calls use singles for this writer's lifetime.
                    // Do not apply this to TUN send_multiple (partial writes)
                    // or to the synthetic short-success error from send_group.
                    stats.gso_disabled = true;
                    #[cfg(test)]
                    {
                        stats.capability_fallbacks += 1;
                    }
                    tracing::debug!(%error, "UDP GSO submission rejected; using individual datagrams");
                    end = begin + 1;
                    #[cfg(test)]
                    {
                        stats.calls += 1;
                    }
                    socket.send_to(&frames[begin].bytes, address).await?
                }
                Err(error) => return Err(error),
            }
        };
        if written == 0 {
            return Ok(false);
        }
        let expected: usize = frames[begin..end]
            .iter()
            .map(|frame| frame.bytes.len())
            .sum();
        if written != expected {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short UDP submission; not replayed",
            ));
        }
        #[cfg(test)]
        {
            stats.packets += (end - begin) as u64;
        }
        begin = end;
    }
    Ok(true)
}

pub(super) async fn forward(
    mut receiver: RingStream,
    socket: &Arc<UdpSocket>,
    address: &SocketAddr,
    conn_id: u32,
    stealth: &Arc<crate::tunnel::stealth::OuterSessionState>,
) -> Option<TunnelError> {
    let mut stats = WriterStats::default();
    let mut frames = Vec::with_capacity(WIRE);
    loop {
        let first = match receiver.next().await? {
            Ok(packet) => packet,
            Err(error) => return Some(error),
        };
        frames.push(frame(first, conn_id, stealth));
        let mut terminal = None;
        // Drain only immediately ready items; no coalescing timer or new task.
        while frames.len() < WIRE {
            match receiver.next().now_or_never() {
                Some(Some(Ok(packet))) => frames.push(frame(packet, conn_id, stealth)),
                Some(Some(Err(error))) => {
                    terminal = Some(Some(error));
                    break;
                }
                Some(None) => {
                    terminal = Some(None);
                    break;
                }
                None => break,
            }
        }
        match send_frames(socket, *address, &frames, &mut stats).await {
            Ok(true) => {}
            Ok(false) => return None,
            Err(error) => return Some(TunnelError::IOError(error)),
        }
        frames.clear();
        if let Some(result) = terminal {
            return result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::SinkExt;
    use futures::task::noop_waker;
    use tokio::time::{Duration, timeout};
    fn packet(payload: &[u8]) -> ZCPacket {
        let offset = ZCPacketType::NIC.get_packet_offsets().payload_offset;
        let mut bytes = BytesMut::zeroed(offset + payload.len());
        bytes[offset..].copy_from_slice(payload);
        let mut packet = ZCPacket::new_from_buf(bytes, ZCPacketType::NIC);
        packet.fill_peer_manager_hdr(1, 2, crate::tunnel::packet_def::PacketType::Data as u8);
        packet
    }
    #[test]
    fn udp_gso_budget_and_independent_wire_vector() {
        assert_eq!(STAGE + RING + WIRE, 128);
        let state = crate::tunnel::stealth::OuterSessionState::disabled();
        let actual = frame(packet(b"abc"), 0x11223344, &state);
        let expected = [
            0x44, 0x33, 0x22, 0x11, 3, 0, 19, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 1, 0, 3, 0, 0, 0,
            b'a', b'b', b'c',
        ];
        assert_eq!(&actual.bytes[..], &expected);
    }
    #[tokio::test]
    async fn udp_gso_preserves_ready_group_and_close() {
        let ring = Arc::new(RingTunnel::new(RING));
        let mut reader = RingStream::new(ring.clone());
        let mut writer = StagedSink::new(RingSink::new(ring));
        for value in 0..STAGE {
            writer.feed(packet(&[value as u8])).await.unwrap();
        }
        assert!(reader.next().now_or_never().is_none());
        writer.close().await.unwrap();
        for value in 0..STAGE {
            let p = timeout(Duration::from_secs(1), reader.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(p.payload(), &[value as u8]);
        }
        assert!(reader.next().await.is_none());
        assert!(writer.feed(packet(b"closed")).await.is_err());
    }
    #[tokio::test]
    async fn udp_gso_backpressure_keeps_pending_items() {
        let ring = Arc::new(RingTunnel::new(RING));
        let mut reader = RingStream::new(ring.clone());
        let mut inner = RingSink::new(ring);
        for _ in 0..RING {
            inner.feed(packet(b"old")).await.unwrap();
        }
        let mut writer = StagedSink::new(inner);
        writer.feed(packet(b"new")).await.unwrap();
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(Pin::new(&mut writer).poll_flush(&mut context).is_pending());
        assert_eq!(writer.pending.len(), 1);
        assert_eq!(reader.next().await.unwrap().unwrap().payload(), b"old");
        writer.flush().await.unwrap();
        writer.close().await.unwrap();
        for _ in 1..RING {
            assert_eq!(reader.next().await.unwrap().unwrap().payload(), b"old");
        }
        assert_eq!(reader.next().await.unwrap().unwrap().payload(), b"new");
        assert!(reader.next().await.is_none());
    }
    #[tokio::test]
    async fn udp_gso_detects_closed_consumer() {
        let ring = Arc::new(RingTunnel::new(RING));
        let reader = RingStream::new(ring.clone());
        let mut writer = StagedSink::new(RingSink::new(ring));
        drop(reader);
        assert!(writer.feed(packet(b"unaccepted")).await.is_err());
        assert!(writer.pending.is_empty());
    }
    #[tokio::test]
    async fn udp_gso_cancelled_flush_retains_order_and_wakes() {
        timeout(Duration::from_secs(3), async {
            let ring = Arc::new(RingTunnel::new(RING));
            let mut reader = RingStream::new(ring.clone());
            let mut inner = RingSink::new(ring);
            for value in 0..RING {
                inner.feed(packet(&[value as u8])).await.unwrap();
            }
            let mut writer = StagedSink::new(inner);
            for value in RING..RING + STAGE {
                writer.feed(packet(&[value as u8])).await.unwrap();
            }

            // Cancel a genuinely pending flush without dropping its owner.
            {
                let mut flush = Box::pin(writer.flush());
                futures::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(flush.as_mut(), cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            assert_eq!(writer.pending.len(), STAGE);

            // join! polls the blocked writer before the reader frees space.
            // Completion therefore also exercises the ring's real wakeup.
            let ((), received) = futures::join!(
                async {
                    writer.flush().await.unwrap();
                    writer.close().await.unwrap();
                    assert!(writer.pending.is_empty());
                },
                async {
                    let mut received = Vec::new();
                    while let Some(value) = reader.next().await {
                        received.push(value.unwrap().payload().to_vec());
                    }
                    received
                }
            );
            let expected = (0..RING + STAGE)
                .map(|value| vec![value as u8])
                .collect::<Vec<_>>();
            assert_eq!(received, expected);
        })
        .await
        .expect("cancelled flush must recover without a lost wakeup");
    }
    #[test]
    fn udp_gso_control_and_short_tail_boundaries() {
        let make = |n, batchable| Frame {
            bytes: Bytes::from(vec![1; n]),
            batchable,
        };
        let frames = [
            make(100, true),
            make(100, true),
            make(40, true),
            make(100, true),
        ];
        assert_eq!(group_end(&frames, 0), 3);
        assert_eq!(group_end(&frames, 2), 3);
        let control = [make(100, true), make(100, false), make(100, true)];
        assert_eq!(group_end(&control, 0), 1);
        assert_eq!(group_end(&control, 1), 2);
    }
    #[test]
    fn udp_gso_seals_each_packet_through_gate_and_outer() {
        let sender = crate::tunnel::stealth::OuterSessionState::new(b"test-only".to_vec(), 60);
        let receiver = crate::tunnel::stealth::OuterSessionState::new(b"test-only".to_vec(), 60);
        let plain = frame(
            packet(b"phase"),
            7,
            &crate::tunnel::stealth::OuterSessionState::disabled(),
        )
        .bytes;
        let gate = frame(packet(b"phase"), 7, &sender);
        assert!(!gate.batchable);
        assert_eq!(receiver.open_datagram(&gate.bytes).unwrap(), plain);
        sender.set_outer_key_with_cipher(b"handshake-test", Some("aes-256-gcm"));
        receiver.set_outer_key_with_cipher(b"handshake-test", Some("aes-256-gcm"));
        let outer1 = frame(packet(b"phase"), 7, &sender);
        let outer2 = frame(packet(b"phase"), 7, &sender);
        assert!(outer1.batchable && outer2.batchable);
        assert_ne!(outer1.bytes, outer2.bytes);
        assert_eq!(receiver.open_datagram(&outer1.bytes).unwrap(), plain);
        assert_eq!(receiver.open_datagram(&outer2.bytes).unwrap(), plain);
    }
    #[tokio::test]
    async fn udp_gso_real_kernel_rejection_falls_back_without_replay() {
        timeout(Duration::from_secs(3), async {
            let tx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let rx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let enabled: libc::c_int = 1;
            // Linux udp_send_skb rejects UDP_SEGMENT with EINVAL when
            // sk_no_check_tx is set. This modifies only this test socket;
            // individual UDP sends remain valid. No injected send results.
            assert_eq!(
                unsafe {
                    libc::setsockopt(
                        tx.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_NO_CHECK,
                        (&enabled as *const libc::c_int).cast(),
                        mem::size_of_val(&enabled) as libc::socklen_t,
                    )
                },
                0
            );
            let state = crate::tunnel::stealth::OuterSessionState::disabled();
            let mut frames = (0..6)
                .map(|id| frame(packet(&[id; 100]), 7, &state))
                .collect::<Vec<_>>();
            frames[0].batchable = false;
            frames[5].batchable = false;
            let mut raw_stats = WriterStats::default();
            let error = send_group(&tx, rx.local_addr().unwrap(), &frames[1..5], &mut raw_stats)
                .await
                .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
            let mut stats = WriterStats::default();
            for pass in 1..=2 {
                assert!(
                    send_frames(&tx, rx.local_addr().unwrap(), &frames, &mut stats)
                        .await
                        .unwrap()
                );
                for expected in &frames {
                    let mut bytes = [0u8; 2048];
                    let (len, source) = rx.recv_from(&mut bytes).await.unwrap();
                    assert_eq!(source, tx.local_addr().unwrap());
                    assert_eq!(&bytes[..len], &expected.bytes[..]);
                }
                assert!(stats.gso_disabled);
                assert_eq!(stats.capability_fallbacks, 1);
                assert_eq!(stats.gso_calls, 1);
                assert_eq!(stats.packets, pass * frames.len() as u64);
            }
            let mut extra = [0u8; 2048];
            assert!(
                timeout(Duration::from_millis(30), rx.recv_from(&mut extra))
                    .await
                    .is_err(),
                "rejected GSO or earlier successful group must not be replayed"
            );
        })
        .await
        .expect("bounded kernel fallback contract");
    }
    #[tokio::test]
    async fn udp_gso_gso_ipv4_ipv6_and_shared_socket_progress() {
        for host in ["127.0.0.1:0", "[::1]:0"] {
            let tx = Arc::new(UdpSocket::bind(host).await.unwrap());
            let mut tasks = Vec::new();
            for id in 1..=2 {
                let rx = UdpSocket::bind(host).await.unwrap();
                let destination = rx.local_addr().unwrap();
                let tx = tx.clone();
                tasks.push(tokio::spawn(async move {
                    let frames = (0..WIRE)
                        .map(|_| {
                            frame(
                                packet(&[id; 100]),
                                id as u32,
                                &crate::tunnel::stealth::OuterSessionState::disabled(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let mut stats = WriterStats::default();
                    send_frames(&tx, destination, &frames, &mut stats)
                        .await
                        .unwrap();
                    assert!(stats.gso_calls > 0);
                    for expected in frames {
                        let mut bytes = [0; 2048];
                        let (len, source) = rx.recv_from(&mut bytes).await.unwrap();
                        assert_eq!(source, tx.local_addr().unwrap());
                        assert_eq!(&bytes[..len], &expected.bytes[..]);
                    }
                }));
            }
            for task in tasks {
                timeout(Duration::from_secs(3), task)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
    }
}
