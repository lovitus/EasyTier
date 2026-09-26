//! Linux GSO-supply/GRO-receive mechanism, not a Core or Tokio benchmark.
//! Both arms use the same bounded buffers, sender and per-datagram validation.
#[cfg(target_os = "linux")]
mod experiment {
    use std::{
        io,
        mem::{size_of, zeroed},
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket},
        os::fd::AsRawFd,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    const SIZE: usize = 1400;
    const GROUP: usize = 8;
    const CAPACITY: usize = 65_536;
    const CONTROL_LIMIT: usize = 32_768;

    fn cpu() -> f64 {
        let mut ts = unsafe { zeroed::<libc::timespec>() };
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
            0
        );
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
    }

    fn option(socket: &UdpSocket, level: i32, name: i32, value: i32) {
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    level,
                    name,
                    (&value as *const i32).cast(),
                    size_of::<i32>() as _,
                )
            },
            0,
            "setsockopt({level}, {name}): {}",
            io::Error::last_os_error()
        );
    }

    fn packet_size(sequence: u64) -> usize {
        if sequence.is_multiple_of(1024) {
            64
        } else if sequence.is_multiple_of(257) {
            333
        } else {
            SIZE
        }
    }

    fn receive(
        socket: &UdpSocket,
        bytes: &mut [u8],
    ) -> io::Result<(usize, usize, SocketAddr, bool)> {
        let mut source = unsafe { zeroed::<libc::sockaddr_storage>() };
        let mut control = [0usize; 16];
        let mut iovec = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut message = unsafe { zeroed::<libc::msghdr>() };
        message.msg_name = (&mut source as *mut libc::sockaddr_storage).cast();
        message.msg_namelen = size_of::<libc::sockaddr_storage>() as _;
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size_of::<[usize; 16]>();
        let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_DONTWAIT) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        assert_eq!(message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC), 0);
        assert!(n > 0 && n as usize <= bytes.len());
        assert!(message.msg_controllen <= size_of::<[usize; 16]>());
        let mut stride = n as usize;
        let mut gro = false;
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
        while !cmsg.is_null() {
            let offset = (cmsg as usize)
                .checked_sub(control.as_ptr() as usize)
                .unwrap();
            assert!(offset + size_of::<libc::cmsghdr>() <= message.msg_controllen);
            let header = unsafe { &*cmsg };
            assert!(header.cmsg_len >= size_of::<libc::cmsghdr>());
            assert!(header.cmsg_len <= message.msg_controllen - offset);
            if header.cmsg_level == libc::IPPROTO_UDP && header.cmsg_type == libc::UDP_GRO {
                assert!(!gro, "duplicate GRO metadata");
                assert!(
                    header.cmsg_len >= unsafe { libc::CMSG_LEN(size_of::<i32>() as _) } as usize
                );
                // Linux UDP_GRO carries a native int, not the u16 used by
                // UDP_SEGMENT cmsg. Kernel selftests fixed this in 436864095a95.
                let value =
                    unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>()) };
                assert!(value > 0 && value as usize <= bytes.len());
                stride = value as usize;
                gro = true;
            }
            cmsg = unsafe { libc::CMSG_NXTHDR(&message, cmsg) };
        }
        let address = match source.ss_family as i32 {
            libc::AF_INET => {
                assert!(message.msg_namelen as usize >= size_of::<libc::sockaddr_in>());
                let addr = unsafe {
                    &*(&source as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>()
                };
                SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes())),
                    u16::from_be(addr.sin_port),
                )
            }
            libc::AF_INET6 => {
                assert!(message.msg_namelen as usize >= size_of::<libc::sockaddr_in6>());
                let addr = unsafe {
                    &*(&source as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>()
                };
                SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(addr.sin6_addr.s6_addr),
                    u16::from_be(addr.sin6_port),
                    u32::from_be(addr.sin6_flowinfo),
                    addr.sin6_scope_id,
                ))
            }
            family => panic!("unexpected address family {family}"),
        };
        Ok((n as usize, stride, address, gro))
    }

    #[derive(Default)]
    struct Received {
        packets: u64,
        bytes: u64,
        calls: u64,
        messages: u64,
        gro_messages: u64,
        max_batch: usize,
        short_tails: u64,
        polls: u64,
        idle_polls: u64,
        cpu: f64,
        controls: Vec<u64>,
    }

    fn trial(ipv6: bool, gro: bool, paced: bool, repeat: usize) {
        let bind = if ipv6 { "[::1]:0" } else { "127.0.0.1:0" };
        let receiver = UdpSocket::bind(bind).unwrap();
        let sender = UdpSocket::bind(bind).unwrap();
        receiver.set_nonblocking(true).unwrap();
        sender.set_nonblocking(true).unwrap();
        sender.connect(receiver.local_addr().unwrap()).unwrap();
        let expected_source = sender.local_addr().unwrap();
        option(&receiver, libc::SOL_SOCKET, libc::SO_RCVBUF, 262_144);
        option(&receiver, libc::IPPROTO_UDP, libc::UDP_GRO, i32::from(gro));
        option(&sender, libc::IPPROTO_UDP, libc::UDP_SEGMENT, SIZE as i32);
        let mut actual_buffer = 0i32;
        let mut option_size = size_of::<i32>() as libc::socklen_t;
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    receiver.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&mut actual_buffer as *mut i32).cast(),
                    &mut option_size,
                )
            },
            0
        );
        let origin = Instant::now();
        let done = Arc::new(AtomicBool::new(false));
        let done_rx = done.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let reader = thread::spawn(move || {
            let mut buffer = vec![0u8; CAPACITY];
            let mut out = Received {
                controls: Vec::with_capacity(CONTROL_LIMIT),
                ..Received::default()
            };
            let mut last = None;
            let mut deadline = None;
            let start_cpu = cpu();
            ready_tx.send(()).unwrap();
            loop {
                if done_rx.load(Ordering::Acquire) && deadline.is_none() {
                    deadline = Some(Instant::now() + Duration::from_millis(100));
                }
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                assert!(
                    origin.elapsed() < Duration::from_secs(6),
                    "receive deadline"
                );
                out.calls += 1;
                let (len, stride, source, has_gro) = match receive(&receiver, &mut buffer) {
                    Ok(value) => value,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        out.polls += 1;
                        out.idle_polls += u64::from(out.packets == 0);
                        let mut pfd = libc::pollfd {
                            fd: receiver.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        let n = unsafe { libc::poll(&mut pfd, 1, 20) };
                        if n < 0 {
                            assert_eq!(
                                io::Error::last_os_error().kind(),
                                io::ErrorKind::Interrupted
                            );
                        } else {
                            assert_eq!(
                                pfd.revents & (libc::POLLERR | libc::POLLNVAL | libc::POLLHUP),
                                0
                            );
                        }
                        continue;
                    }
                    Err(e) => panic!("recvmsg: {e}"),
                };
                assert_eq!(source, expected_source);
                assert!(gro || !has_gro, "unexpected GRO while disabled");
                out.messages += 1;
                out.gro_messages += u64::from(has_gro);
                out.max_batch = out.max_batch.max(len.div_ceil(stride));
                for packet in buffer[..len].chunks(stride) {
                    assert!(packet.len() >= 16);
                    let sequence = u64::from_le_bytes(packet[0..8].try_into().unwrap());
                    let sent = u64::from_le_bytes(packet[8..16].try_into().unwrap());
                    assert!(
                        last.is_none_or(|n| sequence > n),
                        "duplicate or reordered datagram"
                    );
                    last = Some(sequence);
                    assert_eq!(packet.len(), packet_size(sequence));
                    assert!(packet[16..].iter().all(|b| *b == sequence as u8));
                    out.short_tails += u64::from(packet.len() == 333);
                    if sequence.is_multiple_of(1024) {
                        assert!(out.controls.len() < CONTROL_LIMIT, "control sample bound");
                        out.controls
                            .push((origin.elapsed().as_nanos() as u64 - sent) / 1000);
                    }
                    out.packets += 1;
                    out.bytes += packet.len() as u64;
                }
            }
            drop(receiver);
            out.cpu = cpu() - start_cpu;
            out
        });
        ready_rx.recv().unwrap();
        thread::sleep(Duration::from_millis(100));
        let mut sequence = 0u64;
        let mut sent = 0u64;
        let mut control_sent = 0u64;
        let mut send_calls = 0u64;
        let mut blocked = 0u64;
        let mut buffer = [0u8; GROUP * SIZE];
        let start = Instant::now();
        let start_cpu = cpu();
        while start.elapsed() < Duration::from_secs(2) {
            let mut pulse = 0;
            while pulse < 64 {
                let mut packets = 0;
                let mut bytes = 0;
                let mut controls = 0;
                while packets < GROUP.min(64 - pulse) {
                    let seq = sequence + packets as u64;
                    let size = packet_size(seq);
                    let control = seq.is_multiple_of(1024);
                    if packets > 0 && control {
                        break;
                    }
                    let packet = &mut buffer[bytes..bytes + size];
                    packet.fill(seq as u8);
                    packet[0..8].copy_from_slice(&seq.to_le_bytes());
                    packet[8..16]
                        .copy_from_slice(&(origin.elapsed().as_nanos() as u64).to_le_bytes());
                    bytes += size;
                    packets += 1;
                    controls += u64::from(control);
                    if size != SIZE {
                        break;
                    }
                }
                send_calls += 1;
                match sender.send(&buffer[..bytes]) {
                    Ok(n) => {
                        assert_eq!(n, bytes, "short GSO send; never replayed");
                        sent += packets as u64;
                        control_sent += controls;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => blocked += packets as u64,
                    Err(e) => panic!("GSO send: {e}"),
                }
                sequence += packets as u64;
                pulse += packets;
            }
            if paced {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let send_cpu = cpu() - start_cpu;
        let elapsed = start.elapsed().as_secs_f64();
        done.store(true, Ordering::Release);
        let mut got = reader.join().unwrap();
        assert!(got.packets > 0 && got.packets <= sent);
        assert!(got.short_tails > 0 && !got.controls.is_empty());
        assert_eq!(
            got.gro_messages > 0,
            gro,
            "requested mechanism was not observed"
        );
        got.controls.sort_unstable();
        let control_received = got.controls.len();
        assert!(control_received as u64 <= control_sent);
        let p99 = got.controls[(control_received - 1) * 99 / 100];
        let max = *got.controls.last().unwrap();
        let gib = got.bytes as f64 / 1073741824.0;
        println!(
            "{{\"repeat\":{repeat},\"ipv6\":{ipv6},\"gro\":{gro},\"paced\":{paced},\"gso_max_batch\":{GROUP},\"rcvbuf\":{actual_buffer},\"sent\":{sent},\"received\":{},\"lost\":{},\"send_blocked_datagrams\":{blocked},\"elapsed_s\":{elapsed},\"mbps\":{},\"send_cpu_s\":{send_cpu},\"receive_cpu_s\":{},\"receive_cpu_s_per_gib\":{},\"total_cpu_s_per_gib\":{},\"send_syscalls\":{send_calls},\"recv_syscalls\":{},\"nonempty_syscalls\":{},\"gro_messages\":{},\"packets_per_nonempty_call\":{},\"max_receive_batch\":{},\"short_tail_datagrams\":{},\"polls\":{},\"idle_polls\":{},\"control_sent\":{control_sent},\"control_received\":{control_received},\"control_p99_us\":{p99},\"control_max_us\":{max}}}",
            got.packets,
            sent - got.packets,
            got.bytes as f64 * 8.0 / elapsed / 1e6,
            got.cpu,
            got.cpu / gib,
            (got.cpu + send_cpu) / gib,
            got.calls,
            got.messages,
            got.gro_messages,
            got.packets as f64 / got.messages as f64,
            got.max_batch,
            got.short_tails,
            got.polls,
            got.idle_polls
        );
    }

    pub fn run() {
        for repeat in 0..3 {
            for ipv6 in [false, true] {
                for paced in [true, false] {
                    for gro in if repeat % 2 == 0 {
                        [false, true]
                    } else {
                        [true, false]
                    } {
                        trial(ipv6, gro, paced, repeat);
                    }
                }
            }
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    experiment::run();
    #[cfg(not(target_os = "linux"))]
    panic!("Linux-only mechanism experiment, not a portable Core implementation");
}
