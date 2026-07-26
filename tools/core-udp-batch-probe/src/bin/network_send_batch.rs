use std::{
    env, io, mem,
    net::{SocketAddr, UdpSocket},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

const PAYLOAD_LEN: usize = 1200;
const MAX_BATCH: usize = 32;

struct SendBatch {
    payloads: Vec<[u8; PAYLOAD_LEN]>,
    iovecs: Vec<libc::iovec>,
    messages: Vec<libc::mmsghdr>,
    length: usize,
}

impl SendBatch {
    fn new() -> Self {
        let mut payloads = vec![[0; PAYLOAD_LEN]; MAX_BATCH];
        let mut iovecs = Vec::with_capacity(MAX_BATCH);
        let mut messages = Vec::with_capacity(MAX_BATCH);
        for index in 0..MAX_BATCH {
            iovecs.push(libc::iovec {
                iov_base: payloads[index].as_mut_ptr().cast(),
                iov_len: PAYLOAD_LEN,
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
            length: 0,
        }
    }

    fn fill(&mut self, first_sequence: u64, total_remaining: u64) {
        self.length = total_remaining.min(MAX_BATCH as u64) as usize;
        for index in 0..self.length {
            let sequence = first_sequence + index as u64;
            self.payloads[index].fill(0);
            self.payloads[index][..8].copy_from_slice(&sequence.to_be_bytes());
            self.iovecs[index].iov_base = self.payloads[index].as_mut_ptr().cast();
            self.iovecs[index].iov_len = PAYLOAD_LEN;
            self.messages[index].msg_hdr.msg_iov = (&mut self.iovecs[index]) as *mut libc::iovec;
            self.messages[index].msg_hdr.msg_iovlen = 1;
            self.messages[index].msg_len = 0;
        }
    }
}

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

fn wait_writable(socket: &UdpSocket) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 5000) };
    if result > 0 && descriptor.revents & libc::POLLOUT != 0 {
        Ok(())
    } else if result == 0 {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "UDP socket remained blocked",
        ))
    } else {
        Err(io::Error::last_os_error())
    }
}

fn wait_readable(socket: &UdpSocket) -> io::Result<()> {
    let mut descriptor = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, 120000) };
    if result > 0 && descriptor.revents & libc::POLLIN != 0 {
        Ok(())
    } else if result == 0 {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "UDP receive timed out",
        ))
    } else {
        Err(io::Error::last_os_error())
    }
}

fn run_server(bind: SocketAddr, total_packets: u64) -> io::Result<()> {
    let socket = UdpSocket::bind(bind)?;
    set_socket_buffer(&socket, libc::SO_RCVBUF)?;
    socket.set_nonblocking(true)?;
    println!("READY {}", socket.local_addr()?);

    let started = Instant::now();
    let cpu_started = thread_cpu_time()?;
    let mut payload = [0_u8; PAYLOAD_LEN];
    for expected in 0..total_packets {
        let (length, _) = loop {
            wait_readable(&socket)?;
            match socket.recv_from(&mut payload) {
                Ok(received) => break received,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        };
        if length != PAYLOAD_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("length {length}, expected {PAYLOAD_LEN}"),
            ));
        }
        let sequence = u64::from_be_bytes(payload[..8].try_into().unwrap());
        if sequence != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sequence {sequence}, expected {expected}"),
            ));
        }
    }
    let cpu_elapsed = thread_cpu_time()? - cpu_started;
    let wall_elapsed = started.elapsed();
    println!(
        "PASS packets={} wall_ms={:.3} cpu_ms={:.3} payload_mbps={:.3}",
        total_packets,
        wall_elapsed.as_secs_f64() * 1000.0,
        cpu_elapsed.as_secs_f64() * 1000.0,
        total_packets as f64 * PAYLOAD_LEN as f64 * 8.0 / wall_elapsed.as_secs_f64() / 1e6,
    );
    Ok(())
}

fn send_single(socket: &UdpSocket, total_packets: u64, pause: Duration) -> io::Result<u64> {
    let mut payload = [0_u8; PAYLOAD_LEN];
    let mut sequence = 0;
    let mut syscalls = 0;
    while sequence < total_packets {
        payload[..8].copy_from_slice(&sequence.to_be_bytes());
        match socket.send(&payload) {
            Ok(PAYLOAD_LEN) => {
                sequence += 1;
                syscalls += 1;
                if !pause.is_zero() {
                    std::thread::sleep(pause);
                }
            }
            Ok(length) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    format!("partial UDP send {length}"),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_writable(socket)?,
            Err(error) => return Err(error),
        }
    }
    Ok(syscalls)
}

fn send_batch(socket: &UdpSocket, total_packets: u64, pause: Duration) -> io::Result<u64> {
    let mut batch = SendBatch::new();
    let mut first_sequence = 0;
    let mut syscalls = 0;
    while first_sequence < total_packets {
        batch.fill(first_sequence, total_packets - first_sequence);
        let mut offset = 0;
        while offset < batch.length {
            let result = unsafe {
                libc::sendmmsg(
                    socket.as_raw_fd(),
                    batch.messages[offset..].as_mut_ptr(),
                    (batch.length - offset) as libc::c_uint,
                    libc::MSG_DONTWAIT,
                )
            };
            if result > 0 {
                offset += result as usize;
                syscalls += 1;
            } else if result == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "sendmmsg sent zero datagrams",
                ));
            } else {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    wait_writable(socket)?;
                } else {
                    return Err(error);
                }
            }
        }
        first_sequence += batch.length as u64;
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    Ok(syscalls)
}

fn run_client(
    mode: &str,
    destination: SocketAddr,
    total_packets: u64,
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

    let started = Instant::now();
    let cpu_started = thread_cpu_time()?;
    let pause = Duration::from_micros(pause_micros);
    let syscalls = match mode {
        "single" => send_single(&socket, total_packets, pause)?,
        "batch" => send_batch(&socket, total_packets, pause)?,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mode must be single or batch",
            ));
        }
    };
    let cpu_elapsed = thread_cpu_time()? - cpu_started;
    let wall_elapsed = started.elapsed();
    println!(
        "PASS mode={} packets={} syscalls={} packets_per_syscall={:.3} wall_ms={:.3} cpu_ms={:.3} payload_mbps={:.3}",
        mode,
        total_packets,
        syscalls,
        total_packets as f64 / syscalls as f64,
        wall_elapsed.as_secs_f64() * 1000.0,
        cpu_elapsed.as_secs_f64() * 1000.0,
        total_packets as f64 * PAYLOAD_LEN as f64 * 8.0 / wall_elapsed.as_secs_f64() / 1e6,
    );
    Ok(())
}

fn parse_u64(value: Option<String>, name: &str) -> io::Result<u64> {
    value
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {name}")))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid {name}")))
}

fn main() -> io::Result<()> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("server") => {
            let bind: SocketAddr = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing bind"))?
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid bind"))?;
            let total_packets = parse_u64(arguments.next(), "total_packets")?;
            run_server(bind, total_packets)
        }
        Some("client") => {
            let mode = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing mode"))?;
            let destination: SocketAddr = arguments
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing destination"))?
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid destination"))?;
            let total_packets = parse_u64(arguments.next(), "total_packets")?;
            let pause_micros = parse_u64(arguments.next(), "pause_micros")?;
            run_client(&mode, destination, total_packets, pause_micros)
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: network_send_batch server BIND TOTAL | client MODE DEST TOTAL PAUSE_US",
        )),
    }
}
