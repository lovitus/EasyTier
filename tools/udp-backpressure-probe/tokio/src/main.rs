// Only the fixture and frame/stat containers live here. Sending code is included
// verbatim from the actual Core experiment by build.rs, not reimplemented.
use bytes::Bytes;
use std::{future::Future, io, mem, net::SocketAddr, os::fd::AsRawFd, ptr, task::Poll};
use tokio::{
    io::Interest,
    net::UdpSocket,
    time::{Duration, timeout},
};

#[derive(PartialEq)]
enum Mode {
    Gso,
}
struct Frame {
    bytes: Bytes,
    batchable: bool,
}
struct WriterStats {
    mode: Mode,
    calls: u64,
    gso_calls: u64,
    eagain: u64,
    packets: u64,
}
impl WriterStats {
    fn new() -> Self {
        Self {
            mode: Mode::Gso,
            calls: 0,
            gso_calls: 0,
            eagain: 0,
            packets: 0,
        }
    }
}
include!(concat!(env!("OUT_DIR"), "/actual_send.rs"));

fn require(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}
fn packet(sequence: u64) -> [u8; 1200] {
    let mut data = [0x5a; 1200];
    data[..8].copy_from_slice(&sequence.to_be_bytes());
    data
}
fn frames(first: u64) -> Vec<Frame> {
    (first..first + 4)
        .map(|seq| Frame {
            bytes: Bytes::copy_from_slice(&packet(seq)),
            batchable: true,
        })
        .collect()
}
async fn finish(socket: &UdpSocket, destination: SocketAddr, count: u64) -> io::Result<()> {
    let mut marker = [0; 12];
    marker[..4].copy_from_slice(b"DONE");
    marker[4..].copy_from_slice(&count.to_be_bytes());
    let len = socket.send_to(&marker, destination).await?;
    require(len == marker.len(), "short terminal marker")
}

async fn exercise(
    bind: SocketAddr,
    destination: SocketAddr,
    mode: &str,
    second: SocketAddr,
) -> io::Result<(u64, u64, u64)> {
    let raw = std::net::UdpSocket::bind(bind)?;
    raw.connect(destination)?;
    raw.set_nonblocking(true)?;
    socket2::SockRef::from(&raw).set_send_buffer_size(4096)?;
    let socket = UdpSocket::from_std(raw.try_clone()?)?;
    // Prime Tokio readiness before raw writes fill the actual kernel queue. The
    // adapter must encounter EAGAIN itself, not merely await initial readiness.
    socket.writable().await?;
    let mut accepted = 0;
    loop {
        require(accepted < 256, "prefill did not reach EAGAIN")?;
        match raw.send(&packet(accepted)) {
            Ok(1200) => accepted += 1,
            Ok(_) => return Err(io::Error::other("short prefill")),
            Err(error) if error.raw_os_error() == Some(libc::EAGAIN) => break,
            Err(error) => return Err(error),
        }
    }
    let batch = frames(accepted);
    let other_batch = frames(0);
    let mut stats = WriterStats::new();
    let mut other_stats = WriterStats::new();
    let mut pending = Box::pin(send_frames(&socket, destination, &batch, &mut stats));
    let initial = std::future::poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx))).await;
    require(initial.is_pending(), "actual adapter send did not block")?;
    match mode {
        "recover" => require(pending.await?, "adapter ended without sending")?,
        "cancel" => {
            tokio::select! {
                result = &mut pending => {
                    result?;
                    return Err(io::Error::other("send recovered before cancellation checkpoint"));
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
            drop(pending);
            require(
                stats.eagain > 0 && stats.packets == 0,
                "cancelled operation did not retain an unsubmitted batch",
            )?;
            require(
                send_frames(&socket, destination, &batch, &mut stats).await?,
                "cancelled send did not recover",
            )?;
        }
        "shared" => {
            let (first, other) = tokio::try_join!(
                pending,
                send_frames(&socket, second, &other_batch, &mut other_stats)
            )?;
            require(first && other, "shared socket send failed")?;
        }
        _ => return Err(io::Error::other("unknown mode")),
    }
    require(
        stats.eagain > 0,
        "no kernel EAGAIN observed inside actual adapter",
    )?;
    require(
        stats.gso_calls > 0 && stats.packets == 4,
        "GSO batch not sent exactly once",
    )?;
    finish(&socket, destination, accepted + 4).await?;
    if mode == "shared" {
        require(
            other_stats.gso_calls > 0 && other_stats.packets == 4,
            "second destination did not progress",
        )?;
        finish(&socket, second, 4).await?;
    }
    Ok((
        accepted + 4,
        stats.eagain + other_stats.eagain,
        stats.gso_calls + other_stats.gso_calls,
    ))
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    require(
        std::env::var("ET_UDP_BACKPRESSURE").as_deref() == Ok("ISOLATED_NETNS_ONLY"),
        "lab opt-in required",
    )?;
    require(
        std::fs::read_link("/proc/self/ns/net")? != std::fs::read_link("/proc/1/ns/net")?,
        "host namespace forbidden",
    )?;
    let args: Vec<_> = std::env::args().collect();
    require(
        args.len() == 5,
        "usage: probe BIND DESTINATION MODE SECOND_DESTINATION",
    )?;
    let bind = args[1].parse()?;
    let destination = args[2].parse()?;
    let mode = &args[3];
    let second = args[4].parse()?;
    if mode.starts_with("reject-") {
        return timeout(
            Duration::from_secs(8),
            characterize_rejection(bind, destination, mode),
        )
        .await?
        .map_err(Into::into);
    }
    let (finished, mut finish_rx) = tokio::sync::oneshot::channel::<()>();
    let work = async {
        let result = timeout(
            Duration::from_secs(8),
            exercise(bind, destination, mode, second),
        )
        .await;
        drop(finished);
        result
    };
    let heartbeat = async {
        let mut ticks = 0u64;
        loop {
            tokio::select! {
                _ = &mut finish_rx => break ticks,
                _ = tokio::time::sleep(Duration::from_millis(10)) => ticks += 1,
            }
        }
    };
    let (result, ticks) = tokio::join!(work, heartbeat);
    let (sent, eagain, gso_calls) = result??;
    require(ticks > 0, "current-thread timer made no progress")?;
    println!(
        "{{\"kernel_eagain\":true,\"adapter_eagain\":{eagain},\"gso_calls\":{gso_calls},\"sent_datagrams\":{sent},\"heartbeat_ticks\":{ticks},\"mode\":\"{mode}\"}}"
    );
    Ok(())
}

fn socket_option(socket: &UdpSocket, level: i32, option: i32, value: i32) -> io::Result<()> {
    // Only this fixture-owned socket is changed. No interface-wide defaults.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            option,
            (&value as *const i32).cast(),
            size_of::<i32>() as _,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

async fn characterize_rejection(
    bind: SocketAddr,
    destination: SocketAddr,
    mode: &str,
) -> io::Result<()> {
    let socket = UdpSocket::bind(bind).await?;
    let checksum_case = mode == "reject-checksum";
    require(
        checksum_case || mode == "reject-mtu",
        "unknown rejection case",
    )?;
    if checksum_case {
        require(destination.is_ipv4(), "zero-checksum control is IPv4-only")?;
        socket_option(&socket, libc::SOL_SOCKET, libc::SO_NO_CHECK, 1)?;
    } else {
        let (level, option) = if destination.is_ipv4() {
            (libc::IPPROTO_IP, libc::IP_MTU_DISCOVER)
        } else {
            (libc::IPPROTO_IPV6, libc::IPV6_MTU_DISCOVER)
        };
        socket_option(&socket, level, option, libc::IP_PMTUDISC_DO)?;
    }
    let mut rejected = frames(0);
    if !checksum_case {
        // The isolated link MTU is 1280. An individual 1400-byte datagram
        // must not become silently acceptable by removing UDP_SEGMENT.
        for frame in &mut rejected {
            let mut bytes = frame.bytes.to_vec();
            bytes.resize(1400, 0x5a);
            frame.bytes = Bytes::from(bytes);
        }
    }
    let mut stats = WriterStats::new();
    let error = send_frames(&socket, destination, &rejected, &mut stats)
        .await
        .expect_err("kernel must reject this GSO request");
    require(
        error.raw_os_error() == Some(libc::EINVAL),
        "unexpected GSO rejection errno",
    )?;
    require(stats.packets == 0, "rejected group was recorded as sent")?;
    let ordinary_errno;
    let delivered;
    if checksum_case {
        for frame in &rejected {
            require(
                socket.send_to(&frame.bytes, destination).await? == frame.bytes.len(),
                "short ordinary datagram",
            )?;
        }
        socket_option(&socket, libc::SOL_SOCKET, libc::SO_NO_CHECK, 0)?;
        require(
            send_frames(&socket, destination, &frames(4), &mut stats).await?,
            "GSO did not recover after restoring checksum",
        )?;
        ordinary_errno = 0;
        delivered = 8;
    } else {
        let error = socket
            .send_to(&rejected[0].bytes, destination)
            .await
            .expect_err("ordinary send must retain PMTU failure");
        require(
            error.raw_os_error() == Some(libc::EMSGSIZE),
            "ordinary send lost EMSGSIZE",
        )?;
        require(
            send_frames(&socket, destination, &frames(0), &mut stats).await?,
            "valid smaller GSO group failed",
        )?;
        ordinary_errno = libc::EMSGSIZE;
        delivered = 4;
    }
    require(
        stats.packets == 4 && stats.gso_calls >= 2,
        "post-error GSO control did not run",
    )?;
    finish(&socket, destination, delivered).await?;
    println!(
        "{{\"kernel_eagain\":false,\"gso_rejection_errno\":{},\"ordinary_errno\":{ordinary_errno},\"gso_calls\":{},\"gso_after_error\":true,\"sent_datagrams\":{delivered},\"mode\":\"{mode}\"}}",
        libc::EINVAL,
        stats.gso_calls
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
