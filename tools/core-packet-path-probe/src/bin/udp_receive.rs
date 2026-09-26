//! Linux socket mechanism only. No Core, crypto, TUN, or Tokio scheduler claims.
#[cfg(target_os = "linux")]
mod experiment {
    use std::{
        io,
        mem::{size_of, zeroed},
        net::UdpSocket,
        os::fd::AsRawFd,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    const SIZE: usize = 1400;
    const SECONDS: u64 = 2;

    fn cpu() -> f64 {
        let mut ts = unsafe { zeroed::<libc::timespec>() };
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
            0
        );
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
    }

    fn percentile(values: &mut [u64], percent: usize) -> u64 {
        values.sort_unstable();
        values
            .get(values.len().saturating_sub(1) * percent / 100)
            .copied()
            .unwrap_or(0)
    }

    pub fn run() {
        for repeat in 0..3 {
            let batches = if repeat % 2 == 0 {
                [1, 8, 32]
            } else {
                [32, 8, 1]
            };
            for paced in [true, false] {
                for batch in batches {
                    trial(batch, paced, repeat);
                }
            }
        }
    }

    fn trial(batch: usize, paced: bool, repeat: usize) {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_nonblocking(true).unwrap();
        sender.set_nonblocking(true).unwrap();
        sender.connect(receiver.local_addr().unwrap()).unwrap();
        let source_port = sender.local_addr().unwrap().port();
        let fd = receiver.as_raw_fd();
        let buffer: libc::c_int = 262144;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&buffer as *const libc::c_int).cast(),
                    size_of::<libc::c_int>() as _,
                )
            },
            0
        );
        let mut actual_buffer = 0i32;
        let mut option_size = size_of::<i32>() as libc::socklen_t;
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    fd,
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
            // Buffers and their pointer-bearing descriptors remain allocated and unmoved.
            let mut buffers = vec![[0u8; SIZE]; batch];
            let mut addresses = vec![unsafe { zeroed::<libc::sockaddr_in>() }; batch];
            let mut iovecs: Vec<_> = buffers
                .iter_mut()
                .map(|b| libc::iovec {
                    iov_base: b.as_mut_ptr().cast(),
                    iov_len: SIZE,
                })
                .collect();
            let mut messages = vec![unsafe { zeroed::<libc::mmsghdr>() }; batch];
            for i in 0..batch {
                messages[i].msg_hdr.msg_name = (&mut addresses[i] as *mut libc::sockaddr_in).cast();
                messages[i].msg_hdr.msg_iov = &mut iovecs[i];
                messages[i].msg_hdr.msg_iovlen = 1;
            }
            let mut count = 0u64;
            let mut bytes = 0u64;
            let mut calls = 0u64;
            let mut nonempty = 0u64;
            let mut polls = 0u64;
            let mut idle_polls = 0u64;
            let mut last_sequence = None;
            let mut controls = Vec::new();
            let mut drain_deadline = None;
            let start_cpu = cpu();
            ready_tx.send(()).unwrap();
            loop {
                if done_rx.load(Ordering::Acquire) && drain_deadline.is_none() {
                    drain_deadline = Some(Instant::now() + Duration::from_millis(100));
                }
                if drain_deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                assert!(
                    origin.elapsed() < Duration::from_secs(6),
                    "receiver deadline exceeded"
                );
                for msg in &mut messages {
                    msg.msg_hdr.msg_namelen = size_of::<libc::sockaddr_in>() as _;
                    msg.msg_hdr.msg_flags = 0;
                    msg.msg_len = 0;
                }
                calls += 1;
                let n = if batch == 1 {
                    let n =
                        unsafe { libc::recvmsg(fd, &mut messages[0].msg_hdr, libc::MSG_DONTWAIT) };
                    if n >= 0 {
                        messages[0].msg_len = n as u32;
                        1
                    } else {
                        -1
                    }
                } else {
                    unsafe {
                        libc::recvmmsg(
                            fd,
                            messages.as_mut_ptr(),
                            batch as u32,
                            libc::MSG_DONTWAIT,
                            std::ptr::null_mut(),
                        )
                    }
                };
                if n < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    assert_eq!(error.kind(), io::ErrorKind::WouldBlock, "{error}");
                    let mut pfd = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    polls += 1;
                    if count == 0 {
                        idle_polls += 1;
                    }
                    let result = unsafe { libc::poll(&mut pfd, 1, 20) };
                    if result < 0 {
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
                nonempty += 1;
                for i in 0..n as usize {
                    let len = messages[i].msg_len as usize;
                    assert_eq!(messages[i].msg_hdr.msg_flags & libc::MSG_TRUNC, 0);
                    assert_eq!(addresses[i].sin_family as i32, libc::AF_INET);
                    assert_eq!(u16::from_be(addresses[i].sin_port), source_port);
                    assert_eq!(addresses[i].sin_addr.s_addr.to_ne_bytes(), [127, 0, 0, 1]);
                    assert!(len >= 24);
                    let sequence = u64::from_le_bytes(buffers[i][0..8].try_into().unwrap());
                    let sent = u64::from_le_bytes(buffers[i][8..16].try_into().unwrap());
                    assert!(last_sequence.is_none_or(|last| sequence > last));
                    last_sequence = Some(sequence);
                    let control = sequence % 1024 == 0;
                    assert_eq!(len, if control { 64 } else { SIZE });
                    assert!(buffers[i][16..len].iter().all(|b| *b == sequence as u8));
                    if control {
                        controls.push((origin.elapsed().as_nanos() as u64 - sent) / 1000);
                    }
                    count += 1;
                    bytes += len as u64;
                }
            }
            // Keep the socket alive through the last syscall.
            drop(receiver);
            (
                count,
                bytes,
                calls,
                nonempty,
                polls,
                idle_polls,
                cpu() - start_cpu,
                controls,
            )
        });
        ready_rx.recv().unwrap();
        thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        let mut sent = 0u64;
        let mut send_blocked = 0u64;
        let mut control_sent = 0u64;
        let mut sequence = 0u64;
        while start.elapsed() < Duration::from_secs(SECONDS) {
            for _ in 0..64 {
                let mut packet = [sequence as u8; SIZE];
                packet[0..8].copy_from_slice(&sequence.to_le_bytes());
                packet[8..16].copy_from_slice(&(origin.elapsed().as_nanos() as u64).to_le_bytes());
                let control = sequence % 1024 == 0;
                let len = if control { 64 } else { SIZE };
                match sender.send(&packet[..len]) {
                    Ok(n) => {
                        assert_eq!(n, len);
                        sent += 1;
                        control_sent += u64::from(control);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => send_blocked += 1,
                    Err(e) => panic!("send failed: {e}"),
                }
                sequence += 1;
            }
            if paced {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        done.store(true, Ordering::Release);
        let (received, bytes, calls, nonempty, polls, idle_polls, receive_cpu, mut controls) =
            reader.join().unwrap();
        assert!(received > 0 && received <= sent);
        let control_received = controls.len();
        let p99 = percentile(&mut controls, 99);
        let max = controls.last().copied().unwrap_or(0);
        println!(
            "{{\"repeat\":{repeat},\"batch\":{batch},\"paced\":{paced},\"rcvbuf\":{actual_buffer},\"sent\":{sent},\"received\":{received},\"lost\":{},\"send_blocked\":{send_blocked},\"elapsed_s\":{elapsed},\"mbps\":{},\"receive_cpu_s\":{receive_cpu},\"receive_cpu_s_per_gib\":{},\"recv_syscalls\":{calls},\"nonempty_syscalls\":{nonempty},\"packets_per_nonempty_call\":{},\"polls\":{polls},\"idle_polls\":{idle_polls},\"control_sent\":{control_sent},\"control_received\":{control_received},\"control_p99_us\":{p99},\"control_max_us\":{max}}}",
            sent - received,
            bytes as f64 * 8.0 / elapsed / 1e6,
            receive_cpu / (bytes as f64 / 1073741824.0),
            received as f64 / nonempty as f64
        );
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    experiment::run();
    #[cfg(not(target_os = "linux"))]
    panic!("Linux-only experiment; no portable performance claim");
}
