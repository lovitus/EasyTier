use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, bail};
use dashmap::DashMap;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _, BufReader, ReadHalf, WriteHalf},
    net::{TcpListener, TcpSocket, TcpStream, UdpSocket},
};
use tokio_util::sync::CancellationToken;

use crate::{
    common::PeerId,
    gateway::socks5::{DataPlaneTcpListener, DataPlaneTcpStream, DataPlaneUdpSocket, Socks5Server},
    peers::peer_manager::PeerManager,
    policy_proxy::{PolicyProxyCredentials, negotiate_policy_proxy_auth, tune_policy_udp_socket},
    proto::{
        peer_rpc::{
            ClosePolicyUdpRelayRequest, ClosePolicyUdpRelayResponse, OpenPolicyUdpRelayRequest,
            OpenPolicyUdpRelayResponse, PolicyUdpRelayRpc, PolicyUdpRelayRpcClientFactory,
            PolicyUdpRelayRpcServer,
        },
        rpc_impl::RpcController,
        rpc_types::{
            self,
            controller::{BaseController, Controller as _},
        },
    },
};

const ASSOCIATION_LIMIT: usize = 1_024;
const ASSOCIATION_LIMIT_PER_PEER: usize = 256;
const ASSOCIATION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_VERSION: u8 = 1;
const TOKEN_LEN: usize = 16;
const FRAME_HEADER_LEN: usize = 1 + TOKEN_LEN;
const MAX_DATAGRAM_SIZE: usize = u16::MAX as usize;
const UOT_VERSION: u32 = 2;
const UOT_READY: u8 = 0;
const UOT_CONNECT: u8 = 1;
const UOT_IPV4: u8 = 1;
const UOT_IPV6: u8 = 4;
const UOT_STREAM_BUFFER_SIZE: usize = 16 * 1_024;

pub(crate) type AssociationToken = [u8; TOKEN_LEN];

struct AssociationReservation {
    associations: Arc<DashMap<AssociationToken, AssociationOwner>>,
    token: AssociationToken,
    armed: bool,
}

#[derive(Clone)]
struct AssociationOwner {
    source_peer_id: PeerId,
    cancel: CancellationToken,
}

struct AssociationContext {
    token: AssociationToken,
    source_ip: Ipv4Addr,
    control: TcpStream,
    native_udp: UdpSocket,
    upstream_relay: SocketAddr,
    cancel: CancellationToken,
}

impl AssociationReservation {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AssociationReservation {
    fn drop(&mut self) {
        if self.armed {
            self.associations.remove(&self.token);
        }
    }
}

pub(crate) struct MeshUdpRelayService {
    peer_mgr: Weak<PeerManager>,
    data_plane: Arc<Socks5Server>,
    associations: Arc<DashMap<AssociationToken, AssociationOwner>>,
    capacity_lock: tokio::sync::Mutex<()>,
}

impl MeshUdpRelayService {
    pub(crate) fn new(peer_mgr: &Arc<PeerManager>, data_plane: Arc<Socks5Server>) -> Arc<Self> {
        Arc::new(Self {
            peer_mgr: Arc::downgrade(peer_mgr),
            data_plane,
            associations: Arc::new(DashMap::new()),
            capacity_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub(crate) fn register(self: &Arc<Self>) {
        let Some(peer_mgr) = self.peer_mgr.upgrade() else {
            return;
        };
        peer_mgr
            .get_peer_rpc_mgr()
            .rpc_server()
            .registry()
            .register(
                PolicyUdpRelayRpcServer::new_arc(self.clone()),
                &peer_mgr.get_global_ctx().get_network_name(),
            );
    }

    async fn open_association(
        &self,
        source_peer_id: PeerId,
        request: OpenPolicyUdpRelayRequest,
    ) -> anyhow::Result<OpenPolicyUdpRelayResponse> {
        let peer_mgr = self
            .peer_mgr
            .upgrade()
            .context("peer manager is no longer available")?;
        if request.source_peer_id != source_peer_id {
            bail!("policy UDP relay source peer does not match the RPC caller");
        }
        let source_ip = peer_virtual_ipv4(&peer_mgr, source_peer_id)
            .await
            .context("requesting peer has no routed virtual IPv4 address")?;
        let proxy_addr: SocketAddr = request
            .proxy_addr
            .context("policy UDP relay request has no proxy address")?
            .into();
        let local_virtual_ip = peer_mgr
            .get_global_ctx()
            .get_ipv4()
            .context("destination peer has no virtual IPv4 address")?
            .address();
        if proxy_addr.ip() != IpAddr::V4(local_virtual_ip) || proxy_addr.port() == 0 {
            bail!("policy UDP relay only permits a SOCKS server on this peer's exact virtual IPv4");
        }

        let credentials = PolicyProxyCredentials::from_wire(
            request.proxy_username.clone(),
            request.proxy_password.clone(),
        )?;
        let cancel = CancellationToken::new();
        let mut reservation = self.reserve_token(source_peer_id, &cancel).await?;
        let token = reservation.token;
        let (control, native_udp, upstream_relay) =
            open_local_socks_udp(proxy_addr, credentials.as_ref()).await?;
        if request.stream_version >= UOT_VERSION {
            let kernel_listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
                .await
                .context("failed to bind private policy UoT listener")?;
            let stream_port = kernel_listener.local_addr()?.port();
            // The same logical endpoint exists in both independent TCP stacks. KCP proxy
            // responders connect through the kernel listener; capability fallback uses the
            // smoltcp listener without changing Socks5AutoConnector selection semantics.
            let data_plane_listener = self
                .data_plane
                .data_plane_tcp_bind(stream_port, SETUP_TIMEOUT)
                .await
                .context("failed to bind private policy UoT data-plane listener")?;
            let stream_addr = SocketAddr::new(IpAddr::V4(local_virtual_ip), stream_port);
            let associations = self.associations.clone();
            tokio::spawn(async move {
                run_stream_association(
                    AssociationContext {
                        token,
                        source_ip,
                        control,
                        native_udp,
                        upstream_relay,
                        cancel,
                    },
                    kernel_listener,
                    data_plane_listener,
                )
                .await;
                associations.remove(&token);
            });
            reservation.disarm();
            return Ok(OpenPolicyUdpRelayResponse {
                token: token.to_vec(),
                relay_addr: None,
                stream_addr: Some(stream_addr.into()),
                stream_destination: Some(upstream_relay.into()),
            });
        }

        let origin_addr: SocketAddr = request
            .origin_addr
            .context("legacy policy UDP relay request has no origin endpoint")?
            .into();
        if origin_addr.ip() != IpAddr::V4(source_ip) || origin_addr.port() == 0 {
            bail!("policy UDP relay origin endpoint does not match the requesting peer");
        }
        let setup = async {
            let mesh_udp = self
                .data_plane
                .data_plane_udp_bind(0, SETUP_TIMEOUT)
                .await
                .context("failed to bind mesh UDP association socket")?;
            Ok::<_, anyhow::Error>(mesh_udp)
        }
        .await;
        let mesh_udp = match setup {
            Ok(setup) => setup,
            Err(error) => return Err(error),
        };
        let relay_addr = mesh_udp.local_addr();
        // Register the destination-side connected UDP route before returning the RPC. The
        // one-byte payload distinguishes this disposable warmup from the empty challenge ACK.
        let warmup =
            encode_relay_frame(&token, &[0]).expect("the policy relay warmup frame always fits");
        if let Err(error) = mesh_udp.send_to(&warmup, origin_addr).await {
            return Err(error).context("failed to prime policy UDP relay route");
        }
        let associations = self.associations.clone();
        tokio::spawn(async move {
            run_association(
                AssociationContext {
                    token,
                    source_ip,
                    control,
                    native_udp,
                    upstream_relay,
                    cancel,
                },
                origin_addr,
                mesh_udp,
            )
            .await;
            associations.remove(&token);
        });
        reservation.disarm();

        Ok(OpenPolicyUdpRelayResponse {
            token: token.to_vec(),
            relay_addr: Some(relay_addr.into()),
            stream_addr: None,
            stream_destination: None,
        })
    }

    async fn reserve_token(
        &self,
        source_peer_id: PeerId,
        cancel: &CancellationToken,
    ) -> anyhow::Result<AssociationReservation> {
        let _capacity = self.capacity_lock.lock().await;
        if self.associations.len() >= ASSOCIATION_LIMIT {
            bail!("policy UDP relay association table is full");
        }
        if self
            .associations
            .iter()
            .filter(|association| association.value().source_peer_id == source_peer_id)
            .count()
            >= ASSOCIATION_LIMIT_PER_PEER
        {
            bail!("policy UDP relay association limit reached for requesting peer");
        }
        for _ in 0..8 {
            let token = rand::random::<AssociationToken>();
            if let dashmap::mapref::entry::Entry::Vacant(entry) = self.associations.entry(token) {
                entry.insert(AssociationOwner {
                    source_peer_id,
                    cancel: cancel.clone(),
                });
                return Ok(AssociationReservation {
                    associations: self.associations.clone(),
                    token,
                    armed: true,
                });
            }
        }
        bail!("failed to allocate a unique policy UDP relay token")
    }
}

impl Drop for MeshUdpRelayService {
    fn drop(&mut self) {
        for association in self.associations.iter() {
            association.value().cancel.cancel();
        }
    }
}

#[async_trait::async_trait]
impl PolicyUdpRelayRpc for MeshUdpRelayService {
    type Controller = BaseController;

    async fn open_policy_udp_relay(
        &self,
        controller: BaseController,
        request: OpenPolicyUdpRelayRequest,
    ) -> Result<OpenPolicyUdpRelayResponse, rpc_types::error::Error> {
        let source_peer_id = controller.source_peer_id().ok_or_else(|| {
            rpc_types::error::Error::ExecutionError(anyhow::anyhow!(
                "policy UDP relay RPC has no source peer"
            ))
        })?;
        self.open_association(source_peer_id, request)
            .await
            .map_err(rpc_types::error::Error::ExecutionError)
    }

    async fn close_policy_udp_relay(
        &self,
        controller: BaseController,
        request: ClosePolicyUdpRelayRequest,
    ) -> Result<ClosePolicyUdpRelayResponse, rpc_types::error::Error> {
        let source_peer_id = controller.source_peer_id().ok_or_else(|| {
            rpc_types::error::Error::ExecutionError(anyhow::anyhow!(
                "policy UDP relay RPC has no source peer"
            ))
        })?;
        if let Ok(token) = AssociationToken::try_from(request.token.as_slice())
            && let Some(owner) = self.associations.get(&token)
            && owner.source_peer_id == source_peer_id
        {
            owner.cancel.cancel();
        }
        Ok(ClosePolicyUdpRelayResponse {})
    }
}

pub(crate) struct RemoteUdpAssociation {
    pub(crate) token: AssociationToken,
    transport: RemoteUdpTransport,
    dst_peer_id: PeerId,
    peer_mgr: Weak<PeerManager>,
    closed: AtomicBool,
}

enum RemoteUdpTransport {
    Datagram {
        socket: DataPlaneUdpSocket,
        relay_addr: SocketAddr,
        receive_buffer: tokio::sync::Mutex<Vec<u8>>,
    },
    Stream {
        reader: tokio::sync::Mutex<BufReader<ReadHalf<DataPlaneTcpStream>>>,
        writer: tokio::sync::Mutex<UotStreamWriter>,
    },
}

struct UotStreamWriter {
    stream: WriteHalf<DataPlaneTcpStream>,
    frame: Vec<u8>,
}

impl RemoteUdpAssociation {
    pub(crate) async fn open(
        peer_mgr: &Arc<PeerManager>,
        data_plane: &Socks5Server,
        dst_peer_id: PeerId,
        proxy_addr: SocketAddr,
        credentials: Option<&PolicyProxyCredentials>,
    ) -> anyhow::Result<Self> {
        let stream_attempt = async {
            let response = request_remote_association(
                peer_mgr,
                dst_peer_id,
                proxy_addr,
                None,
                UOT_VERSION,
                credentials,
            )
            .await?;
            let token = AssociationToken::try_from(response.token.as_slice())
                .map_err(|_| anyhow::anyhow!("policy UoT relay returned an invalid token"))?;
            let result = async {
                let stream_addr: SocketAddr = response
                    .stream_addr
                    .context("destination does not support policy UoT v2")?
                    .into();
                let destination: SocketAddr = response
                    .stream_destination
                    .context("policy UoT relay returned no destination")?
                    .into();
                let mut stream = data_plane
                    .data_plane_tcp_connect_mesh_uot(stream_addr, SETUP_TIMEOUT)
                    .await
                    .context("failed to connect private policy UoT stream")?;
                stream.write_all(&token).await?;
                stream
                    .write_all(&encode_uot_connect_request(destination))
                    .await?;
                stream.flush().await?;
                let ready = tokio::time::timeout(SETUP_TIMEOUT, stream.read_u8())
                    .await
                    .context("policy UoT readiness timed out")??;
                if ready != UOT_READY {
                    bail!("policy UoT relay returned an invalid readiness byte");
                }
                let (reader, writer) = tokio::io::split(stream);
                Ok::<_, anyhow::Error>((
                    token,
                    RemoteUdpTransport::Stream {
                        reader: tokio::sync::Mutex::new(BufReader::with_capacity(
                            UOT_STREAM_BUFFER_SIZE,
                            reader,
                        )),
                        writer: tokio::sync::Mutex::new(UotStreamWriter {
                            stream: writer,
                            frame: Vec::with_capacity(UOT_STREAM_BUFFER_SIZE),
                        }),
                    },
                ))
            }
            .await;
            if result.is_err() {
                close_remote_association(Arc::downgrade(peer_mgr), dst_peer_id, token).await;
            }
            result
        }
        .await;

        let (token, transport) = match stream_attempt {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!(%dst_peer_id, %error, "policy UoT unavailable; falling back to legacy datagram relay");
                open_legacy_remote_association(
                    peer_mgr,
                    data_plane,
                    dst_peer_id,
                    proxy_addr,
                    credentials,
                )
                .await?
            }
        };
        let association = Self {
            token,
            transport,
            dst_peer_id,
            peer_mgr: Arc::downgrade(peer_mgr),
            closed: AtomicBool::new(false),
        };
        Ok(association)
    }

    pub(crate) async fn send(&self, payload: &[u8]) -> anyhow::Result<()> {
        if payload.len() > MAX_DATAGRAM_SIZE {
            bail!("policy UDP payload exceeds UoT frame limit");
        }
        match &self.transport {
            RemoteUdpTransport::Datagram {
                socket, relay_addr, ..
            } => {
                let Some(frame) = encode_relay_frame(&self.token, payload) else {
                    bail!("policy UDP payload exceeds datagram relay limit");
                };
                socket.send_to(&frame, *relay_addr).await?;
            }
            RemoteUdpTransport::Stream { writer, .. } => {
                let mut writer = writer.lock().await;
                let UotStreamWriter { stream, frame } = &mut *writer;
                frame.clear();
                frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                frame.extend_from_slice(payload);
                stream.write_all(frame).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn recv(&self, payload: &mut [u8]) -> anyhow::Result<usize> {
        match &self.transport {
            RemoteUdpTransport::Datagram {
                socket,
                relay_addr,
                receive_buffer,
            } => loop {
                let mut frame = receive_buffer.lock().await;
                let (length, source) = socket.recv_from(&mut frame).await?;
                if source != *relay_addr {
                    continue;
                }
                let Some(decoded) = decode_relay_frame(&self.token, &frame[..length]) else {
                    continue;
                };
                if decoded.len() > payload.len() {
                    bail!("policy UDP receive buffer is too small");
                }
                payload[..decoded.len()].copy_from_slice(decoded);
                return Ok(decoded.len());
            },
            RemoteUdpTransport::Stream { reader, .. } => {
                let mut reader = reader.lock().await;
                let length = reader.read_u16().await? as usize;
                if length > payload.len() {
                    bail!("policy UoT receive buffer is too small");
                }
                reader.read_exact(&mut payload[..length]).await?;
                Ok(length)
            }
        }
    }

    pub(crate) async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        close_remote_association(self.peer_mgr.clone(), self.dst_peer_id, self.token).await;
    }
}

impl Drop for RemoteUdpAssociation {
    fn drop(&mut self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let peer_mgr = self.peer_mgr.clone();
        let dst_peer_id = self.dst_peer_id;
        let token = self.token;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(close_remote_association(peer_mgr, dst_peer_id, token));
        }
    }
}

async fn request_remote_association(
    peer_mgr: &Arc<PeerManager>,
    dst_peer_id: PeerId,
    proxy_addr: SocketAddr,
    origin_addr: Option<SocketAddr>,
    stream_version: u32,
    credentials: Option<&PolicyProxyCredentials>,
) -> anyhow::Result<OpenPolicyUdpRelayResponse> {
    let (proxy_username, proxy_password) = credentials
        .map(|credentials| (credentials.username.clone(), credentials.password.clone()))
        .unwrap_or_default();
    let client = peer_mgr
        .get_peer_rpc_mgr()
        .rpc_client()
        .scoped_client::<PolicyUdpRelayRpcClientFactory<RpcController>>(
            peer_mgr.my_peer_id(),
            dst_peer_id,
            peer_mgr.get_global_ctx().get_network_name(),
        );
    tokio::time::timeout(
        SETUP_TIMEOUT,
        client.open_policy_udp_relay(
            RpcController::default(),
            OpenPolicyUdpRelayRequest {
                source_peer_id: peer_mgr.my_peer_id(),
                proxy_addr: Some(proxy_addr.into()),
                origin_addr: origin_addr.map(Into::into),
                stream_version,
                proxy_username,
                proxy_password,
            },
        ),
    )
    .await
    .context("policy UDP relay RPC timed out")?
    .map_err(anyhow::Error::from)
}

async fn open_legacy_remote_association(
    peer_mgr: &Arc<PeerManager>,
    data_plane: &Socks5Server,
    dst_peer_id: PeerId,
    proxy_addr: SocketAddr,
    credentials: Option<&PolicyProxyCredentials>,
) -> anyhow::Result<(AssociationToken, RemoteUdpTransport)> {
    let mesh_udp = data_plane
        .data_plane_udp_bind(0, SETUP_TIMEOUT)
        .await
        .context("failed to bind legacy policy UDP association socket")?;
    let response = request_remote_association(
        peer_mgr,
        dst_peer_id,
        proxy_addr,
        Some(mesh_udp.local_addr()),
        0,
        credentials,
    )
    .await?;
    let token = AssociationToken::try_from(response.token.as_slice())
        .map_err(|_| anyhow::anyhow!("policy UDP relay returned an invalid token"))?;
    let result = async {
        let relay_addr = response
            .relay_addr
            .context("legacy policy UDP relay returned no endpoint")?
            .into();
        let challenge =
            encode_relay_frame(&token, &[]).expect("an empty policy relay challenge always fits");
        mesh_udp
            .send_to(&challenge, relay_addr)
            .await
            .context("failed to prime local policy UDP relay route")?;
        tokio::time::timeout(SETUP_TIMEOUT, async {
            let mut ready = [0u8; FRAME_HEADER_LEN + 1];
            loop {
                let (length, source) = mesh_udp.recv_from(&mut ready).await?;
                if source == relay_addr
                    && decode_relay_frame(&token, &ready[..length]) == Some(&[][..])
                {
                    return Ok::<_, std::io::Error>(());
                }
            }
        })
        .await
        .context("legacy policy UDP readiness timed out")??;
        Ok::<_, anyhow::Error>(RemoteUdpTransport::Datagram {
            socket: mesh_udp,
            relay_addr,
            receive_buffer: tokio::sync::Mutex::new(vec![0u8; MAX_DATAGRAM_SIZE]),
        })
    }
    .await;
    if result.is_err() {
        close_remote_association(Arc::downgrade(peer_mgr), dst_peer_id, token).await;
    }
    result.map(|transport| (token, transport))
}

async fn close_remote_association(
    peer_mgr: Weak<PeerManager>,
    dst_peer_id: PeerId,
    token: AssociationToken,
) {
    let Some(peer_mgr) = peer_mgr.upgrade() else {
        return;
    };
    let client = peer_mgr
        .get_peer_rpc_mgr()
        .rpc_client()
        .scoped_client::<PolicyUdpRelayRpcClientFactory<RpcController>>(
            peer_mgr.my_peer_id(),
            dst_peer_id,
            peer_mgr.get_global_ctx().get_network_name(),
        );
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        client.close_policy_udp_relay(
            RpcController::default(),
            ClosePolicyUdpRelayRequest {
                token: token.to_vec(),
            },
        ),
    )
    .await;
}

async fn peer_virtual_ipv4(peer_mgr: &PeerManager, peer_id: PeerId) -> Option<Ipv4Addr> {
    peer_mgr
        .list_routes()
        .await
        .into_iter()
        .find(|route| route.peer_id == peer_id)
        .and_then(|route| route.ipv4_addr)
        .map(cidr::Ipv4Inet::from)
        .map(|inet| inet.address())
}

async fn open_local_socks_udp(
    proxy_addr: SocketAddr,
    credentials: Option<&PolicyProxyCredentials>,
) -> anyhow::Result<(TcpStream, UdpSocket, SocketAddr)> {
    let socket = match proxy_addr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    let mut control = tokio::time::timeout(SETUP_TIMEOUT, socket.connect(proxy_addr))
        .await
        .context("local SOCKS TCP connect timed out")??;
    control.set_nodelay(true)?;
    negotiate_policy_proxy_auth(&mut control, credentials).await?;

    let local_ip = control.local_addr()?.ip();
    let native_udp = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
    tune_policy_udp_socket(&native_udp);
    let request_addr = native_udp.local_addr()?;
    control
        .write_all(&socks_udp_associate_request(request_addr))
        .await?;
    let (reply_code, relay_port) = read_socks_udp_reply(&mut control).await?;
    if reply_code != 0 {
        bail!("local SOCKS UDP ASSOCIATE failed with reply {reply_code}");
    }
    let relay_addr = SocketAddr::new(proxy_addr.ip(), relay_port);
    native_udp.connect(relay_addr).await?;
    Ok((control, native_udp, relay_addr))
}

fn encode_uot_connect_request(destination: SocketAddr) -> Vec<u8> {
    let mut request = Vec::with_capacity(1 + 1 + 16 + 2);
    request.push(UOT_CONNECT);
    match destination {
        SocketAddr::V4(destination) => {
            request.push(UOT_IPV4);
            request.extend_from_slice(&destination.ip().octets());
            request.extend_from_slice(&destination.port().to_be_bytes());
        }
        SocketAddr::V6(destination) => {
            request.push(UOT_IPV6);
            request.extend_from_slice(&destination.ip().octets());
            request.extend_from_slice(&destination.port().to_be_bytes());
        }
    }
    request
}

async fn read_uot_connect_request<R>(reader: &mut R) -> anyhow::Result<SocketAddr>
where
    R: tokio::io::AsyncRead + Unpin,
{
    if reader.read_u8().await? != UOT_CONNECT {
        bail!("policy UoT requires connected UDP mode");
    }
    match reader.read_u8().await? {
        UOT_IPV4 => {
            let mut address = [0u8; 4];
            reader.read_exact(&mut address).await?;
            let port = reader.read_u16().await?;
            Ok(SocketAddr::new(IpAddr::V4(address.into()), port))
        }
        UOT_IPV6 => {
            let mut address = [0u8; 16];
            reader.read_exact(&mut address).await?;
            let port = reader.read_u16().await?;
            Ok(SocketAddr::new(IpAddr::V6(address.into()), port))
        }
        address_type => bail!("unsupported policy UoT address type {address_type}"),
    }
}

trait PolicyUotIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

impl<T> PolicyUotIo for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

async fn accept_uot_candidate(
    kernel_listener: &TcpListener,
    data_plane_listener: &mut DataPlaneTcpListener,
) -> std::io::Result<Box<dyn PolicyUotIo>> {
    tokio::select! {
        accepted = kernel_listener.accept() => {
            let (stream, _) = accepted?;
            stream.set_nodelay(true)?;
            Ok(Box::new(stream))
        }
        accepted = data_plane_listener.accept() => {
            let (stream, _) = accepted?;
            Ok(Box::new(stream))
        }
    }
}

async fn run_stream_association(
    context: AssociationContext,
    kernel_listener: TcpListener,
    mut data_plane_listener: DataPlaneTcpListener,
) {
    let AssociationContext {
        token,
        source_ip,
        mut control,
        native_udp,
        upstream_relay,
        cancel,
    } = context;
    let mut setup_control_byte = [0u8; 1];
    let accepted = tokio::select! {
        _ = cancel.cancelled() => None,
        _ = control.read(&mut setup_control_byte) => None,
        accepted = tokio::time::timeout(SETUP_TIMEOUT, async {
            for _ in 0..8 {
                let mut stream = accept_uot_candidate(
                    &kernel_listener,
                    &mut data_plane_listener,
                )
                .await?;
                let authenticated = tokio::time::timeout(SETUP_TIMEOUT, async {
                    let mut received_token = [0u8; TOKEN_LEN];
                    stream.read_exact(&mut received_token).await?;
                    if !constant_time_eq(&received_token, &token) {
                        bail!("invalid policy UoT token");
                    }
                    if read_uot_connect_request(&mut stream).await? != upstream_relay {
                        bail!("policy UoT destination does not match the reserved SOCKS relay");
                    }
                    stream.write_u8(UOT_READY).await?;
                    stream.flush().await?;
                    Ok::<_, anyhow::Error>(stream)
                })
                .await;
                if let Ok(Ok(stream)) = authenticated {
                    return Ok::<_, std::io::Error>(stream);
                }
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "policy UoT authentication attempts exhausted",
            ))
        }) => accepted.ok().and_then(Result::ok),
    };
    let Some(stream) = accepted else {
        tracing::debug!(%source_ip, %upstream_relay, "policy UoT setup failed");
        return;
    };

    let native_udp = Arc::new(native_udp);
    let (stream_reader, mut stream_writer) = tokio::io::split(stream);
    let mut stream_reader = BufReader::with_capacity(UOT_STREAM_BUFFER_SIZE, stream_reader);
    let (activity_tx, mut activity_rx) = tokio::sync::watch::channel(tokio::time::Instant::now());
    let stream_to_udp = {
        let native_udp = native_udp.clone();
        let activity_tx = activity_tx.clone();
        async move {
            let mut packet = vec![0u8; MAX_DATAGRAM_SIZE];
            loop {
                let length = stream_reader.read_u16().await? as usize;
                stream_reader.read_exact(&mut packet[..length]).await?;
                native_udp.send(&packet[..length]).await?;
                let _ = activity_tx.send(tokio::time::Instant::now());
            }
            #[allow(unreachable_code)]
            Ok::<(), std::io::Error>(())
        }
    };
    let udp_to_stream = {
        let native_udp = native_udp.clone();
        async move {
            let mut packet = vec![0u8; MAX_DATAGRAM_SIZE];
            let mut frame = Vec::with_capacity(UOT_STREAM_BUFFER_SIZE);
            loop {
                let length = native_udp.recv(&mut packet).await?;
                frame.clear();
                frame.extend_from_slice(&(length as u16).to_be_bytes());
                frame.extend_from_slice(&packet[..length]);
                stream_writer.write_all(&frame).await?;
                let _ = activity_tx.send(tokio::time::Instant::now());
            }
            #[allow(unreachable_code)]
            Ok::<(), std::io::Error>(())
        }
    };
    let idle = async move {
        loop {
            let deadline = *activity_rx.borrow() + ASSOCIATION_IDLE_TIMEOUT;
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return,
                changed = activity_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    };
    let mut control_byte = [0u8; 1];
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = idle => {}
        _ = control.read(&mut control_byte) => {}
        _ = stream_to_udp => {}
        _ = udp_to_stream => {}
    }
    tracing::debug!(%source_ip, %upstream_relay, "policy UoT association closed");
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

async fn run_association(
    context: AssociationContext,
    origin_addr: SocketAddr,
    mesh_udp: DataPlaneUdpSocket,
) {
    let AssociationContext {
        token,
        source_ip,
        mut control,
        native_udp,
        upstream_relay,
        cancel,
    } = context;
    let mut mesh_packet = vec![0u8; MAX_DATAGRAM_SIZE];
    let mut native_packet = vec![0u8; MAX_DATAGRAM_SIZE - FRAME_HEADER_LEN];
    let mut control_byte = [0u8; 1];
    let idle = tokio::time::sleep(ASSOCIATION_IDLE_TIMEOUT);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = &mut idle => break,
            result = control.read(&mut control_byte) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(_) => break,
                }
            }
            result = mesh_udp.recv_from(&mut mesh_packet) => {
                let Ok((length, source)) = result else { break };
                if source.ip() != IpAddr::V4(source_ip)
                    || length < FRAME_HEADER_LEN
                    || mesh_packet[0] != FRAME_VERSION
                    || mesh_packet[1..FRAME_HEADER_LEN] != token
                {
                    continue;
                }
                if source != origin_addr {
                    continue;
                }
                if length == FRAME_HEADER_LEN {
                    if mesh_udp.send_to(&mesh_packet[..length], origin_addr).await.is_err() {
                        break;
                    }
                    idle.as_mut().reset(tokio::time::Instant::now() + ASSOCIATION_IDLE_TIMEOUT);
                    continue;
                }
                if native_udp.send(&mesh_packet[FRAME_HEADER_LEN..length]).await.is_err() {
                    break;
                }
                idle.as_mut().reset(tokio::time::Instant::now() + ASSOCIATION_IDLE_TIMEOUT);
            }
            result = native_udp.recv(&mut native_packet) => {
                let Ok(length) = result else { break };
                let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + length);
                frame.push(FRAME_VERSION);
                frame.extend_from_slice(&token);
                frame.extend_from_slice(&native_packet[..length]);
                if mesh_udp.send_to(&frame, origin_addr).await.is_err() {
                    break;
                }
                idle.as_mut().reset(tokio::time::Instant::now() + ASSOCIATION_IDLE_TIMEOUT);
            }
        }
    }
    tracing::debug!(%source_ip, %upstream_relay, "policy UDP relay association closed");
}

pub(crate) fn encode_relay_frame(token: &AssociationToken, payload: &[u8]) -> Option<Vec<u8>> {
    (payload.len() <= MAX_DATAGRAM_SIZE - FRAME_HEADER_LEN).then(|| {
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
        frame.push(FRAME_VERSION);
        frame.extend_from_slice(token);
        frame.extend_from_slice(payload);
        frame
    })
}

pub(crate) fn decode_relay_frame<'a>(
    token: &AssociationToken,
    frame: &'a [u8],
) -> Option<&'a [u8]> {
    if frame.len() < FRAME_HEADER_LEN
        || frame[0] != FRAME_VERSION
        || frame[1..FRAME_HEADER_LEN] != *token
    {
        return None;
    }
    Some(&frame[FRAME_HEADER_LEN..])
}

fn socks_udp_associate_request(address: SocketAddr) -> Vec<u8> {
    let mut request = vec![5, 3, 0];
    match address {
        SocketAddr::V4(address) => {
            request.push(1);
            request.extend_from_slice(&address.ip().octets());
            request.extend_from_slice(&address.port().to_be_bytes());
        }
        SocketAddr::V6(address) => {
            request.push(4);
            request.extend_from_slice(&address.ip().octets());
            request.extend_from_slice(&address.port().to_be_bytes());
        }
    }
    request
}

async fn read_socks_udp_reply(control: &mut TcpStream) -> anyhow::Result<(u8, u16)> {
    let mut header = [0u8; 4];
    control.read_exact(&mut header).await?;
    if header[0] != 5 || header[2] != 0 {
        bail!("invalid SOCKS UDP ASSOCIATE reply");
    }
    let port = match header[3] {
        1 => {
            let mut address = [0u8; 6];
            control.read_exact(&mut address).await?;
            u16::from_be_bytes([address[4], address[5]])
        }
        4 => {
            let mut address = [0u8; 18];
            control.read_exact(&mut address).await?;
            u16::from_be_bytes([address[16], address[17]])
        }
        3 => {
            let length = control.read_u8().await? as usize;
            let mut address = vec![0u8; length + 2];
            control.read_exact(&mut address).await?;
            u16::from_be_bytes([address[length], address[length + 1]])
        }
        value => bail!("unsupported SOCKS relay address type {value}"),
    };
    if port == 0 {
        bail!("SOCKS UDP relay returned port 0");
    }
    Ok((header[1], port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        peers::tests::{connect_peer_manager, create_mock_peer_manager, wait_route_appear},
        tunnel::common::tests::wait_for_condition,
    };

    #[test]
    fn relay_frame_is_versioned_and_token_bound() {
        let token = [7u8; TOKEN_LEN];
        let frame = encode_relay_frame(&token, b"voice").unwrap();
        assert_eq!(
            decode_relay_frame(&token, &frame),
            Some(b"voice".as_slice())
        );
        assert!(decode_relay_frame(&[8u8; TOKEN_LEN], &frame).is_none());
        assert_eq!(
            decode_relay_frame(&token, &frame[..FRAME_HEADER_LEN]),
            Some([].as_slice())
        );
        assert!(decode_relay_frame(&token, &frame[..FRAME_HEADER_LEN - 1]).is_none());
    }

    #[test]
    fn udp_associate_request_preserves_bound_endpoint() {
        assert_eq!(
            socks_udp_associate_request("127.0.0.1:32123".parse().unwrap()),
            [5, 3, 0, 1, 127, 0, 0, 1, 0x7d, 0x7b]
        );
    }

    #[test]
    fn uot_v2_connect_request_matches_sagernet_wire_format() {
        assert_eq!(
            encode_uot_connect_request("192.0.2.7:32123".parse().unwrap()),
            [UOT_CONNECT, UOT_IPV4, 192, 0, 2, 7, 0x7d, 0x7b]
        );
        assert_eq!(
            encode_uot_connect_request("[2001:db8::7]:53".parse().unwrap()),
            [
                UOT_CONNECT,
                UOT_IPV6,
                0x20,
                0x01,
                0x0d,
                0xb8,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                7,
                0,
                53,
            ]
        );
    }

    #[tokio::test]
    async fn reads_uot_v2_connect_request_without_consuming_payload() {
        let destination: SocketAddr = "192.0.2.9:5353".parse().unwrap();
        let mut wire = encode_uot_connect_request(destination);
        wire.extend_from_slice(&5u16.to_be_bytes());
        wire.extend_from_slice(b"voice");
        let mut wire = wire.as_slice();
        assert_eq!(
            read_uot_connect_request(&mut wire).await.unwrap(),
            destination
        );
        assert_eq!(wire, [0, 5, b'v', b'o', b'i', b'c', b'e']);
    }

    #[tokio::test]
    async fn uot_stream_relays_unpaced_burst_over_smoltcp_fallback() {
        let peer_a = create_mock_peer_manager().await;
        let peer_b = create_mock_peer_manager().await;
        connect_peer_manager(peer_a.clone(), peer_b.clone()).await;
        let ip_a: cidr::Ipv4Inet = "10.178.0.1/24".parse().unwrap();
        let ip_b: cidr::Ipv4Inet = "10.178.0.2/24".parse().unwrap();
        peer_a.get_global_ctx().set_ipv4(Some(ip_a));
        peer_b.get_global_ctx().set_ipv4(Some(ip_b));
        wait_route_appear(peer_a.clone(), peer_b.clone())
            .await
            .unwrap();

        let data_plane_a = Socks5Server::new(peer_a.get_global_ctx(), peer_a.clone(), None);
        let data_plane_b = Socks5Server::new(peer_b.get_global_ctx(), peer_b.clone(), None);
        data_plane_a.run(None).await.unwrap();
        data_plane_b.run(None).await.unwrap();

        let kernel_listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
        let stream_port = kernel_listener.local_addr().unwrap().port();
        let data_plane_listener = data_plane_b
            .data_plane_tcp_bind(stream_port, SETUP_TIMEOUT)
            .await
            .unwrap();

        let control_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let control_client = TcpStream::connect(control_listener.local_addr().unwrap())
            .await
            .unwrap();
        let (control, _) = control_listener.accept().await.unwrap();

        let echo = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        let native_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        native_udp
            .connect(echo.local_addr().unwrap())
            .await
            .unwrap();
        let upstream_relay = echo.local_addr().unwrap();
        let echo_task = {
            let echo = echo.clone();
            tokio::spawn(async move {
                let mut packet = vec![0u8; 1500];
                for _ in 0..128 {
                    let (length, source) = echo.recv_from(&mut packet).await.unwrap();
                    echo.send_to(&packet[..length], source).await.unwrap();
                }
            })
        };

        let token = [0x5au8; TOKEN_LEN];
        let cancel = CancellationToken::new();
        let relay_task = {
            let cancel = cancel.clone();
            tokio::spawn(run_stream_association(
                AssociationContext {
                    token,
                    source_ip: ip_a.address(),
                    control,
                    native_udp,
                    upstream_relay,
                    cancel,
                },
                kernel_listener,
                data_plane_listener,
            ))
        };

        let stream_addr = SocketAddr::new(IpAddr::V4(ip_b.address()), stream_port);
        let mut stream = data_plane_a
            .data_plane_tcp_connect_mesh_only(stream_addr, SETUP_TIMEOUT)
            .await
            .unwrap();
        stream.write_all(&token).await.unwrap();
        stream
            .write_all(&encode_uot_connect_request(upstream_relay))
            .await
            .unwrap();
        stream.flush().await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), UOT_READY);

        for sequence in 0u8..128 {
            let mut packet = vec![sequence; 1200];
            packet[1..5].copy_from_slice(b"uot2");
            stream.write_u16(packet.len() as u16).await.unwrap();
            stream.write_all(&packet).await.unwrap();
        }

        let mut seen = [false; 128];
        let mut packet = vec![0u8; 1500];
        for _ in 0..128 {
            let length = tokio::time::timeout(SETUP_TIMEOUT, stream.read_u16())
                .await
                .unwrap()
                .unwrap() as usize;
            stream.read_exact(&mut packet[..length]).await.unwrap();
            assert_eq!(&packet[1..5], b"uot2");
            seen[usize::from(packet[0])] = true;
        }
        assert!(seen.into_iter().all(|seen| seen));

        cancel.cancel();
        drop(stream);
        drop(control_client);
        tokio::time::timeout(SETUP_TIMEOUT, relay_task)
            .await
            .unwrap()
            .unwrap();
        echo_task.await.unwrap();
    }

    #[tokio::test]
    async fn relays_udp_from_the_destination_peer_source_identity() {
        let peer_a = create_mock_peer_manager().await;
        let peer_b = create_mock_peer_manager().await;
        connect_peer_manager(peer_a.clone(), peer_b.clone()).await;
        // Mock peers do not install their virtual addresses in the runner kernel. Distinct
        // loopback addresses keep the native SOCKS side local while the data plane still routes
        // between two different virtual peer addresses.
        let ip_a: cidr::Ipv4Inet = "127.77.0.1/24".parse().unwrap();
        let ip_b: cidr::Ipv4Inet = "127.77.0.2/24".parse().unwrap();
        peer_a.get_global_ctx().set_ipv4(Some(ip_a));
        peer_b.get_global_ctx().set_ipv4(Some(ip_b));
        wait_route_appear(peer_a.clone(), peer_b.clone())
            .await
            .unwrap();

        let data_plane_a = Socks5Server::new(peer_a.get_global_ctx(), peer_a.clone(), None);
        let data_plane_b = Socks5Server::new(peer_b.get_global_ctx(), peer_b.clone(), None);
        data_plane_a.run(None).await.unwrap();
        data_plane_b.run(None).await.unwrap();
        let relay_service = MeshUdpRelayService::new(&peer_b, data_plane_b);
        relay_service.register();
        let mut peer_reservations = Vec::with_capacity(ASSOCIATION_LIMIT_PER_PEER);
        for _ in 0..ASSOCIATION_LIMIT_PER_PEER {
            peer_reservations.push(
                relay_service
                    .reserve_token(peer_a.my_peer_id(), &CancellationToken::new())
                    .await
                    .unwrap(),
            );
        }
        assert!(
            relay_service
                .reserve_token(peer_a.my_peer_id(), &CancellationToken::new())
                .await
                .err()
                .expect("the per-peer association limit must reject one more reservation")
                .to_string()
                .contains("requesting peer")
        );
        let other_peer_reservation = relay_service
            .reserve_token(peer_b.my_peer_id(), &CancellationToken::new())
            .await
            .unwrap();
        drop(other_peer_reservation);
        drop(peer_reservations);
        assert!(relay_service.associations.is_empty());

        let mut forged_controller = BaseController::default();
        forged_controller.set_source_peer_id(peer_a.my_peer_id());
        let forged = relay_service
            .open_policy_udp_relay(
                forged_controller,
                OpenPolicyUdpRelayRequest {
                    source_peer_id: peer_b.my_peer_id(),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(forged.to_string().contains("RPC caller"));

        let pending_cancel = CancellationToken::new();
        let pending = relay_service
            .reserve_token(peer_a.my_peer_id(), &pending_cancel)
            .await
            .unwrap();
        assert_eq!(relay_service.associations.len(), 1);
        let mut wrong_peer_controller = BaseController::default();
        wrong_peer_controller.set_source_peer_id(peer_b.my_peer_id());
        relay_service
            .close_policy_udp_relay(
                wrong_peer_controller,
                ClosePolicyUdpRelayRequest {
                    token: pending.token.to_vec(),
                },
            )
            .await
            .unwrap();
        assert!(!pending_cancel.is_cancelled());
        drop(pending);
        assert!(relay_service.associations.is_empty());

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let proxy_ip = ip_b.address();
        let fake_server = tokio::spawn(async move {
            // Loopback virtual addresses intentionally make the v2 mesh-only stream
            // unroutable. The first association must be canceled and the second legacy
            // association must remain usable rather than leaking the failed attempt.
            for attempt in 0..2 {
                let (mut control, control_source) = listener.accept().await.unwrap();
                let mut greeting = [0u8; 3];
                control.read_exact(&mut greeting).await.unwrap();
                assert_eq!(greeting, [5, 1, 0]);
                control.write_all(&[5, 0]).await.unwrap();
                let mut request = [0u8; 10];
                control.read_exact(&mut request).await.unwrap();
                assert_eq!(&request[..4], &[5, 3, 0, 1]);
                let udp = UdpSocket::bind((proxy_ip, 0)).await.unwrap();
                control
                    .write_all(&socks_reply_for_test(udp.local_addr().unwrap()))
                    .await
                    .unwrap();
                if attempt == 0 {
                    let mut closed = [0u8; 1];
                    assert_eq!(control.read(&mut closed).await.unwrap(), 0);
                    continue;
                }
                let mut packet = [0u8; 256];
                for _ in 0..1 {
                    let (length, source) = udp.recv_from(&mut packet).await.unwrap();
                    assert_eq!(source.ip(), control_source.ip());
                    udp.send_to(&packet[..length], source).await.unwrap();
                }
                let mut closed = [0u8; 1];
                assert_eq!(control.read(&mut closed).await.unwrap(), 0);
            }
        });

        let association = RemoteUdpAssociation::open(
            &peer_a,
            &data_plane_a,
            peer_b.my_peer_id(),
            SocketAddr::new(IpAddr::V4(ip_b.address()), proxy_port),
            None,
        )
        .await
        .unwrap();
        let payload = b"\0\0\0\x01\x7f\0\0\x01\0\x35voice";
        association.send(payload).await.unwrap();
        let mut response = [0u8; 256];
        let length = tokio::time::timeout(Duration::from_secs(5), association.recv(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&response[..length], payload);
        association.close().await;
        wait_for_condition(
            || async { relay_service.associations.is_empty() },
            Duration::from_secs(2),
        )
        .await;
        fake_server.await.unwrap();
    }

    fn socks_reply_for_test(address: SocketAddr) -> Vec<u8> {
        let SocketAddr::V4(address) = address else {
            unreachable!()
        };
        let mut reply = vec![5, 0, 0, 1];
        reply.extend_from_slice(&address.ip().octets());
        reply.extend_from_slice(&address.port().to_be_bytes());
        reply
    }
}
