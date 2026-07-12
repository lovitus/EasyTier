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
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpSocket, TcpStream, UdpSocket},
};
use tokio_util::sync::CancellationToken;

use crate::{
    common::PeerId,
    gateway::socks5::{DataPlaneUdpSocket, Socks5Server},
    peers::peer_manager::PeerManager,
    proto::{
        peer_rpc::{
            ClosePolicyUdpRelayRequest, ClosePolicyUdpRelayResponse, OpenPolicyUdpRelayRequest,
            OpenPolicyUdpRelayResponse, PolicyUdpRelayRpc, PolicyUdpRelayRpcClientFactory,
            PolicyUdpRelayRpcServer,
        },
        rpc_impl::RpcController,
        rpc_types::{self, controller::BaseController},
    },
};

const ASSOCIATION_LIMIT: usize = 1_024;
const ASSOCIATION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_VERSION: u8 = 1;
const TOKEN_LEN: usize = 16;
const FRAME_HEADER_LEN: usize = 1 + TOKEN_LEN;
const MAX_DATAGRAM_SIZE: usize = u16::MAX as usize;

pub(crate) type AssociationToken = [u8; TOKEN_LEN];

pub(crate) struct MeshUdpRelayService {
    peer_mgr: Weak<PeerManager>,
    data_plane: Arc<Socks5Server>,
    associations: Arc<DashMap<AssociationToken, CancellationToken>>,
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
        request: OpenPolicyUdpRelayRequest,
    ) -> anyhow::Result<OpenPolicyUdpRelayResponse> {
        let peer_mgr = self
            .peer_mgr
            .upgrade()
            .context("peer manager is no longer available")?;
        let source_ip = peer_virtual_ipv4(&peer_mgr, request.source_peer_id)
            .await
            .context("requesting peer has no routed virtual IPv4 address")?;
        let proxy_addr: SocketAddr = request
            .proxy_addr
            .context("policy UDP relay request has no proxy address")?
            .into();
        let origin_addr: SocketAddr = request
            .origin_addr
            .context("policy UDP relay request has no origin endpoint")?
            .into();
        if origin_addr.ip() != IpAddr::V4(source_ip) || origin_addr.port() == 0 {
            bail!("policy UDP relay origin endpoint does not match the requesting peer");
        }
        let local_virtual_ip = peer_mgr
            .get_global_ctx()
            .get_ipv4()
            .context("destination peer has no virtual IPv4 address")?
            .address();
        if proxy_addr.ip() != IpAddr::V4(local_virtual_ip) || proxy_addr.port() == 0 {
            bail!("policy UDP relay only permits a SOCKS server on this peer's exact virtual IPv4");
        }

        let cancel = CancellationToken::new();
        let token = self.reserve_token(&cancel).await?;
        let setup = async {
            let (control, native_udp, upstream_relay) = open_local_socks_udp(proxy_addr).await?;
            let mesh_udp = self
                .data_plane
                .data_plane_udp_bind(0, SETUP_TIMEOUT)
                .await
                .context("failed to bind mesh UDP association socket")?;
            Ok::<_, anyhow::Error>((control, native_udp, upstream_relay, mesh_udp))
        }
        .await;
        let (control, native_udp, upstream_relay, mesh_udp) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                self.associations.remove(&token);
                return Err(error);
            }
        };
        let relay_addr = mesh_udp.local_addr();
        let ready = encode_relay_frame(&token, &[])
            .expect("an empty policy relay readiness frame always fits");
        if let Err(error) = mesh_udp.send_to(&ready, origin_addr).await {
            self.associations.remove(&token);
            return Err(error).context("failed to prime policy UDP relay route");
        }
        let associations = self.associations.clone();
        tokio::spawn(async move {
            run_association(
                token,
                source_ip,
                origin_addr,
                mesh_udp,
                control,
                native_udp,
                upstream_relay,
                cancel,
            )
            .await;
            associations.remove(&token);
        });

        Ok(OpenPolicyUdpRelayResponse {
            token: token.to_vec(),
            relay_addr: Some(relay_addr.into()),
        })
    }

    async fn reserve_token(&self, cancel: &CancellationToken) -> anyhow::Result<AssociationToken> {
        let _capacity = self.capacity_lock.lock().await;
        if self.associations.len() >= ASSOCIATION_LIMIT {
            bail!("policy UDP relay association table is full");
        }
        for _ in 0..8 {
            let token = rand::random::<AssociationToken>();
            if let dashmap::mapref::entry::Entry::Vacant(entry) = self.associations.entry(token) {
                entry.insert(cancel.clone());
                return Ok(token);
            }
        }
        bail!("failed to allocate a unique policy UDP relay token")
    }
}

impl Drop for MeshUdpRelayService {
    fn drop(&mut self) {
        for association in self.associations.iter() {
            association.value().cancel();
        }
    }
}

#[async_trait::async_trait]
impl PolicyUdpRelayRpc for MeshUdpRelayService {
    type Controller = BaseController;

    async fn open_policy_udp_relay(
        &self,
        _: BaseController,
        request: OpenPolicyUdpRelayRequest,
    ) -> Result<OpenPolicyUdpRelayResponse, rpc_types::error::Error> {
        self.open_association(request)
            .await
            .map_err(rpc_types::error::Error::ExecutionError)
    }

    async fn close_policy_udp_relay(
        &self,
        _: BaseController,
        request: ClosePolicyUdpRelayRequest,
    ) -> Result<ClosePolicyUdpRelayResponse, rpc_types::error::Error> {
        if let Ok(token) = AssociationToken::try_from(request.token.as_slice())
            && let Some((_, cancel)) = self.associations.remove(&token)
        {
            cancel.cancel();
        }
        Ok(ClosePolicyUdpRelayResponse {})
    }
}

pub(crate) struct RemoteUdpAssociation {
    pub(crate) token: AssociationToken,
    pub(crate) relay_addr: SocketAddr,
    dst_peer_id: PeerId,
    peer_mgr: Weak<PeerManager>,
    closed: AtomicBool,
}

impl RemoteUdpAssociation {
    pub(crate) async fn open(
        peer_mgr: &Arc<PeerManager>,
        dst_peer_id: PeerId,
        proxy_addr: SocketAddr,
        mesh_udp: &DataPlaneUdpSocket,
    ) -> anyhow::Result<Self> {
        let origin_addr = mesh_udp.local_addr();
        let client = peer_mgr
            .get_peer_rpc_mgr()
            .rpc_client()
            .scoped_client::<PolicyUdpRelayRpcClientFactory<RpcController>>(
                peer_mgr.my_peer_id(),
                dst_peer_id,
                peer_mgr.get_global_ctx().get_network_name(),
            );
        let response = tokio::time::timeout(
            SETUP_TIMEOUT,
            client.open_policy_udp_relay(
                RpcController::default(),
                OpenPolicyUdpRelayRequest {
                    source_peer_id: peer_mgr.my_peer_id(),
                    proxy_addr: Some(proxy_addr.into()),
                    origin_addr: Some(origin_addr.into()),
                },
            ),
        )
        .await
        .context("policy UDP relay RPC timed out")??;
        let token = AssociationToken::try_from(response.token.as_slice())
            .map_err(|_| anyhow::anyhow!("policy UDP relay returned an invalid token"))?;
        let relay_addr = response
            .relay_addr
            .context("policy UDP relay returned no endpoint")?
            .into();
        let association = Self {
            token,
            relay_addr,
            dst_peer_id,
            peer_mgr: Arc::downgrade(peer_mgr),
            closed: AtomicBool::new(false),
        };
        let mut ready = [0u8; FRAME_HEADER_LEN];
        let readiness = tokio::time::timeout(SETUP_TIMEOUT, mesh_udp.recv_from(&mut ready)).await;
        let valid = matches!(
            readiness,
            Ok(Ok((length, source)))
                if source == association.relay_addr
                    && decode_relay_frame(&association.token, &ready[..length]) == Some(&[][..])
        );
        if !valid {
            association.close().await;
            bail!("policy UDP relay returned an invalid readiness frame");
        }
        Ok(association)
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
) -> anyhow::Result<(TcpStream, UdpSocket, SocketAddr)> {
    let socket = match proxy_addr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    let mut control = tokio::time::timeout(SETUP_TIMEOUT, socket.connect(proxy_addr))
        .await
        .context("local SOCKS TCP connect timed out")??;
    control.set_nodelay(true)?;
    control.write_all(&[5, 1, 0]).await?;
    let mut greeting = [0u8; 2];
    control.read_exact(&mut greeting).await?;
    if greeting != [5, 0] {
        bail!("local SOCKS server does not permit no-authentication mode");
    }

    let local_ip = control.local_addr()?.ip();
    let native_udp = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
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

async fn run_association(
    token: AssociationToken,
    source_ip: Ipv4Addr,
    origin_addr: SocketAddr,
    mesh_udp: DataPlaneUdpSocket,
    mut control: TcpStream,
    native_udp: UdpSocket,
    upstream_relay: SocketAddr,
    cancel: CancellationToken,
) {
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
                    || length <= FRAME_HEADER_LEN
                    || mesh_packet[0] != FRAME_VERSION
                    || mesh_packet[1..FRAME_HEADER_LEN] != token
                {
                    continue;
                }
                if source != origin_addr {
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
    (frame.len() >= FRAME_HEADER_LEN
        && frame[0] == FRAME_VERSION
        && frame[1..FRAME_HEADER_LEN] == *token)
        .then_some(&frame[FRAME_HEADER_LEN..])
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

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let fake_server = tokio::spawn(async move {
            let (mut control, control_source) = listener.accept().await.unwrap();
            let mut greeting = [0u8; 3];
            control.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            control.write_all(&[5, 0]).await.unwrap();
            let mut request = [0u8; 10];
            control.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[..4], &[5, 3, 0, 1]);
            let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
            control
                .write_all(&socks_reply_for_test(udp.local_addr().unwrap()))
                .await
                .unwrap();
            let mut packet = [0u8; 256];
            let (length, source) = udp.recv_from(&mut packet).await.unwrap();
            assert_eq!(source.ip(), control_source.ip());
            udp.send_to(&packet[..length], source).await.unwrap();
            let mut closed = [0u8; 1];
            assert_eq!(control.read(&mut closed).await.unwrap(), 0);
        });

        let mesh_udp = data_plane_a
            .data_plane_udp_bind(0, Duration::from_secs(5))
            .await
            .unwrap();
        let association = RemoteUdpAssociation::open(
            &peer_a,
            peer_b.my_peer_id(),
            SocketAddr::new(IpAddr::V4(ip_b.address()), proxy_port),
            &mesh_udp,
        )
        .await
        .unwrap();
        let payload = b"\0\0\0\x01\x7f\0\0\x01\0\x35voice";
        let frame = encode_relay_frame(&association.token, payload).unwrap();
        mesh_udp
            .send_to(&frame, association.relay_addr)
            .await
            .unwrap();
        let mut response = [0u8; 256];
        let (length, source) =
            tokio::time::timeout(Duration::from_secs(5), mesh_udp.recv_from(&mut response))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(source, association.relay_addr);
        assert_eq!(
            decode_relay_frame(&association.token, &response[..length]),
            Some(payload.as_slice())
        );
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
