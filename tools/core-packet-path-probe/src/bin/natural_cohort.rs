//! Isolated Linux kernel-path experiment, NOT an EasyTier tunnel.
//! Real TUN GSO cohorts; no synthetic burst queue, crypto, routing, or peer model.

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs, io, mem,
        net::{SocketAddrV4, UdpSocket},
        os::fd::AsRawFd,
        ptr,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    use tun_rs::{DeviceBuilder, IDEAL_BATCH_SIZE, VIRTIO_NET_HDR_LEN};

    const MTU: usize = 1360;
    const MAX_SUBMISSION: usize = 60_000;
    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn stop(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Single,
        Mmsg,
        Gso,
        GsoGro,
    }

    #[derive(Default)]
    struct Stats {
        cohorts: u64,
        histogram: Vec<u64>,
        tx_packets: u64,
        rx_packets: u64,
        tx_bytes: u64,
        rx_bytes: u64,
        send_calls: u64,
        mmsg_calls: u64,
        gso_calls: u64,
        gso_segments: u64,
        send_eagain: u64,
        tun_writes: u64,
        rx_buffers: u64,
        rx_gro_buffers: u64,
        max_rx_segments: usize,
    }

    fn error(text: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, text)
    }

    fn enable_gro(socket: &UdpSocket) -> io::Result<()> {
        let enabled: libc::c_int = 1;
        // SAFETY: a live socket and a correctly sized int option value.
        if unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_UDP,
                libc::UDP_GRO,
                ptr::from_ref(&enabled).cast(),
                mem::size_of_val(&enabled) as _,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    // Linux v6.8 udp_cmsg_recv emits an int, unlike UDP_SEGMENT's u16.
    // Parse by value after checking bounds; no aligned pointer into kernel
    // control data escapes this function, on either GNU or musl.
    fn gro_size(control: &[u8], flags: libc::c_int) -> io::Result<Option<usize>> {
        if flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
            return Err(error("truncated UDP payload or ancillary data"));
        }
        let header_len = unsafe { libc::CMSG_LEN(0) } as usize;
        let alignment = mem::size_of::<usize>();
        let mut at = 0usize;
        let mut size = None;
        while control.len().saturating_sub(at) >= header_len {
            // SAFETY: an entire initialized cmsghdr is in bounds. Its integer
            // fields have no invalid bit patterns, and alignment is not assumed.
            let header =
                unsafe { ptr::read_unaligned(control[at..].as_ptr().cast::<libc::cmsghdr>()) };
            let len = header.cmsg_len as usize;
            if len < header_len || len > control.len() - at {
                return Err(error("invalid ancillary length"));
            }
            if header.cmsg_level == libc::IPPROTO_UDP && header.cmsg_type == libc::UDP_GRO {
                if size.is_some() || len - header_len != mem::size_of::<libc::c_int>() {
                    return Err(error("duplicate or malformed UDP_GRO value"));
                }
                let value =
                    i32::from_ne_bytes(control[at + header_len..at + len].try_into().unwrap());
                if value <= 0 || value as usize > MTU {
                    return Err(error("invalid UDP_GRO segment size"));
                }
                size = Some(value as usize);
            }
            at += (len + alignment - 1) & !(alignment - 1);
        }
        Ok(size)
    }

    fn recv_gro(
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> io::Result<(usize, std::net::SocketAddr, Option<usize>)> {
        let mut control = [0usize; 8];
        let mut iov = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        // SAFETY: zeroed integer/pointer C records, completed before the call.
        let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = ptr::from_mut(&mut addr).cast();
        msg.msg_namelen = mem::size_of_val(&addr) as _;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = mem::size_of_val(&control) as _;
        // SAFETY: all writable buffers stay owned for this synchronous call.
        let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize > buffer.len()
            || msg.msg_controllen as usize > mem::size_of_val(&control)
            || msg.msg_namelen as usize != mem::size_of_val(&addr)
            || addr.sin_family != libc::AF_INET as _
        {
            return Err(error("invalid received UDP metadata"));
        }
        // SAFETY: bounded view of the fully initialized control storage.
        let bytes = unsafe {
            std::slice::from_raw_parts(control.as_ptr().cast::<u8>(), msg.msg_controllen as usize)
        };
        let segment = gro_size(bytes, msg.msg_flags)?;
        let source = SocketAddrV4::new(
            addr.sin_addr.s_addr.to_ne_bytes().into(),
            u16::from_be(addr.sin_port),
        );
        Ok((n as usize, source.into(), segment))
    }

    fn segment_length(length: usize, gro: Option<usize>) -> io::Result<usize> {
        if length == 0 {
            return Err(error("empty UDP payload"));
        }
        let segment = gro.unwrap_or(length);
        if segment == 0 || segment > MTU || segment > length {
            return Err(error("invalid datagram boundary"));
        }
        Ok(segment)
    }

    fn tun_frame(
        buffer: &mut [u8],
        begin: usize,
        length: usize,
    ) -> io::Result<std::ops::Range<usize>> {
        let head = begin
            .checked_sub(VIRTIO_NET_HDR_LEN)
            .ok_or_else(|| error("missing TUN headroom"))?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| error("packet length overflow"))?;
        let packet = buffer
            .get(begin..end)
            .ok_or_else(|| error("packet exceeds receive buffer"))?;
        if length < 20 || length > MTU || packet[0] >> 4 != 4 {
            return Err(error("invalid isolated IPv4 packet"));
        }
        // Previously written datagrams are no longer borrowed. Reuse their last
        // header-sized bytes for the next virtio prefix, without copying payload.
        buffer[head..begin].fill(0);
        Ok(head..end)
    }

    // A GSO send may end with one short segment, but must not combine a
    // later segment after that short tail. Never wait for another TUN read.
    fn group_end(sizes: &[usize], begin: usize) -> usize {
        let size = sizes[begin];
        let mut end = begin + 1;
        let mut bytes = size;
        while end < sizes.len() && end - begin < 64 {
            let next = sizes[end];
            if next > size || bytes + next > MAX_SUBMISSION {
                break;
            }
            bytes += next;
            end += 1;
            if next < size {
                break;
            }
        }
        end
    }

    fn wait_fd(fd: libc::c_int, event: libc::c_short, deadline: Instant) -> io::Result<()> {
        loop {
            if STOP.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut poll = libc::pollfd {
                fd,
                events: event,
                revents: 0,
            };
            // SAFETY: one live, writable pollfd; millisecond timeout is bounded.
            let ret = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().min(1000) as i32) };
            if ret > 0 {
                if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    return Err(error("descriptor error while waiting for I/O"));
                }
                if poll.revents & event != 0 {
                    return Ok(());
                }
            } else if ret < 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
    }

    fn retry_send(fd: libc::c_int, deadline: Instant, stats: &mut Stats) -> io::Result<()> {
        let e = io::Error::last_os_error();
        match e.kind() {
            io::ErrorKind::Interrupted => Ok(()),
            io::ErrorKind::WouldBlock => {
                stats.send_eagain += 1;
                wait_fd(fd, libc::POLLOUT, deadline)
            }
            _ => Err(e),
        }
    }

    fn send_one(
        socket: &UdpSocket,
        peer: SocketAddrV4,
        packet: &[u8],
        stats: &mut Stats,
    ) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            stats.send_calls += 1;
            match socket.send_to(packet, peer) {
                Ok(n) if n == packet.len() => return Ok(()),
                Ok(_) => return Err(error("short UDP send")),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    stats.send_eagain += 1;
                    wait_fd(socket.as_raw_fd(), libc::POLLOUT, deadline)?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn address(peer: SocketAddrV4) -> libc::sockaddr_in {
        libc::sockaddr_in {
            sin_family: libc::AF_INET as _,
            sin_port: peer.port().to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(peer.ip().octets()),
            },
            sin_zero: [0; 8],
        }
    }

    fn send_mmsg(
        socket: &UdpSocket,
        peer: SocketAddrV4,
        packets: &[Vec<u8>],
        sizes: &[usize],
        stats: &mut Stats,
    ) -> io::Result<()> {
        let addr = address(peer);
        let mut iov: Vec<_> = packets
            .iter()
            .zip(sizes)
            .map(|(p, n)| libc::iovec {
                iov_base: p.as_ptr() as *mut libc::c_void,
                iov_len: *n,
            })
            .collect();
        let mut messages: Vec<libc::mmsghdr> = iov
            .iter_mut()
            .map(|io| {
                // SAFETY: zero is valid initialization for msghdr; all borrowed
                // payload/address/iovec memory remains live and unmoved until return.
                let mut m: libc::mmsghdr = unsafe { mem::zeroed() };
                m.msg_hdr.msg_name = ptr::from_ref(&addr) as *mut libc::c_void;
                m.msg_hdr.msg_namelen = mem::size_of_val(&addr) as _;
                m.msg_hdr.msg_iov = io;
                m.msg_hdr.msg_iovlen = 1;
                m
            })
            .collect();
        let mut sent = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while sent < messages.len() {
            stats.send_calls += 1;
            stats.mmsg_calls += 1;
            // SAFETY: the remaining initialized array and referenced buffers
            // live through this synchronous call; no pointer is retained.
            let n = unsafe {
                libc::sendmmsg(
                    socket.as_raw_fd(),
                    messages[sent..].as_mut_ptr(),
                    (messages.len() - sent) as _,
                    libc::MSG_DONTWAIT as _,
                )
            };
            if n < 0 {
                retry_send(socket.as_raw_fd(), deadline, stats)?;
                continue;
            }
            if n == 0 {
                return Err(error("sendmmsg made no progress"));
            }
            for i in sent..sent + n as usize {
                if messages[i].msg_len as usize != sizes[i] {
                    return Err(error("short sendmmsg datagram"));
                }
            }
            sent += n as usize; // only retry the unsent suffix
        }
        Ok(())
    }

    fn send_gso(
        socket: &UdpSocket,
        peer: SocketAddrV4,
        packets: &[Vec<u8>],
        sizes: &[usize],
        scratch: &mut Vec<u8>,
        stats: &mut Stats,
    ) -> io::Result<()> {
        let mut begin = 0;
        while begin < sizes.len() {
            let end = group_end(sizes, begin);
            if end == begin + 1 {
                send_one(socket, peer, &packets[begin][..sizes[begin]], stats)?;
                begin = end;
                continue;
            }
            scratch.clear();
            for i in begin..end {
                scratch.extend_from_slice(&packets[i][..sizes[i]]);
            }
            let addr = address(peer);
            let mut iov = libc::iovec {
                iov_base: scratch.as_mut_ptr().cast(),
                iov_len: scratch.len(),
            };
            // usize storage provides cmsghdr alignment on the tested Linux ABI.
            let mut control = [0usize; 4];
            // SAFETY: initialized storage for msghdr and one u16 ancillary item.
            let mut msg: libc::msghdr = unsafe { mem::zeroed() };
            msg.msg_name = ptr::from_ref(&addr) as *mut libc::c_void;
            msg.msg_namelen = mem::size_of_val(&addr) as _;
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(mem::size_of::<u16>() as _) } as _;
            if msg.msg_controllen as usize > mem::size_of_val(&control) {
                return Err(error("ancillary storage too short"));
            }
            // SAFETY: CMSG_FIRSTHDR refers into aligned initialized control,
            // sized above for the header and its u16 segment-size payload.
            unsafe {
                let c = libc::CMSG_FIRSTHDR(&msg);
                if c.is_null() {
                    return Err(error("missing cmsg header"));
                }
                (*c).cmsg_level = libc::IPPROTO_UDP;
                (*c).cmsg_type = libc::UDP_SEGMENT;
                (*c).cmsg_len = libc::CMSG_LEN(mem::size_of::<u16>() as _) as _;
                ptr::write_unaligned(libc::CMSG_DATA(c).cast::<u16>(), sizes[begin] as u16);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                stats.send_calls += 1;
                stats.gso_calls += 1;
                // SAFETY: all data and ancillary buffers remain owned by this
                // call; sendmsg does not mutate or retain them.
                let n = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, libc::MSG_DONTWAIT) };
                if n < 0 {
                    retry_send(socket.as_raw_fd(), deadline, stats)?;
                    continue;
                }
                if n as usize != scratch.len() {
                    return Err(error("short UDP GSO send"));
                }
                stats.gso_segments += (end - begin) as u64;
                break;
            }
            begin = end;
        }
        Ok(())
    }

    pub fn run() -> io::Result<()> {
        if std::env::var("ET_KERNEL_COHORT_LAB").as_deref() != Ok("1")
            || fs::read_link("/proc/self/ns/net")? == fs::read_link("/proc/1/ns/net")?
        {
            return Err(error(
                "requires ET_KERNEL_COHORT_LAB=1 inside an isolated netns",
            ));
        }
        let args: Vec<_> = std::env::args().collect();
        if args.len() != 4 {
            return Err(error(
                "usage: natural_cohort single|mmsg|gso|gso-gro BIND_V4 PEER_V4",
            ));
        }
        let mode = match args[1].as_str() {
            "single" => Mode::Single,
            "mmsg" => Mode::Mmsg,
            "gso" => Mode::Gso,
            "gso-gro" => Mode::GsoGro,
            _ => return Err(error("unknown mode")),
        };
        let local: SocketAddrV4 = args[2].parse().map_err(|_| error("invalid bind"))?;
        let peer: SocketAddrV4 = args[3].parse().map_err(|_| error("invalid peer"))?;
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        if mode == Mode::GsoGro {
            enable_gro(&socket)?;
        }
        let device = DeviceBuilder::new()
            .name("cohort0")
            .mtu(MTU as u16)
            .offload(true)
            .enable(false)
            .packet_information(false)
            .build_sync()?;
        if !device.tcp_gso() {
            return Err(error("TUN TCP GSO not active"));
        }
        // SAFETY: owned TUN fd; only adding nonblocking I/O to its current flags.
        unsafe {
            let flags = libc::fcntl(device.as_raw_fd(), libc::F_GETFL);
            if flags < 0
                || libc::fcntl(device.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
            {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        }
        let mut packets = vec![vec![0u8; 4096]; IDEAL_BATCH_SIZE];
        let mut sizes = vec![0usize; IDEAL_BATCH_SIZE];
        let mut original = vec![0; VIRTIO_NET_HDR_LEN + 65535];
        let mut receive = vec![0u8; VIRTIO_NET_HDR_LEN + 65535];
        let mut scratch = Vec::with_capacity(MAX_SUBMISSION);
        let mut stats = Stats {
            histogram: vec![0; IDEAL_BATCH_SIZE + 1],
            ..Stats::default()
        };
        println!("{{\"ready\":true,\"tun\":\"cohort0\",\"crypto\":false}}");
        let result = (|| -> io::Result<()> {
            while !STOP.load(Ordering::Relaxed) {
                let mut fds = [
                    libc::pollfd {
                        fd: device.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: socket.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: two writable pollfd records, bounded shutdown latency.
                if unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) } < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                if fds
                    .iter()
                    .any(|f| f.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0)
                {
                    return Err(error("poll descriptor failure"));
                }
                if fds[1].revents & libc::POLLIN != 0 {
                    let mut received = 0;
                    while received < 64 {
                        let incoming = if mode == Mode::GsoGro {
                            recv_gro(&socket, &mut receive[VIRTIO_NET_HDR_LEN..])
                        } else {
                            socket
                                .recv_from(&mut receive[VIRTIO_NET_HDR_LEN..])
                                .map(|(n, addr)| (n, addr, None))
                        };
                        let (n, source, gro) = match incoming {
                            Ok(v) => v,
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err(e),
                        };
                        if source != peer.into() {
                            return Err(error("unexpected UDP source"));
                        }
                        let segment = segment_length(n, gro)?;
                        let segments = n.div_ceil(segment);
                        stats.rx_buffers += 1;
                        stats.rx_gro_buffers += u64::from(gro.is_some());
                        stats.max_rx_segments = stats.max_rx_segments.max(segments);
                        // Finish an owned aggregate before polling another fd.
                        // No waiting for packets; work is bounded by this buffer.
                        for offset in (0..n).step_by(segment) {
                            let length = segment.min(n - offset);
                            if gro.is_some() {
                                let start = VIRTIO_NET_HDR_LEN + offset;
                                if length < 20
                                    || u16::from_be_bytes([receive[start + 2], receive[start + 3]])
                                        as usize
                                        != length
                                {
                                    return Err(error("GRO boundary disagrees with IPv4 length"));
                                }
                            }
                            let range =
                                tun_frame(&mut receive, VIRTIO_NET_HDR_LEN + offset, length)?;
                            let frame = &receive[range];
                            let deadline = Instant::now() + Duration::from_secs(3);
                            loop {
                                // SAFETY: fd remains owned and the initialized
                                // zero virtio header plus IP bytes live until return.
                                let written = unsafe {
                                    libc::write(
                                        device.as_raw_fd(),
                                        frame.as_ptr().cast(),
                                        frame.len(),
                                    )
                                };
                                if written == frame.len() as isize {
                                    break;
                                }
                                if written >= 0 {
                                    return Err(error("short TUN write"));
                                }
                                let e = io::Error::last_os_error();
                                if e.kind() == io::ErrorKind::Interrupted {
                                    continue;
                                }
                                if e.kind() == io::ErrorKind::WouldBlock {
                                    wait_fd(device.as_raw_fd(), libc::POLLOUT, deadline)?;
                                    continue;
                                }
                                return Err(e);
                            }
                            stats.rx_packets += 1;
                            stats.rx_bytes += length as u64;
                            stats.tun_writes += 1;
                            received += 1;
                        }
                    }
                }
                if fds[0].revents & libc::POLLIN != 0 {
                    let count =
                        match device.recv_multiple(&mut original, &mut packets, &mut sizes, 0) {
                            Ok(n) => n,
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                                ) =>
                            {
                                continue;
                            }
                            Err(e) => return Err(e),
                        };
                    if count == 0
                        || count > IDEAL_BATCH_SIZE
                        || sizes[..count].iter().any(|n| *n == 0 || *n > MTU)
                    {
                        return Err(error("invalid TUN cohort"));
                    }
                    stats.cohorts += 1;
                    stats.histogram[count] += 1;
                    if mode == Mode::Single || count == 1 {
                        for i in 0..count {
                            send_one(&socket, peer, &packets[i][..sizes[i]], &mut stats)?;
                        }
                    } else if mode == Mode::Mmsg {
                        for begin in (0..count).step_by(64) {
                            let end = (begin + 64).min(count);
                            send_mmsg(
                                &socket,
                                peer,
                                &packets[begin..end],
                                &sizes[begin..end],
                                &mut stats,
                            )?;
                        }
                    } else {
                        send_gso(
                            &socket,
                            peer,
                            &packets[..count],
                            &sizes[..count],
                            &mut scratch,
                            &mut stats,
                        )?;
                    }
                    stats.tx_packets += count as u64;
                    stats.tx_bytes += sizes[..count].iter().map(|n| *n as u64).sum::<u64>();
                }
            }
            Ok(())
        })();
        println!(
            "{{\"mode\":\"{}\",\"cohorts\":{},\"histogram\":{:?},\"tx_packets\":{},\"rx_packets\":{},\"tx_bytes\":{},\"rx_bytes\":{},\"send_calls\":{},\"mmsg_calls\":{},\"gso_calls\":{},\"gso_segments\":{},\"send_eagain\":{},\"tun_writes\":{},\"rx_buffers\":{},\"rx_gro_buffers\":{},\"max_rx_segments\":{}}}",
            args[1],
            stats.cohorts,
            stats.histogram,
            stats.tx_packets,
            stats.rx_packets,
            stats.tx_bytes,
            stats.rx_bytes,
            stats.send_calls,
            stats.mmsg_calls,
            stats.gso_calls,
            stats.gso_segments,
            stats.send_eagain,
            stats.tun_writes,
            stats.rx_buffers,
            stats.rx_gro_buffers,
            stats.max_rx_segments
        );
        match result {
            Err(e) if STOP.load(Ordering::Relaxed) && e.kind() == io::ErrorKind::Interrupted => {
                Ok(())
            }
            other => other,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn control(value: i32) -> Vec<u8> {
            let head = unsafe { libc::CMSG_LEN(0) } as usize;
            let len = unsafe { libc::CMSG_LEN(4) } as usize;
            let mut bytes = vec![0u8; unsafe { libc::CMSG_SPACE(4) } as usize];
            let mut header: libc::cmsghdr = unsafe { mem::zeroed() };
            header.cmsg_level = libc::IPPROTO_UDP;
            header.cmsg_type = libc::UDP_GRO;
            header.cmsg_len = len as _;
            // SAFETY: enough initialized storage; unaligned write is explicit.
            unsafe {
                ptr::write_unaligned(bytes.as_mut_ptr().cast::<libc::cmsghdr>(), header);
            }
            bytes[head..len].copy_from_slice(&value.to_ne_bytes());
            bytes
        }
        #[test]
        fn gro_metadata_rejects_truncation_invalid_sizes_and_duplicates() {
            assert_eq!(gro_size(&control(100), 0).unwrap(), Some(100));
            assert_eq!(gro_size(&[], 0).unwrap(), None);
            for flag in [libc::MSG_TRUNC, libc::MSG_CTRUNC] {
                assert!(gro_size(&control(100), flag).is_err());
            }
            for value in [0, -1, MTU as i32 + 1] {
                assert!(gro_size(&control(value), 0).is_err());
            }
            let mut duplicate = control(100);
            duplicate.extend(control(100));
            assert!(gro_size(&duplicate, 0).is_err());
            let malformed = &control(100)[..mem::size_of::<libc::cmsghdr>()];
            assert!(gro_size(malformed, 0).is_err());
            assert_eq!(segment_length(240, Some(100)).unwrap(), 100);
            assert!(segment_length(80, Some(100)).is_err());
            assert!(segment_length(0, None).is_err());
            assert!(segment_length(MTU + 1, None).is_err());
        }
        #[test]
        fn sliding_virtio_prefix_preserves_every_datagram() {
            let mut first = vec![1u8; 40];
            first[0] = 0x45;
            let mut second = vec![2u8; 24];
            second[0] = 0x45;
            let mut buffer = vec![0u8; VIRTIO_NET_HDR_LEN];
            buffer.extend(&first);
            buffer.extend(&second);
            let a = tun_frame(&mut buffer, VIRTIO_NET_HDR_LEN, first.len()).unwrap();
            assert_eq!(&buffer[a][VIRTIO_NET_HDR_LEN..], first);
            let b = tun_frame(&mut buffer, VIRTIO_NET_HDR_LEN + first.len(), second.len()).unwrap();
            assert_eq!(&buffer[b][VIRTIO_NET_HDR_LEN..], second);
            assert!(tun_frame(&mut buffer, 0, 20).is_err());
            let outside = buffer.len();
            assert!(tun_frame(&mut buffer, outside, 20).is_err());
        }
        #[test]
        fn kernel_gro_recovers_segments_short_tail_and_plain_datagram() {
            let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
            rx.set_nonblocking(true).unwrap();
            enable_gro(&rx).unwrap();
            let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
            tx.set_nonblocking(true).unwrap();
            let peer = match rx.local_addr().unwrap() {
                std::net::SocketAddr::V4(v) => v,
                _ => unreachable!(),
            };
            let packets = vec![vec![1; 100], vec![2; 100], vec![3; 40], vec![4; 100]];
            send_gso(
                &tx,
                peer,
                &packets,
                &[100, 100, 40, 100],
                &mut Vec::new(),
                &mut Stats::default(),
            )
            .unwrap();
            let mut received = Vec::new();
            let mut aggregated = false;
            let deadline = Instant::now() + Duration::from_secs(2);
            while received.len() < packets.len() {
                wait_fd(rx.as_raw_fd(), libc::POLLIN, deadline).unwrap();
                let mut buf = [0u8; 1024];
                let (n, source, gro) = recv_gro(&rx, &mut buf).unwrap();
                assert_eq!(source, tx.local_addr().unwrap());
                let size = segment_length(n, gro).unwrap();
                aggregated |= n > size;
                for packet in buf[..n].chunks(size) {
                    received.push(packet.to_vec());
                }
            }
            assert!(aggregated, "kernel did not deliver a GRO aggregate");
            assert_eq!(received, packets);
        }
        #[test]
        fn short_tail_ends_group_without_waiting_for_more_packets() {
            assert_eq!(group_end(&[100, 100, 40, 100], 0), 3);
            assert_eq!(group_end(&[40, 100], 0), 1);
            assert_eq!(group_end(&[100], 0), 1);
        }
        #[test]
        fn kernel_submission_is_bounded_by_count_and_bytes() {
            assert_eq!(group_end(&[100; 128], 0), 64);
            assert_eq!(group_end(&[1360; 128], 0), 44);
        }
        #[test]
        fn kernel_mmsg_and_gso_preserve_each_datagram_and_short_tail() {
            for mode in [Mode::Mmsg, Mode::Gso] {
                let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
                rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
                tx.set_nonblocking(true).unwrap();
                let peer = match rx.local_addr().unwrap() {
                    std::net::SocketAddr::V4(v) => v,
                    _ => unreachable!(),
                };
                let packets = vec![vec![1; 100], vec![2; 100], vec![3; 40], vec![4; 100]];
                let sizes = [100, 100, 40, 100];
                let mut stats = Stats::default();
                if mode == Mode::Mmsg {
                    send_mmsg(&tx, peer, &packets, &sizes, &mut stats).unwrap();
                } else {
                    send_gso(&tx, peer, &packets, &sizes, &mut Vec::new(), &mut stats).unwrap();
                }
                for packet in packets {
                    let mut out = [0; 200];
                    let (n, source) = rx.recv_from(&mut out).unwrap();
                    assert_eq!(source, tx.local_addr().unwrap());
                    assert_eq!(&out[..n], packet);
                }
                if mode == Mode::Gso {
                    assert_eq!(stats.gso_segments, 3);
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    panic!("natural_cohort is a Linux-only isolated mechanism probe");
}
