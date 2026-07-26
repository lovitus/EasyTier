use std::{
    env, io, mem,
    net::{SocketAddr, UdpSocket},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

const MAX_BATCH: usize = 32;
const MAX_PAYLOAD_LEN: usize = 2000;

fn set_socket_buffer(socket: &UdpSocket, option: libc::c_int) -> io::Result<()> {
    let bytes: libc::c_int = 16 * 1024 * 1024;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&bytes as *const libc::c_int).cast(),
            mem::size_of_val(&bytes) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn thread_cpu_time() -> io::Result<Duration> {
    let mut value: libc::timespec = unsafe { mem::zeroed() };
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

fn poll_socket(socket: &UdpSocket, events: libc::c_short) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: socket.as_raw_fd(),
        events,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 120000) };
    if result > 0 && descriptor.revents & events != 0 {
        Ok(())
    } else if result == 0 {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "UDP socket poll timed out",
        ))
    } else {
        Err(io::Error::last_os_error())
    }
}

fn validate_payload(payload: &[u8], expected: u64, payload_len: usize) -> io::Result<()> {
    if payload.len() != payload_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("length {}, expected {payload_len}", payload.len()),
        ));
    }
    let sequence = u64::from_be_bytes(payload[..8].try_into().unwrap());
    if sequence != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sequence {sequence}, expected {expected}"),
        ));
    }
    Ok(())
}

struct ReceiveBatch {
    payloads: Vec<Vec<u8>>,
    addresses: Vec<libc::sockaddr_storage>,
    iovecs: Vec<libc::iovec>,
    messages: Vec<libc::mmsghdr>,
}

impl ReceiveBatch {
    fn new(payload_len: usize) -> Self {
        let mut payloads = vec![vec![0_u8; payload_len]; MAX_BATCH];
        let mut addresses = vec![unsafe { mem::zeroed() }; MAX_BATCH];
        let mut iovecs = Vec::with_capacity(MAX_BATCH);
        let mut messages = Vec::with_capacity(MAX_BATCH);
        for index in 0..MAX_BATCH {
            iovecs.push(libc::iovec {
                iov_base: payloads[index].as_mut_ptr().cast(),
                iov_len: payload_len,
            });
            let mut header: libc::msghdr = unsafe { mem::zeroed() };
            header.msg_name = (&mut addresses[index] as *mut libc::sockaddr_storage).cast();
            header.msg_namelen = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            header.msg_iov = (&mut iovecs[index]) as *mut libc::iovec;
            header.msg_iovlen = 1;
            messages.push(libc::mmsghdr {
                msg_hdr: header,
                msg_len: 0,
            });
        }
        Self {
            payloads,
            addresses,
            iovecs,
            messages,
        }
    }

    fn reset(&mut self) {
        for index in 0..MAX_BATCH {
            self.iovecs[index].iov_base = self.payloads[index].as_mut_ptr().cast();
            self.messages[index].msg_hdr.msg_name =
                (&mut self.addresses[index] as *mut libc::sockaddr_storage).cast();
            self.messages[index].msg_hdr.msg_namelen =
                mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            self.messages[index].msg_hdr.msg_iov = (&mut self.iovecs[index]) as *mut libc::iovec;
            self.messages[index].msg_len = 0;
        }
    }
}

fn receive_single(socket: &UdpSocket, total_packets: u64, payload_len: usize) -> io::Result<u64> {
    let mut payload = vec![0_u8; payload_len];
    let mut received = 0;
    let mut syscalls = 0;
    while received < total_packets {
        poll_socket(socket, libc::POLLIN)?;
        match socket.recv_from(&mut payload) {
            Ok((length, _)) => {
                validate_payload(&payload[..length], received, payload_len)?;
                received += 1;
                syscalls += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(syscalls)
}

fn receive_batch(socket: &UdpSocket, total_packets: u64, payload_len: usize) -> io::Result<u64> {
    let mut batch = ReceiveBatch::new(payload_len);
    let mut received = 0;
    let mut syscalls = 0;
    while received < total_packets {
        poll_socket(socket, libc::POLLIN)?;
        batch.reset();
        let remaining = (total_packets - received).min(MAX_BATCH as u64) as libc::c_uint;
        let result = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                batch.messages.as_mut_ptr(),
                remaining,
                libc::MSG_DONTWAIT as _,
                std::ptr::null_mut(),
            )
        };
        if result > 0 {
            let count = result as usize;
            for index in 0..count {
                let length = batch.messages[index].msg_len as usize;
                validate_payload(
                    &batch.payloads[index][..length],
                    received + index as u64,
                    payload_len,
                )?;
            }
            received += count as u64;
            syscalls += 1;
        } else if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "recvmmsg returned zero",
            ));
        } else {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
        }
    }
    Ok(syscalls)
}

fn run_server(
    mode: &str,
    bind: SocketAddr,
    total_packets: u64,
    payload_len: usize,
) -> io::Result<()> {
    let socket = UdpSocket::bind(bind)?;
    set_socket_buffer(&socket, libc::SO_RCVBUF)?;
    socket.set_nonblocking(true)?;
    println!("READY mode={mode} addr={}", socket.local_addr()?);

    let wall_started = Instant::now();
    let cpu_started = thread_cpu_time()?;
    let syscalls = match mode {
        "single" => receive_single(&socket, total_packets, payload_len)?,
        "batch" => receive_batch(&socket, total_packets, payload_len)?,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mode must be single or batch",
            ));
        }
    };
    let cpu_elapsed = thread_cpu_time()? - cpu_started;
    let wall_elapsed = wall_started.elapsed();
    println!(
        "PASS mode={} packets={} syscalls={} packets_per_syscall={:.3} wall_ms={:.3} cpu_ms={:.3}",
        mode,
        total_packets,
        syscalls,
        total_packets as f64 / syscalls as f64,
        wall_elapsed.as_secs_f64() * 1000.0,
        cpu_elapsed.as_secs_f64() * 1000.0,
    );
    Ok(())
}

struct SendBatch {
    payloads: Vec<Vec<u8>>,
    iovecs: Vec<libc::iovec>,
    messages: Vec<libc::mmsghdr>,
}

impl SendBatch {
    fn new(payload_len: usize) -> Self {
        let mut payloads = vec![vec![0_u8; payload_len]; MAX_BATCH];
        let mut iovecs = Vec::with_capacity(MAX_BATCH);
        let mut messages = Vec::with_capacity(MAX_BATCH);
        for index in 0..MAX_BATCH {
            iovecs.push(libc::iovec {
                iov_base: payloads[index].as_mut_ptr().cast(),
                iov_len: payload_len,
            });
            let mut header: libc::msghdr = unsafe { mem::zeroed() };
            header.msg_iov = (&mut iovecs[index]) as *mut libc::iovec;
            header.msg_iovlen = 1;
            messages.push(libc::mmsghdr {
                msg_hdr: header,
                msg_len: 0,
            });
        }
        Self {
            payloads,
            iovecs,
            messages,
        }
    }

    fn prepare(&mut self, first_sequence: u64, count: usize) {
        for index in 0..count {
            self.payloads[index].fill(0);
            self.payloads[index][..8]
                .copy_from_slice(&(first_sequence + index as u64).to_be_bytes());
            self.iovecs[index].iov_base = self.payloads[index].as_mut_ptr().cast();
            self.messages[index].msg_hdr.msg_iov = (&mut self.iovecs[index]) as *mut libc::iovec;
            self.messages[index].msg_len = 0;
        }
    }
}

fn run_client(
    destination: SocketAddr,
    total_packets: u64,
    payload_len: usize,
    pause_micros: u64,
) -> io::Result<()> {
    let bind = if destination.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)?;
    socket.connect(destination)?;
    set_socket_buffer(&socket, libc::SO_SNDBUF)?;
    socket.set_nonblocking(true)?;

    let mut batch = SendBatch::new(payload_len);
    let pause = Duration::from_micros(pause_micros);
    let wall_started = Instant::now();
    let cpu_started = thread_cpu_time()?;
    let mut first_sequence = 0;
    let mut syscalls = 0;
    while first_sequence < total_packets {
        let count = (total_packets - first_sequence).min(MAX_BATCH as u64) as usize;
        batch.prepare(first_sequence, count);
        let mut offset = 0;
        while offset < count {
            let result = unsafe {
                libc::sendmmsg(
                    socket.as_raw_fd(),
                    batch.messages[offset..].as_mut_ptr(),
                    (count - offset) as libc::c_uint,
                    libc::MSG_DONTWAIT as _,
                )
            };
            if result > 0 {
                offset += result as usize;
                syscalls += 1;
            } else if result == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "sendmmsg returned zero",
                ));
            } else {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    poll_socket(&socket, libc::POLLOUT)?;
                } else {
                    return Err(error);
                }
            }
        }
        first_sequence += count as u64;
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    let cpu_elapsed = thread_cpu_time()? - cpu_started;
    let wall_elapsed = wall_started.elapsed();
    println!(
        "PASS packets={} send_syscalls={} packets_per_syscall={:.3} wall_ms={:.3} cpu_ms={:.3}",
        total_packets,
        syscalls,
        total_packets as f64 / syscalls as f64,
        wall_elapsed.as_secs_f64() * 1000.0,
        cpu_elapsed.as_secs_f64() * 1000.0,
    );
    Ok(())
}

fn parse_u64(value: Option<String>, name: &str) -> io::Result<u64> {
    value
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {name}")))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid {name}")))
}

fn parse_payload_len(value: Option<String>) -> io::Result<usize> {
    let payload_len = parse_u64(value, "payload_len")? as usize;
    if !(8..=MAX_PAYLOAD_LEN).contains(&payload_len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload_len must be in 8..=2000",
        ));
    }
    Ok(payload_len)
}

fn main() -> io::Result<()> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("server") => {
            let mode = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing mode"))?;
            let bind: SocketAddr = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing bind"))?
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid bind"))?;
            let total_packets = parse_u64(arguments.next(), "total_packets")?;
            let payload_len = parse_payload_len(arguments.next())?;
            run_server(&mode, bind, total_packets, payload_len)
        }
        Some("client") => {
            let destination: SocketAddr = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing destination"))?
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid destination"))?;
            let total_packets = parse_u64(arguments.next(), "total_packets")?;
            let payload_len = parse_payload_len(arguments.next())?;
            let pause_micros = parse_u64(arguments.next(), "pause_micros")?;
            run_client(destination, total_packets, payload_len, pause_micros)
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: network_receive_batch server MODE BIND TOTAL PAYLOAD | client DEST TOTAL PAYLOAD PAUSE_US",
        )),
    }
}
