use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use hickory_proto::runtime::{
    RuntimeProvider, TokioHandle, TokioRuntimeProvider, TokioTime, iocompat::AsyncIoTokioAsStd,
};
use hickory_proto::xfer::Protocol;
use hickory_resolver::Resolver;
use hickory_resolver::config::{LookupIpStrategy, NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::name_server::GenericConnector;
use hickory_resolver::system_conf::read_system_conf;
use once_cell::sync::Lazy;
use tokio::net::{TcpSocket, TcpStream, UdpSocket, lookup_host};

use super::error::Error;

pub fn get_default_resolver_config() -> ResolverConfig {
    let mut default_resolve_config = ResolverConfig::new();
    default_resolve_config.add_name_server(NameServerConfig::new(
        "223.5.5.5:53".parse().unwrap(),
        Protocol::Udp,
    ));
    default_resolve_config.add_name_server(NameServerConfig::new(
        "180.184.1.1:53".parse().unwrap(),
        Protocol::Udp,
    ));
    default_resolve_config
}

pub static ALLOW_USE_SYSTEM_DNS_RESOLVER: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(true));
static CONTROL_PLANE_SOCKET_MARK: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Default)]
struct ControlPlaneRuntimeProvider(TokioRuntimeProvider);

impl RuntimeProvider for ControlPlaneRuntimeProvider {
    type Handle = TokioHandle;
    type Timer = TokioTime;
    type Udp = UdpSocket;
    type Tcp = AsyncIoTokioAsStd<TcpStream>;

    fn create_handle(&self) -> Self::Handle {
        self.0.create_handle()
    }

    fn connect_tcp(
        &self,
        server_addr: SocketAddr,
        bind_addr: Option<SocketAddr>,
        wait_for: Option<Duration>,
    ) -> Pin<Box<dyn Send + Future<Output = io::Result<Self::Tcp>>>> {
        Box::pin(async move {
            let socket = match server_addr {
                SocketAddr::V4(_) => TcpSocket::new_v4(),
                SocketAddr::V6(_) => TcpSocket::new_v6(),
            }?;
            apply_control_plane_mark(&socket)?;
            if let Some(bind_addr) = bind_addr {
                socket.bind(bind_addr)?;
            }
            socket.set_nodelay(true)?;
            let wait_for = wait_for.unwrap_or(Duration::from_secs(5));
            let stream = tokio::time::timeout(wait_for, socket.connect(server_addr))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "DNS TCP connect timed out")
                })??;
            Ok(AsyncIoTokioAsStd(stream))
        })
    }

    fn bind_udp(
        &self,
        local_addr: SocketAddr,
        _server_addr: SocketAddr,
    ) -> Pin<Box<dyn Send + Future<Output = io::Result<Self::Udp>>>> {
        Box::pin(async move {
            let socket = match local_addr {
                SocketAddr::V4(_) => socket2::Socket::new(
                    socket2::Domain::IPV4,
                    socket2::Type::DGRAM,
                    Some(socket2::Protocol::UDP),
                ),
                SocketAddr::V6(_) => socket2::Socket::new(
                    socket2::Domain::IPV6,
                    socket2::Type::DGRAM,
                    Some(socket2::Protocol::UDP),
                ),
            }?;
            apply_control_plane_mark(&socket)?;
            socket.set_nonblocking(true)?;
            socket.bind(&local_addr.into())?;
            UdpSocket::from_std(socket.into())
        })
    }
}

#[cfg(target_os = "linux")]
fn apply_control_plane_mark<T: std::os::fd::AsFd>(socket: &T) -> io::Result<()> {
    let mark = CONTROL_PLANE_SOCKET_MARK.load(Ordering::Acquire);
    if mark != 0 {
        socket2::SockRef::from(socket).set_mark(mark)?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn apply_control_plane_mark<T>(_socket: &T) -> io::Result<()> {
    Ok(())
}

pub fn set_control_plane_socket_mark(mark: Option<u32>) {
    CONTROL_PLANE_SOCKET_MARK.store(mark.unwrap_or(0), Ordering::Release);
    ALLOW_USE_SYSTEM_DNS_RESOLVER.store(mark.is_none(), Ordering::Release);
}

pub(crate) static RESOLVER: Lazy<Arc<Resolver<GenericConnector<ControlPlaneRuntimeProvider>>>> =
    Lazy::new(|| {
        let system_cfg = read_system_conf();
        let mut cfg = get_default_resolver_config();
        let mut opt = ResolverOpts::default();
        if let Ok(s) = system_cfg {
            for ns in s.0.name_servers() {
                cfg.add_name_server(ns.clone());
            }
            opt = s.1;
        }
        opt.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let provider = GenericConnector::new(ControlPlaneRuntimeProvider::default());
        let builder = Resolver::builder_with_config(cfg, provider).with_options(opt);
        Arc::new(builder.build())
    });

pub async fn lookup_control_plane_host(host: &str) -> io::Result<Vec<SocketAddr>> {
    if CONTROL_PLANE_SOCKET_MARK.load(Ordering::Acquire) == 0 {
        return Ok(lookup_host(host).await?.collect());
    }
    let (host, port) = host
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "host has no port"))?;
    let port = port
        .parse::<u16>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    RESOLVER
        .lookup_ip(host.trim_matches(['[', ']']))
        .await
        .map(|lookup| lookup.iter().map(|ip| SocketAddr::new(ip, port)).collect())
        .map_err(io::Error::other)
}

pub async fn resolve_txt_record(domain_name: &str) -> Result<String, Error> {
    let r = RESOLVER.clone();
    let response = r
        .txt_lookup(domain_name)
        .await
        .with_context(|| format!("txt_lookup failed, domain_name: {}", domain_name))?;

    let txt_record = response
        .iter()
        .next()
        .with_context(|| format!("no txt record found, domain_name: {}", domain_name))?;

    let txt_data = String::from_utf8_lossy(&txt_record.txt_data()[0]);
    tracing::info!(?txt_data, ?domain_name, "get txt record");

    Ok(txt_data.to_string())
}

pub async fn socket_addrs(
    url: &url::Url,
    default_port_number: impl Fn() -> Option<u16>,
) -> Result<Vec<SocketAddr>, Error> {
    let host = url.host().ok_or(Error::InvalidUrl(url.to_string()))?;
    let port = url
        .port()
        .or_else(default_port_number)
        .ok_or(Error::InvalidUrl(url.to_string()))?;

    // if host is an ip address, return it directly
    match host {
        url::Host::Ipv4(ip) => return Ok(vec![SocketAddr::new(std::net::IpAddr::V4(ip), port)]),
        url::Host::Ipv6(ip) => return Ok(vec![SocketAddr::new(std::net::IpAddr::V6(ip), port)]),
        _ => {}
    }
    let host = host.to_string();

    if ALLOW_USE_SYSTEM_DNS_RESOLVER.load(std::sync::atomic::Ordering::Relaxed) {
        let socket_addr = format!("{}:{}", host, port);
        match lookup_host(socket_addr).await {
            Ok(a) => {
                let a = a.collect();
                tracing::debug!(?a, "system dns lookup done");
                return Ok(a);
            }
            Err(e) => {
                tracing::error!(?e, "system dns lookup failed");
            }
        }
    }

    // use hickory_resolver
    let ret = RESOLVER.lookup_ip(&host).await.with_context(|| {
        format!(
            "hickory dns lookup_ip failed, host: {}, port: {}",
            host, port
        )
    })?;
    Ok(ret
        .iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guarden::defer;

    #[tokio::test]
    async fn test_socket_addrs() {
        let url = url::Url::parse("tcp://github-ci-test.easytier.cn:80").unwrap();
        let addrs = socket_addrs(&url, || Some(80)).await.unwrap();
        assert_eq!(2, addrs.len(), "addrs: {:?}", addrs);
        println!("addrs: {:?}", addrs);

        ALLOW_USE_SYSTEM_DNS_RESOLVER.store(false, std::sync::atomic::Ordering::Relaxed);
        defer!(
            ALLOW_USE_SYSTEM_DNS_RESOLVER.store(true, std::sync::atomic::Ordering::Relaxed);
        );
        let addrs = socket_addrs(&url, || Some(80)).await.unwrap();
        assert_eq!(2, addrs.len(), "addrs: {:?}", addrs);
        println!("addrs2: {:?}", addrs);
    }

    #[tokio::test]
    async fn socket_addrs_preserves_explicit_zero_port() {
        let cases = [
            ("ws://127.0.0.1:0", 80, 0),
            ("wss://127.0.0.1:0", 443, 0),
            ("ws://127.0.0.1", 80, 80),
            ("wss://127.0.0.1", 443, 443),
        ];

        for (raw_url, default_port, expected_port) in cases {
            let url = url::Url::parse(raw_url).unwrap();
            let addrs = socket_addrs(&url, || Some(default_port)).await.unwrap();
            assert_eq!(
                addrs,
                vec![SocketAddr::from(([127, 0, 0, 1], expected_port))]
            );
        }
    }
}
