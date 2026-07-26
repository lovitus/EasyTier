#[cfg(not(target_os = "linux"))]
compile_error!("core-udp-batch-probe currently supports Linux only");

use std::{
    io,
    mem::{self, MaybeUninit},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket as StdUdpSocket},
    os::fd::{AsRawFd, RawFd},
    ptr,
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread,
    time::{Duration, Instant},
};

use tokio::{io::unix::AsyncFd, net::UdpSocket};

const PAYLOAD_LEN: usize = 1200;
const MAX_BATCH: usize = 32;
const DEFAULT_ROUNDS: usize = 5_000;
const BURSTS: [usize; 6] = [1, 2, 4, 8, 16, 32];

#[derive(Clone, Copy, Debug)]
enum Mode {
    TokioSingle,
    AsyncFdRecvmmsg,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::TokioSingle => "tokio_recv_from",
            Self::AsyncFdRecvmmsg => "asyncfd_recvmmsg",
        }
    }
}

#[derive(Debug)]
struct RunResult {
    mode: Mode,
    burst: usize,
    rounds: usize,
    packets: usize,
    receive_syscalls: usize,
    receive_time: Duration,
    receiver_cpu_time: Duration,
    wall_time: Duration,
    receive_buffer_bytes: libc::c_int,
}

impl RunResult {
    fn print_header() {
        println!(
            "mode,burst,rounds,packets,recv_syscalls,packets_per_syscall,\
             receive_ns_per_packet,receiver_cpu_ns_per_packet,wall_mpps,rcvbuf_bytes"
        );
    }

    fn print(&self) {
        let packets = self.packets as f64;
        let packets_per_syscall = packets / self.receive_syscalls as f64;
        let receive_ns_per_packet = self.receive_time.as_nanos() as f64 / packets;
        let cpu_ns_per_packet = self.receiver_cpu_time.as_nanos() as f64 / packets;
        let wall_mpps = packets / self.wall_time.as_secs_f64() / 1_000_000.0;
        println!(
            "{},{},{},{},{},{:.3},{:.1},{:.1},{:.4},{}",
            self.mode.label(),
            self.burst,
            self.rounds,
            self.packets,
            self.receive_syscalls,
            packets_per_syscall,
            receive_ns_per_packet,
            cpu_ns_per_packet,
            wall_mpps,
            self.receive_buffer_bytes,
        );
    }
}

struct SenderControl {
    start_round: SyncSender<usize>,
    round_sent: Receiver<Result<(), String>>,
    handle: thread::JoinHandle<Result<(), String>>,
}

impl SenderControl {
    fn start_round(&self, burst: usize) -> io::Result<()> {
        self.start_round
            .send(burst)
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))?;
        self.round_sent
            .recv()
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))?
            .map_err(io::Error::other)
    }

    fn finish(self) -> io::Result<()> {
        drop(self.start_round);
        self.handle
            .join()
            .map_err(|_| io::Error::other("sender thread panicked"))?
            .map_err(io::Error::other)
    }
}

struct SocketPair {
    receiver: StdUdpSocket,
    sender_control: SenderControl,
    sender_addr: SocketAddr,
    receive_buffer_bytes: libc::c_int,
}

struct BatchBuffers {
    payloads: Vec<[u8; PAYLOAD_LEN]>,
    addresses: Vec<libc::sockaddr_storage>,
    iovecs: Vec<libc::iovec>,
    messages: Vec<libc::mmsghdr>,
}

impl BatchBuffers {
    fn new() -> Self {
        let mut payloads = vec![[0u8; PAYLOAD_LEN]; MAX_BATCH];
        let mut addresses = (0..MAX_BATCH)
            .map(|_| unsafe { mem::zeroed::<libc::sockaddr_storage>() })
            .collect::<Vec<_>>();
        let mut iovecs = Vec::with_capacity(MAX_BATCH);
        let mut messages = Vec::with_capacity(MAX_BATCH);

        for index in 0..MAX_BATCH {
            iovecs.push(libc::iovec {
                iov_base: payloads[index].as_mut_ptr().cast(),
                iov_len: PAYLOAD_LEN,
            });
            messages.push(unsafe { mem::zeroed::<libc::mmsghdr>() });
        }

        for index in 0..MAX_BATCH {
            messages[index].msg_hdr.msg_name =
                (&mut addresses[index] as *mut libc::sockaddr_storage).cast();
            messages[index].msg_hdr.msg_namelen =
                mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            messages[index].msg_hdr.msg_iov = &mut iovecs[index];
            messages[index].msg_hdr.msg_iovlen = 1;
        }

        Self {
            payloads,
            addresses,
            iovecs,
            messages,
        }
    }

    fn prepare(&mut self, limit: usize) {
        for index in 0..limit {
            self.iovecs[index].iov_base = self.payloads[index].as_mut_ptr().cast();
            self.iovecs[index].iov_len = PAYLOAD_LEN;
            self.messages[index].msg_len = 0;
            self.messages[index].msg_hdr.msg_name =
                (&mut self.addresses[index] as *mut libc::sockaddr_storage).cast();
            self.messages[index].msg_hdr.msg_namelen =
                mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            self.messages[index].msg_hdr.msg_iov = &mut self.iovecs[index];
            self.messages[index].msg_hdr.msg_iovlen = 1;
            self.messages[index].msg_hdr.msg_control = ptr::null_mut();
            self.messages[index].msg_hdr.msg_controllen = 0;
            self.messages[index].msg_hdr.msg_flags = 0;
        }
    }

    fn recv(&mut self, fd: RawFd, limit: usize) -> io::Result<usize> {
        self.prepare(limit);
        let received = unsafe {
            libc::recvmmsg(
                fd,
                self.messages.as_mut_ptr(),
                limit as libc::c_uint,
                libc::MSG_DONTWAIT,
                ptr::null_mut(),
            )
        };
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(received as usize)
    }

    fn packet(&self, index: usize) -> io::Result<(&[u8], SocketAddr)> {
        let length = self.messages[index].msg_len as usize;
        if length > PAYLOAD_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("recvmmsg returned oversized payload: {length}"),
            ));
        }
        let source = sockaddr_to_socket_addr(
            &self.addresses[index],
            self.messages[index].msg_hdr.msg_namelen,
        )?;
        Ok((&self.payloads[index][..length], source))
    }
}

fn sockaddr_to_socket_addr(
    storage: &libc::sockaddr_storage,
    length: libc::socklen_t,
) -> io::Result<SocketAddr> {
    if storage.ss_family as libc::c_int != libc::AF_INET
        || length < mem::size_of::<libc::sockaddr_in>() as libc::socklen_t
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unexpected source address family={} length={length}",
                storage.ss_family
            ),
        ));
    }
    let address = unsafe { &*(storage as *const _ as *const libc::sockaddr_in) };
    Ok(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)),
        u16::from_be(address.sin_port),
    )))
}

fn set_socket_buffer(fd: RawFd, option: libc::c_int, bytes: libc::c_int) -> io::Result<()> {
    let result = unsafe {
        libc::setsockopt(
            fd,
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

fn socket_buffer(fd: RawFd, option: libc::c_int) -> io::Result<libc::c_int> {
    let mut value = MaybeUninit::<libc::c_int>::uninit();
    let mut length = mem::size_of::<libc::c_int>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            value.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { value.assume_init() })
}

fn make_socket_pair(rounds: usize) -> io::Result<SocketPair> {
    let receiver = StdUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    receiver.set_nonblocking(true)?;
    set_socket_buffer(receiver.as_raw_fd(), libc::SO_RCVBUF, 4 * 1024 * 1024)?;
    let receive_buffer_bytes = socket_buffer(receiver.as_raw_fd(), libc::SO_RCVBUF)?;
    let receiver_addr = receiver.local_addr()?;

    let sender = StdUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    set_socket_buffer(sender.as_raw_fd(), libc::SO_SNDBUF, 4 * 1024 * 1024)?;
    sender.connect(receiver_addr)?;
    let sender_addr = sender.local_addr()?;

    let (start_round_tx, start_round_rx) = sync_channel::<usize>(0);
    let (round_sent_tx, round_sent_rx) = sync_channel::<Result<(), String>>(0);
    let handle = thread::spawn(move || {
        let mut payload = [0x5au8; PAYLOAD_LEN];
        for round in 0..rounds {
            let burst = match start_round_rx.recv() {
                Ok(burst) => burst,
                Err(_) => return Ok(()),
            };
            let result = (|| -> io::Result<()> {
                for index in 0..burst {
                    let sequence = (round * burst + index) as u64;
                    payload[..8].copy_from_slice(&sequence.to_le_bytes());
                    payload[8..10].copy_from_slice(&(burst as u16).to_le_bytes());
                    let sent = sender.send(&payload)?;
                    if sent != PAYLOAD_LEN {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            format!("short UDP send: {sent}"),
                        ));
                    }
                }
                Ok(())
            })();
            round_sent_tx
                .send(result.map_err(|error| error.to_string()))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });

    Ok(SocketPair {
        receiver,
        sender_control: SenderControl {
            start_round: start_round_tx,
            round_sent: round_sent_rx,
            handle,
        },
        sender_addr,
        receive_buffer_bytes,
    })
}

fn verify_packet(
    packet: &[u8],
    source: SocketAddr,
    sender_addr: SocketAddr,
    expected_sequence: usize,
    burst: usize,
) -> io::Result<()> {
    if packet.len() != PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "payload length mismatch: expected={PAYLOAD_LEN} actual={}",
                packet.len()
            ),
        ));
    }
    if source != sender_addr {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source mismatch: expected={sender_addr} actual={source}"),
        ));
    }
    let sequence = u64::from_le_bytes(packet[..8].try_into().unwrap()) as usize;
    if sequence != expected_sequence {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sequence mismatch: expected={expected_sequence} actual={sequence}"),
        ));
    }
    let encoded_burst = u16::from_le_bytes(packet[8..10].try_into().unwrap()) as usize;
    if encoded_burst != burst {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("burst mismatch: expected={burst} actual={encoded_burst}"),
        ));
    }
    Ok(())
}

fn thread_cpu_time() -> io::Result<Duration> {
    let mut value = MaybeUninit::<libc::timespec>::uninit();
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, value.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let value = unsafe { value.assume_init() };
    Ok(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

async fn run_tokio_single(rounds: usize, burst: usize) -> io::Result<RunResult> {
    let pair = make_socket_pair(rounds)?;
    let socket = UdpSocket::from_std(pair.receiver)?;
    let mut payload = [0u8; PAYLOAD_LEN];
    let packets = rounds * burst;
    let wall_start = Instant::now();
    let cpu_start = thread_cpu_time()?;
    let mut receive_time = Duration::ZERO;

    for round in 0..rounds {
        pair.sender_control.start_round(burst)?;
        let receive_start = Instant::now();
        for index in 0..burst {
            let (length, source) = socket.recv_from(&mut payload).await?;
            verify_packet(
                &payload[..length],
                source,
                pair.sender_addr,
                round * burst + index,
                burst,
            )?;
        }
        receive_time += receive_start.elapsed();
    }

    let receiver_cpu_time = thread_cpu_time()?.saturating_sub(cpu_start);
    let wall_time = wall_start.elapsed();
    pair.sender_control.finish()?;
    Ok(RunResult {
        mode: Mode::TokioSingle,
        burst,
        rounds,
        packets,
        receive_syscalls: packets,
        receive_time,
        receiver_cpu_time,
        wall_time,
        receive_buffer_bytes: pair.receive_buffer_bytes,
    })
}

async fn recv_ready_batch(
    socket: &AsyncFd<StdUdpSocket>,
    buffers: &mut BatchBuffers,
    limit: usize,
) -> io::Result<usize> {
    loop {
        let mut guard = socket.readable().await?;
        match guard.try_io(|inner| buffers.recv(inner.get_ref().as_raw_fd(), limit)) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

async fn run_asyncfd_recvmmsg(rounds: usize, burst: usize) -> io::Result<RunResult> {
    let pair = make_socket_pair(rounds)?;
    let socket = AsyncFd::new(pair.receiver)?;
    let mut buffers = BatchBuffers::new();
    let packets = rounds * burst;
    let wall_start = Instant::now();
    let cpu_start = thread_cpu_time()?;
    let mut receive_time = Duration::ZERO;
    let mut receive_syscalls = 0usize;

    for round in 0..rounds {
        pair.sender_control.start_round(burst)?;
        let receive_start = Instant::now();
        let mut received_in_round = 0usize;
        while received_in_round < burst {
            let received = recv_ready_batch(
                &socket,
                &mut buffers,
                (burst - received_in_round).min(MAX_BATCH),
            )
            .await?;
            if received == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "recvmmsg returned zero packets",
                ));
            }
            receive_syscalls += 1;
            for index in 0..received {
                let (payload, source) = buffers.packet(index)?;
                verify_packet(
                    payload,
                    source,
                    pair.sender_addr,
                    round * burst + received_in_round + index,
                    burst,
                )?;
            }
            received_in_round += received;
        }
        receive_time += receive_start.elapsed();
    }

    let receiver_cpu_time = thread_cpu_time()?.saturating_sub(cpu_start);
    let wall_time = wall_start.elapsed();
    pair.sender_control.finish()?;
    Ok(RunResult {
        mode: Mode::AsyncFdRecvmmsg,
        burst,
        rounds,
        packets,
        receive_syscalls,
        receive_time,
        receiver_cpu_time,
        wall_time,
        receive_buffer_bytes: pair.receive_buffer_bytes,
    })
}

fn parse_rounds() -> io::Result<usize> {
    let Some(value) = std::env::args().nth(1) else {
        return Ok(DEFAULT_ROUNDS);
    };
    let rounds = value.parse::<usize>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid rounds value {value:?}: {error}"),
        )
    })?;
    if rounds == 0 || rounds > 100_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rounds must be in 1..=100000",
        ));
    }
    Ok(rounds)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let rounds = parse_rounds()?;
    eprintln!("core-udp-batch-probe: rounds={rounds} payload={PAYLOAD_LEN} max_batch={MAX_BATCH}");
    RunResult::print_header();
    for burst in BURSTS {
        run_tokio_single(rounds, burst).await?.print();
        run_asyncfd_recvmmsg(rounds, burst).await?.print();
    }
    Ok(())
}
