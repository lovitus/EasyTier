//! Disposable Linux Core experiment, not a production feature.
//! One reader owns UDP_GRO; split before existing parsing/authentication/rings.
use bytes::BytesMut;
use nix::libc;
use std::{
    io, mem,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    os::fd::AsRawFd,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{io::Interest, net::UdpSocket};

const CAPACITY: usize = 65_536;
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Default, serde::Serialize)]
struct Stats {
    enabled: bool,
    option_error: Option<i32>,
    syscalls: u64,
    batches: u64,
    gro_batches: u64,
    datagrams: u64,
    copied_bytes: u64,
    truncated_datagrams: u64,
    rejected_batches: u64,
    max_batch: usize,
    scratch_bytes: usize,
}

struct Batch {
    bytes: Box<[u8]>,
    used: usize,
    offset: usize,
    stride: usize,
    address: SocketAddr,
    pending: bool,
}

pub(super) struct Receiver {
    socket: Arc<UdpSocket>,
    batch: Option<Batch>,
    stats: Stats,
    id: usize,
}

fn set_gro(socket: &UdpSocket, enabled: bool) -> io::Result<()> {
    let value = libc::c_int::from(enabled);
    // SAFETY: the borrowed descriptor and initialized option value are live.
    let ret = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_UDP,
            libc::UDP_GRO,
            (&value as *const libc::c_int).cast(),
            mem::size_of_val(&value) as _,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl Receiver {
    pub(super) fn new(socket: Arc<UdpSocket>) -> Self {
        Self::with_mode(
            socket,
            std::env::var("ET_ISSUE4_UDP_GRO").as_deref() == Ok("on"),
        )
    }

    fn with_mode(socket: Arc<UdpSocket>, enabled: bool) -> Self {
        let mut stats = Stats::default();
        let batch = if enabled {
            match set_gro(&socket, true) {
                Ok(()) => {
                    stats.enabled = true;
                    stats.scratch_bytes = CAPACITY;
                    Some(Batch {
                        bytes: vec![0; CAPACITY].into_boxed_slice(),
                        used: 0,
                        offset: 0,
                        stride: 0,
                        address: SocketAddr::from(([0, 0, 0, 0], 0)),
                        pending: false,
                    })
                }
                Err(error) => {
                    stats.option_error = error.raw_os_error();
                    None
                }
            }
        } else {
            None
        };
        Self {
            socket,
            batch,
            stats,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub(super) async fn recv(&mut self, output: &mut BytesMut) -> io::Result<(usize, SocketAddr)> {
        let Some(batch) = self.batch.as_mut() else {
            return self.socket.recv_buf_from(output).await;
        };
        loop {
            // A ready GRO batch must not bypass Tokio's per-datagram fairness.
            // Cancellation here preserves the next unsent segment in this owner.
            tokio::task::consume_budget().await;
            if !batch.pending {
                let result = self
                    .socket
                    .async_io(Interest::READABLE, || {
                        self.stats.syscalls += 1;
                        receive_batch(&self.socket, &mut batch.bytes)
                    })
                    .await;
                let received = match result {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    other => other?,
                };
                let Some((used, stride, address, gro)) = received else {
                    self.stats.rejected_batches += 1;
                    continue;
                };
                self.stats.batches += 1;
                self.stats.gro_batches += u64::from(gro);
                self.stats.max_batch =
                    self.stats
                        .max_batch
                        .max(if used == 0 { 1 } else { used.div_ceil(stride) });
                batch.used = used;
                batch.stride = stride;
                batch.offset = 0;
                batch.address = address;
                batch.pending = true;
            }
            let end = batch.offset + batch.stride.min(batch.used - batch.offset);
            // Match recv_buf_from's current spare capacity, including truncation.
            // Never retain the 64 KiB batch allocation in a queued ZCPacket.
            let copied = (end - batch.offset).min(output.capacity() - output.len());
            output.extend_from_slice(&batch.bytes[batch.offset..batch.offset + copied]);
            self.stats.truncated_datagrams += u64::from(copied != end - batch.offset);
            batch.offset = end;
            batch.pending = end < batch.used;
            self.stats.datagrams += 1;
            self.stats.copied_bytes += copied as u64;
            return Ok((copied, batch.address));
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if self.stats.enabled {
            // Current audited callers never hand an established data socket back
            // to a handshake reader. Restore the option before releasing our Arc.
            let _ = set_gro(&self.socket, false);
        }
        if let Some(directory) = std::env::var_os("ET_ISSUE4_GRO_METRICS") {
            let path = std::path::PathBuf::from(directory).join(format!(
                "{}-{}-{}.json",
                std::process::id(),
                self.socket.as_raw_fd(),
                self.id
            ));
            if let Ok(data) = serde_json::to_vec(&self.stats) {
                if let Err(error) = std::fs::write(path, data) {
                    eprintln!("ISSUE4_GRO_METRICS_FAILED: {error}");
                }
            }
        }
    }
}

// Ancillary integers are native c_int, not the UDP_SEGMENT cmsg's u16.
fn gro_stride(control: &[usize], length: usize, flags: libc::c_int) -> Option<Option<usize>> {
    if flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 || length > mem::size_of_val(control) {
        return None;
    }
    let header_len = unsafe { libc::CMSG_LEN(0) } as usize;
    let mut offset = 0;
    let mut stride = None;
    while length - offset >= mem::size_of::<libc::cmsghdr>() {
        // SAFETY: bounded read of a complete header; alignment is not assumed.
        let header = unsafe {
            ptr::read_unaligned(
                control
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<libc::cmsghdr>(),
            )
        };
        let size = header.cmsg_len as usize;
        if size < header_len || size > length - offset {
            return None;
        }
        if header.cmsg_level == libc::IPPROTO_UDP && header.cmsg_type == libc::UDP_GRO {
            if stride.is_some() || size != header_len + mem::size_of::<libc::c_int>() {
                return None;
            }
            // SAFETY: cmsg size above proves the full native integer is present.
            let value = unsafe {
                ptr::read_unaligned(
                    control
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + header_len)
                        .cast::<libc::c_int>(),
                )
            };
            if value <= 0 {
                return None;
            }
            stride = Some(value as usize);
        }
        let aligned = (size + mem::size_of::<usize>() - 1) & !(mem::size_of::<usize>() - 1);
        if aligned > length - offset {
            break;
        }
        offset += aligned;
    }
    Some(stride)
}

type Received = (usize, usize, SocketAddr, bool);
fn receive_batch(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<Option<Received>> {
    let mut control = [0usize; 8];
    // SAFETY: initialized C storage is writable for this recvmsg only. No raw
    // pointer crosses an await or escapes this synchronous readiness closure.
    let mut address: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    let mut vector = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    message.msg_name = (&mut address as *mut libc::sockaddr_storage).cast();
    message.msg_namelen = mem::size_of_val(&address) as _;
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = mem::size_of_val(&control) as _;
    let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_DONTWAIT) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    let Some(stride) = gro_stride(&control, message.msg_controllen as usize, message.msg_flags)
    else {
        return Ok(None);
    };
    if length as usize > buffer.len() {
        return Ok(None);
    }
    let source = match libc::c_int::from(address.ss_family) {
        libc::AF_INET if message.msg_namelen as usize >= mem::size_of::<libc::sockaddr_in>() => {
            let addr = unsafe {
                ptr::read_unaligned(
                    (&address as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>(),
                )
            };
            SocketAddr::from((
                Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
                u16::from_be(addr.sin_port),
            ))
        }
        libc::AF_INET6 if message.msg_namelen as usize >= mem::size_of::<libc::sockaddr_in6>() => {
            let addr = unsafe {
                ptr::read_unaligned(
                    (&address as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>(),
                )
            };
            SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(addr.sin6_addr.s6_addr),
                u16::from_be(addr.sin6_port),
                u32::from_be(addr.sin6_flowinfo),
                addr.sin6_scope_id,
            ))
        }
        _ => return Ok(None),
    };
    Ok(Some((
        length as usize,
        stride.unwrap_or(length as usize),
        source,
        stride.is_some(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicBool, time::Duration};

    fn send_gso(socket: &std::net::UdpSocket, address: SocketAddr, data: &[u8]) {
        let value: libc::c_int = 1400;
        let ret = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_UDP,
                libc::UDP_SEGMENT,
                (&value as *const libc::c_int).cast(),
                mem::size_of_val(&value) as _,
            )
        };
        assert_eq!(
            ret,
            0,
            "UDP_SEGMENT prerequisite: {}",
            io::Error::last_os_error()
        );
        assert_eq!(socket.send_to(data, address).unwrap(), data.len());
    }

    #[tokio::test]
    async fn issue4_gro_kernel_contract_and_unsplit_negative_control() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let socket = Arc::new(UdpSocket::bind(bind).await.unwrap());
            let sender = std::net::UdpSocket::bind(bind).unwrap();
            let control = std::net::UdpSocket::bind(bind).unwrap();
            let mut reader = Receiver::with_mode(socket.clone(), true);
            assert!(
                reader.stats.enabled,
                "GRO capability required for this experiment"
            );
            let payload: Vec<u8> = (0..(8 * 1400 + 333)).map(|i| (i / 1400) as u8).collect();
            send_gso(&sender, socket.local_addr().unwrap(), &payload);
            let mut raw = BytesMut::with_capacity(CAPACITY);
            // Negative control: retain the kernel option but bypass the splitter.
            // The old raw reader cannot meet the per-datagram boundary contract.
            let (length, _) =
                tokio::time::timeout(Duration::from_secs(2), socket.recv_buf_from(&mut raw))
                    .await
                    .unwrap()
                    .unwrap();
            assert!(
                length > 1400,
                "negative control did not observe a GRO aggregate"
            );
            println!(
                "GRO negative control: unsplit reader returned {length} bytes instead of 1400"
            );
            send_gso(&sender, socket.local_addr().unwrap(), &payload);
            control
                .send_to(b"control", socket.local_addr().unwrap())
                .unwrap();
            control.send_to(&[], socket.local_addr().unwrap()).unwrap();
            for expected in payload.chunks(1400) {
                let mut packet = BytesMut::with_capacity(2000);
                let (length, source) =
                    tokio::time::timeout(Duration::from_secs(2), reader.recv(&mut packet))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(source, sender.local_addr().unwrap());
                assert_eq!(length, expected.len());
                assert_eq!(&packet[..], expected);
                assert!(
                    packet.capacity() < CAPACITY,
                    "queued packet retained aggregate capacity"
                );
            }
            for expected in [b"control".as_slice(), b"".as_slice()] {
                let mut packet = BytesMut::with_capacity(2000);
                let (_, source) =
                    tokio::time::timeout(Duration::from_secs(2), reader.recv(&mut packet))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(source, control.local_addr().unwrap());
                assert_eq!(&packet[..], expected);
            }
            assert_eq!(reader.stats.datagrams, 11);
            assert!(reader.stats.gro_batches > 0 && reader.stats.max_batch >= 8);
            assert_eq!(reader.stats.rejected_batches, 0);
            drop(reader);
            control
                .send_to(b"after-drop", socket.local_addr().unwrap())
                .unwrap();
            let mut packet = BytesMut::with_capacity(2000);
            tokio::time::timeout(Duration::from_secs(2), socket.recv_buf_from(&mut packet))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&packet[..], b"after-drop");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn issue4_gro_buffered_segments_preserve_cooperative_progress() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let mut reader = Receiver::with_mode(socket, true);
        let batch = reader.batch.as_mut().expect("GRO required");
        batch.used = 512;
        batch.stride = 1;
        batch.pending = true;
        let progress = Arc::new(AtomicBool::new(false));
        let notified = progress.clone();
        let task = tokio::spawn(async move {
            notified.store(true, Ordering::SeqCst);
        });
        for _ in 0..512 {
            let mut packet = BytesMut::with_capacity(1);
            assert_eq!(reader.recv(&mut packet).await.unwrap().0, 1);
        }
        assert!(
            progress.load(Ordering::SeqCst),
            "buffered packets starved another ready task"
        );
        task.await.unwrap();
        assert_eq!(reader.stats.syscalls, 0);
    }

    #[test]
    fn issue4_gro_ancillary_security_boundaries() {
        let mut control = [0usize; 8];
        let size = unsafe { libc::CMSG_LEN(mem::size_of::<libc::c_int>() as _) } as usize;
        unsafe {
            let header = control.as_mut_ptr().cast::<libc::cmsghdr>();
            (*header).cmsg_level = libc::IPPROTO_UDP;
            (*header).cmsg_type = libc::UDP_GRO;
            (*header).cmsg_len = size as _;
            ptr::write_unaligned(libc::CMSG_DATA(header).cast::<libc::c_int>(), 1400);
        }
        assert_eq!(gro_stride(&control, size, 0), Some(Some(1400)));
        for (length, flags) in [
            (size, libc::MSG_TRUNC),
            (size, libc::MSG_CTRUNC),
            (size - 1, 0),
            (1000, 0),
        ] {
            assert_eq!(gro_stride(&control, length, flags), None);
        }
        for value in [0, -1] {
            unsafe {
                ptr::write_unaligned(
                    libc::CMSG_DATA(control.as_mut_ptr().cast()).cast::<libc::c_int>(),
                    value,
                );
            }
            assert_eq!(gro_stride(&control, size, 0), None);
        }
        assert_eq!(gro_stride(&[], 0, 0), Some(None));
    }
}
