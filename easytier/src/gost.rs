use anyhow::{Context as _, bail};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::UdpSocket,
    process::{Child, Command},
};
use tokio_util::sync::CancellationToken;

use crate::managed_child::{ManagedChild, configure_command};

pub const DEFAULT_PORT_CANDIDATES: [u16; 3] = [11080, 11081, 11082];
const UDP_BUFFER_SIZE: usize = 65_535;
const UDP_SOURCE_CHECK: &str = "first-packet";
const UDP_READINESS_PAYLOAD_SIZE: usize = 8 * 1024;
const READINESS_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct GostProcessConfig {
    pub executable: PathBuf,
    pub listen_address: IpAddr,
    pub port_candidates: Vec<u16>,
    pub setup_timeout: Duration,
}

impl GostProcessConfig {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            listen_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port_candidates: DEFAULT_PORT_CANDIDATES.to_vec(),
            setup_timeout: Duration::from_secs(5),
        }
    }
}

pub struct GostRuntime {
    process: ManagedChild,
    endpoint: SocketAddr,
}

impl GostRuntime {
    pub async fn start(config: GostProcessConfig) -> anyhow::Result<Self> {
        if config.port_candidates.is_empty() {
            bail!("GOST mesh entry requires at least one port candidate");
        }
        if !config.listen_address.is_loopback() {
            bail!("GOST mesh entry must listen on a loopback address");
        }
        if !config.executable.is_file() {
            bail!(
                "managed GOST mesh-entry executable is missing: {}",
                config.executable.display()
            );
        }

        let mut failures = Vec::new();
        for port in config.port_candidates.iter().copied() {
            if port == 0 {
                failures.push("port 0 is not a valid managed GOST candidate".to_owned());
                continue;
            }
            let endpoint = SocketAddr::new(config.listen_address, port);
            match Self::start_candidate(&config.executable, endpoint, config.setup_timeout).await {
                Ok(process) => return Ok(Self { process, endpoint }),
                Err(error) => failures.push(format!("{endpoint}: {error:#}")),
            }
        }

        bail!(
            "no managed GOST mesh-entry candidate became ready: {}",
            failures.join("; ")
        )
    }

    async fn start_candidate(
        executable: &std::path::Path,
        endpoint: SocketAddr,
        setup_timeout: Duration,
    ) -> anyhow::Result<ManagedChild> {
        let mut command = Command::new(executable);
        command
            .arg("-L")
            // This exact 65,535-byte value is the qualified project baseline:
            // policy_proxy_validation_2026_07_13.md rejects the prior 4 MiB
            // experiment and records bounded 20/50/100 Mbit/s behavior here.
            .arg(gost_listener_url(endpoint))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("failed to start managed GOST at {endpoint}"))?;
        let mut process = ManagedChild::attach(child, executable).await?;
        if let Err(error) =
            wait_for_socks5_tcp_udp(process.child_mut(), endpoint, setup_timeout).await
        {
            process.terminate(Duration::from_secs(3)).await;
            return Err(error);
        }
        Ok(process)
    }

    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    pub async fn run_until_cancel(&mut self, cancel: CancellationToken) -> anyhow::Result<()> {
        let status = tokio::select! {
            status = self.process.wait() => Some(status),
            _ = cancel.cancelled() => None,
        };
        if let Some(status) = status {
            let status = status.context("failed waiting for managed GOST")?;
            bail!("managed GOST exited with {status}");
        }
        self.process.terminate(Duration::from_secs(3)).await;
        Ok(())
    }
}

fn gost_listener_url(endpoint: SocketAddr) -> String {
    format!(
        "socks5://{endpoint}?udp=true&udpBufferSize={UDP_BUFFER_SIZE}&udpSourceCheck={UDP_SOURCE_CHECK}"
    )
}

impl Drop for GostRuntime {
    fn drop(&mut self) {
        self.process.start_kill();
    }
}

async fn wait_for_socks5_tcp_udp(
    child: &mut Child,
    endpoint: SocketAddr,
    setup_timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + setup_timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect managed GOST status")?
        {
            bail!("managed GOST exited before readiness with {status}");
        }
        if tokio::time::timeout(READINESS_ATTEMPT_TIMEOUT, probe_socks5_tcp_udp(endpoint))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("managed GOST TCP/UDP readiness timed out");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn probe_socks5_tcp_udp(endpoint: SocketAddr) -> anyhow::Result<()> {
    let mut stream = tokio::net::TcpStream::connect(endpoint)
        .await
        .context("SOCKS5 TCP listener is not ready")?;
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut greeting = [0_u8; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting != [0x05, 0x00] {
        bail!("SOCKS5 server rejected no-authentication readiness probe");
    }

    stream
        .write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 || header[1] != 0x00 || header[2] != 0x00 {
        bail!(
            "SOCKS5 UDP ASSOCIATE readiness failed with reply {:02x?}",
            header
        );
    }
    let mut relay_endpoint = read_socks5_socket_addr(&mut stream, header[3]).await?;
    if relay_endpoint.port() == 0 {
        bail!("SOCKS5 UDP ASSOCIATE returned port 0");
    }
    if relay_endpoint.ip().is_unspecified() {
        relay_endpoint.set_ip(endpoint.ip());
    }
    probe_socks5_udp_relay(relay_endpoint).await
}

async fn read_socks5_socket_addr(
    stream: &mut tokio::net::TcpStream,
    atyp: u8,
) -> anyhow::Result<SocketAddr> {
    let ip = match atyp {
        0x01 => {
            let mut address = [0_u8; 4];
            stream.read_exact(&mut address).await?;
            IpAddr::V4(Ipv4Addr::from(address))
        }
        0x04 => {
            let mut address = [0_u8; 16];
            stream.read_exact(&mut address).await?;
            IpAddr::V6(Ipv6Addr::from(address))
        }
        0x03 => {
            bail!("SOCKS5 UDP ASSOCIATE returned a domain instead of a bound IP address")
        }
        _ => bail!("SOCKS5 UDP ASSOCIATE returned invalid ATYP {atyp:#04x}"),
    };
    let mut port = [0_u8; 2];
    stream.read_exact(&mut port).await?;
    Ok(SocketAddr::new(ip, u16::from_be_bytes(port)))
}

async fn probe_socks5_udp_relay(relay_endpoint: SocketAddr) -> anyhow::Result<()> {
    let loopback = if relay_endpoint.is_ipv4() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    };
    let echo = UdpSocket::bind(SocketAddr::new(loopback, 0))
        .await
        .context("failed to bind GOST UDP readiness echo socket")?;
    let client = UdpSocket::bind(SocketAddr::new(loopback, 0))
        .await
        .context("failed to bind GOST UDP readiness client socket")?;
    let echo_endpoint = echo.local_addr()?;
    let payload: Vec<u8> = (0..UDP_READINESS_PAYLOAD_SIZE)
        .map(|index| (index % 251) as u8)
        .collect();
    let request = encode_socks5_udp_request(echo_endpoint, &payload);
    client
        .send_to(&request, relay_endpoint)
        .await
        .context("failed to send GOST UDP readiness request")?;

    let mut echo_buffer = vec![0_u8; UDP_READINESS_PAYLOAD_SIZE + 1];
    let (echo_len, source) = echo
        .recv_from(&mut echo_buffer)
        .await
        .context("GOST UDP readiness request did not reach the echo socket")?;
    if echo_buffer[..echo_len] != payload {
        bail!("GOST UDP readiness request payload was corrupted");
    }
    echo.send_to(&echo_buffer[..echo_len], source)
        .await
        .context("failed to return GOST UDP readiness echo")?;

    let mut response = vec![0_u8; UDP_BUFFER_SIZE];
    let (response_len, _) = client
        .recv_from(&mut response)
        .await
        .context("GOST UDP readiness response was not relayed")?;
    let response_payload = socks5_udp_payload(&response[..response_len])?;
    if response_payload != payload {
        bail!("GOST UDP readiness response payload was corrupted");
    }
    Ok(())
}

fn encode_socks5_udp_request(destination: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len() + 22);
    packet.extend_from_slice(&[0, 0, 0]);
    match destination.ip() {
        IpAddr::V4(ip) => {
            packet.push(0x01);
            packet.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            packet.push(0x04);
            packet.extend_from_slice(&ip.octets());
        }
    }
    packet.extend_from_slice(&destination.port().to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn socks5_udp_payload(packet: &[u8]) -> anyhow::Result<&[u8]> {
    if packet.len() < 4 || packet[0..2] != [0, 0] {
        bail!("invalid SOCKS5 UDP response header");
    }
    if packet[2] != 0 {
        bail!("fragmented SOCKS5 UDP readiness response is unsupported");
    }
    let payload_offset = match packet[3] {
        0x01 => 10,
        0x04 => 22,
        0x03 => {
            let name_len = usize::from(
                *packet
                    .get(4)
                    .context("truncated SOCKS5 UDP domain response")?,
            );
            7 + name_len
        }
        atyp => bail!("invalid SOCKS5 UDP response ATYP {atyp:#04x}"),
    };
    packet
        .get(payload_offset..)
        .context("truncated SOCKS5 UDP response address")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks5_udp_readiness_packet_round_trips_large_ipv4_payload() {
        let payload = vec![0x5a; UDP_READINESS_PAYLOAD_SIZE];
        let packet = encode_socks5_udp_request(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53),
            &payload,
        );

        assert_eq!(socks5_udp_payload(&packet).unwrap(), payload);
        assert!(packet.len() > 4096);
    }

    #[test]
    fn socks5_udp_payload_rejects_fragmented_and_truncated_packets() {
        assert!(socks5_udp_payload(&[0, 0, 1, 1, 0, 0, 0, 0, 0, 53]).is_err());
        assert!(socks5_udp_payload(&[0, 0, 0, 4]).is_err());
    }

    #[test]
    fn managed_gost_uses_bounded_udp_and_first_packet_source_pinning() {
        let listener = gost_listener_url(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            DEFAULT_PORT_CANDIDATES[0],
        ));
        assert!(listener.contains("udpBufferSize=65535"));
        assert!(listener.contains("udpSourceCheck=first-packet"));
    }
}
