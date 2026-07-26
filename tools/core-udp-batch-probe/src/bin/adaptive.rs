use std::{
    io,
    mem::{self, MaybeUninit},
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    os::fd::AsRawFd,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use tokio::{io::Interest, net::UdpSocket};

const PAYLOAD_LEN: usize = 1200;
const MAX_BATCH: usize = 32;
const ENTER_SAMPLE_PACKETS: u64 = 64;
const ENTER_WINDOW: Duration = Duration::from_millis(5);
const EXIT_SAMPLE_CALLS: u64 = 32;
const EXIT_MAX_PACKETS: u64 = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiveMode {
    Single,
    Batch,
}

#[derive(Default)]
struct Stats {
    packets: u64,
    syscalls: u64,
    batch_syscalls: u64,
    transitions_to_batch: u64,
    transitions_to_single: u64,
}

struct AdaptiveState {
    mode: ReceiveMode,
    single_window_started: Instant,
    single_window_packets: u64,
    batch_window_calls: u64,
    batch_window_packets: u64,
}

impl AdaptiveState {
    fn new() -> Self {
        Self {
            mode: ReceiveMode::Single,
            single_window_started: Instant::now(),
            single_window_packets: 0,
            batch_window_calls: 0,
            batch_window_packets: 0,
        }
    }

    fn record_single(&mut self, stats: &mut Stats) {
        self.single_window_packets += 1;
        if self.single_window_packets < ENTER_SAMPLE_PACKETS {
            return;
        }

        let elapsed = self.single_window_started.elapsed();
        self.single_window_packets = 0;
        self.single_window_started = Instant::now();
        if elapsed <= ENTER_WINDOW {
            self.mode = ReceiveMode::Batch;
            self.batch_window_calls = 0;
            self.batch_window_packets = 0;
            stats.transitions_to_batch += 1;
        }
    }

    fn record_batch(&mut self, packets: u64, stats: &mut Stats) {
        self.batch_window_calls += 1;
        self.batch_window_packets += packets;
        if self.batch_window_calls < EXIT_SAMPLE_CALLS {
            return;
        }

        let should_exit = self.batch_window_packets <= EXIT_MAX_PACKETS;
        self.batch_window_calls = 0;
        self.batch_window_packets = 0;
        if should_exit {
            self.mode = ReceiveMode::Single;
            self.single_window_packets = 0;
            self.single_window_started = Instant::now();
            stats.transitions_to_single += 1;
        }
    }
}

struct BatchBuffers {
    payloads: Vec<[u8; PAYLOAD_LEN]>,
    _addresses: Vec<libc::sockaddr_storage>,
    iovecs: Vec<libc::iovec>,
    messages: Vec<libc::mmsghdr>,
}

impl BatchBuffers {
    fn new() -> Self {
        let mut payloads = vec![[0; PAYLOAD_LEN]; MAX_BATCH];
        let mut addresses = vec![unsafe { MaybeUninit::zeroed().assume_init() }; MAX_BATCH];
        let mut iovecs = Vec::with_capacity(MAX_BATCH);
        let mut messages = Vec::with_capacity(MAX_BATCH);

        for index in 0..MAX_BATCH {
            iovecs.push(libc::iovec {
                iov_base: payloads[index].as_mut_ptr().cast(),
                iov_len: PAYLOAD_LEN,
            });
            messages.push(libc::mmsghdr {
                msg_hdr: libc::msghdr {
                    msg_name: (&mut addresses[index] as *mut libc::sockaddr_storage).cast(),
                    msg_namelen: mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
                    msg_iov: (&mut iovecs[index]) as *mut libc::iovec,
                    msg_iovlen: 1,
                    msg_control: std::ptr::null_mut(),
                    msg_controllen: 0,
                    msg_flags: 0,
                },
                msg_len: 0,
            });
        }

        Self {
            payloads,
            _addresses: addresses,
            iovecs,
            messages,
        }
    }

    fn reset(&mut self) {
        for index in 0..MAX_BATCH {
            self.messages[index].msg_len = 0;
            self.messages[index].msg_hdr.msg_namelen =
                mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            self.messages[index].msg_hdr.msg_iov = (&mut self.iovecs[index]) as *mut libc::iovec;
            self.messages[index].msg_hdr.msg_iovlen = 1;
        }
    }
}

fn set_socket_buffer(socket: &StdUdpSocket) -> io::Result<()> {
    let bytes: libc::c_int = 4 * 1024 * 1024;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
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

fn socket_pair() -> io::Result<(UdpSocket, StdUdpSocket, SocketAddr)> {
    let receiver = StdUdpSocket::bind("127.0.0.1:0")?;
    set_socket_buffer(&receiver)?;
    receiver.set_nonblocking(true)?;
    let receiver_addr = receiver.local_addr()?;

    let sender = StdUdpSocket::bind("127.0.0.1:0")?;
    sender.connect(receiver_addr)?;
    Ok((UdpSocket::from_std(receiver)?, sender, receiver_addr))
}

fn sender_thread(
    sender: StdUdpSocket,
    bursts: Vec<(usize, Duration)>,
) -> (mpsc::Receiver<()>, thread::JoinHandle<io::Result<()>>) {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || {
        let mut sequence = 0_u64;
        let mut payload = [0_u8; PAYLOAD_LEN];
        for (burst, delay) in bursts {
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            for _ in 0..burst {
                payload[..8].copy_from_slice(&sequence.to_be_bytes());
                sender.send(&payload)?;
                sequence += 1;
            }
        }
        ready_tx
            .send(())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "receiver stopped"))?;
        Ok(())
    });
    (ready_rx, handle)
}

async fn receive_one(socket: &UdpSocket, expected: u64) -> io::Result<()> {
    let mut payload = [0_u8; PAYLOAD_LEN];
    let (length, source) = socket.recv_from(&mut payload).await?;
    if length != PAYLOAD_LEN || !source.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "single receive metadata mismatch",
        ));
    }
    let sequence = u64::from_be_bytes(payload[..8].try_into().unwrap());
    if sequence != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("single sequence {sequence}, expected {expected}"),
        ));
    }
    Ok(())
}

async fn receive_batch(
    socket: &UdpSocket,
    buffers: &mut BatchBuffers,
    expected: u64,
    remaining: u64,
) -> io::Result<usize> {
    loop {
        socket.readable().await?;
        buffers.reset();
        let batch_limit = remaining.min(MAX_BATCH as u64) as usize;
        let result = socket.try_io(Interest::READABLE, || {
            let count = unsafe {
                libc::recvmmsg(
                    socket.as_raw_fd(),
                    buffers.messages.as_mut_ptr(),
                    batch_limit as libc::c_uint,
                    libc::MSG_DONTWAIT,
                    std::ptr::null_mut(),
                )
            };
            if count >= 0 {
                Ok(count as usize)
            } else {
                Err(io::Error::last_os_error())
            }
        });

        match result {
            Ok(count) => {
                for index in 0..count {
                    if buffers.messages[index].msg_len as usize != PAYLOAD_LEN {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "batch receive length mismatch",
                        ));
                    }
                    let sequence =
                        u64::from_be_bytes(buffers.payloads[index][..8].try_into().unwrap());
                    let wanted = expected + index as u64;
                    if sequence != wanted {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("batch sequence {sequence}, expected {wanted}"),
                        ));
                    }
                }
                return Ok(count);
            }
            Err(_would_block) => continue,
        }
    }
}

async fn run_case(name: &str, bursts: Vec<(usize, Duration)>) -> io::Result<()> {
    let total_packets: u64 = bursts.iter().map(|(burst, _)| *burst as u64).sum();
    let (receiver, sender, _receiver_addr) = socket_pair()?;
    let (sender_done, sender_handle) = sender_thread(sender, bursts);
    let started = Instant::now();
    let mut cpu_started: libc::timespec = unsafe { mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_started);
    }

    let mut state = AdaptiveState::new();
    let mut stats = Stats::default();
    let mut buffers = BatchBuffers::new();
    while stats.packets < total_packets {
        match state.mode {
            ReceiveMode::Single => {
                receive_one(&receiver, stats.packets).await?;
                stats.packets += 1;
                stats.syscalls += 1;
                state.record_single(&mut stats);
            }
            ReceiveMode::Batch => {
                let count = receive_batch(
                    &receiver,
                    &mut buffers,
                    stats.packets,
                    total_packets - stats.packets,
                )
                .await?;
                stats.packets += count as u64;
                stats.syscalls += 1;
                stats.batch_syscalls += 1;
                state.record_batch(count as u64, &mut stats);
            }
        }
    }

    let mut cpu_finished: libc::timespec = unsafe { mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_finished);
    }
    sender_done
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "sender did not finish"))?;
    sender_handle
        .join()
        .map_err(|_| io::Error::other("sender panicked"))??;

    let cpu_ns = ((cpu_finished.tv_sec - cpu_started.tv_sec) as i128 * 1_000_000_000
        + (cpu_finished.tv_nsec - cpu_started.tv_nsec) as i128) as u128;
    println!(
        "{name},{},{},{:.3},{:.1},{:.1},{},{},{}",
        stats.packets,
        stats.syscalls,
        stats.packets as f64 / stats.syscalls as f64,
        started.elapsed().as_nanos() as f64 / stats.packets as f64,
        cpu_ns as f64 / stats.packets as f64,
        stats.batch_syscalls,
        stats.transitions_to_batch,
        stats.transitions_to_single,
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    println!(
        "case,packets,recv_syscalls,packets_per_syscall,wall_ns_per_packet,cpu_ns_per_packet,batch_syscalls,to_batch,to_single"
    );

    let low = vec![(1, Duration::from_millis(1)); 512];
    let high = vec![(32, Duration::ZERO); 5000];
    let mut mixed = vec![(1, Duration::from_millis(1)); 256];
    mixed.extend(vec![(32, Duration::ZERO); 2500]);
    mixed.extend(vec![(1, Duration::from_millis(1)); 256]);

    run_case("adaptive_low", low).await?;
    run_case("adaptive_high", high).await?;
    run_case("adaptive_mixed", mixed).await
}
