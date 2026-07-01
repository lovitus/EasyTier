mod netfilter;
mod packet;
mod stack;

use bytes::BytesMut;
use futures::{Sink, Stream, StreamExt as _, stream::FuturesUnordered};
use network_interface::NetworkInterfaceConfig;
use pnet::util::MacAddr;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::{io::AsyncReadExt, net::TcpStream};

use crate::tunnel::{
    FromUrl, IpVersion, SinkError, SinkItem, StreamItem, Tunnel, TunnelConnector, TunnelError,
    TunnelInfo, TunnelListener,
    common::{StealthFramedReader, StealthTcpZCPacketToBytes, TunnelWrapper, ZCPacketToBytes},
    fake_tcp::netfilter::create_tun,
    packet_def::{PEER_MANAGER_HEADER_SIZE, TCP_TUNNEL_HEADER_SIZE, ZCPacket, ZCPacketType},
};

use futures::Future;
use tokio_util::task::AbortOnDropHandle;

use dashmap::DashMap;

struct IpToIfNameCache {
    ip_to_ifname: DashMap<IpAddr, (String, Option<MacAddr>)>,
}

impl IpToIfNameCache {
    fn new() -> Self {
        Self {
            ip_to_ifname: DashMap::new(),
        }
    }

    fn reload_ip_to_ifname(&self) {
        self.ip_to_ifname.clear();
        let Ok(interfaces) = network_interface::NetworkInterface::show() else {
            tracing::warn!("failed to enumerate interfaces when reloading faketcp ip cache");
            return;
        };
        for iface in interfaces {
            let mac = iface.mac_addr.as_deref().and_then(|mac| {
                mac.parse::<MacAddr>().map_err(|e| {
                    tracing::debug!(iface = %iface.name, mac, ?e, "failed to parse interface mac")
                }).ok()
            });
            for ip in iface.addr.iter() {
                self.ip_to_ifname.insert(ip.ip(), (iface.name.clone(), mac));
            }
        }
    }

    fn get_ifname(&self, ip: &IpAddr) -> Option<(String, Option<MacAddr>)> {
        if let Some(ifname) = self.ip_to_ifname.get(ip) {
            Some(ifname.clone())
        } else {
            self.reload_ip_to_ifname();
            self.ip_to_ifname.get(ip).map(|s| s.clone())
        }
    }
}

fn get_faketcp_tunnel_type_str(driver_type: &str) -> String {
    format!("faketcp_{}", driver_type)
}

async fn create_tun_off_runtime(
    interface_name: String,
    src_addr: Option<SocketAddr>,
    dst_addr: SocketAddr,
) -> Result<Arc<dyn stack::Tun>, TunnelError> {
    tokio::task::spawn_blocking(move || create_tun(&interface_name, src_addr, dst_addr))
        .await
        .map_err(|e| TunnelError::InternalError(format!("faketcp create_tun task failed: {e}")))?
        .map_err(Into::into)
}

pub struct FakeTcpTunnelListener {
    addr: url::Url,
    os_listener: Option<tokio::net::TcpListener>,
    // interface_name -> fake tcp stack
    stack_map: DashMap<String, Arc<stack::Stack>>,
    // a cache from ip addr to interface name
    ip_to_ifname: IpToIfNameCache,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    replay_guard: Arc<crate::tunnel::stealth::GateReplayGuard>,
}

impl FakeTcpTunnelListener {
    pub fn new(addr: url::Url) -> Self {
        // Define filter: Capture all packets (or refine this if needed)
        // For FakeTCP, we probably want to capture packets destined to us?
        // But `stack::Stack` handles IP/TCP logic.
        // Maybe we just capture everything for now as a raw tunnel?
        // Or better, filter based on some criteria?
        // The user said "satisfy filter function".
        // Let's create a filter that accepts everything for now, or maybe only IP packets?
        FakeTcpTunnelListener {
            addr,
            os_listener: None,
            stack_map: DashMap::new(),
            ip_to_ifname: IpToIfNameCache::new(),
            stealth: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            replay_guard: Arc::new(crate::tunnel::stealth::GateReplayGuard::default()),
        }
    }

    pub fn set_stealth(&mut self, stealth: Arc<crate::tunnel::stealth::OuterSessionState>) {
        self.stealth = stealth;
    }

    async fn do_accept(&self) -> Result<AcceptResult, TunnelError> {
        loop {
            match self.os_listener.as_ref().unwrap().accept().await {
                Ok((s, remote_addr)) => {
                    let Ok(local_addr) = s.local_addr() else {
                        tracing::warn!("accept fail with local_addr error");
                        continue;
                    };
                    let Some((interface_name, mac)) =
                        self.ip_to_ifname.get_ifname(&local_addr.ip())
                    else {
                        tracing::warn!("accept fail with interface_name error");
                        continue;
                    };
                    return Ok(AcceptResult {
                        socket: s,
                        local_addr,
                        remote_addr,
                        interface_name,
                        mac,
                    });
                }
                Err(e) => {
                    use std::io::ErrorKind::*;
                    if matches!(
                        e.kind(),
                        NotConnected | ConnectionAborted | ConnectionRefused | ConnectionReset
                    ) {
                        tracing::warn!(?e, "accept fail with retryable error: {:?}", e);
                        continue;
                    }
                    tracing::warn!(?e, "accept fail");
                    return Err(e.into());
                }
            }
        }
    }

    async fn get_stack(
        &self,
        accept_result: &AcceptResult,
    ) -> Result<Arc<stack::Stack>, TunnelError> {
        let local_socket_addr = accept_result.local_addr;

        let interface_name = &accept_result.interface_name;

        let (local_ip, local_ip6) = match local_socket_addr.ip() {
            IpAddr::V4(ip) => (Some(ip), None),
            IpAddr::V6(ip) => (None, Some(ip)),
        };

        if let Some(entry) = self.stack_map.get(interface_name) {
            let stack = entry.clone();
            drop(entry);

            if !stack.is_closed() {
                return Ok(stack);
            }

            tracing::warn!(
                interface_name,
                "fake_tcp stack reader_task finished, recreating stack"
            );
            self.stack_map.remove(interface_name);
        }

        let tun =
            create_tun_off_runtime(interface_name.to_string(), None, local_socket_addr).await?;
        tracing::info!(
            ?local_socket_addr,
            "create new stack with interface_name: {:?}",
            interface_name
        );
        let stack = Arc::new(stack::Stack::new(
            tun,
            local_ip.unwrap_or(Ipv4Addr::UNSPECIFIED),
            local_ip6,
            accept_result.mac,
        ));
        self.stack_map
            .insert(interface_name.to_string(), stack.clone());

        Ok(stack)
    }

    async fn prepare_raw_accept(
        &self,
    ) -> Result<(AcceptResult, Arc<stack::Stack>, stack::Socket), TunnelError> {
        loop {
            let res = self.do_accept().await?;
            let stack = self.get_stack(&res).await?;
            let socket = stack.try_alloc_established_socket(
                res.local_addr,
                res.remote_addr,
                stack::State::Established,
            );
            let Some(socket) = socket else {
                tracing::warn!(
                    interface_name = res.interface_name,
                    "fake_tcp stack closed while accepting connection, dropping accepted socket"
                );
                self.stack_map.remove(&res.interface_name);
                continue;
            };
            return Ok((res, stack, socket));
        }
    }

    async fn finish_accept(
        res: AcceptResult,
        stack: Arc<stack::Stack>,
        socket: stack::Socket,
        local_url: url::Url,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
        replay_guard: Arc<crate::tunnel::stealth::GateReplayGuard>,
    ) -> Option<Box<dyn Tunnel>> {
        let socket = Arc::new(socket);
        let mut initial = BytesMut::new();
        let mut handshake_duplicate = None;
        let connection_stealth = stealth.fork_for_connection();
        if connection_stealth.is_enabled() {
            let authenticated = match tokio::time::timeout(
                std::time::Duration::from_secs(1),
                socket.recv(&mut initial),
            )
            .await
            {
                Ok(Some(_)) if initial.len() >= crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN => {
                    let preface = initial.split_to(crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN);
                    let verified = crate::tunnel::stealth::verify_stream_gate_preface(
                        connection_stealth.as_ref(),
                        replay_guard.as_ref(),
                        &preface,
                    );
                    verified.map(|verified| {
                        let raw = preface.as_ref().try_into().unwrap();
                        (verified, raw)
                    })
                }
                _ => None,
            };
            let Some((verified_preface, preface)) = authenticated else {
                tracing::trace!(?res, "rejected FakeTCP stealth connection");
                socket.close();
                return None;
            };
            let ack = crate::tunnel::stealth::build_stream_gate_ack(
                connection_stealth.as_ref(),
                &verified_preface,
            );
            socket.try_send(&ack)?;
            handshake_duplicate = Some((preface, Some(ack)));
        }

        tracing::info!(
            ?res,
            remote = socket.remote_addr().to_string(),
            "FakeTcpTunnelListener accepted connection"
        );
        let info = TunnelInfo {
            tunnel_type: get_faketcp_tunnel_type_str(stack.driver_type()),
            local_addr: Some(local_url.into()),
            remote_addr: Some(
                crate::tunnel::build_url_from_socket_addr(
                    &socket.remote_addr().to_string(),
                    "faketcp",
                )
                .into(),
            ),
            resolved_remote_addr: Some(
                crate::tunnel::build_url_from_socket_addr(
                    &socket.remote_addr().to_string(),
                    "faketcp",
                )
                .into(),
            ),
        };
        let reader = FakeTcpStream::new(
            socket.clone(),
            connection_stealth.clone(),
            initial,
            Some(Box::new(build_os_socket_reader_task(res.socket))),
            handshake_duplicate,
        );
        let writer = FakeTcpSink::new(socket, connection_stealth.clone());
        let associate_data = connection_stealth
            .is_enabled()
            .then(|| Box::new(connection_stealth) as Box<dyn std::any::Any + Send>);
        Some(Box::new(TunnelWrapper::new_with_associate_data(
            reader,
            writer,
            Some(info),
            associate_data,
        )))
    }
}

fn build_os_socket_reader_task(mut socket: TcpStream) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        // read the os socket until it's closed
        let mut buf = [0u8; 1024];
        while let Ok(size) = socket.read(&mut buf).await {
            tracing::trace!("read {} bytes from os socket", size);
            if size == 0 {
                break;
            }
        }
        tracing::info!("FakeTcpTunnelListener os socket closed");
    }))
}

#[derive(Debug)]
struct AcceptResult {
    socket: TcpStream,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    interface_name: String,
    mac: Option<MacAddr>,
}

#[async_trait::async_trait]
impl TunnelListener for FakeTcpTunnelListener {
    async fn listen(&mut self) -> Result<(), TunnelError> {
        let port = self.addr.port().unwrap_or(0);
        let bind_addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let os_listener = tokio::net::TcpListener::bind(bind_addr).await?;
        tracing::info!(port, "FakeTcpTunnelListener listening");
        self.os_listener = Some(os_listener);
        Ok(())
    }

    async fn accept(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        tracing::debug!("FakeTcpTunnelListener waiting for accept");
        if !self.stealth.is_enabled() {
            loop {
                let (res, stack, socket) = self.prepare_raw_accept().await?;
                if let Some(tunnel) = Self::finish_accept(
                    res,
                    stack,
                    socket,
                    self.local_url(),
                    self.stealth.clone(),
                    self.replay_guard.clone(),
                )
                .await
                {
                    return Ok(tunnel);
                }
            }
        }

        const MAX_PENDING_ACCEPTS: usize = 256;
        let mut pending = FuturesUnordered::new();
        loop {
            if pending.len() >= MAX_PENDING_ACCEPTS {
                if let Some(Some(tunnel)) = pending.next().await {
                    return Ok(tunnel);
                }
                continue;
            }
            tokio::select! {
                accepted = self.prepare_raw_accept() => {
                    let (res, stack, socket) = accepted?;
                    pending.push(Self::finish_accept(
                        res,
                        stack,
                        socket,
                        self.local_url(),
                        self.stealth.clone(),
                        self.replay_guard.clone(),
                    ));
                }
                result = pending.next(), if !pending.is_empty() => {
                    if let Some(Some(tunnel)) = result {
                        return Ok(tunnel);
                    }
                }
            }
        }
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

pub struct FakeTcpTunnelConnector {
    addr: url::Url,
    ip_to_if_name: IpToIfNameCache,
    resolved_addr: Option<SocketAddr>,
    socket_mark: Option<u32>,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_candidate: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_mode: FakeTcpStealthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FakeTcpStealthMode {
    Disabled,
    Required,
    PreferLegacyFallback,
}

impl FakeTcpTunnelConnector {
    pub fn new(addr: url::Url) -> Self {
        FakeTcpTunnelConnector {
            addr,
            ip_to_if_name: IpToIfNameCache::new(),
            resolved_addr: None,
            socket_mark: None,
            stealth: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_candidate: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_mode: FakeTcpStealthMode::Disabled,
        }
    }

    pub fn set_stealth_candidate(
        &mut self,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth_mode = if stealth.is_enabled() {
            FakeTcpStealthMode::PreferLegacyFallback
        } else {
            FakeTcpStealthMode::Disabled
        };
        self.stealth_candidate = stealth;
    }
}

fn get_local_ip_for_destination(destination: IpAddr) -> Option<IpAddr> {
    // 使用一个不可路由的、私有的、或回环地址创建一个临时的 socket，让内核自动选择源接口。
    // 对于 IPv4，使用 0.0.0.0; 对于 IPv6，使用 ::
    let bind_addr = if destination.is_ipv4() {
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0))
    };

    // 绑定到一个临时端口 (0)
    let socket = UdpSocket::bind((bind_addr, 0)).ok()?;

    // 尝试连接到目标地址。这不会真正发送数据包，只是让内核确定路由。
    socket.connect((destination, 80)).ok()?; // 使用一个常见的端口，例如 80

    // 获取 socket 的本地地址信息
    socket.local_addr().map(|addr| addr.ip()).ok()
}

#[async_trait::async_trait]
impl TunnelConnector for FakeTcpTunnelConnector {
    async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        let remote_addr = match self.resolved_addr {
            Some(addr) => addr,
            None => SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?,
        };
        let connector_addr = self.addr.clone();
        let socket_mark = self.socket_mark;
        let ip_to_if_name = &self.ip_to_if_name;
        let stealth_mode = self.stealth_mode;
        let required_stealth = self.stealth.clone();
        let preferred_stealth = self.stealth_candidate.clone();
        let connect_once = |connection_stealth: Arc<crate::tunnel::stealth::OuterSessionState>| {
            let connector_addr = connector_addr.clone();
            async move {
                let connection_stealth = connection_stealth.fork_for_connection();
                let local_ip = get_local_ip_for_destination(remote_addr.ip())
                    .ok_or(TunnelError::InternalError("Failed to get local ip".into()))?;

                let os_socket = tokio::net::TcpSocket::new_v4()?;
                // SO_MARK applies only to the kernel-visible "decoy" socket below.
                // The actual FakeTCP payload travels via crafted segments written
                // straight to the TUN device, which the kernel doesn't tag with
                // SO_MARK. Operators relying on fwmark for FakeTCP must mark the
                // TUN device's traffic with a separate nftables/iptables rule.
                crate::tunnel::common::apply_socket_mark(
                    &socket2::SockRef::from(&os_socket),
                    socket_mark,
                )?;
                os_socket.bind("0.0.0.0:0".parse().unwrap())?;
                let local_port = os_socket.local_addr()?.port();
                let local_addr = SocketAddr::new(local_ip, local_port);

                let (interface_name, mac) =
                    ip_to_if_name
                        .get_ifname(&local_ip)
                        .ok_or(TunnelError::InternalError(
                            "Failed to get interface name".into(),
                        ))?;

                let (local_ip, local_ip6) = match local_ip {
                    IpAddr::V4(ip) => (Some(ip), None),
                    IpAddr::V6(ip) => (None, Some(ip)),
                };

                let tun =
                    create_tun_off_runtime(interface_name.clone(), Some(remote_addr), local_addr)
                        .await?;
                let local_ip = local_ip.unwrap_or("0.0.0.0".parse().unwrap());
                let stack = stack::Stack::new(tun, local_ip, local_ip6, mac);
                let driver_type = stack.driver_type();

                let socket = Arc::new(
                    stack
                        .try_alloc_established_socket(
                            local_addr,
                            remote_addr,
                            stack::State::SynSent,
                        )
                        .ok_or(TunnelError::InternalError(
                            "FakeTCP stack closed while allocating socket".into(),
                        ))?,
                );

                let os_stream = os_socket.connect(remote_addr).await?;

                tracing::info!(?remote_addr, "FakeTcpTunnelConnector connecting");

                let mut buf = BytesMut::new();
                socket
                    .recv(&mut buf)
                    .await
                    .ok_or(TunnelError::InternalError(
                        "Failed to recv bytes to establish connection".into(),
                    ))?;
                let mut handshake_duplicate = None;
                if connection_stealth.is_enabled() {
                    let preface = crate::tunnel::stealth::build_stream_gate_preface(
                        connection_stealth.as_ref(),
                    );
                    let send_preface = async {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_millis(100));
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            interval.tick().await;
                            socket.try_send(&preface).ok_or_else(|| {
                                TunnelError::InternalError(
                                    "failed to send FakeTCP stealth preface".to_string(),
                                )
                            })?;
                        }
                    };
                    let wait_for_ack = async {
                        loop {
                            let mut ack = BytesMut::new();
                            socket.recv(&mut ack).await.ok_or_else(|| {
                                TunnelError::InternalError(
                                    "FakeTCP stealth listener closed".to_string(),
                                )
                            })?;
                            if crate::tunnel::stealth::verify_stream_gate_ack(
                                connection_stealth.as_ref(),
                                &preface,
                                &ack,
                            ) {
                                return Ok::<
                                    [u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN],
                                    TunnelError,
                                >(
                                    ack.as_ref().try_into().unwrap()
                                );
                            }
                        }
                    };
                    let ack = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                        tokio::select! {
                            result = send_preface => result,
                            result = wait_for_ack => result,
                        }
                    })
                    .await
                    .map_err(|_| {
                        TunnelError::InternalError(
                            "FakeTCP stealth preface exchange timed out".to_string(),
                        )
                    })??;
                    handshake_duplicate = Some((ack, None));
                }

                tracing::info!(local_addr = ?socket.local_addr(), "FakeTcpTunnelConnector connected");

                let info = TunnelInfo {
                    tunnel_type: get_faketcp_tunnel_type_str(driver_type),
                    local_addr: Some(
                        crate::tunnel::build_url_from_socket_addr(
                            &socket.local_addr().to_string(),
                            "faketcp",
                        )
                        .into(),
                    ),
                    remote_addr: Some(connector_addr.into()),
                    resolved_remote_addr: Some(
                        crate::tunnel::build_url_from_socket_addr(
                            &remote_addr.to_string(),
                            "faketcp",
                        )
                        .into(),
                    ),
                };

                let reader = FakeTcpStream::new(
                    socket.clone(),
                    connection_stealth.clone(),
                    BytesMut::new(),
                    Some(Box::new((build_os_socket_reader_task(os_stream), stack))),
                    handshake_duplicate,
                );
                let writer = FakeTcpSink::new(socket, connection_stealth.clone());
                let associate_data = connection_stealth
                    .is_enabled()
                    .then(|| Box::new(connection_stealth) as Box<dyn std::any::Any + Send>);

                Ok::<Box<dyn Tunnel>, TunnelError>(Box::new(
                    TunnelWrapper::new_with_associate_data(
                        reader,
                        writer,
                        Some(info),
                        associate_data,
                    ),
                ))
            }
        };

        match stealth_mode {
            FakeTcpStealthMode::Disabled => {
                connect_once(Arc::new(
                    crate::tunnel::stealth::OuterSessionState::disabled(),
                ))
                .await
            }
            FakeTcpStealthMode::Required => connect_once(required_stealth).await,
            FakeTcpStealthMode::PreferLegacyFallback => {
                match connect_once(preferred_stealth).await {
                    Ok(tunnel) => Ok(tunnel),
                    Err(error) => {
                        tracing::info!(
                            ?error,
                            ?remote_addr,
                            "FakeTCP stealth preface failed, retrying legacy wire format"
                        );
                        connect_once(Arc::new(
                            crate::tunnel::stealth::OuterSessionState::disabled(),
                        ))
                        .await
                    }
                }
            }
        }
    }

    fn remote_url(&self) -> url::Url {
        self.addr.clone()
    }

    fn set_resolved_addr(&mut self, addr: SocketAddr) {
        self.resolved_addr = Some(addr);
    }

    fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    fn disable_stealth(&mut self) {
        self.stealth = Arc::new(crate::tunnel::stealth::OuterSessionState::disabled());
        self.stealth_mode = FakeTcpStealthMode::Disabled;
    }

    fn require_stealth(&mut self) {
        if self.stealth_candidate.is_enabled() {
            self.stealth = self.stealth_candidate.clone();
            self.stealth_mode = FakeTcpStealthMode::Required;
        }
    }
}

type RecvFut = Pin<Box<dyn Future<Output = Option<(BytesMut, usize)>> + Send + Sync>>;

enum FakeTcpStreamState {
    ConsumingBuf(BytesMut),
    PollFuture(RecvFut),
    Closed,
}

struct FakeTcpStream {
    socket: Arc<stack::Socket>,
    state: FakeTcpStreamState,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    _transport_keepalive: Option<Box<dyn std::any::Any + Send>>,
    handshake_duplicate: Option<(
        [u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN],
        Option<[u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN]>,
    )>,
}

impl FakeTcpStream {
    fn new(
        socket: Arc<stack::Socket>,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
        initial: BytesMut,
        transport_keepalive: Option<Box<dyn std::any::Any + Send>>,
        handshake_duplicate: Option<(
            [u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN],
            Option<[u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN]>,
        )>,
    ) -> Self {
        Self {
            socket,
            state: FakeTcpStreamState::ConsumingBuf(initial),
            stealth,
            _transport_keepalive: transport_keepalive,
            handshake_duplicate,
        }
    }
}

impl Stream for FakeTcpStream {
    type Item = StreamItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let s = self.get_mut();
        loop {
            let state = std::mem::replace(&mut s.state, FakeTcpStreamState::Closed);
            match state {
                FakeTcpStreamState::ConsumingBuf(mut buf) if s.stealth.is_enabled() => {
                    if let Some((control, response)) = s.handshake_duplicate.as_ref() {
                        // Raw FakeTCP control frames can be retransmitted or reordered around data.
                        while buf.starts_with(control) {
                            let _ = buf.split_to(control.len());
                            if let Some(response) = response
                                && s.socket.try_send(response).is_none()
                            {
                                s.state = FakeTcpStreamState::Closed;
                                return Poll::Ready(None);
                            }
                        }
                    }
                    if let Some(packet) =
                        StealthFramedReader::<tokio::io::Empty>::extract_one_packet(
                            &mut buf,
                            2000,
                            s.stealth.as_ref(),
                        )
                    {
                        s.state = FakeTcpStreamState::ConsumingBuf(buf);
                        return Poll::Ready(Some(packet));
                    }

                    let socket = s.socket.clone();
                    s.state = FakeTcpStreamState::PollFuture(Box::pin(async move {
                        let ret = socket.recv(&mut buf).await;
                        ret.map(|size| (buf, size))
                    }));
                }
                FakeTcpStreamState::ConsumingBuf(buf) => {
                    let buf_len = buf.len();
                    // check peer manager header and split buf out
                    let packet = ZCPacket::new_from_buf(buf, ZCPacketType::TCP);
                    if let Some(tcp_hdr) = packet.tcp_tunnel_header() {
                        let expected_payload_len = tcp_hdr.len.get() as usize;
                        let min_packet_len = TCP_TUNNEL_HEADER_SIZE + PEER_MANAGER_HEADER_SIZE;
                        if expected_payload_len < min_packet_len {
                            tracing::warn!(
                                "drop fake tcp packet with invalid length: expected_payload_len={}, min_required={}",
                                expected_payload_len,
                                min_packet_len
                            );
                            s.state = FakeTcpStreamState::Closed;
                            return Poll::Ready(None);
                        }

                        if expected_payload_len <= buf_len {
                            let mut buf = packet.inner();
                            let new_inner = buf.split_to(expected_payload_len);
                            s.state = FakeTcpStreamState::ConsumingBuf(buf);
                            return Poll::Ready(Some(Ok(ZCPacket::new_from_buf(
                                new_inner,
                                ZCPacketType::TCP,
                            ))));
                        }
                    }

                    let mut buf = packet.inner();
                    buf.truncate(0);

                    let socket = s.socket.clone();
                    s.state = FakeTcpStreamState::PollFuture(Box::pin(async move {
                        let ret = socket.recv(&mut buf).await;
                        ret.map(|s| (buf, s))
                    }));
                }
                FakeTcpStreamState::PollFuture(mut fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(Some((buf, _sz))) => {
                        s.state = FakeTcpStreamState::ConsumingBuf(buf);
                    }
                    Poll::Ready(None) => {
                        s.state = FakeTcpStreamState::Closed;
                    }
                    Poll::Pending => {
                        s.state = FakeTcpStreamState::PollFuture(fut);
                        return Poll::Pending;
                    }
                },
                FakeTcpStreamState::Closed => {
                    return Poll::Ready(None);
                }
            }
        }
    }
}

struct FakeTcpSink {
    socket: Arc<stack::Socket>,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
}

impl FakeTcpSink {
    fn new(
        socket: Arc<stack::Socket>,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Self {
        Self { socket, stealth }
    }
}

impl Sink<SinkItem> for FakeTcpSink {
    type Error = SinkError;

    fn poll_ready(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> Result<(), Self::Error> {
        let bytes = if self.stealth.is_enabled() {
            StealthTcpZCPacketToBytes::new(self.stealth.clone()).zcpacket_into_bytes(item)?
        } else {
            let mut packet = item.convert_type(ZCPacketType::TCP);
            let len = packet.buf_len();
            packet.mut_tcp_tunnel_header().unwrap().len.set(len as u32);
            packet.into_bytes()
        };
        self.socket.try_send(&bytes);

        Ok(())
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.socket.close();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use crate::tunnel::common::tests::_tunnel_pingpong;

    use super::*;

    #[tokio::test]
    async fn faketcp_pingpong() {
        #[cfg(target_family = "unix")]
        {
            if unsafe { nix::libc::geteuid() } != 0 {
                return;
            }
        }

        let listener = FakeTcpTunnelListener::new("faketcp://0.0.0.0:31011".parse().unwrap());
        let connector = FakeTcpTunnelConnector::new("faketcp://127.0.0.1:31011".parse().unwrap());

        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn faketcp_stealth_pingpong() {
        #[cfg(target_family = "unix")]
        {
            if unsafe { nix::libc::geteuid() } != 0 {
                return;
            }
        }

        let mut listener = FakeTcpTunnelListener::new("faketcp://0.0.0.0:31012".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("faketcp-secret"),
            true,
            true,
            0,
        ));
        let mut connector =
            FakeTcpTunnelConnector::new("faketcp://127.0.0.1:31012".parse().unwrap());
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("faketcp-secret"),
            true,
            true,
            0,
        ));
        TunnelConnector::require_stealth(&mut connector);

        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn faketcp_stealth_wrong_secret_is_rejected() {
        #[cfg(target_family = "unix")]
        {
            if unsafe { nix::libc::geteuid() } != 0 {
                return;
            }
        }

        let mut listener = FakeTcpTunnelListener::new("faketcp://0.0.0.0:31013".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("listener-secret"),
            true,
            true,
            0,
        ));
        listener.listen().await.unwrap();
        let accept_task = tokio::spawn(async move { listener.accept().await });

        let mut connector =
            FakeTcpTunnelConnector::new("faketcp://127.0.0.1:31013".parse().unwrap());
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("wrong-secret"),
            true,
            true,
            0,
        ));
        TunnelConnector::require_stealth(&mut connector);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(2), connector.connect()).await;
        assert!(
            matches!(result, Ok(Err(_))),
            "wrong-secret FakeTCP connection was not rejected: {result:?}"
        );
        accept_task.abort();
    }
}
