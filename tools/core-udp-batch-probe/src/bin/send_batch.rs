use std::{
    io, mem,
    net::UdpSocket as StdUdpSocket,
    os::fd::AsRawFd,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use tokio::{io::Interest, net::UdpSocket, sync::mpsc as tokio_mpsc};

const PAYLOAD_LEN: usize = 1200;
const MAX_BATCH: usize = 32;
const CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone, Copy)]
enum SendMode {
    Single,
    DrainThenBatch,
}

#[derive(Default)]
struct Stats {
    packets: u64,
    send_syscalls: u64,
    batched_syscalls: u64,
}

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
            messages.push(libc::mmsghdr {
                msg_hdr: libc::msghdr {
                    msg_name: std::ptr::null_mut(),
                    msg_namelen: 0,
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
            iovecs,
            messages,
            length: 0,
        }
    }

    fn push(&mut self, payload: [u8; PAYLOAD_LEN]) {
        self.payloads[self.length] = payload;
        self.length += 1;
    }

    fn prepare(&mut self, offset: usize) {
        for index in offset..self.length {
            self.iovecs[index].iov_base = self.payloads[index].as_mut_ptr().cast();
            self.iovecs[index].iov_len = PAYLOAD_LEN;
            self.messages[index].msg_hdr.msg_iov = (&mut self.iovecs[index]) as *mut libc::iovec;
            self.messages[index].msg_hdr.msg_iovlen = 1;
            self.messages[index].msg_len = 0;
        }
    }

    fn clear(&mut self) {
        self.length = 0;
    }
}

fn set_socket_buffer(socket: &StdUdpSocket, option: libc::c_int) -> io::Result<()> {
    let bytes: libc::c_int = 4 * 1024 * 1024;
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

fn socket_pair() -> io::Result<(UdpSocket, StdUdpSocket)> {
    let receiver = StdUdpSocket::bind("127.0.0.1:0")?;
    set_socket_buffer(&receiver, libc::SO_RCVBUF)?;
    receiver.set_read_timeout(Some(Duration::from_secs(5)))?;
    let receiver_addr = receiver.local_addr()?;

    let sender = StdUdpSocket::bind("127.0.0.1:0")?;
    set_socket_buffer(&sender, libc::SO_SNDBUF)?;
    sender.connect(receiver_addr)?;
    sender.set_nonblocking(true)?;
    Ok((UdpSocket::from_std(sender)?, receiver))
}

fn receiver_thread(
    receiver: StdUdpSocket,
    total_packets: u64,
) -> (mpsc::Receiver<()>, thread::JoinHandle<io::Result<()>>) {
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || {
        let mut payload = [0_u8; PAYLOAD_LEN];
        for expected in 0..total_packets {
            let length = receiver.recv(&mut payload)?;
            if length != PAYLOAD_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "receiver length mismatch",
                ));
            }
            let sequence = u64::from_be_bytes(payload[..8].try_into().unwrap());
            if sequence != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("receiver sequence {sequence}, expected {expected}"),
                ));
            }
        }
        done_tx
            .send(())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "consumer stopped"))?;
        Ok(())
    });
    (done_rx, handle)
}

fn producer_thread(
    sender: tokio_mpsc::Sender<[u8; PAYLOAD_LEN]>,
    total_packets: u64,
    delay: Duration,
) -> thread::JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        for sequence in 0..total_packets {
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut payload = [0_u8; PAYLOAD_LEN];
            payload[..8].copy_from_slice(&sequence.to_be_bytes());
            sender
                .blocking_send(payload)
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer stopped"))?;
        }
        Ok(())
    })
}

async fn send_many(socket: &UdpSocket, batch: &mut SendBatch, stats: &mut Stats) -> io::Result<()> {
    let mut offset = 0;
    while offset < batch.length {
        batch.prepare(offset);
        socket.writable().await?;
        let result = socket.try_io(Interest::WRITABLE, || {
            let count = unsafe {
                libc::sendmmsg(
                    socket.as_raw_fd(),
                    batch.messages[offset..].as_mut_ptr(),
                    (batch.length - offset) as libc::c_uint,
                    libc::MSG_DONTWAIT,
                )
            };
            if count >= 0 {
                Ok(count as usize)
            } else {
                Err(io::Error::last_os_error())
            }
        });
        match result {
            Ok(sent) => {
                if sent == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "sendmmsg sent zero datagrams",
                    ));
                }
                offset += sent;
                stats.send_syscalls += 1;
                stats.batched_syscalls += 1;
            }
            Err(_would_block) => continue,
        }
    }
    Ok(())
}

async fn run_case(
    name: &str,
    mode: SendMode,
    total_packets: u64,
    producer_delay: Duration,
) -> io::Result<()> {
    let (socket, receiver) = socket_pair()?;
    let (receiver_done, receiver_handle) = receiver_thread(receiver, total_packets);
    let (queue_tx, mut queue_rx) = tokio_mpsc::channel(CHANNEL_CAPACITY);
    let producer_handle = producer_thread(queue_tx, total_packets, producer_delay);
    let started = Instant::now();
    let mut cpu_started: libc::timespec = unsafe { mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_started);
    }

    let mut stats = Stats::default();
    let mut batch = SendBatch::new();
    while let Some(first) = queue_rx.recv().await {
        match mode {
            SendMode::Single => {
                socket.send(&first).await?;
                stats.packets += 1;
                stats.send_syscalls += 1;
            }
            SendMode::DrainThenBatch => {
                batch.push(first);
                while batch.length < MAX_BATCH {
                    match queue_rx.try_recv() {
                        Ok(payload) => batch.push(payload),
                        Err(_) => break,
                    }
                }
                if batch.length == 1 {
                    socket.send(&batch.payloads[0]).await?;
                    stats.send_syscalls += 1;
                } else {
                    send_many(&socket, &mut batch, &mut stats).await?;
                }
                stats.packets += batch.length as u64;
                batch.clear();
            }
        }
    }

    let mut cpu_finished: libc::timespec = unsafe { mem::zeroed() };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_finished);
    }
    producer_handle
        .join()
        .map_err(|_| io::Error::other("producer panicked"))??;
    receiver_done
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "receiver did not finish"))?;
    receiver_handle
        .join()
        .map_err(|_| io::Error::other("receiver panicked"))??;

    let cpu_ns = ((cpu_finished.tv_sec - cpu_started.tv_sec) as i128 * 1_000_000_000
        + (cpu_finished.tv_nsec - cpu_started.tv_nsec) as i128) as u128;
    println!(
        "{name},{},{},{},{:.3},{:.1},{:.1},{}",
        match mode {
            SendMode::Single => "single",
            SendMode::DrainThenBatch => "drain_then_sendmmsg",
        },
        stats.packets,
        stats.send_syscalls,
        stats.packets as f64 / stats.send_syscalls as f64,
        started.elapsed().as_nanos() as f64 / stats.packets as f64,
        cpu_ns as f64 / stats.packets as f64,
        stats.batched_syscalls,
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    println!(
        "case,mode,packets,send_syscalls,packets_per_syscall,wall_ns_per_packet,cpu_ns_per_packet,batched_syscalls"
    );
    run_case("low", SendMode::Single, 512, Duration::from_millis(1)).await?;
    run_case(
        "low",
        SendMode::DrainThenBatch,
        512,
        Duration::from_millis(1),
    )
    .await?;
    run_case("high", SendMode::Single, 160_000, Duration::ZERO).await?;
    run_case("high", SendMode::DrainThenBatch, 160_000, Duration::ZERO).await
}
