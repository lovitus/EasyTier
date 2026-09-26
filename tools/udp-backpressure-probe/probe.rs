// Linux kernel readiness fixture only; not a Core or Tokio performance test.
use std::ffi::{c_int, c_void};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
    fn poll(fds: *mut PollFd, count: usize, timeout: c_int) -> c_int;
}

fn packet(sequence: u64) -> [u8; 1200] {
    let mut data = [0x5a; 1200];
    data[..8].copy_from_slice(&sequence.to_be_bytes());
    data
}

fn require(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn send(socket: &UdpSocket, data: &[u8]) -> io::Result<()> {
    require(socket.send(data)? == data.len(), "short UDP send")
}

fn resume(socket: &UdpSocket, data: &[u8], deadline: Instant) -> io::Result<(u32, u128)> {
    let started = Instant::now();
    let mut notifications = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        require(
            !remaining.is_zero(),
            "writability recovery deadline exceeded",
        )?;
        let mut descriptor = PollFd {
            fd: socket.as_raw_fd(),
            events: 4,
            revents: 0,
        };
        // POLLOUT on the actual socket, not an injected WouldBlock result.
        let result = unsafe {
            poll(
                &mut descriptor,
                1,
                remaining.as_millis().clamp(1, 5000) as c_int,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        require(result > 0, "socket did not become writable")?;
        require(
            descriptor.revents & (8 | 16 | 32) == 0,
            "socket poll reported an error",
        )?;
        require(descriptor.revents & 4 != 0, "missing POLLOUT")?;
        notifications += 1;
        require(notifications <= 16, "excessive writable retries")?;
        match send(socket, data) {
            Ok(()) => return Ok((notifications, started.elapsed().as_micros())),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    require(cfg!(target_os = "linux"), "Linux only")?;
    require(
        std::env::var("ET_UDP_BACKPRESSURE").as_deref() == Ok("ISOLATED_NETNS_ONLY"),
        "explicit lab opt-in required",
    )?;
    require(
        std::fs::read_link("/proc/self/ns/net")? != std::fs::read_link("/proc/1/ns/net")?,
        "host namespace forbidden",
    )?;
    let args: Vec<_> = std::env::args().collect();
    require(args.len() == 3, "usage: probe BIND_ADDRESS DESTINATION")?;
    let bind: SocketAddr = args[1].parse()?;
    let destination: SocketAddr = args[2].parse()?;
    let socket = UdpSocket::bind(bind)?;
    socket.connect(destination)?;
    socket.set_nonblocking(true)?;
    let send_buffer: c_int = 4096;
    // Linux SOL_SOCKET=1, SO_SNDBUF=7; no global socket or sysctl changes.
    let result = unsafe {
        setsockopt(
            socket.as_raw_fd(),
            1,
            7,
            (&send_buffer as *const c_int).cast(),
            size_of::<c_int>() as u32,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }

    let started = Instant::now();
    let mut accepted = 0u64;
    loop {
        require(
            accepted < 256 && started.elapsed() < Duration::from_secs(2),
            "fixture did not produce kernel EAGAIN within its packet/time budget",
        )?;
        match send(&socket, &packet(accepted)) {
            Ok(()) => accepted += 1,
            Err(error) if error.raw_os_error() == Some(11) => break,
            Err(error) => return Err(error.into()),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let (notifications, wait_us) = resume(&socket, &packet(accepted), deadline)?;
    accepted += 1;
    let mut finish = [0u8; 12];
    finish[..4].copy_from_slice(b"DONE");
    finish[4..].copy_from_slice(&accepted.to_be_bytes());
    match send(&socket, &finish) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            resume(&socket, &finish, deadline)?;
        }
        Err(error) => return Err(error.into()),
    }
    println!(
        "{{\"kernel_eagain\":true,\"errno\":11,\"accepted_before_eagain\":{},\"sent_datagrams\":{},\"writable_notifications\":{},\"recovery_wait_us\":{},\"requested_sndbuf\":4096}}",
        accepted - 1,
        accepted,
        notifications,
        wait_us
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequence_and_payload_are_deterministic() {
        for sequence in [0, 1, 255, 256] {
            let data = packet(sequence);
            assert_eq!(u64::from_be_bytes(data[..8].try_into().unwrap()), sequence);
            assert!(data[8..].iter().all(|byte| *byte == 0x5a));
        }
    }
}
