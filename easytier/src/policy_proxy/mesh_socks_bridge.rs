use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use easytier_policy::{PolicyRevision, ProxyVia, ResolvedMeshServer};
use rand::{RngCore as _, rngs::OsRng};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::{
    gateway::socks5::{DataPlaneTcpStream, Socks5Server},
    peers::peer_manager::PeerManager,
    policy_proxy::{
        PolicyProxyCredentials, RemoteUdpAssociation, negotiate_policy_proxy_auth,
        tune_policy_udp_socket,
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ACTIVE_SESSIONS: usize = 1024;
const MAX_NEW_SESSIONS_PER_SECOND: u32 = 1000;
const MAX_SOCKS_MESSAGE: usize = 1024;
const MAX_DATAGRAM_SIZE: usize = u16::MAX as usize;

struct SessionRateLimit {
    window_started: tokio::time::Instant,
    accepted: u32,
}

impl SessionRateLimit {
    fn new() -> Self {
        Self {
            window_started: tokio::time::Instant::now(),
            accepted: 0,
        }
    }

    fn try_accept(&mut self) -> bool {
        if self.window_started.elapsed() >= Duration::from_secs(1) {
            self.window_started = tokio::time::Instant::now();
            self.accepted = 0;
        }
        if self.accepted >= MAX_NEW_SESSIONS_PER_SECOND {
            return false;
        }
        self.accepted += 1;
        true
    }
}

pub(crate) struct MeshProxyBridgeSet {
    endpoints: BTreeMap<String, ResolvedMeshServer>,
    remotes: BTreeMap<String, Arc<RemoteSlot>>,
    cancel: CancellationToken,
    listeners: Vec<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeshProxyTarget {
    pub(crate) peer_id: u32,
    endpoints: [SocketAddr; 3],
    endpoint_count: u8,
}

impl MeshProxyTarget {
    pub(crate) fn explicit(peer_id: u32, endpoint: SocketAddr) -> Self {
        Self {
            peer_id,
            endpoints: [endpoint; 3],
            endpoint_count: 1,
        }
    }

    pub(crate) fn built_in(peer_id: u32, address: IpAddr) -> Self {
        let ports = easytier_socks_egress::DEFAULT_PORT_CANDIDATES;
        Self {
            peer_id,
            endpoints: ports.map(|port| SocketAddr::new(address, port)),
            endpoint_count: ports.len() as u8,
        }
    }

    pub(crate) fn endpoints(&self) -> &[SocketAddr] {
        &self.endpoints[..usize::from(self.endpoint_count)]
    }
}

struct RemoteState {
    target: Option<MeshProxyTarget>,
    generation: CancellationToken,
}

struct RemoteSlot {
    state: Mutex<RemoteState>,
}

struct RelaySocksContext {
    data_plane: Arc<Socks5Server>,
    peer_mgr: Arc<PeerManager>,
    remote: MeshProxyTarget,
    udp_enabled: bool,
    password: String,
    credentials: Option<PolicyProxyCredentials>,
    generation: CancellationToken,
}

impl RemoteSlot {
    fn new(target: MeshProxyTarget) -> Self {
        Self {
            state: Mutex::new(RemoteState {
                target: Some(target),
                generation: CancellationToken::new(),
            }),
        }
    }

    fn snapshot(&self) -> Option<(MeshProxyTarget, CancellationToken)> {
        let state = self.state.lock().unwrap();
        state
            .target
            .map(|target| (target, state.generation.child_token()))
    }

    fn replace(&self, target: Option<MeshProxyTarget>) {
        let mut state = self.state.lock().unwrap();
        if state.target == target {
            return;
        }
        state.generation.cancel();
        state.target = target;
        state.generation = CancellationToken::new();
    }
}

impl MeshProxyBridgeSet {
    pub(crate) async fn start(
        data_plane: Arc<Socks5Server>,
        peer_mgr: Arc<PeerManager>,
        revision: &PolicyRevision,
        resolved: &BTreeMap<String, MeshProxyTarget>,
    ) -> anyhow::Result<Self> {
        let cancel = CancellationToken::new();
        let permits = Arc::new(Semaphore::new(MAX_ACTIVE_SESSIONS));
        let rate_limit = Arc::new(Mutex::new(SessionRateLimit::new()));
        let mut endpoints = BTreeMap::new();
        let mut remotes = BTreeMap::new();
        let mut pending = Vec::new();

        for (name, proxy) in &revision.document.proxies {
            if proxy.via != ProxyVia::Mesh {
                continue;
            }
            let remote = *resolved
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("mesh proxy {name} has no resolved endpoint"))?;
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            let local = listener.local_addr()?;
            let mut secret = [0u8; 32];
            OsRng.fill_bytes(&mut secret);
            let password = secret
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            endpoints.insert(
                name.clone(),
                ResolvedMeshServer {
                    endpoint: local,
                    username: "easytier".to_owned(),
                    password: password.clone(),
                },
            );
            let remote = Arc::new(RemoteSlot::new(remote));
            remotes.insert(name.clone(), remote.clone());
            pending.push((
                name.clone(),
                proxy.udp,
                password,
                PolicyProxyCredentials::from_proxy(proxy),
                remote,
                listener,
            ));
        }

        let mut listeners = Vec::with_capacity(pending.len());
        for (name, udp_enabled, password, credentials, remote, listener) in pending {
            let data_plane = data_plane.clone();
            let peer_mgr = peer_mgr.clone();
            let listener_cancel = cancel.child_token();
            let permits = permits.clone();
            let rate_limit = rate_limit.clone();
            listeners.push(tokio::spawn(async move {
                run_listener(
                    listener,
                    data_plane,
                    peer_mgr,
                    remote,
                    udp_enabled,
                    password,
                    credentials,
                    permits,
                    rate_limit,
                    listener_cancel,
                    name,
                )
                .await;
            }));
        }

        Ok(Self {
            endpoints,
            remotes,
            cancel,
            listeners,
        })
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<ResolvedMeshServer> {
        self.endpoints.get(name).cloned()
    }

    pub(crate) fn update_remote(&self, name: &str, remote: MeshProxyTarget) -> anyhow::Result<()> {
        let current = self
            .remotes
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown mesh proxy bridge {name}"))?;
        current.replace(Some(remote));
        Ok(())
    }

    pub(crate) fn disable_all(&self) {
        for remote in self.remotes.values() {
            remote.replace(None);
        }
    }
}

impl easytier_policy::MeshServerResolver for MeshProxyBridgeSet {
    fn resolve(
        &self,
        proxy_name: &str,
        _instance_id: Option<uuid::Uuid>,
        _virtual_ip: Option<IpAddr>,
        _port: Option<u16>,
    ) -> Option<ResolvedMeshServer> {
        self.resolve(proxy_name)
    }
}

impl Drop for MeshProxyBridgeSet {
    fn drop(&mut self) {
        self.cancel.cancel();
        for listener in &self.listeners {
            listener.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_listener(
    listener: TcpListener,
    data_plane: Arc<Socks5Server>,
    peer_mgr: Arc<PeerManager>,
    remote: Arc<RemoteSlot>,
    udp_enabled: bool,
    password: String,
    credentials: Option<PolicyProxyCredentials>,
    permits: Arc<Semaphore>,
    rate_limit: Arc<Mutex<SessionRateLimit>>,
    cancel: CancellationToken,
    name: String,
) {
    let mut sessions = JoinSet::new();
    let mut rejected_for_limit = 0u64;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            Some(result) = sessions.join_next(), if !sessions.is_empty() => {
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    tracing::warn!(proxy = %name, ?error, "mesh proxy session task failed");
                }
            }
            result = listener.accept() => {
                let Ok((client, client_addr)) = result else {
                    tracing::warn!(proxy = %name, "mesh proxy loopback listener stopped");
                    break;
                };
                if !rate_limit.lock().unwrap().try_accept() {
                    rejected_for_limit = rejected_for_limit.saturating_add(1);
                    if rejected_for_limit.is_power_of_two() {
                        tracing::warn!(proxy = %name, rejected_for_limit, "mesh proxy connection-rate limit reached");
                    }
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    rejected_for_limit = rejected_for_limit.saturating_add(1);
                    if rejected_for_limit.is_power_of_two() {
                        tracing::warn!(proxy = %name, rejected_for_limit, "mesh proxy session limit reached");
                    }
                    continue;
                };
                if let Err(error) = client.set_nodelay(true) {
                    tracing::debug!(proxy = %name, ?error, "failed to set loopback TCP_NODELAY");
                }
                let data_plane = data_plane.clone();
                let peer_mgr = peer_mgr.clone();
                let session_name = name.clone();
                let password = password.clone();
                let credentials = credentials.clone();
                let Some((remote, generation)) = remote.snapshot() else {
                    rejected_for_limit = rejected_for_limit.saturating_add(1);
                    if rejected_for_limit.is_power_of_two() {
                        tracing::warn!(proxy = %name, rejected_for_limit, "mesh proxy endpoint is unavailable");
                    }
                    continue;
                };
                sessions.spawn(async move {
                    let _permit = permit;
                    let result = relay_socks5(
                        client,
                        client_addr,
                        RelaySocksContext {
                            data_plane,
                            peer_mgr,
                            remote,
                            udp_enabled,
                            password,
                            credentials,
                            generation,
                        },
                    )
                    .await;
                    if let Err(error) = result {
                        tracing::debug!(proxy = %session_name, peer_id = remote.peer_id, endpoints = ?remote.endpoints(), ?error, "mesh proxy session ended");
                    }
                });
            }
        }
    }
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
}

async fn connect_remote(
    data_plane: &Socks5Server,
    remote: MeshProxyTarget,
) -> anyhow::Result<DataPlaneTcpStream> {
    let mut failures = Vec::new();
    for endpoint in remote.endpoints() {
        match data_plane
            .data_plane_tcp_connect_mesh_only(*endpoint, CONNECT_TIMEOUT)
            .await
        {
            Ok(stream) => return Ok(stream),
            Err(error) => failures.push(format!("{endpoint}: {error}")),
        }
    }
    anyhow::bail!(
        "mesh SOCKS endpoint candidates failed: {}",
        failures.join("; ")
    )
}

async fn relay_socks5(
    mut client: TcpStream,
    client_addr: SocketAddr,
    context: RelaySocksContext,
) -> anyhow::Result<()> {
    let RelaySocksContext {
        data_plane,
        peer_mgr,
        remote,
        udp_enabled,
        password,
        credentials,
        generation,
    } = context;
    let command = tokio::select! {
        _ = generation.cancelled() => anyhow::bail!("mesh proxy route generation changed"),
        result = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            authenticate_local(&mut client, &password).await?;
            read_and_validate_socks_command(&mut client, udp_enabled).await
        }) => result??,
    };
    let (request, command) = command;

    match command {
        1 => {
            let mut upstream = connect_remote(&data_plane, remote).await?;
            negotiate_policy_proxy_auth(&mut upstream, credentials.as_ref()).await?;
            relay_socks_connect_command(&mut client, &mut upstream, request).await?;
            tokio::select! {
                _ = generation.cancelled() => {}
                result = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {
                    result?;
                }
            }
        }
        3 => {
            let mut association = None;
            let mut failures = Vec::new();
            for endpoint in remote.endpoints() {
                match RemoteUdpAssociation::open(
                    &peer_mgr,
                    &data_plane,
                    remote.peer_id,
                    *endpoint,
                    credentials.as_ref(),
                )
                .await
                {
                    Ok(opened) => {
                        association = Some(opened);
                        break;
                    }
                    Err(error) => failures.push(format!("{endpoint}: {error:#}")),
                }
            }
            let association = association.ok_or_else(|| {
                anyhow::anyhow!(
                    "mesh SOCKS UDP endpoint candidates failed: {}",
                    failures.join("; ")
                )
            })?;
            relay_socks_udp(client, client_addr.ip(), association, generation).await?;
        }
        _ => unreachable!("SOCKS command was validated before dispatch"),
    }
    Ok(())
}

enum SocksAddress {
    Ip(SocketAddr),
    Domain(u16),
}

async fn authenticate_local<S>(client: &mut S, password: &str) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let greeting = read_socks_greeting(client).await?;
    if !greeting[2..].contains(&2) {
        client.write_all(&[5, 0xff]).await?;
        anyhow::bail!("local SOCKS client did not offer username/password authentication");
    }
    client.write_all(&[5, 2]).await?;
    let auth = read_user_password_auth(client).await?;
    let username_length = usize::from(auth[1]);
    let username = &auth[2..2 + username_length];
    let password_length_index = 2 + username_length;
    let supplied_password = &auth[password_length_index + 1..];
    let authenticated = constant_time_eq(username, b"easytier")
        && constant_time_eq(supplied_password, password.as_bytes());
    client
        .write_all(&[1, if authenticated { 0 } else { 1 }])
        .await?;
    if !authenticated {
        anyhow::bail!("local mesh bridge authentication failed");
    }
    Ok(())
}

async fn relay_socks_connect_command(
    client: &mut TcpStream,
    upstream: &mut DataPlaneTcpStream,
    request: Vec<u8>,
) -> anyhow::Result<()> {
    upstream.write_all(&request).await?;
    let (reply, code, _) = read_socks_reply(upstream).await?;
    client.write_all(&reply).await?;
    if code != 0 {
        anyhow::bail!("upstream SOCKS command failed with reply {code}");
    }
    Ok(())
}

async fn read_and_validate_socks_command(
    client: &mut TcpStream,
    udp_enabled: bool,
) -> anyhow::Result<(Vec<u8>, u8)> {
    let (request, command, _) = read_socks_request(client).await?;
    if !matches!(command, 1 | 3) {
        client
            .write_all(&socks_reply(
                7,
                SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
            ))
            .await?;
        anyhow::bail!("unsupported SOCKS command {command}");
    }
    if command == 3 && !udp_enabled {
        client
            .write_all(&socks_reply(
                7,
                SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
            ))
            .await?;
        anyhow::bail!("UDP ASSOCIATE is disabled for this actor");
    }
    Ok((request, command))
}

async fn relay_socks_udp(
    mut control: TcpStream,
    client_ip: IpAddr,
    association: RemoteUdpAssociation,
    generation: CancellationToken,
) -> anyhow::Result<()> {
    let local = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?);
    tune_policy_udp_socket(&local);
    let local_addr = local.local_addr()?;
    control.write_all(&socks_reply(0, local_addr)).await?;
    control.flush().await?;

    let (client_endpoint_tx, mut client_endpoint_rx) = tokio::sync::watch::channel(None);
    let local_to_remote = {
        let local = local.clone();
        let association = &association;
        async move {
            let mut client_endpoint = None;
            let mut packet = vec![0u8; MAX_DATAGRAM_SIZE];
            loop {
                let (length, source) = local.recv_from(&mut packet).await?;
                if source.ip() != client_ip || !source.ip().is_loopback() {
                    tracing::warn!(%source, "dropping SOCKS UDP datagram from unexpected source");
                    continue;
                }
                if let Some(expected) = client_endpoint {
                    if expected != source {
                        continue;
                    }
                } else {
                    client_endpoint = Some(source);
                    let _ = client_endpoint_tx.send(client_endpoint);
                }
                association.send(&packet[..length]).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        }
    };
    let remote_to_local = {
        let local = local.clone();
        let association = &association;
        async move {
            let mut packet = vec![0u8; MAX_DATAGRAM_SIZE];
            loop {
                let length = association.recv(&mut packet).await?;
                let client_endpoint = loop {
                    if let Some(client_endpoint) = *client_endpoint_rx.borrow() {
                        break client_endpoint;
                    }
                    client_endpoint_rx
                        .changed()
                        .await
                        .map_err(|_| anyhow::anyhow!("SOCKS UDP client endpoint was dropped"))?;
                };
                local.send_to(&packet[..length], client_endpoint).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        }
    };
    let mut client_control_byte = [0u8; 1];
    let result = tokio::select! {
        _ = generation.cancelled() => Ok(()),
        result = control.read(&mut client_control_byte) => match result {
            Ok(0) => Ok(()),
            Ok(_) => Err(anyhow::anyhow!("unexpected data on SOCKS UDP control stream")),
            Err(error) => Err(error.into()),
        },
        result = local_to_remote => result,
        result = remote_to_local => result,
    };
    association.close().await;
    result
}

async fn read_socks_greeting<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
) -> anyhow::Result<Vec<u8>> {
    let mut greeting = read_exact_vec(stream, 2).await?;
    if greeting[0] != 5 {
        anyhow::bail!("invalid SOCKS version");
    }
    if greeting[1] == 0 {
        anyhow::bail!("SOCKS greeting has no authentication method");
    }
    let methods = read_exact_vec(stream, usize::from(greeting[1])).await?;
    greeting.extend_from_slice(&methods);
    Ok(greeting)
}

async fn read_user_password_auth<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
) -> anyhow::Result<Vec<u8>> {
    let mut auth = read_exact_vec(stream, 2).await?;
    if auth[0] != 1 {
        anyhow::bail!("invalid SOCKS username/password version");
    }
    let username = read_exact_vec(stream, usize::from(auth[1])).await?;
    auth.extend_from_slice(&username);
    let password_len = read_exact_vec(stream, 1).await?;
    let password = read_exact_vec(stream, usize::from(password_len[0])).await?;
    auth.extend_from_slice(&password_len);
    auth.extend_from_slice(&password);
    if auth.len() > MAX_SOCKS_MESSAGE {
        anyhow::bail!("SOCKS authentication message is too large");
    }
    Ok(auth)
}

async fn read_socks_request(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> anyhow::Result<(Vec<u8>, u8, SocksAddress)> {
    let header = read_exact_vec(stream, 4).await?;
    if header[0] != 5 || header[2] != 0 {
        anyhow::bail!("invalid SOCKS request header");
    }
    let mut message = header.clone();
    let address = read_socks_address(stream, header[3], &mut message).await?;
    Ok((message, header[1], address))
}

async fn read_socks_reply(
    stream: &mut DataPlaneTcpStream,
) -> anyhow::Result<(Vec<u8>, u8, SocksAddress)> {
    let header = read_exact_vec(stream, 4).await?;
    if header[0] != 5 || header[2] != 0 {
        anyhow::bail!("invalid upstream SOCKS reply header");
    }
    let mut message = header.clone();
    let address = read_socks_address(stream, header[3], &mut message).await?;
    Ok((message, header[1], address))
}

async fn read_socks_address<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
    address_type: u8,
    message: &mut Vec<u8>,
) -> anyhow::Result<SocksAddress> {
    let address = match address_type {
        1 => {
            let bytes = read_exact_vec(stream, 6).await?;
            let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
            let port = u16::from_be_bytes([bytes[4], bytes[5]]);
            message.extend_from_slice(&bytes);
            SocksAddress::Ip(SocketAddr::new(ip.into(), port))
        }
        4 => {
            let bytes = read_exact_vec(stream, 18).await?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&bytes[..16]);
            let port = u16::from_be_bytes([bytes[16], bytes[17]]);
            message.extend_from_slice(&bytes);
            SocksAddress::Ip(SocketAddr::new(octets.into(), port))
        }
        3 => {
            let length = read_exact_vec(stream, 1).await?;
            if length[0] == 0 {
                anyhow::bail!("SOCKS domain is empty");
            }
            let bytes = read_exact_vec(stream, usize::from(length[0]) + 2).await?;
            std::str::from_utf8(&bytes[..bytes.len() - 2])?;
            let port = u16::from_be_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
            message.extend_from_slice(&length);
            message.extend_from_slice(&bytes);
            SocksAddress::Domain(port)
        }
        _ => anyhow::bail!("unsupported SOCKS address type {address_type}"),
    };
    if message.len() > MAX_SOCKS_MESSAGE {
        anyhow::bail!("SOCKS message is too large");
    }
    Ok(address)
}

async fn read_exact_vec<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
    length: usize,
) -> anyhow::Result<Vec<u8>> {
    if length > MAX_SOCKS_MESSAGE {
        anyhow::bail!("SOCKS field is too large");
    }
    let mut bytes = vec![0u8; length];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

fn socks_reply(code: u8, address: SocketAddr) -> Vec<u8> {
    let mut reply = vec![5, code, 0];
    match address {
        SocketAddr::V4(address) => {
            reply.push(1);
            reply.extend_from_slice(&address.ip().octets());
            reply.extend_from_slice(&address.port().to_be_bytes());
        }
        SocketAddr::V6(address) => {
            reply.push(4);
            reply.extend_from_slice(&address.ip().octets());
            reply.extend_from_slice(&address.port().to_be_bytes());
        }
    }
    reply
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_fragmented_socks_request_without_overread() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            writer.write_all(&[5, 1, 0, 3, 11]).await.unwrap();
            writer.write_all(b"example.com").await.unwrap();
            writer.write_all(&443u16.to_be_bytes()).await.unwrap();
            writer.write_all(b"payload").await.unwrap();
        });
        let (request, command, address) = read_socks_request(&mut reader).await.unwrap();
        assert_eq!(command, 1);
        assert_eq!(request.len(), 4 + 1 + 11 + 2);
        assert!(matches!(address, SocksAddress::Domain(443)));
        let mut payload = [0u8; 7];
        reader.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"payload");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn local_bridge_requires_ephemeral_credentials() {
        let (mut client, mut server) = tokio::io::duplex(128);
        let auth = tokio::spawn(async move { authenticate_local(&mut server, "secret").await });
        client.write_all(&[5, 2, 0, 2]).await.unwrap();
        let mut method = [0u8; 2];
        client.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 2]);
        client
            .write_all(&[1, 8, b'e', b'a', b's', b'y', b't', b'i', b'e', b'r', 6])
            .await
            .unwrap();
        client.write_all(b"secret").await.unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [1, 0]);
        auth.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn local_bridge_rejects_invalid_credentials_before_mesh_dial() {
        let (mut client, mut server) = tokio::io::duplex(128);
        let auth = tokio::spawn(async move { authenticate_local(&mut server, "secret").await });
        client.write_all(&[5, 1, 2]).await.unwrap();
        let mut method = [0u8; 2];
        client.read_exact(&mut method).await.unwrap();
        client
            .write_all(&[1, 8, b'e', b'a', b's', b'y', b't', b'i', b'e', b'r', 5])
            .await
            .unwrap();
        client.write_all(b"wrong").await.unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [1, 1]);
        assert!(auth.await.unwrap().is_err());
    }

    #[test]
    fn encodes_ipv4_and_ipv6_socks_replies() {
        assert_eq!(
            socks_reply(0, "127.0.0.1:1080".parse().unwrap()),
            [5, 0, 0, 1, 127, 0, 0, 1, 4, 56]
        );
        let ipv6 = socks_reply(0, "[::1]:1080".parse().unwrap());
        assert_eq!(&ipv6[..4], &[5, 0, 0, 4]);
        assert_eq!(&ipv6[20..], &[4, 56]);
    }

    #[test]
    fn session_rate_limit_is_shared_and_bounded() {
        let mut limit = SessionRateLimit::new();
        for _ in 0..MAX_NEW_SESSIONS_PER_SECOND {
            assert!(limit.try_accept());
        }
        assert!(!limit.try_accept());
    }

    #[tokio::test]
    async fn route_identity_change_cancels_only_the_old_generation() {
        let first = MeshProxyTarget::explicit(7, "10.44.0.7:1080".parse().unwrap());
        let slot = RemoteSlot::new(first);
        let (_, old_generation) = slot.snapshot().unwrap();
        slot.replace(Some(first));
        assert!(!old_generation.is_cancelled());

        let second = MeshProxyTarget::explicit(8, first.endpoints()[0]);
        slot.replace(Some(second));
        assert!(old_generation.is_cancelled());
        let (current, new_generation) = slot.snapshot().unwrap();
        assert_eq!(current, second);
        assert!(!new_generation.is_cancelled());

        slot.replace(None);
        assert!(new_generation.is_cancelled());
        assert!(slot.snapshot().is_none());
    }
}
