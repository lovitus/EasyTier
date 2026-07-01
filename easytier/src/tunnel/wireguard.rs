use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex as StdMutex, atomic::AtomicBool},
    time::{Duration, Instant as StdInstant},
};

use hotpath::instant::Instant;

use super::{
    FromUrl, IpVersion, Tunnel, TunnelError, TunnelInfo, TunnelListener, TunnelUrl, ZCPacketSink,
    ZCPacketStream,
    common::wait_for_connect_futures,
    generate_digest_from_str,
    packet_def::{PEER_MANAGER_HEADER_SIZE, ZCPacketType},
    ring::create_ring_tunnel_pair,
};
use crate::tunnel::common::{BindDev, bind};
use crate::{
    common::shrink_dashmap,
    tunnel::{
        build_url_from_socket_addr,
        common::TunnelWrapper,
        packet_def::{WG_TUNNEL_HEADER_SIZE, ZCPacket},
    },
};
use anyhow::Context;
use async_recursion::async_recursion;
use async_trait::async_trait;
use boringtun::{
    noise::{Tunn, TunnResult, errors::WireGuardError},
    x25519::{PublicKey, StaticSecret},
};
use bytes::BytesMut;
use crossbeam::atomic::AtomicCell;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt, stream::FuturesUnordered};
use rand::RngCore;
use tokio::{net::UdpSocket, sync::Mutex, task::JoinSet};

const MAX_PACKET: usize = 2048;
const WG_STEALTH_FALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
const WG_STEALTH_OUTER_SEND_DELAY: Duration = Duration::from_secs(1);
const WG_STEALTH_GATE_RECV_GRACE: Duration = Duration::from_secs(5);
const WG_STEALTH_MAX_REPLAY_NONCES: usize = 4096;
const WG_STEALTH_MAX_SESSIONS: usize = 4096;

#[derive(Debug)]
struct WgStealthSession {
    state: Arc<crate::tunnel::stealth::OuterSessionState>,
    outer_seen_at: StdMutex<Option<StdInstant>>,
}

impl WgStealthSession {
    fn new(state: Arc<crate::tunnel::stealth::OuterSessionState>) -> Self {
        Self {
            state,
            outer_seen_at: StdMutex::new(None),
        }
    }

    fn outer_elapsed(&self, now: StdInstant) -> Option<Duration> {
        self.state.outer_key()?;
        let mut seen = self.outer_seen_at.lock().unwrap();
        let first_seen = *seen.get_or_insert(now);
        Some(now.saturating_duration_since(first_seen))
    }

    fn seal(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        match self.outer_elapsed(StdInstant::now()) {
            Some(elapsed) if elapsed >= WG_STEALTH_OUTER_SEND_DELAY => {
                self.state.seal_datagram(plaintext)
            }
            _ => self.state.seal_gate_datagram(plaintext),
        }
    }

    fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        match self.outer_elapsed(StdInstant::now()) {
            Some(elapsed) => self.state.open_datagram(sealed).or_else(|| {
                (elapsed <= WG_STEALTH_GATE_RECV_GRACE)
                    .then(|| self.state.open_gate_datagram(sealed))
                    .flatten()
            }),
            None => self.state.open_gate_datagram(sealed),
        }
    }
}

#[derive(Debug, Default)]
struct WgStealthReplayGuard {
    seen: StdMutex<HashMap<[u8; crate::tunnel::stealth::OUTER_NONCE_LEN], StdInstant>>,
}

impl WgStealthReplayGuard {
    fn accept(&self, state: &crate::tunnel::stealth::OuterSessionState, sealed: &[u8]) -> bool {
        let Ok(nonce) = sealed
            .get(..crate::tunnel::stealth::OUTER_NONCE_LEN)
            .unwrap_or_default()
            .try_into()
        else {
            return false;
        };
        let now = StdInstant::now();
        let ttl = Duration::from_secs(state.window_secs().saturating_mul(2).max(1));
        let mut seen = self.seen.lock().unwrap();
        seen.retain(|_, at| now.saturating_duration_since(*at) <= ttl);
        if seen.contains_key(&nonce) {
            return false;
        }
        while seen.len() >= WG_STEALTH_MAX_REPLAY_NONCES {
            let Some(oldest) = seen
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(nonce, _)| *nonce)
            else {
                break;
            };
            seen.remove(&oldest);
        }
        seen.insert(nonce, now);
        true
    }
}

fn is_wg_handshake_initiation(packet: &[u8]) -> bool {
    packet.len() >= 148 && packet[..4] == [1, 0, 0, 0]
}

#[derive(Debug, Clone)]
enum WgType {
    // used by easytier peer, need remove/add ip header for in/out wg msg
    InternalUse,
    // used by wireguard peer, keep original ip header
    ExternalUse,
}

#[derive(Clone)]
pub struct WgConfig {
    my_secret_key: StaticSecret,
    my_public_key: PublicKey,

    peer_secret_key: StaticSecret,
    peer_public_key: PublicKey,

    wg_type: WgType,
}

impl WgConfig {
    pub fn new_from_network_identity(network_name: &str, network_secret: &str) -> Self {
        let mut my_sec = [0u8; 32];
        generate_digest_from_str(network_name, network_secret, &mut my_sec);

        let my_secret_key = StaticSecret::from(my_sec);
        let my_public_key = PublicKey::from(&my_secret_key);
        let peer_secret_key = StaticSecret::from(my_sec);
        let peer_public_key = my_public_key;

        WgConfig {
            my_secret_key,
            my_public_key,
            peer_secret_key,
            peer_public_key,

            wg_type: WgType::InternalUse,
        }
    }

    pub fn new_for_portal(server_key_seed: &str, client_key_seed: &str) -> Self {
        let server_cfg = Self::new_from_network_identity("server", server_key_seed);
        let client_cfg = Self::new_from_network_identity("client", client_key_seed);
        Self {
            my_secret_key: server_cfg.my_secret_key,
            my_public_key: server_cfg.my_public_key,
            peer_secret_key: client_cfg.my_secret_key,
            peer_public_key: client_cfg.my_public_key,

            wg_type: WgType::ExternalUse,
        }
    }

    pub fn my_secret_key(&self) -> &[u8] {
        self.my_secret_key.as_bytes()
    }

    pub fn peer_secret_key(&self) -> &[u8] {
        self.peer_secret_key.as_bytes()
    }

    pub fn my_public_key(&self) -> &[u8] {
        self.my_public_key.as_bytes()
    }

    pub fn peer_public_key(&self) -> &[u8] {
        self.peer_public_key.as_bytes()
    }
}

#[derive(Clone)]
struct WgPeerData {
    udp: Arc<UdpSocket>, // only for send
    endpoint: SocketAddr,
    tunn: Arc<Mutex<Tunn>>,
    wg_type: WgType,
    stopped: Arc<AtomicBool>,
    stealth: Option<Arc<WgStealthSession>>,
}

impl Debug for WgPeerData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgPeerData")
            .field("endpoint", &self.endpoint)
            .field("local", &self.udp.local_addr())
            .finish()
    }
}

impl WgPeerData {
    async fn send_network_packet(&self, packet: &[u8]) -> Result<(), std::io::Error> {
        if let Some(stealth) = &self.stealth {
            let sealed = stealth.seal(packet).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "WG stealth sealing failed")
            })?;
            self.udp.send_to(&sealed, self.endpoint).await?;
        } else {
            self.udp.send_to(packet, self.endpoint).await?;
        }
        Ok(())
    }

    #[tracing::instrument]
    async fn handle_one_packet_from_me(&self, zc_packet: ZCPacket) -> Result<(), anyhow::Error> {
        let mut send_buf = vec![0u8; MAX_PACKET];

        let packet = if matches!(self.wg_type, WgType::InternalUse) {
            let mut zc_packet = zc_packet.convert_type(ZCPacketType::WG);
            Self::fill_ip_header(&mut zc_packet);
            zc_packet.into_bytes()
        } else {
            zc_packet.convert_type(ZCPacketType::WG).into_bytes()
        };
        tracing::trace!(?packet, "Sending packet to peer");

        let encapsulate_result = {
            let mut peer = self.tunn.lock().await;
            peer.encapsulate(&packet, &mut send_buf)
        };

        tracing::trace!(
            ?encapsulate_result,
            "Received {} bytes from me",
            packet.len()
        );

        match encapsulate_result {
            TunnResult::WriteToNetwork(packet) => {
                self.send_network_packet(packet)
                    .await
                    .context("Failed to send encrypted IP packet to WireGuard endpoint.")?;
                tracing::debug!(
                    "Sent {} bytes to WireGuard endpoint (encrypted IP packet)",
                    packet.len()
                );
            }
            TunnResult::Err(e) => {
                tracing::error!("Failed to encapsulate IP packet: {:?}", e);
            }
            TunnResult::Done => {
                // Ignored
            }
            other => {
                tracing::error!(
                    "Unexpected WireGuard state during encapsulation: {:?}",
                    other
                );
            }
        };
        Ok(())
    }

    /// WireGuard consumption task. Receives encrypted packets from the WireGuard endpoint,
    /// decapsulates them, and dispatches newly received IP packets.
    #[tracing::instrument(skip(sink))]
    pub async fn handle_one_packet_from_peer<S: ZCPacketSink + Unpin>(
        &self,
        mut sink: S,
        recv_buf: &[u8],
    ) -> bool {
        let mut send_buf = vec![0u8; MAX_PACKET];
        let data = recv_buf;
        let decapsulate_result = {
            let mut peer = self.tunn.lock().await;
            peer.decapsulate(None, data, &mut send_buf)
        };

        tracing::debug!("Decapsulation result: {:?}", decapsulate_result);

        match decapsulate_result {
            TunnResult::WriteToNetwork(packet) => {
                match self.send_network_packet(packet).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(
                            "Failed to send decapsulation-instructed packet to WireGuard endpoint: {:?}",
                            e
                        );
                        return false;
                    }
                };
                let mut peer = self.tunn.lock().await;
                loop {
                    let mut send_buf = vec![0u8; MAX_PACKET];
                    match peer.decapsulate(None, &[], &mut send_buf) {
                        TunnResult::WriteToNetwork(packet) => {
                            match self.send_network_packet(packet).await {
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to send decapsulation-instructed packet to WireGuard endpoint: {:?}",
                                        e
                                    );
                                    break;
                                }
                            };
                        }
                        _ => {
                            break;
                        }
                    }
                }
                true
            }
            TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                tracing::debug!(
                    ?packet,
                    "receive IP packet from peer: {} bytes",
                    packet.len()
                );
                let mut b = BytesMut::new();
                if matches!(self.wg_type, WgType::InternalUse) {
                    b.resize(WG_TUNNEL_HEADER_SIZE, 0);
                    b.extend_from_slice(self.remove_ip_header(packet, packet[0] >> 4 == 4));
                } else {
                    b.extend_from_slice(packet);
                };
                let zc_packet = ZCPacket::new_from_buf(b, ZCPacketType::WG);
                tracing::trace!(?zc_packet, "forward zc_packet to sink");
                let ret = sink.send(zc_packet).await;
                if ret.is_err() {
                    tracing::error!("Failed to send packet to tunnel: {:?}", ret);
                }
                ret.is_ok()
            }
            TunnResult::Done | TunnResult::Err(_) => {
                tracing::debug!(
                    "Unexpected WireGuard state during decapsulation: {:?}",
                    decapsulate_result
                );
                false
            }
        }
    }

    #[tracing::instrument]
    #[async_recursion]
    async fn handle_routine_tun_result<'a: 'async_recursion>(&self, result: TunnResult<'a>) -> () {
        match result {
            TunnResult::WriteToNetwork(packet) => {
                tracing::debug!(
                    "Sending routine packet of {} bytes to WireGuard endpoint",
                    packet.len()
                );
                match self.send_network_packet(packet).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(
                            "Failed to send routine packet to WireGuard endpoint: {:?}",
                            e
                        );
                    }
                };
            }
            TunnResult::Err(WireGuardError::ConnectionExpired) => {
                tracing::warn!("Wireguard handshake has expired!");

                let mut buf = vec![0u8; MAX_PACKET];
                let result = self
                    .tunn
                    .lock()
                    .await
                    .format_handshake_initiation(&mut buf[..], false);

                self.handle_routine_tun_result(result).await
            }
            TunnResult::Err(e) => {
                tracing::error!(
                    "Failed to prepare routine packet for WireGuard endpoint: {:?}",
                    e
                );
            }
            TunnResult::Done => {
                // Sleep for a bit
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            other => {
                tracing::warn!("Unexpected WireGuard routine task state: {:?}", other);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
    }

    /// WireGuard Routine task. Handles Handshake, keep-alive, etc.
    pub async fn routine_task(self) {
        loop {
            let mut send_buf = vec![0u8; MAX_PACKET];
            let tun_result = { self.tunn.lock().await.update_timers(&mut send_buf) };
            self.handle_routine_tun_result(tun_result).await;
        }
    }

    fn fill_ip_header(zc_packet: &mut ZCPacket) {
        let len = zc_packet.payload_len() + PEER_MANAGER_HEADER_SIZE;
        let ip_header = &mut zc_packet.mut_wg_tunnel_header().unwrap().ipv4_header;
        ip_header[0] = 0x45;
        ip_header[1] = 0;
        ip_header[2..4].copy_from_slice(&((len + 20) as u16).to_be_bytes());
        ip_header[4..6].copy_from_slice(&0u16.to_be_bytes());
        ip_header[6..8].copy_from_slice(&0u16.to_be_bytes());
        ip_header[8] = 64;
        ip_header[9] = 0;
        ip_header[10..12].copy_from_slice(&0u16.to_be_bytes());
        ip_header[12..16].copy_from_slice(&0u32.to_be_bytes());
        ip_header[16..20].copy_from_slice(&0u32.to_be_bytes());
    }

    fn remove_ip_header<'a>(&self, packet: &'a [u8], is_v4: bool) -> &'a [u8] {
        if is_v4 { &packet[20..] } else { &packet[40..] }
    }
}

struct WgPeer {
    tunn: Option<Mutex<Tunn>>,
    udp: Arc<UdpSocket>, // only for send
    config: WgConfig,
    endpoint: SocketAddr,
    stealth: Option<Arc<WgStealthSession>>,

    sink: std::sync::Mutex<Option<Pin<Box<dyn ZCPacketSink>>>>,

    data: Option<WgPeerData>,
    tasks: JoinSet<()>,

    access_time: AtomicCell<Instant>,
}

impl WgPeer {
    fn new(
        udp: Arc<UdpSocket>,
        config: WgConfig,
        endpoint: SocketAddr,
        stealth: Option<Arc<WgStealthSession>>,
    ) -> Self {
        WgPeer {
            tunn: Some(Mutex::new(Tunn::new(
                config.my_secret_key.clone(),
                config.peer_public_key,
                None,
                None,
                rand::thread_rng().next_u32(),
                None,
            ))),

            udp,
            config,
            endpoint,
            stealth,
            sink: std::sync::Mutex::new(None),

            data: None,
            tasks: JoinSet::new(),

            access_time: AtomicCell::new(Instant::now()),
        }
    }

    async fn handle_packet_from_me<S: ZCPacketStream + Unpin>(mut stream: S, data: WgPeerData) {
        while let Some(Ok(packet)) = stream.next().await {
            let ret = data.handle_one_packet_from_me(packet).await;
            if let Err(e) = ret {
                tracing::error!("Failed to handle packet from me: {}", e);
            }
        }
        data.stopped
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    async fn handle_packet_from_peer(&self, packet: &[u8]) -> bool {
        self.access_time.store(Instant::now());
        tracing::trace!("Received {} bytes from peer", packet.len());
        let data = self.data.as_ref().unwrap();
        // TODO: improve this
        let mut sink = self.sink.lock().unwrap().take().unwrap();
        let accepted = data.handle_one_packet_from_peer(&mut sink, packet).await;
        self.sink.lock().unwrap().replace(sink);
        accepted
    }

    fn start_and_get_tunnel(&mut self) -> Box<dyn Tunnel> {
        let (stunnel, ctunnel) = create_ring_tunnel_pair();

        let (stream, sink) = stunnel.split();

        let data = WgPeerData {
            udp: self.udp.clone(),
            endpoint: self.endpoint,
            tunn: Arc::new(self.tunn.take().unwrap()),
            wg_type: self.config.wg_type.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
            stealth: self.stealth.clone(),
        };

        self.data = Some(data.clone());
        self.sink.lock().unwrap().replace(sink);

        self.tasks
            .spawn(Self::handle_packet_from_me(stream, data.clone()));
        self.tasks.spawn(data.routine_task());

        ctunnel
    }

    fn stopped(&self) -> bool {
        self.data
            .as_ref()
            .unwrap()
            .stopped
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn create_handshake_init(&self) -> Vec<u8> {
        let mut dst = vec![0u8; 2048];
        let handshake_init = self
            .tunn
            .as_ref()
            .unwrap()
            .lock()
            .await
            .format_handshake_initiation(&mut dst, false);
        assert!(matches!(handshake_init, TunnResult::WriteToNetwork(_)));
        let handshake_init = if let TunnResult::WriteToNetwork(sent) = handshake_init {
            sent
        } else {
            unreachable!();
        };

        handshake_init.into()
    }

    fn udp_socket(&self) -> Arc<UdpSocket> {
        self.udp.clone()
    }

    fn open_network_packet(&self, packet: &[u8]) -> Option<Vec<u8>> {
        match &self.stealth {
            Some(stealth) => stealth.open(packet),
            None => Some(packet.to_vec()),
        }
    }

    async fn send_network_packet(&self, packet: &[u8]) -> Result<(), std::io::Error> {
        if let Some(stealth) = &self.stealth {
            let sealed = stealth.seal(packet).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "WG stealth sealing failed")
            })?;
            self.udp.send_to(&sealed, self.endpoint).await?;
        } else {
            self.udp.send_to(packet, self.endpoint).await?;
        }
        Ok(())
    }
}

type ConnSender = tokio::sync::mpsc::UnboundedSender<Box<dyn Tunnel>>;
type ConnReceiver = tokio::sync::mpsc::UnboundedReceiver<Box<dyn Tunnel>>;

pub struct WgTunnelListener {
    addr: url::Url,
    config: WgConfig,

    udp: Option<Arc<UdpSocket>>,
    conn_recv: ConnReceiver,
    conn_send: Option<ConnSender>,

    wg_peer_map: Arc<DashMap<SocketAddr, Arc<WgPeer>>>,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_replay: Arc<WgStealthReplayGuard>,

    tasks: JoinSet<()>,
    socket_mark: Option<u32>,
}

impl WgTunnelListener {
    pub fn new(addr: url::Url, config: WgConfig) -> Self {
        let (conn_send, conn_recv) = hotpath::channel!(tokio::sync::mpsc::unbounded_channel());
        WgTunnelListener {
            addr,
            config,

            udp: None,
            conn_recv,
            conn_send: Some(conn_send),

            wg_peer_map: Arc::new(DashMap::new()),
            stealth: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_replay: Arc::new(WgStealthReplayGuard::default()),

            tasks: JoinSet::new(),
            socket_mark: None,
        }
    }

    pub fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    pub fn set_stealth(&mut self, stealth: Arc<crate::tunnel::stealth::OuterSessionState>) {
        self.stealth = stealth;
    }

    fn get_udp_socket(&self) -> Arc<UdpSocket> {
        self.udp.as_ref().unwrap().clone()
    }

    async fn handle_udp_incoming(
        socket: Arc<UdpSocket>,
        config: WgConfig,
        conn_sender: ConnSender,
        peer_map: Arc<DashMap<SocketAddr, Arc<WgPeer>>>,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
        stealth_replay: Arc<WgStealthReplayGuard>,
    ) {
        let mut tasks = JoinSet::new();

        let peer_map_clone: Arc<DashMap<SocketAddr, Arc<WgPeer>>> = peer_map.clone();
        tasks.spawn(async move {
            loop {
                peer_map_clone.retain(|_, peer| {
                    peer.access_time.load().elapsed().as_secs() < 61 && !peer.stopped()
                });
                shrink_dashmap(&peer_map_clone, None);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        let mut buf = vec![0u8; MAX_PACKET + crate::tunnel::stealth::OUTER_OVERHEAD];
        loop {
            let Ok((n, addr)) = socket.recv_from(&mut buf).await else {
                tracing::error!("Failed to receive from UDP socket");
                break;
            };

            let wire_packet = &buf[..n];
            tracing::trace!(?n, ?addr, "Received bytes from peer");

            let existing = peer_map.get(&addr).map(|peer| peer.clone());
            let mut connection_stealth = None;
            let mut replace_existing = false;
            let plaintext = if let Some(peer) = &existing {
                if peer
                    .stealth
                    .as_ref()
                    .is_some_and(|session| session.state.outer_key().is_some())
                {
                    let candidate = Arc::new(WgStealthSession::new(
                        stealth.fork_for_transport_delayed_transition(),
                    ));
                    if let Some(plaintext) = candidate.open(wire_packet)
                        && is_wg_handshake_initiation(&plaintext)
                        && stealth_replay.accept(&stealth, wire_packet)
                    {
                        connection_stealth = Some(candidate);
                        replace_existing = true;
                        Some(plaintext)
                    } else {
                        peer.open_network_packet(wire_packet)
                    }
                } else {
                    peer.open_network_packet(wire_packet)
                }
            } else if stealth.is_enabled() {
                let candidate = Arc::new(WgStealthSession::new(
                    stealth.fork_for_transport_delayed_transition(),
                ));
                let Some(plaintext) = candidate.open(wire_packet) else {
                    continue;
                };
                if !is_wg_handshake_initiation(&plaintext)
                    || !stealth_replay.accept(&stealth, wire_packet)
                {
                    continue;
                }
                connection_stealth = Some(candidate);
                Some(plaintext)
            } else {
                Some(wire_packet.to_vec())
            };
            let Some(plaintext) = plaintext else {
                continue;
            };

            if existing.is_none() || replace_existing {
                if stealth.is_enabled()
                    && existing.is_none()
                    && peer_map.len() >= WG_STEALTH_MAX_SESSIONS
                {
                    tracing::warn!(
                        max_sessions = WG_STEALTH_MAX_SESSIONS,
                        "drop new WG stealth session because the bounded table is full"
                    );
                    continue;
                }
                tracing::info!("New peer: {}", addr);
                let mut wg = WgPeer::new(
                    socket.clone(),
                    config.clone(),
                    addr,
                    connection_stealth.clone(),
                );
                let (stream, sink) = wg.start_and_get_tunnel().split();
                let tunnel = Box::new(TunnelWrapper::new_with_associate_data(
                    stream,
                    sink,
                    Some(TunnelInfo {
                        tunnel_type: "wg".to_owned(),
                        local_addr: Some(
                            build_url_from_socket_addr(
                                &socket.local_addr().unwrap().to_string(),
                                "wg",
                            )
                            .into(),
                        ),
                        remote_addr: Some(
                            build_url_from_socket_addr(&addr.to_string(), "wg").into(),
                        ),
                        resolved_remote_addr: Some(
                            build_url_from_socket_addr(&addr.to_string(), "wg").into(),
                        ),
                    }),
                    connection_stealth.map(|session| {
                        Box::new(crate::tunnel::stealth::OuterSessionAssociation::new(
                            session.state.clone(),
                            None,
                        )) as Box<dyn std::any::Any + Send>
                    }),
                ));
                if !wg.handle_packet_from_peer(&plaintext).await {
                    continue;
                }
                if let Err(e) = conn_sender.send(tunnel) {
                    tracing::error!("Failed to send tunnel to conn_sender: {}", e);
                }
                peer_map.insert(addr, Arc::new(wg));
                continue;
            }

            let peer = peer_map.get(&addr).unwrap().clone();
            let _ = peer.handle_packet_from_peer(&plaintext).await;
        }
    }
}

#[async_trait]
impl TunnelListener for WgTunnelListener {
    async fn listen(&mut self) -> Result<(), TunnelError> {
        let addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let tunnel_url: TunnelUrl = self.addr.clone().into();
        self.udp = Some(Arc::new(
            bind()
                .addr(addr)
                .only_v6(true)
                .maybe_dev(tunnel_url.bind_dev())
                .maybe_socket_mark(self.socket_mark)
                .call()?,
        ));
        self.addr
            .set_port(Some(self.udp.as_ref().unwrap().local_addr()?.port()))
            .unwrap();

        self.tasks.spawn(Self::handle_udp_incoming(
            self.get_udp_socket(),
            self.config.clone(),
            self.conn_send.take().unwrap(),
            self.wg_peer_map.clone(),
            self.stealth.clone(),
            self.stealth_replay.clone(),
        ));

        Ok(())
    }

    async fn accept(&mut self) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        if let Some(tunnel) = self.conn_recv.recv().await {
            tracing::info!(?tunnel, "Accepted tunnel");
            return Ok(tunnel);
        }
        Err(TunnelError::Shutdown)
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

#[derive(Clone)]
pub struct WgTunnelConnector {
    addr: url::Url,
    config: WgConfig,
    udp: Option<Arc<UdpSocket>>,

    bind_addrs: Vec<SocketAddr>,
    ip_version: IpVersion,
    resolved_addr: Option<SocketAddr>,
    socket_mark: Option<u32>,
    stealth_candidate: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_mode: WgStealthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WgStealthMode {
    Disabled,
    Required,
    PreferLegacyFallback,
}

impl Debug for WgTunnelConnector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgTunnelConnector")
            .field("addr", &self.addr)
            .field("udp", &self.udp)
            .finish()
    }
}

impl WgTunnelConnector {
    pub fn new(addr: url::Url, config: WgConfig) -> Self {
        WgTunnelConnector {
            addr,
            config,
            udp: None,
            bind_addrs: vec![],
            ip_version: IpVersion::Both,
            resolved_addr: None,
            socket_mark: None,
            stealth_candidate: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_mode: WgStealthMode::Disabled,
        }
    }

    pub fn set_stealth_candidate(
        &mut self,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth_mode = if stealth.is_enabled() {
            WgStealthMode::PreferLegacyFallback
        } else {
            WgStealthMode::Disabled
        };
        self.stealth_candidate = stealth;
    }

    #[tracing::instrument(skip(config))]
    async fn connect_with_socket(
        addr_url: url::Url,
        config: WgConfig,
        udp: UdpSocket,
        addr: SocketAddr,
        stealth: Option<Arc<crate::tunnel::stealth::OuterSessionState>>,
    ) -> Result<Box<dyn super::Tunnel>, super::TunnelError> {
        tracing::warn!("wg connect: {:?}", addr);
        let local_addr = udp
            .local_addr()
            .with_context(|| "Failed to get local addr")?
            .to_string();

        let connection_stealth = stealth.map(|template| {
            Arc::new(WgStealthSession::new(
                template.fork_for_transport_delayed_transition(),
            ))
        });
        let mut wg_peer = WgPeer::new(
            Arc::new(udp),
            config.clone(),
            addr,
            connection_stealth.clone(),
        );
        let udp = wg_peer.udp_socket();

        // do handshake here so we will return after receive first packet
        let handshake = wg_peer.create_handshake_init().await;
        wg_peer.send_network_packet(&handshake).await?;
        let mut buf = [0u8; MAX_PACKET + crate::tunnel::stealth::OUTER_OVERHEAD];
        let (n, recv_addr) = match udp.recv_from(&mut buf).await {
            Ok(ret) => ret,
            Err(e) => {
                tracing::error!("Failed to receive handshake response: {}", e);
                return Err(TunnelError::IOError(e));
            }
        };

        if recv_addr != addr {
            tracing::warn!(?recv_addr, "Received packet from changed address");
        }
        let response = wg_peer
            .open_network_packet(&buf[..n])
            .ok_or_else(|| anyhow::anyhow!("failed to open WG stealth handshake response"))?;

        let tunnel = wg_peer.start_and_get_tunnel();
        let data = wg_peer.data.as_ref().unwrap().clone();
        let mut sink = wg_peer.sink.lock().unwrap().take().unwrap();
        wg_peer.tasks.spawn(async move {
            let _ = data.handle_one_packet_from_peer(&mut sink, &response).await;
            loop {
                let mut buf = vec![0u8; MAX_PACKET + crate::tunnel::stealth::OUTER_OVERHEAD];
                let (n, _) = match udp.recv_from(&mut buf).await {
                    Ok(ret) => ret,
                    Err(e) => {
                        tracing::error!("Failed to receive wg packet: {}", e);
                        break;
                    }
                };
                let Some(packet) = data.stealth.as_ref().map_or_else(
                    || Some(buf[..n].to_vec()),
                    |stealth| stealth.open(&buf[..n]),
                ) else {
                    continue;
                };
                let _ = data.handle_one_packet_from_peer(&mut sink, &packet).await;
            }
        });

        let (stream, sink) = tunnel.split();
        let associate_data: Box<dyn std::any::Any + Send> =
            if let Some(session) = connection_stealth {
                Box::new(crate::tunnel::stealth::OuterSessionAssociation::new(
                    session.state.clone(),
                    Some(Box::new(wg_peer)),
                ))
            } else {
                Box::new(wg_peer)
            };
        let ret = Box::new(TunnelWrapper::new_with_associate_data(
            stream,
            sink,
            Some(TunnelInfo {
                tunnel_type: "wg".to_owned(),
                local_addr: Some(super::build_url_from_socket_addr(&local_addr, "wg").into()),
                remote_addr: Some(addr_url.into()),
                resolved_remote_addr: Some(
                    super::build_url_from_socket_addr(&addr.to_string(), "wg").into(),
                ),
            }),
            Some(associate_data),
        ));

        Ok(ret)
    }

    async fn connect_with_ipv6(&self, addr: SocketAddr) -> Result<Box<dyn Tunnel>, TunnelError> {
        let bind_addr = "[::]:0".parse().unwrap();
        let socket = Self::bind_connector_socket(bind_addr, self.socket_mark, true)?;
        Self::connect_with_mode(
            self.addr.clone(),
            self.config.clone(),
            socket,
            bind_addr,
            addr,
            self.socket_mark,
            true,
            self.stealth_mode,
            self.stealth_candidate.clone(),
        )
        .await
    }

    fn bind_connector_socket(
        bind_addr: SocketAddr,
        socket_mark: Option<u32>,
        disable_bind_dev: bool,
    ) -> Result<UdpSocket, TunnelError> {
        let builder = bind()
            .addr(bind_addr)
            .only_v6(true)
            .maybe_socket_mark(socket_mark);
        if disable_bind_dev {
            Ok(builder.dev(BindDev::Disabled).call()?)
        } else {
            Ok(builder.call()?)
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn connect_with_mode(
        addr_url: url::Url,
        config: WgConfig,
        socket: UdpSocket,
        bind_addr: SocketAddr,
        addr: SocketAddr,
        socket_mark: Option<u32>,
        disable_bind_dev: bool,
        mode: WgStealthMode,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<Box<dyn Tunnel>, TunnelError> {
        match mode {
            WgStealthMode::Disabled => {
                Self::connect_with_socket(addr_url, config, socket, addr, None).await
            }
            WgStealthMode::Required => {
                Self::connect_with_socket(addr_url, config, socket, addr, Some(stealth)).await
            }
            WgStealthMode::PreferLegacyFallback => {
                let first = tokio::time::timeout(
                    WG_STEALTH_FALLBACK_TIMEOUT,
                    Self::connect_with_socket(
                        addr_url.clone(),
                        config.clone(),
                        socket,
                        addr,
                        Some(stealth),
                    ),
                )
                .await;
                match first {
                    Ok(Ok(tunnel)) => return Ok(tunnel),
                    result => tracing::info!(
                        ?addr,
                        ?result,
                        "WG stealth attempt failed, retrying legacy wire format"
                    ),
                }
                let socket = Self::bind_connector_socket(bind_addr, socket_mark, disable_bind_dev)?;
                Self::connect_with_socket(addr_url, config, socket, addr, None).await
            }
        }
    }
}

#[async_trait]
impl super::TunnelConnector for WgTunnelConnector {
    #[tracing::instrument]
    async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        let addr = match self.resolved_addr {
            Some(addr) => addr,
            None => SocketAddr::from_url(self.addr.clone(), self.ip_version).await?,
        };

        if addr.is_ipv6() {
            return self.connect_with_ipv6(addr).await;
        }

        let bind_addrs = if self.bind_addrs.is_empty() {
            vec!["0.0.0.0:0".parse().unwrap()]
        } else {
            self.bind_addrs.clone()
        };
        let futures = FuturesUnordered::new();
        for bind_addr in bind_addrs.into_iter() {
            tracing::info!(?bind_addr, ?addr, "bind addr");
            match Self::bind_connector_socket(bind_addr, self.socket_mark, false) {
                Ok(socket) => futures.push(Self::connect_with_mode(
                    self.addr.clone(),
                    self.config.clone(),
                    socket,
                    bind_addr,
                    addr,
                    self.socket_mark,
                    false,
                    self.stealth_mode,
                    self.stealth_candidate.clone(),
                )),
                Err(error) => {
                    tracing::error!(?error, ?bind_addr, ?addr, "bind addr fail");
                    continue;
                }
            }
        }

        wait_for_connect_futures(futures).await
    }

    fn remote_url(&self) -> url::Url {
        self.addr.clone()
    }

    fn set_bind_addrs(&mut self, addrs: Vec<SocketAddr>) {
        self.bind_addrs = addrs;
    }

    fn set_ip_version(&mut self, ip_version: IpVersion) {
        self.ip_version = ip_version;
    }

    fn set_resolved_addr(&mut self, addr: SocketAddr) {
        self.resolved_addr = Some(addr);
    }

    fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    fn disable_stealth(&mut self) {
        self.stealth_mode = WgStealthMode::Disabled;
    }

    fn require_stealth(&mut self) {
        if self.stealth_candidate.is_enabled() {
            self.stealth_mode = WgStealthMode::Required;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::tunnel::{
        TunnelConnector,
        common::tests::{_tunnel_bench, _tunnel_pingpong},
    };
    use boringtun::*;

    pub fn create_wg_config() -> (WgConfig, WgConfig) {
        let my_secret_key = x25519::StaticSecret::random_from_rng(rand::thread_rng());
        let my_public_key = x25519::PublicKey::from(&my_secret_key);

        let their_secret_key = x25519::StaticSecret::random_from_rng(rand::thread_rng());
        let their_public_key = x25519::PublicKey::from(&their_secret_key);

        let server_cfg = WgConfig {
            my_secret_key: my_secret_key.clone(),
            my_public_key,
            peer_secret_key: their_secret_key.clone(),
            peer_public_key: their_public_key,
            wg_type: WgType::InternalUse,
        };

        let client_cfg = WgConfig {
            my_secret_key: their_secret_key,
            my_public_key: their_public_key,
            peer_secret_key: my_secret_key,
            peer_public_key: my_public_key,
            wg_type: WgType::InternalUse,
        };

        (server_cfg, client_cfg)
    }

    fn stealth(secret: &str) -> Arc<crate::tunnel::stealth::OuterSessionState> {
        crate::tunnel::stealth::build_outer_session(Some(secret), true, true, 60)
    }

    #[tokio::test]
    async fn wg_pingpong() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://0.0.0.0:5599".parse().unwrap(), server_cfg);
        let connector = WgTunnelConnector::new("wg://127.0.0.1:5599".parse().unwrap(), client_cfg);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn wg_stealth_pingpong() {
        let (server_cfg, client_cfg) = create_wg_config();
        let mut listener =
            WgTunnelListener::new("wg://127.0.0.1:5594".parse().unwrap(), server_cfg);
        listener.set_stealth(stealth("wg-secret"));
        let mut connector =
            WgTunnelConnector::new("wg://127.0.0.1:5594".parse().unwrap(), client_cfg);
        connector.set_stealth_candidate(stealth("wg-secret"));
        TunnelConnector::require_stealth(&mut connector);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn wg_unknown_capability_falls_back_to_plain() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://127.0.0.1:5593".parse().unwrap(), server_cfg);
        let mut connector =
            WgTunnelConnector::new("wg://127.0.0.1:5593".parse().unwrap(), client_cfg);
        connector.set_stealth_candidate(stealth("wg-secret"));
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn wg_stealth_listener_rejects_plain_and_wrong_secret() {
        for connector_secret in [None, Some("wrong-secret")] {
            let (server_cfg, client_cfg) = create_wg_config();
            let mut listener =
                WgTunnelListener::new("wg://127.0.0.1:0".parse().unwrap(), server_cfg);
            listener.set_stealth(stealth("listener-secret"));
            listener.listen().await.unwrap();

            let mut connector = WgTunnelConnector::new(listener.local_url(), client_cfg);
            if let Some(secret) = connector_secret {
                connector.set_stealth_candidate(stealth(secret));
                TunnelConnector::require_stealth(&mut connector);
            }
            let accept_task = tokio::spawn(async move { listener.accept().await });
            let _ = tokio::time::timeout(Duration::from_millis(300), connector.connect()).await;
            assert!(
                !accept_task.is_finished(),
                "strict WG stealth listener exposed an unauthenticated connection"
            );
            accept_task.abort();
        }
    }

    #[test]
    fn wg_stealth_session_transitions_from_gate_to_outer_key() {
        let sender =
            WgStealthSession::new(stealth("wg-secret").fork_for_transport_delayed_transition());
        let receiver =
            WgStealthSession::new(stealth("wg-secret").fork_for_transport_delayed_transition());

        let gate = sender.seal(b"gate").unwrap();
        assert_eq!(receiver.open(&gate).unwrap(), b"gate");
        sender
            .state
            .set_outer_key_from_handshake_hash(b"wg-handshake");
        receiver
            .state
            .set_outer_key_from_handshake_hash(b"wg-handshake");
        *sender.outer_seen_at.lock().unwrap() =
            Some(StdInstant::now() - WG_STEALTH_OUTER_SEND_DELAY);
        *receiver.outer_seen_at.lock().unwrap() =
            Some(StdInstant::now() - WG_STEALTH_OUTER_SEND_DELAY);

        let outer = sender.seal(b"outer").unwrap();
        assert_eq!(receiver.open(&outer).unwrap(), b"outer");
        *receiver.outer_seen_at.lock().unwrap() =
            Some(StdInstant::now() - WG_STEALTH_GATE_RECV_GRACE - Duration::from_millis(1));
        let stale_gate = sender.state.seal_gate_datagram(b"stale").unwrap();
        assert!(receiver.open(&stale_gate).is_none());
    }

    #[tokio::test]
    async fn wg_bench() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://0.0.0.0:5598".parse().unwrap(), server_cfg);
        let connector = WgTunnelConnector::new("wg://127.0.0.1:5598".parse().unwrap(), client_cfg);
        _tunnel_bench(listener, connector).await
    }

    #[tokio::test]
    async fn wg_bench_with_bind() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://127.0.0.1:5597".parse().unwrap(), server_cfg);
        let mut connector =
            WgTunnelConnector::new("wg://127.0.0.1:5597".parse().unwrap(), client_cfg);
        connector.set_bind_addrs(vec!["127.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    #[should_panic]
    async fn wg_bench_with_bind_fail() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://127.0.0.1:5596".parse().unwrap(), server_cfg);
        let mut connector =
            WgTunnelConnector::new("wg://127.0.0.1:5596".parse().unwrap(), client_cfg);
        connector.set_bind_addrs(vec!["10.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn wg_server_erase_from_map_after_close() {
        let (server_cfg, client_cfg) = create_wg_config();
        let mut listener =
            WgTunnelListener::new("wg://127.0.0.1:5595".parse().unwrap(), server_cfg);
        listener.listen().await.unwrap();

        const CONN_COUNT: usize = 10;

        tokio::spawn(async move {
            let mut tunnels = vec![];
            for _ in 0..CONN_COUNT {
                let mut connector = WgTunnelConnector::new(
                    "wg://127.0.0.1:5595".parse().unwrap(),
                    client_cfg.clone(),
                );
                let ret = connector.connect().await;
                assert!(ret.is_ok());
                let t = ret.unwrap();
                let (_stream, mut sink) = t.split();
                sink.send(ZCPacket::new_with_payload("payload".as_bytes()))
                    .await
                    .unwrap();
                tunnels.push(t);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        });

        for _ in 0..CONN_COUNT {
            println!("accepting");
            let conn = listener.accept().await;
            let (mut stream, _sink) = conn.unwrap().split();
            let packet = stream.next().await.unwrap().unwrap();
            assert_eq!("payload".as_bytes(), packet.payload());
            println!("accepting drop");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        assert_eq!(0, listener.wg_peer_map.len());
    }

    #[tokio::test]
    async fn bind_same_port() {
        let (server_cfg, _client_cfg) = create_wg_config();
        let mut listener = WgTunnelListener::new("wg://[::1]:31015".parse().unwrap(), server_cfg);
        let (server_cfg, _client_cfg) = create_wg_config();
        let mut listener2 = WgTunnelListener::new("wg://[::1]:31015".parse().unwrap(), server_cfg);
        listener.listen().await.unwrap();
        listener2.listen().await.unwrap();
    }

    #[tokio::test]
    async fn ipv6_pingpong() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://[::1]:31015".parse().unwrap(), server_cfg);
        let connector = WgTunnelConnector::new("wg://[::1]:31015".parse().unwrap(), client_cfg);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn ipv6_domain_pingpong() {
        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://[::1]:31016".parse().unwrap(), server_cfg);
        let mut connector =
            WgTunnelConnector::new("wg://test.easytier.top:31016".parse().unwrap(), client_cfg);
        connector.set_ip_version(IpVersion::V6);
        _tunnel_pingpong(listener, connector).await;

        let (server_cfg, client_cfg) = create_wg_config();
        let listener = WgTunnelListener::new("wg://127.0.0.1:31016".parse().unwrap(), server_cfg);
        let mut connector =
            WgTunnelConnector::new("wg://test.easytier.top:31016".parse().unwrap(), client_cfg);
        connector.set_ip_version(IpVersion::V4);
        _tunnel_pingpong(listener, connector).await;
    }

    #[tokio::test]
    async fn test_alloc_port() {
        // v4
        let (server_cfg, _client_cfg) = create_wg_config();
        let mut listener = WgTunnelListener::new("wg://0.0.0.0:0".parse().unwrap(), server_cfg);
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);

        // v6
        let (server_cfg, _client_cfg) = create_wg_config();
        let mut listener = WgTunnelListener::new("wg://[::]:0".parse().unwrap(), server_cfg);
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);
    }
}
