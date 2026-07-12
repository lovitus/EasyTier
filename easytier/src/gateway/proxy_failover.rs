use std::{
    collections::HashSet,
    hash::Hash,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use atomic_shim::AtomicU64;
use dashmap::DashMap;
use pnet::packet::{
    Packet,
    ip::IpNextHeaderProtocols,
    ipv4::Ipv4Packet,
    tcp::{TcpFlags, TcpPacket},
};
use prost::Message;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
    task::AbortHandle,
};

use crate::{
    common::PeerId,
    peers::{NicPacketContext, NicPacketFilter, NicPacketFilterAction, peer_manager::PeerManager},
    proto::{
        api::instance::{
            ListTcpProxyEntryRequest, ListTcpProxyEntryResponse, TcpProxyEntry, TcpProxyEntryState,
            TcpProxyEntryTransportType, TcpProxyRpc,
        },
        peer_rpc::{ProxyPrepareAck, ProxyPrepareAckStatus},
        rpc_types::{self, controller::BaseController},
    },
    tunnel::packet_def::ZCPacket,
};

use super::tcp_proxy::ClaimedNatDstStream;

const PENDING_TTL: Duration = Duration::from_secs(5);
const DECISION_TTL: Duration = Duration::from_secs(30);
const PREPARED_TTL: Duration = Duration::from_secs(5);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(1);
const HALF_OPEN_INTERVAL: Duration = Duration::from_secs(30);
const MAX_PENDING_FLOWS: usize = 4096;
const MAX_STATUS_ENTRIES: usize = 256;
const MAX_HEALTH_ENTRIES: usize = 4096;
const HEALTH_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_ROUTE_RESTARTS: u8 = 2;
const PROXY_PREPARE_ACK_MAX_FRAME: usize = 64;
pub(crate) const PROXY_TARGET_CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
pub(crate) use crate::common::constants::PROXY_PREPARE_ACK_VERSION;

pub(crate) fn requested_proxy_prepare_version(remote_version: u32) -> u32 {
    if remote_version >= PROXY_PREPARE_ACK_VERSION {
        PROXY_PREPARE_ACK_VERSION
    } else {
        0
    }
}

fn retain_latest_statuses(statuses: &mut Vec<TcpProxyEntry>) {
    statuses.sort_unstable_by(|left, right| {
        right
            .start_time
            .cmp(&left.start_time)
            .then_with(|| right.generation.cmp(&left.generation))
    });
    statuses.truncate(MAX_STATUS_ENTRIES);
}

pub(crate) fn normalize_local_proxy_destination(
    destination: SocketAddr,
    send_to_self: bool,
    no_tun: bool,
) -> SocketAddr {
    if !send_to_self || !no_tun {
        return destination;
    }
    let loopback = match destination.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(loopback, destination.port())
}

#[async_trait]
pub trait ProxyStream: AsyncRead + AsyncWrite + Unpin + Send + Sync {
    async fn shutdown_gracefully(&mut self) -> std::io::Result<()>;
}
pub type BoxProxyStream = Box<dyn ProxyStream>;

#[cfg(test)]
#[async_trait]
impl ProxyStream for tokio::io::DuplexStream {
    async fn shutdown_gracefully(&mut self) -> std::io::Result<()> {
        self.shutdown().await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyPrepareFailureClass {
    Transport,
    Destination,
    Policy,
    RouteStale,
    BusinessTimeout,
    AmbiguousTimeout,
}

#[derive(Debug, thiserror::Error)]
#[error("{class:?}: {source}")]
pub struct ProxyPrepareError {
    pub class: ProxyPrepareFailureClass,
    #[source]
    source: anyhow::Error,
}

impl ProxyPrepareError {
    pub fn new(class: ProxyPrepareFailureClass, source: impl Into<anyhow::Error>) -> Self {
        Self {
            class,
            source: source.into(),
        }
    }

    pub fn transport(source: impl Into<anyhow::Error>) -> Self {
        Self::new(ProxyPrepareFailureClass::Transport, source)
    }

    pub fn destination(source: impl Into<anyhow::Error>) -> Self {
        Self::new(ProxyPrepareFailureClass::Destination, source)
    }

    pub fn policy(source: impl Into<anyhow::Error>) -> Self {
        Self::new(ProxyPrepareFailureClass::Policy, source)
    }

    pub fn business_timeout(source: impl Into<anyhow::Error>) -> Self {
        Self::new(ProxyPrepareFailureClass::BusinessTimeout, source)
    }

    pub fn ambiguous_timeout(source: impl Into<anyhow::Error>) -> Self {
        Self::new(ProxyPrepareFailureClass::AmbiguousTimeout, source)
    }
}

pub(crate) async fn write_proxy_prepare_ack(
    stream: &mut (impl AsyncWrite + Unpin + ?Sized),
    status: ProxyPrepareAckStatus,
) -> anyhow::Result<()> {
    let payload = ProxyPrepareAck {
        status: status.into(),
    }
    .encode_to_vec();
    anyhow::ensure!(
        payload.len() <= PROXY_PREPARE_ACK_MAX_FRAME,
        "proxy prepare ACK frame is too large"
    );
    stream.write_u16(payload.len() as u16).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_proxy_prepare_ack(
    stream: &mut (impl AsyncRead + Unpin + ?Sized),
) -> anyhow::Result<ProxyPrepareAckStatus> {
    let len = stream.read_u16().await? as usize;
    anyhow::ensure!(
        len <= PROXY_PREPARE_ACK_MAX_FRAME,
        "proxy prepare ACK frame is too large: {len}"
    );
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let ack = ProxyPrepareAck::decode(payload.as_slice())?;
    ProxyPrepareAckStatus::try_from(ack.status)
        .map_err(|_| anyhow::anyhow!("unknown proxy prepare ACK status: {}", ack.status))
}

pub(crate) async fn await_proxy_prepare_ready(
    mut stream: BoxProxyStream,
    deadline: Instant,
) -> Result<BoxProxyStream, ProxyPrepareError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let first = tokio::time::timeout(remaining, read_proxy_prepare_ack(stream.as_mut()))
        .await
        .map_err(|_| ProxyPrepareError::transport(anyhow::anyhow!("proxy ACCEPTED timed out")))?
        .map_err(ProxyPrepareError::transport)?;
    match first {
        ProxyPrepareAckStatus::Accepted => {}
        ProxyPrepareAckStatus::Ready => return Ok(stream),
        ProxyPrepareAckStatus::DestinationFailed => {
            return Err(ProxyPrepareError::destination(anyhow::anyhow!(
                "proxy destination connection failed"
            )));
        }
        ProxyPrepareAckStatus::PolicyDenied => {
            return Err(ProxyPrepareError::policy(anyhow::anyhow!(
                "proxy destination denied by policy"
            )));
        }
        ProxyPrepareAckStatus::BusinessTimeout => {
            return Err(ProxyPrepareError::business_timeout(anyhow::anyhow!(
                "proxy destination connection timed out"
            )));
        }
        ProxyPrepareAckStatus::Unspecified => {
            return Err(ProxyPrepareError::transport(anyhow::anyhow!(
                "unspecified proxy prepare ACK"
            )));
        }
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ProxyPrepareError::ambiguous_timeout(anyhow::anyhow!(
            "proxy READY timed out after ACCEPTED"
        )));
    }
    let second = tokio::time::timeout(remaining, read_proxy_prepare_ack(stream.as_mut()))
        .await
        .map_err(|_| {
            ProxyPrepareError::ambiguous_timeout(anyhow::anyhow!(
                "proxy READY timed out after ACCEPTED"
            ))
        })?
        .map_err(ProxyPrepareError::transport)?;
    match second {
        ProxyPrepareAckStatus::Ready => Ok(stream),
        ProxyPrepareAckStatus::DestinationFailed => Err(ProxyPrepareError::destination(
            anyhow::anyhow!("proxy destination connection failed"),
        )),
        ProxyPrepareAckStatus::PolicyDenied => Err(ProxyPrepareError::policy(anyhow::anyhow!(
            "proxy destination denied by policy"
        ))),
        ProxyPrepareAckStatus::BusinessTimeout => Err(ProxyPrepareError::business_timeout(
            anyhow::anyhow!("proxy destination connection timed out"),
        )),
        ProxyPrepareAckStatus::Accepted | ProxyPrepareAckStatus::Unspecified => {
            Err(ProxyPrepareError::transport(anyhow::anyhow!(
                "invalid final proxy prepare ACK: {second:?}"
            )))
        }
    }
}

#[async_trait]
trait ProxySelectorRuntime: Send + Sync {
    async fn target_snapshot(&self, dst_ip: Ipv4Addr) -> Option<(PeerId, CapabilitySnapshot)>;

    fn is_local_virtual_ip(&self, dst_ip: Ipv4Addr) -> bool;

    async fn send_after_pipeline(
        &self,
        packet: ZCPacket,
        context: NicPacketContext,
    ) -> anyhow::Result<()>;

    fn my_peer_id(&self) -> PeerId;
}

struct PeerManagerSelectorRuntime {
    peer_manager: Arc<PeerManager>,
}

#[async_trait]
impl ProxySelectorRuntime for PeerManagerSelectorRuntime {
    async fn target_snapshot(&self, dst_ip: Ipv4Addr) -> Option<(PeerId, CapabilitySnapshot)> {
        let dst_peer_id = self
            .peer_manager
            .get_peer_map()
            .get_peer_id_by_ipv4(&dst_ip)
            .await?;
        let info = self
            .peer_manager
            .get_route()
            .get_peer_info(dst_peer_id)
            .await?;
        let feature = info.feature_flag.unwrap_or_default();
        Some((
            dst_peer_id,
            CapabilitySnapshot {
                quic: feature.quic_input,
                kcp: feature.kcp_input,
                prepare_ack_version: feature.proxy_prepare_ack_version,
            },
        ))
    }

    async fn send_after_pipeline(
        &self,
        packet: ZCPacket,
        context: NicPacketContext,
    ) -> anyhow::Result<()> {
        self.peer_manager
            .send_msg_after_nic_pipeline(packet, context)
            .await
            .map_err(Into::into)
    }

    fn is_local_virtual_ip(&self, dst_ip: Ipv4Addr) -> bool {
        self.peer_manager
            .get_global_ctx()
            .is_ip_local_virtual_ip(&IpAddr::V4(dst_ip))
    }

    fn my_peer_id(&self) -> PeerId {
        self.peer_manager.my_peer_id()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyTransport {
    Quic,
    Kcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowKey {
    pub src: SocketAddr,
    pub dst: SocketAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PreparedKey {
    flow: FlowKey,
    initial_sequence: u32,
    generation: u64,
    dst_peer_id: PeerId,
    transport: ProxyTransport,
}

struct PreparedStream {
    stream: BoxProxyStream,
    expires_at: Instant,
}

pub(crate) struct PreparedProxyStream(pub BoxProxyStream);

#[derive(Default)]
pub struct PreparedProxyStore {
    streams: DashMap<PreparedKey, PreparedStream>,
    active: DashMap<(FlowKey, ProxyTransport), PreparedKey>,
}

impl std::fmt::Debug for PreparedProxyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedProxyStore")
            .field("streams", &self.streams.len())
            .field("active", &self.active.len())
            .finish()
    }
}

impl PreparedProxyStore {
    fn cleanup(&self) {
        let now = Instant::now();
        let expired = self
            .streams
            .iter()
            .filter(|entry| entry.expires_at <= now)
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();
        for key in expired {
            self.streams.remove(&key);
            self.active
                .remove_if(&(key.flow, key.transport), |_, active| *active == key);
        }
    }

    fn insert(&self, key: PreparedKey, stream: BoxProxyStream) {
        self.cleanup();
        if let Some(old) = self.active.insert((key.flow, key.transport), key) {
            self.streams.remove(&old);
        }
        self.streams.insert(
            key,
            PreparedStream {
                stream,
                expires_at: Instant::now() + PREPARED_TTL,
            },
        );
    }

    pub(crate) fn claim_for_syn(
        &self,
        flow: FlowKey,
        transport: ProxyTransport,
        initial_sequence: u32,
    ) -> Option<ClaimedNatDstStream> {
        self.cleanup();
        let key = *self.active.get(&(flow, transport))?.value();
        if key.initial_sequence != initial_sequence {
            return None;
        }
        self.active
            .remove_if(&(flow, transport), |_, active| *active == key)?;
        self.streams.remove(&key).map(|(_, prepared)| {
            ClaimedNatDstStream(Box::new(PreparedProxyStream(prepared.stream)))
        })
    }

    fn remove(&self, key: PreparedKey) {
        self.streams.remove(&key);
        self.active
            .remove_if(&(key.flow, key.transport), |_, active| *active == key);
    }
}

#[async_trait]
pub trait ProxyPrepareTransport: Send + Sync {
    fn transport(&self) -> ProxyTransport;

    async fn prepare(
        &self,
        flow: FlowKey,
        dst_peer_id: PeerId,
        prepare_ack_version: u32,
        deadline: Instant,
    ) -> Result<BoxProxyStream, ProxyPrepareError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowDecision {
    Pending,
    Native,
    Proxy(ProxyTransport),
}

#[derive(Debug, Clone, Copy)]
struct CapabilitySnapshot {
    quic: bool,
    kcp: bool,
    prepare_ack_version: u32,
}

struct DeferredFlow {
    initial_sequence: u32,
    generation: u64,
    original_syn: ZCPacket,
    context: NicPacketContext,
    dst_peer_id: PeerId,
    capabilities: CapabilitySnapshot,
    route_restarts: u8,
    decision: FlowDecision,
    fallback_reason: String,
    created_unix: u64,
    created_at: Instant,
    updated_at: Instant,
    prepare_task: Option<AbortHandle>,
}

#[derive(Debug)]
struct TransportHealth {
    consecutive_failures: u8,
    consecutive_successes: u8,
    ambiguous_timeout_strikes: u32,
    degraded: bool,
    last_probe: Option<Instant>,
    updated_at: Instant,
}

impl Default for TransportHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            consecutive_successes: 0,
            ambiguous_timeout_strikes: 0,
            degraded: false,
            last_probe: None,
            updated_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthTransition {
    None,
    Degraded,
    Recovered,
}

impl TransportHealth {
    fn allows_attempt(&mut self, now: Instant) -> bool {
        self.updated_at = now;
        if !self.degraded {
            return true;
        }
        if self
            .last_probe
            .is_none_or(|last| now.duration_since(last) >= HALF_OPEN_INTERVAL)
        {
            self.last_probe = Some(now);
            true
        } else {
            false
        }
    }

    fn record_result(&mut self, now: Instant, success: bool) -> HealthTransition {
        self.updated_at = now;
        if success {
            self.consecutive_failures = 0;
            self.ambiguous_timeout_strikes = 0;
            if self.degraded {
                self.consecutive_successes = self.consecutive_successes.saturating_add(1);
                if self.consecutive_successes >= 3 {
                    self.degraded = false;
                    self.consecutive_successes = 0;
                    self.last_probe = None;
                    return HealthTransition::Recovered;
                }
            }
            return HealthTransition::None;
        }

        self.ambiguous_timeout_strikes = 0;
        self.record_hard_failure(now)
    }

    fn record_hard_failure(&mut self, now: Instant) -> HealthTransition {
        self.updated_at = now;
        self.consecutive_successes = 0;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= 3 && !self.degraded {
            self.degraded = true;
            self.last_probe = Some(now);
            return HealthTransition::Degraded;
        }
        HealthTransition::None
    }

    fn record_ambiguous_timeout(&mut self, now: Instant) -> HealthTransition {
        self.updated_at = now;
        self.consecutive_successes = 0;
        self.ambiguous_timeout_strikes = self.ambiguous_timeout_strikes.saturating_add(1);
        if self.ambiguous_timeout_strikes < 2 {
            return HealthTransition::None;
        }
        self.ambiguous_timeout_strikes = 0;
        self.record_hard_failure(now)
    }

    fn clear_ambiguous_timeout(&mut self, now: Instant) {
        self.updated_at = now;
        self.ambiguous_timeout_strikes = 0;
    }
}

type TransportHealthMap = DashMap<(PeerId, ProxyTransport), Arc<Mutex<TransportHealth>>>;

#[derive(Clone)]
pub struct DeferredProxySelector {
    runtime: Arc<dyn ProxySelectorRuntime>,
    transports: Arc<Vec<Arc<dyn ProxyPrepareTransport>>>,
    prepared: Arc<PreparedProxyStore>,
    flows: Arc<DashMap<FlowKey, Arc<Mutex<DeferredFlow>>>>,
    health: Arc<TransportHealthMap>,
    next_generation: Arc<AtomicU64>,
}

impl DeferredProxySelector {
    pub fn new(
        peer_manager: Arc<PeerManager>,
        transports: Vec<Arc<dyn ProxyPrepareTransport>>,
        prepared: Arc<PreparedProxyStore>,
    ) -> Self {
        Self::new_with_runtime(
            Arc::new(PeerManagerSelectorRuntime { peer_manager }),
            transports,
            prepared,
        )
    }

    fn new_with_runtime(
        runtime: Arc<dyn ProxySelectorRuntime>,
        transports: Vec<Arc<dyn ProxyPrepareTransport>>,
        prepared: Arc<PreparedProxyStore>,
    ) -> Self {
        Self {
            runtime,
            transports: Arc::new(transports),
            prepared,
            flows: Arc::new(DashMap::new()),
            health: Arc::new(DashMap::new()),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    fn parse_syn(packet: &ZCPacket) -> Option<(FlowKey, u32)> {
        let ip = Ipv4Packet::new(packet.payload())?;
        if ip.get_version() != 4 || ip.get_next_level_protocol() != IpNextHeaderProtocols::Tcp {
            return None;
        }
        let tcp = TcpPacket::new(ip.payload())?;
        if tcp.get_flags() & TcpFlags::SYN == 0 || tcp.get_flags() & TcpFlags::ACK != 0 {
            return None;
        }
        Some((
            FlowKey {
                src: SocketAddr::V4(SocketAddrV4::new(ip.get_source(), tcp.get_source())),
                dst: SocketAddr::V4(SocketAddrV4::new(
                    ip.get_destination(),
                    tcp.get_destination(),
                )),
            },
            tcp.get_sequence(),
        ))
    }

    async fn target_snapshot(&self, dst_ip: Ipv4Addr) -> Option<(PeerId, CapabilitySnapshot)> {
        self.runtime.target_snapshot(dst_ip).await
    }

    fn available_transports(
        &self,
        capabilities: CapabilitySnapshot,
    ) -> Vec<Arc<dyn ProxyPrepareTransport>> {
        let mut seen = HashSet::new();
        let mut transports = self
            .transports
            .iter()
            .filter(|transport| match transport.transport() {
                ProxyTransport::Quic => capabilities.quic,
                ProxyTransport::Kcp => capabilities.kcp,
            })
            .filter(|transport| seen.insert(transport.transport()))
            .cloned()
            .collect::<Vec<_>>();
        transports.sort_by_key(|transport| match transport.transport() {
            ProxyTransport::Quic => 0,
            ProxyTransport::Kcp => 1,
        });
        transports
    }

    async fn health_allows(&self, peer: PeerId, transport: ProxyTransport) -> bool {
        let health = self.health_entry(peer, transport).await;
        health.lock().await.allows_attempt(Instant::now())
    }

    async fn record_prepare_result(&self, peer: PeerId, transport: ProxyTransport, success: bool) {
        let health = self.health_entry(peer, transport).await;
        match health.lock().await.record_result(Instant::now(), success) {
            HealthTransition::Degraded => {
                tracing::warn!(?peer, ?transport, "proxy transport degraded");
            }
            HealthTransition::Recovered => {
                tracing::info!(?peer, ?transport, "proxy transport recovered");
            }
            HealthTransition::None => {}
        }
    }

    async fn record_ambiguous_timeout(&self, peer: PeerId, transport: ProxyTransport) {
        let health = self.health_entry(peer, transport).await;
        match health.lock().await.record_ambiguous_timeout(Instant::now()) {
            HealthTransition::Degraded => {
                tracing::warn!(?peer, ?transport, "proxy transport degraded");
            }
            HealthTransition::Recovered => {}
            HealthTransition::None => {
                tracing::warn!(
                    ?peer,
                    ?transport,
                    "proxy transport READY timed out ambiguously"
                );
            }
        }
    }

    async fn clear_ambiguous_timeout(&self, peer: PeerId, transport: ProxyTransport) {
        let Some(health) = self
            .health
            .get(&(peer, transport))
            .map(|entry| entry.clone())
        else {
            return;
        };
        health.lock().await.clear_ambiguous_timeout(Instant::now());
    }

    async fn health_entry(
        &self,
        peer: PeerId,
        transport: ProxyTransport,
    ) -> Arc<Mutex<TransportHealth>> {
        if let Some(health) = self.health.get(&(peer, transport)) {
            return health.clone();
        }
        self.cleanup_health().await;
        self.health
            .entry((peer, transport))
            .or_insert_with(|| Arc::new(Mutex::new(TransportHealth::default())))
            .clone()
    }

    async fn cleanup_health(&self) {
        let entries = self
            .health
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entries.len());
        for (key, health) in entries {
            let updated_at = health.lock().await.updated_at;
            snapshots.push((key, health, updated_at));
        }
        let now = Instant::now();
        for (key, health, updated_at) in &snapshots {
            if now.saturating_duration_since(*updated_at) >= HEALTH_TTL
                && now.saturating_duration_since(health.lock().await.updated_at) >= HEALTH_TTL
            {
                self.health
                    .remove_if(key, |_, current| Arc::ptr_eq(current, health));
            }
        }
        if self.health.len() < MAX_HEALTH_ENTRIES {
            return;
        }
        snapshots.sort_by_key(|(_, _, updated_at)| *updated_at);
        let remove_count = self.health.len().saturating_sub(MAX_HEALTH_ENTRIES - 1);
        for (key, health, _) in snapshots.into_iter().take(remove_count) {
            self.health
                .remove_if(&key, |_, current| Arc::ptr_eq(current, &health));
        }
    }

    fn mark_proxy_packet(&self, packet: &mut ZCPacket, transport: ProxyTransport) {
        let header = packet.mut_peer_manager_header().unwrap();
        header.from_peer_id.set(self.runtime.my_peer_id());
        header.to_peer_id.set(self.runtime.my_peer_id());
        match transport {
            ProxyTransport::Quic => {
                header.mark_quic_src_modified();
            }
            ProxyTransport::Kcp => {
                header.mark_kcp_src_modified();
            }
        }
        header.set_deferred_proxy(true);
    }

    fn flow_is_current(&self, flow: FlowKey, entry: &Arc<Mutex<DeferredFlow>>) -> bool {
        self.flows
            .get(&flow)
            .is_some_and(|current| Arc::ptr_eq(current.value(), entry))
    }

    async fn inject_native(
        &self,
        flow: FlowKey,
        entry: &Arc<Mutex<DeferredFlow>>,
        reason: impl Into<String>,
    ) {
        if !self.flow_is_current(flow, entry) {
            return;
        }
        let reason = reason.into();
        let (packet, context, dst_peer_id, generation) = {
            let mut entry = entry.lock().await;
            if entry.decision == FlowDecision::Native {
                return;
            }
            entry.decision = FlowDecision::Native;
            entry.fallback_reason = reason.clone();
            entry.updated_at = Instant::now();
            (
                entry.original_syn.clone(),
                entry.context,
                entry.dst_peer_id,
                entry.generation,
            )
        };
        tracing::info!(
            ?flow,
            dst_peer_id,
            generation,
            fallback_reason = %reason,
            "proxy selector chose native transport"
        );
        if let Err(error) = self.runtime.send_after_pipeline(packet, context).await {
            tracing::warn!(?error, "failed to inject native fallback SYN");
        }
    }

    async fn inject_native_for_route(
        &self,
        flow: FlowKey,
        entry: &Arc<Mutex<DeferredFlow>>,
        peer: PeerId,
        capabilities: CapabilitySnapshot,
        reason: &'static str,
    ) {
        if !self.flow_is_current(flow, entry) {
            return;
        }
        {
            let mut entry = entry.lock().await;
            if entry.decision != FlowDecision::Pending {
                return;
            }
            entry.dst_peer_id = peer;
            entry.capabilities = capabilities;
        }
        self.inject_native(flow, entry, reason).await;
    }

    async fn resolve_pending(self, flow: FlowKey, entry: Arc<Mutex<DeferredFlow>>) {
        loop {
            let (generation, peer, capabilities, restarts, created) = {
                let entry = entry.lock().await;
                (
                    entry.generation,
                    entry.dst_peer_id,
                    entry.capabilities,
                    entry.route_restarts,
                    entry.created_at,
                )
            };
            if created.elapsed() >= PENDING_TTL {
                self.inject_native(flow, &entry, "pending_timeout").await;
                return;
            }

            let transports = self.available_transports(capabilities);
            let mut prepared_result = None;
            let mut route_change = None;
            let mut fallback_reasons = Vec::new();
            for transport in transports {
                let kind = transport.transport();
                if !self.health_allows(peer, kind).await {
                    fallback_reasons.push(match kind {
                        ProxyTransport::Quic => "quic_degraded",
                        ProxyTransport::Kcp => "kcp_degraded",
                    });
                    continue;
                }
                let remaining = PENDING_TTL.saturating_sub(created.elapsed());
                if remaining.is_zero() {
                    self.inject_native(flow, &entry, "pending_timeout").await;
                    return;
                }
                let candidate_budget = PREPARE_TIMEOUT.min(remaining);
                let deadline = Instant::now() + candidate_budget;
                let result = tokio::time::timeout(
                    candidate_budget + Duration::from_millis(10),
                    transport.prepare(flow, peer, capabilities.prepare_ack_version, deadline),
                )
                .await;
                match result {
                    Ok(Ok(stream)) => {
                        prepared_result = Some((kind, stream));
                        break;
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(?peer, ?kind, ?error, "proxy prepare failed");
                        let current = match flow.dst {
                            SocketAddr::V4(dst) => self.target_snapshot(*dst.ip()).await,
                            SocketAddr::V6(_) => None,
                        };
                        if current.as_ref().map(|(id, _)| *id) != Some(peer) {
                            route_change = Some(current);
                            break;
                        }
                        match error.class {
                            ProxyPrepareFailureClass::Transport => {
                                self.record_prepare_result(peer, kind, false).await;
                            }
                            ProxyPrepareFailureClass::AmbiguousTimeout => {
                                self.record_ambiguous_timeout(peer, kind).await;
                            }
                            ProxyPrepareFailureClass::Destination
                            | ProxyPrepareFailureClass::Policy
                            | ProxyPrepareFailureClass::BusinessTimeout => {
                                self.clear_ambiguous_timeout(peer, kind).await;
                            }
                            ProxyPrepareFailureClass::RouteStale => {}
                        }
                        fallback_reasons.push(match kind {
                            ProxyTransport::Quic => match error.class {
                                ProxyPrepareFailureClass::Transport => "quic_prepare_failed",
                                ProxyPrepareFailureClass::Destination => "quic_destination_failed",
                                ProxyPrepareFailureClass::Policy => "quic_policy_denied",
                                ProxyPrepareFailureClass::RouteStale => "quic_route_stale",
                                ProxyPrepareFailureClass::BusinessTimeout => {
                                    "quic_business_timeout"
                                }
                                ProxyPrepareFailureClass::AmbiguousTimeout => {
                                    "quic_ambiguous_timeout"
                                }
                            },
                            ProxyTransport::Kcp => match error.class {
                                ProxyPrepareFailureClass::Transport => "kcp_prepare_failed",
                                ProxyPrepareFailureClass::Destination => "kcp_destination_failed",
                                ProxyPrepareFailureClass::Policy => "kcp_policy_denied",
                                ProxyPrepareFailureClass::RouteStale => "kcp_route_stale",
                                ProxyPrepareFailureClass::BusinessTimeout => "kcp_business_timeout",
                                ProxyPrepareFailureClass::AmbiguousTimeout => {
                                    "kcp_ambiguous_timeout"
                                }
                            },
                        });
                    }
                    Err(_) => {
                        if created.elapsed() >= PENDING_TTL {
                            self.inject_native(flow, &entry, "pending_timeout").await;
                            return;
                        }
                        tracing::warn!(?peer, ?kind, "proxy prepare timed out");
                        let current = match flow.dst {
                            SocketAddr::V4(dst) => self.target_snapshot(*dst.ip()).await,
                            SocketAddr::V6(_) => None,
                        };
                        if current.as_ref().map(|(id, _)| *id) != Some(peer) {
                            route_change = Some(current);
                            break;
                        }
                        self.record_prepare_result(peer, kind, false).await;
                        fallback_reasons.push(match kind {
                            ProxyTransport::Quic => "quic_prepare_timeout",
                            ProxyTransport::Kcp => "kcp_prepare_timeout",
                        });
                    }
                }
            }

            if let Some(current) = route_change {
                let Some((current_peer, current_capabilities)) = current else {
                    self.inject_native(flow, &entry, "route_missing").await;
                    return;
                };
                if restarts >= MAX_ROUTE_RESTARTS {
                    self.inject_native_for_route(
                        flow,
                        &entry,
                        current_peer,
                        current_capabilities,
                        "route_restart_limit",
                    )
                    .await;
                    return;
                }
                let mut entry = entry.lock().await;
                if entry.generation != generation || entry.decision != FlowDecision::Pending {
                    return;
                }
                entry.generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                entry.dst_peer_id = current_peer;
                entry.capabilities = current_capabilities;
                entry.route_restarts += 1;
                entry.updated_at = Instant::now();
                continue;
            }

            let Some((transport, stream)) = prepared_result else {
                let reason = if fallback_reasons.is_empty() {
                    "all_proxy_failed".to_string()
                } else {
                    fallback_reasons.join(",")
                };
                self.inject_native(flow, &entry, reason).await;
                return;
            };
            if created.elapsed() >= PENDING_TTL {
                drop(stream);
                self.inject_native(flow, &entry, "pending_timeout").await;
                return;
            }

            let current = match flow.dst {
                SocketAddr::V4(dst) => self.target_snapshot(*dst.ip()).await,
                SocketAddr::V6(_) => None,
            };
            let Some((current_peer, current_capabilities)) = current else {
                drop(stream);
                self.inject_native(flow, &entry, "route_missing").await;
                return;
            };
            if current_peer != peer {
                drop(stream);
                if restarts >= MAX_ROUTE_RESTARTS {
                    self.inject_native_for_route(
                        flow,
                        &entry,
                        current_peer,
                        current_capabilities,
                        "route_restart_limit",
                    )
                    .await;
                    return;
                }
                let mut entry = entry.lock().await;
                if entry.generation != generation || entry.decision != FlowDecision::Pending {
                    return;
                }
                entry.generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                entry.dst_peer_id = current_peer;
                entry.capabilities = current_capabilities;
                entry.route_restarts += 1;
                entry.updated_at = Instant::now();
                continue;
            }
            let key = PreparedKey {
                flow,
                initial_sequence: {
                    let entry = entry.lock().await;
                    entry.initial_sequence
                },
                generation,
                dst_peer_id: peer,
                transport,
            };
            let (mut packet, context) = {
                if !self.flow_is_current(flow, &entry) {
                    drop(stream);
                    return;
                }
                let mut entry = entry.lock().await;
                if entry.generation != generation
                    || entry.dst_peer_id != peer
                    || entry.decision != FlowDecision::Pending
                {
                    drop(stream);
                    return;
                }
                self.prepared.insert(key, stream);
                entry.decision = FlowDecision::Proxy(transport);
                entry.fallback_reason = fallback_reasons.join(",");
                entry.updated_at = Instant::now();
                (entry.original_syn.clone(), entry.context)
            };
            self.record_prepare_result(peer, transport, true).await;
            tracing::info!(
                ?flow,
                ?transport,
                dst_peer_id = peer,
                generation,
                fallback_reason = %fallback_reasons.join(","),
                "proxy selector chose wrapped transport"
            );
            self.mark_proxy_packet(&mut packet, transport);
            if let Err(error) = self.runtime.send_after_pipeline(packet, context).await {
                tracing::warn!(?error, ?transport, "failed to inject prepared proxy SYN");
                self.prepared.remove(key);
                self.inject_native(flow, &entry, "proxy_dispatch_failed")
                    .await;
            }
            return;
        }
    }

    async fn cleanup_flows(&self) {
        let entries = self
            .flows
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<Vec<_>>();
        for (key, entry) in entries {
            let (pending_expired, decision_expired) = {
                let entry = entry.lock().await;
                (
                    entry.decision == FlowDecision::Pending
                        && entry.created_at.elapsed() >= PENDING_TTL,
                    entry.decision != FlowDecision::Pending
                        && entry.updated_at.elapsed() >= DECISION_TTL,
                )
            };
            if pending_expired {
                if let Some(task) = entry.lock().await.prepare_task.take() {
                    task.abort();
                }
                self.inject_native(key, &entry, "pending_timeout").await;
                continue;
            }
            if decision_expired
                && let Some((_, removed)) = self
                    .flows
                    .remove_if(&key, |_, current| Arc::ptr_eq(current, &entry))
            {
                let mut removed = removed.lock().await;
                if let FlowDecision::Proxy(transport) = removed.decision {
                    self.prepared.remove(PreparedKey {
                        flow: key,
                        initial_sequence: removed.initial_sequence,
                        generation: removed.generation,
                        dst_peer_id: removed.dst_peer_id,
                        transport,
                    });
                }
                if let Some(task) = removed.prepare_task.take() {
                    task.abort();
                }
            }
        }
        self.prepared.cleanup();
        self.cleanup_health().await;
    }

    pub async fn list_statuses(&self) -> Vec<TcpProxyEntry> {
        self.cleanup_flows().await;
        let entries = self
            .flows
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<Vec<_>>();
        let mut statuses = Vec::with_capacity(entries.len());
        for (flow, entry) in entries {
            let (
                selected,
                transport_type,
                state,
                created_unix,
                fallback_reason,
                dst_peer_id,
                generation,
                health_transport,
                capabilities,
            ) = {
                let entry = entry.lock().await;
                let (selected, transport_type, state) = match entry.decision {
                    FlowDecision::Pending => (
                        "pending",
                        TcpProxyEntryTransportType::Tcp,
                        TcpProxyEntryState::SynReceived,
                    ),
                    FlowDecision::Native => (
                        "native",
                        TcpProxyEntryTransportType::Tcp,
                        TcpProxyEntryState::Connected,
                    ),
                    FlowDecision::Proxy(ProxyTransport::Quic) => (
                        "quic",
                        TcpProxyEntryTransportType::Quic,
                        TcpProxyEntryState::Connected,
                    ),
                    FlowDecision::Proxy(ProxyTransport::Kcp) => (
                        "kcp",
                        TcpProxyEntryTransportType::Kcp,
                        TcpProxyEntryState::Connected,
                    ),
                };
                (
                    selected,
                    transport_type,
                    state,
                    entry.created_unix,
                    entry.fallback_reason.clone(),
                    entry.dst_peer_id,
                    entry.generation,
                    match entry.decision {
                        FlowDecision::Proxy(transport) => Some(transport),
                        _ => None,
                    },
                    entry.capabilities,
                )
            };
            let available = self.available_transports(capabilities);
            let mut requested = available
                .iter()
                .map(|transport| match transport.transport() {
                    ProxyTransport::Quic => "quic",
                    ProxyTransport::Kcp => "kcp",
                })
                .collect::<Vec<_>>();
            requested.push("native");
            let mut health_snapshots = Vec::new();
            for transport in available {
                let kind = transport.transport();
                if let Some(health) = self
                    .health
                    .get(&(dst_peer_id, kind))
                    .map(|health| health.clone())
                {
                    let health = health.lock().await;
                    health_snapshots.push((
                        kind,
                        health.degraded,
                        health.consecutive_failures,
                        health.consecutive_successes,
                        health.ambiguous_timeout_strikes,
                    ));
                }
            }
            let health = health_transport
                .and_then(|selected| {
                    health_snapshots
                        .iter()
                        .find(|(transport, ..)| *transport == selected)
                })
                .or_else(|| {
                    health_snapshots
                        .iter()
                        .max_by_key(|(_, degraded, failures, _, ambiguous)| {
                            (*degraded, *failures, *ambiguous)
                        })
                });
            let (degraded, failures, successes, ambiguous_timeout_strikes) = health
                .map(|(_, degraded, failures, successes, ambiguous)| {
                    (
                        *degraded,
                        u32::from(*failures),
                        u32::from(*successes),
                        *ambiguous,
                    )
                })
                .unwrap_or((false, 0, 0, 0));
            statuses.push(TcpProxyEntry {
                src: Some(flow.src.into()),
                dst: Some(flow.dst.into()),
                start_time: created_unix,
                state: state.into(),
                transport_type: transport_type.into(),
                requested_transport: requested.join(","),
                selected_transport: selected.to_string(),
                fallback_reason,
                dst_peer_id,
                transport_degraded: degraded,
                consecutive_failures: failures,
                consecutive_successes: successes,
                generation,
                ambiguous_timeout_strikes,
            });
        }
        retain_latest_statuses(&mut statuses);
        statuses
    }
}

#[derive(Clone)]
pub struct ProxyFailoverRpcService {
    selector: DeferredProxySelector,
}

impl ProxyFailoverRpcService {
    pub fn new(selector: DeferredProxySelector) -> Self {
        Self { selector }
    }
}

#[async_trait]
impl TcpProxyRpc for ProxyFailoverRpcService {
    type Controller = BaseController;

    async fn list_tcp_proxy_entry(
        &self,
        _: BaseController,
        _: ListTcpProxyEntryRequest,
    ) -> Result<ListTcpProxyEntryResponse, rpc_types::error::Error> {
        Ok(ListTcpProxyEntryResponse {
            entries: self.selector.list_statuses().await,
        })
    }
}

#[async_trait]
impl NicPacketFilter for DeferredProxySelector {
    async fn try_process_packet_from_nic(
        &self,
        packet: &mut ZCPacket,
        context: &NicPacketContext,
    ) -> NicPacketFilterAction {
        if packet.bypass_proxy_interception() {
            return NicPacketFilterAction::Continue;
        }
        let Some((flow, sequence)) = Self::parse_syn(packet) else {
            return NicPacketFilterAction::Continue;
        };

        let SocketAddr::V4(dst) = flow.dst else {
            return NicPacketFilterAction::Continue;
        };
        if self.runtime.is_local_virtual_ip(*dst.ip()) {
            return NicPacketFilterAction::Continue;
        }

        self.cleanup_flows().await;

        if let Some(existing_entry) = self.flows.get(&flow).map(|entry| entry.clone()) {
            let mut existing = existing_entry.lock().await;
            if existing.initial_sequence == sequence {
                return match existing.decision {
                    FlowDecision::Pending => NicPacketFilterAction::Consume,
                    FlowDecision::Native => NicPacketFilterAction::StopAndSend,
                    FlowDecision::Proxy(transport) => {
                        self.mark_proxy_packet(packet, transport);
                        NicPacketFilterAction::StopAndSend
                    }
                };
            }
            if let Some(task) = existing.prepare_task.take() {
                task.abort();
            }
            if let FlowDecision::Proxy(transport) = existing.decision {
                self.prepared.remove(PreparedKey {
                    flow,
                    initial_sequence: existing.initial_sequence,
                    generation: existing.generation,
                    dst_peer_id: existing.dst_peer_id,
                    transport,
                });
            }
            self.flows
                .remove_if(&flow, |_, current| Arc::ptr_eq(current, &existing_entry));
            drop(existing);
        }

        if self.flows.len() >= MAX_PENDING_FLOWS {
            tracing::warn!(?flow, "proxy selector pending table is full; using native");
            return NicPacketFilterAction::StopAndSend;
        }
        let Some((dst_peer_id, capabilities)) = self.target_snapshot(*dst.ip()).await else {
            return NicPacketFilterAction::Continue;
        };
        if self.available_transports(capabilities).is_empty() {
            return NicPacketFilterAction::Continue;
        }

        let entry = Arc::new(Mutex::new(DeferredFlow {
            initial_sequence: sequence,
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            original_syn: packet.clone(),
            context: *context,
            dst_peer_id,
            capabilities,
            route_restarts: 0,
            decision: FlowDecision::Pending,
            fallback_reason: String::new(),
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            created_at: Instant::now(),
            updated_at: Instant::now(),
            prepare_task: None,
        }));
        self.flows.insert(flow, entry.clone());
        let task = tokio::spawn(self.clone().resolve_pending(flow, entry.clone()));
        let abort_handle = task.abort_handle();
        if self.flow_is_current(flow, &entry) {
            entry.lock().await.prepare_task = Some(abort_handle);
        } else {
            abort_handle.abort();
        }
        NicPacketFilterAction::Consume
    }

    fn priority(&self) -> i16 {
        100
    }

    fn id(&self) -> String {
        "deferred-proxy-selector".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, net::IpAddr, sync::atomic::AtomicUsize};

    use pnet::packet::{MutablePacket, ipv4::MutableIpv4Packet, tcp::MutableTcpPacket};
    use tokio::sync::Notify;

    use super::*;

    fn ack_frame(status: ProxyPrepareAckStatus) -> Vec<u8> {
        let payload = ProxyPrepareAck {
            status: status.into(),
        }
        .encode_to_vec();
        let mut frame = Vec::with_capacity(2 + payload.len());
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn proxy_prepare_version_negotiates_v1_with_newer_remote() {
        assert_eq!(requested_proxy_prepare_version(0), 0);
        assert_eq!(requested_proxy_prepare_version(1), 1);
        assert_eq!(requested_proxy_prepare_version(2), 1);
    }

    #[test]
    fn proxy_status_output_is_bounded_and_newest_first() {
        let mut statuses = (0..MAX_STATUS_ENTRIES + 10)
            .map(|generation| TcpProxyEntry {
                start_time: (generation / 2) as u64,
                generation: generation as u64,
                ..Default::default()
            })
            .collect::<Vec<_>>();

        retain_latest_statuses(&mut statuses);

        assert_eq!(statuses.len(), MAX_STATUS_ENTRIES);
        assert_eq!(statuses[0].generation, (MAX_STATUS_ENTRIES + 9) as u64);
        assert!(statuses.windows(2).all(|pair| {
            pair[0].start_time > pair[1].start_time
                || (pair[0].start_time == pair[1].start_time
                    && pair[0].generation >= pair[1].generation)
        }));
    }

    #[test]
    fn local_proxy_destination_only_uses_family_loopback_without_tun() {
        let ipv4 = "10.44.0.3:443".parse().unwrap();
        let ipv6 = "[fd00::3]:443".parse().unwrap();

        assert_eq!(normalize_local_proxy_destination(ipv4, false, true), ipv4);
        assert_eq!(normalize_local_proxy_destination(ipv4, true, false), ipv4);
        assert_eq!(
            normalize_local_proxy_destination(ipv4, true, true),
            "127.0.0.1:443".parse().unwrap()
        );
        assert_eq!(
            normalize_local_proxy_destination(ipv6, true, true),
            "[::1]:443".parse().unwrap()
        );
    }

    #[tokio::test]
    async fn ready_ack_preserves_immediately_following_payload() {
        let (client, mut server) = tokio::io::duplex(256);
        let payload = b"server-first-payload";
        let mut wire = ack_frame(ProxyPrepareAckStatus::Accepted);
        wire.extend_from_slice(&ack_frame(ProxyPrepareAckStatus::Ready));
        wire.extend_from_slice(payload);
        tokio::spawn(async move {
            server.write_all(&wire).await.unwrap();
        });

        let mut stream =
            await_proxy_prepare_ready(Box::new(client), Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
        let mut received = vec![0u8; payload.len()];
        stream.read_exact(&mut received).await.unwrap();
        assert_eq!(received, payload);
    }

    #[tokio::test]
    async fn timeout_after_accepted_is_ambiguous() {
        let (client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            server
                .write_all(&ack_frame(ProxyPrepareAckStatus::Accepted))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let error = match await_proxy_prepare_ready(
            Box::new(client),
            Instant::now() + Duration::from_millis(20),
        )
        .await
        {
            Ok(_) => panic!("missing READY unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.class, ProxyPrepareFailureClass::AmbiguousTimeout);
    }

    #[tokio::test]
    async fn timeout_before_accepted_is_transport_failure() {
        let (client, server) = tokio::io::duplex(64);
        let _keep_open = server;
        let error = match await_proxy_prepare_ready(
            Box::new(client),
            Instant::now() + Duration::from_millis(20),
        )
        .await
        {
            Ok(_) => panic!("missing ACCEPTED unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.class, ProxyPrepareFailureClass::Transport);
    }

    #[tokio::test]
    async fn explicit_prepare_failures_keep_their_health_classification() {
        for (status, expected) in [
            (
                ProxyPrepareAckStatus::DestinationFailed,
                ProxyPrepareFailureClass::Destination,
            ),
            (
                ProxyPrepareAckStatus::PolicyDenied,
                ProxyPrepareFailureClass::Policy,
            ),
            (
                ProxyPrepareAckStatus::BusinessTimeout,
                ProxyPrepareFailureClass::BusinessTimeout,
            ),
        ] {
            let (client, mut server) = tokio::io::duplex(64);
            tokio::spawn(async move {
                server
                    .write_all(&ack_frame(ProxyPrepareAckStatus::Accepted))
                    .await
                    .unwrap();
                server.write_all(&ack_frame(status)).await.unwrap();
            });

            let error = match await_proxy_prepare_ready(
                Box::new(client),
                Instant::now() + Duration::from_secs(1),
            )
            .await
            {
                Ok(_) => panic!("explicit proxy failure unexpectedly succeeded"),
                Err(error) => error,
            };
            assert_eq!(error.class, expected);
        }
    }

    #[tokio::test]
    async fn eof_after_accepted_is_transport_failure() {
        let (client, mut server) = tokio::io::duplex(64);
        server
            .write_all(&ack_frame(ProxyPrepareAckStatus::Accepted))
            .await
            .unwrap();
        drop(server);

        let error = match await_proxy_prepare_ready(
            Box::new(client),
            Instant::now() + Duration::from_secs(1),
        )
        .await
        {
            Ok(_) => panic!("EOF after ACCEPTED unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.class, ProxyPrepareFailureClass::Transport);
    }

    #[test]
    fn ambiguous_timeouts_are_scoped_soft_strikes() {
        let now = Instant::now();
        let mut health = TransportHealth::default();
        assert_eq!(health.record_ambiguous_timeout(now), HealthTransition::None);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.ambiguous_timeout_strikes, 1);
        assert_eq!(health.record_ambiguous_timeout(now), HealthTransition::None);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.ambiguous_timeout_strikes, 0);
        health.record_result(now, true);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.ambiguous_timeout_strikes, 0);
    }

    #[test]
    fn non_ambiguous_result_breaks_soft_strike_sequence() {
        let now = Instant::now();
        let mut health = TransportHealth::default();
        health.record_ambiguous_timeout(now);
        assert_eq!(health.ambiguous_timeout_strikes, 1);

        health.record_result(now, false);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.ambiguous_timeout_strikes, 0);

        health.record_ambiguous_timeout(now);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.ambiguous_timeout_strikes, 1);

        health.clear_ambiguous_timeout(now);
        assert_eq!(health.ambiguous_timeout_strikes, 0);
    }

    #[tokio::test]
    async fn ambiguous_strikes_do_not_cross_peer_or_transport_health_keys() {
        let selector = fake_selector(Arc::new(FakeRuntime::default()), Vec::new());
        selector
            .record_ambiguous_timeout(9, ProxyTransport::Quic)
            .await;
        selector
            .record_ambiguous_timeout(9, ProxyTransport::Quic)
            .await;

        let quic = selector
            .health
            .get(&(9, ProxyTransport::Quic))
            .unwrap()
            .clone();
        assert_eq!(quic.lock().await.consecutive_failures, 1);
        assert!(!selector.health.contains_key(&(9, ProxyTransport::Kcp)));
        assert!(!selector.health.contains_key(&(10, ProxyTransport::Quic)));
    }

    #[derive(Default)]
    struct FakeRuntime {
        routes: DashMap<Ipv4Addr, (PeerId, CapabilitySnapshot)>,
        local_virtual_ips: DashMap<Ipv4Addr, ()>,
        sent: Mutex<Vec<(ZCPacket, NicPacketContext)>>,
        sent_notify: Notify,
    }

    impl FakeRuntime {
        fn set_route(&self, dst: Ipv4Addr, peer: PeerId, capabilities: CapabilitySnapshot) {
            self.routes.insert(dst, (peer, capabilities));
        }

        fn set_local_virtual_ip(&self, dst: Ipv4Addr) {
            self.local_virtual_ips.insert(dst, ());
        }

        async fn wait_for_sent(&self, count: usize) {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if self.sent.lock().await.len() >= count {
                        return;
                    }
                    self.sent_notify.notified().await;
                }
            })
            .await
            .expect("selector did not dispatch packet");
        }
    }

    #[async_trait]
    impl ProxySelectorRuntime for FakeRuntime {
        async fn target_snapshot(&self, dst_ip: Ipv4Addr) -> Option<(PeerId, CapabilitySnapshot)> {
            self.routes.get(&dst_ip).map(|entry| *entry.value())
        }

        async fn send_after_pipeline(
            &self,
            packet: ZCPacket,
            context: NicPacketContext,
        ) -> anyhow::Result<()> {
            self.sent.lock().await.push((packet, context));
            self.sent_notify.notify_waiters();
            Ok(())
        }

        fn is_local_virtual_ip(&self, dst_ip: Ipv4Addr) -> bool {
            self.local_virtual_ips.contains_key(&dst_ip)
        }

        fn my_peer_id(&self) -> PeerId {
            77
        }
    }

    enum FakePrepareResult {
        Success,
        Failure(ProxyPrepareFailureClass),
        WaitThenSuccess(Arc<Notify>),
        WaitThenFailure(Arc<Notify>, ProxyPrepareFailureClass),
    }

    struct FakeTransport {
        transport: ProxyTransport,
        results: Mutex<VecDeque<FakePrepareResult>>,
        calls: Mutex<Vec<PeerId>>,
        call_notify: Notify,
        live_streams: AtomicUsize,
    }

    impl FakeTransport {
        fn new(
            transport: ProxyTransport,
            results: impl IntoIterator<Item = FakePrepareResult>,
        ) -> Self {
            Self {
                transport,
                results: Mutex::new(results.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
                call_notify: Notify::new(),
                live_streams: AtomicUsize::new(0),
            }
        }

        async fn wait_for_calls(&self, count: usize) {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if self.calls.lock().await.len() >= count {
                        return;
                    }
                    self.call_notify.notified().await;
                }
            })
            .await
            .expect("proxy prepare was not called");
        }
    }

    #[async_trait]
    impl ProxyPrepareTransport for FakeTransport {
        fn transport(&self) -> ProxyTransport {
            self.transport
        }

        async fn prepare(
            &self,
            _flow: FlowKey,
            dst_peer_id: PeerId,
            _prepare_ack_version: u32,
            _deadline: Instant,
        ) -> Result<BoxProxyStream, ProxyPrepareError> {
            self.calls.lock().await.push(dst_peer_id);
            self.call_notify.notify_waiters();
            let result =
                self.results
                    .lock()
                    .await
                    .pop_front()
                    .unwrap_or(FakePrepareResult::Failure(
                        ProxyPrepareFailureClass::Transport,
                    ));
            match result {
                FakePrepareResult::Failure(class) => {
                    return Err(ProxyPrepareError::new(
                        class,
                        anyhow::anyhow!("fake transport failure"),
                    ));
                }
                FakePrepareResult::WaitThenSuccess(release) => {
                    release.notified().await;
                }
                FakePrepareResult::WaitThenFailure(release, class) => {
                    release.notified().await;
                    return Err(ProxyPrepareError::new(
                        class,
                        anyhow::anyhow!("delayed fake transport failure"),
                    ));
                }
                FakePrepareResult::Success => {}
            }
            self.live_streams.fetch_add(1, Ordering::Relaxed);
            let (stream, peer) = tokio::io::duplex(64);
            drop(peer);
            Ok(Box::new(stream))
        }
    }

    fn tcp_syn(sequence: u32) -> (ZCPacket, Vec<u8>) {
        let mut bytes = vec![0u8; 48];
        let mut ip = MutableIpv4Packet::new(&mut bytes).unwrap();
        ip.set_version(4);
        ip.set_header_length(5);
        ip.set_total_length(48);
        ip.set_next_level_protocol(IpNextHeaderProtocols::Tcp);
        ip.set_source("10.44.0.2".parse().unwrap());
        ip.set_destination("10.44.0.3".parse().unwrap());
        let mut tcp = MutableTcpPacket::new(ip.payload_mut()).unwrap();
        tcp.set_source(32000);
        tcp.set_destination(443);
        tcp.set_sequence(sequence);
        tcp.set_data_offset(6);
        tcp.set_flags(TcpFlags::SYN);
        tcp.packet_mut()[20..24].copy_from_slice(&[2, 4, 0x05, 0xb4]);
        tcp.packet_mut()[24..28].copy_from_slice(b"TFO!");
        let original = bytes.clone();
        let mut packet = ZCPacket::new_with_payload(&bytes);
        packet.fill_peer_manager_hdr(77, 0, crate::tunnel::packet_def::PacketType::Data as u8);
        (packet, original)
    }

    fn fake_selector(
        runtime: Arc<FakeRuntime>,
        transports: Vec<Arc<dyn ProxyPrepareTransport>>,
    ) -> DeferredProxySelector {
        DeferredProxySelector::new_with_runtime(
            runtime,
            transports,
            Arc::new(PreparedProxyStore::default()),
        )
    }

    #[tokio::test]
    async fn exact_local_virtual_ip_bypasses_proxy_without_creating_state() {
        let dst = "10.44.0.3".parse().unwrap();
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_local_virtual_ip(dst);
        runtime.set_route(
            dst,
            runtime.my_peer_id(),
            CapabilitySnapshot {
                quic: true,
                kcp: true,
                prepare_ack_version: 1,
            },
        );
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::Success],
        ));
        let selector = fake_selector(runtime, vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: dst.into(),
            not_send_to_self: true,
        };
        let (mut packet, _) = tcp_syn(100);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Continue
        );
        assert!(selector.flows.is_empty());
        assert!(selector.prepared.streams.is_empty());
        assert!(transport.calls.lock().await.is_empty());
    }

    #[tokio::test]
    async fn self_peer_subnet_destination_still_uses_proxy() {
        let dst = "10.44.0.3".parse().unwrap();
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            dst,
            runtime.my_peer_id(),
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::WaitThenSuccess(release.clone())],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: dst.into(),
            not_send_to_self: true,
        };
        let (mut packet, _) = tcp_syn(100);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        assert_eq!(
            transport.calls.lock().await.as_slice(),
            &[runtime.my_peer_id()]
        );

        release.notify_one();
        runtime.wait_for_sent(1).await;
    }

    #[tokio::test]
    async fn prepared_stream_is_bound_to_generation_and_taken_once() {
        let store = PreparedProxyStore::default();
        let flow = FlowKey {
            src: "10.44.0.2:32000".parse().unwrap(),
            dst: "10.44.0.3:443".parse().unwrap(),
        };
        let key = PreparedKey {
            flow,
            initial_sequence: 100,
            generation: 7,
            dst_peer_id: 9,
            transport: ProxyTransport::Quic,
        };
        let (stream, _other) = tokio::io::duplex(64);
        store.insert(key, Box::new(stream));

        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Kcp, 100)
                .is_none()
        );
        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Quic, 99)
                .is_none()
        );
        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Quic, 100)
                .is_some()
        );
        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Quic, 100)
                .is_none()
        );
    }

    #[tokio::test]
    async fn newer_prepared_generation_replaces_stale_stream() {
        let store = PreparedProxyStore::default();
        let flow = FlowKey {
            src: "10.44.0.2:32000".parse().unwrap(),
            dst: "10.44.0.3:443".parse().unwrap(),
        };
        let old_key = PreparedKey {
            flow,
            initial_sequence: 100,
            generation: 7,
            dst_peer_id: 9,
            transport: ProxyTransport::Quic,
        };
        let new_key = PreparedKey {
            generation: 8,
            ..old_key
        };
        let (old_stream, _old_peer) = tokio::io::duplex(64);
        let (new_stream, _new_peer) = tokio::io::duplex(64);

        store.insert(old_key, Box::new(old_stream));
        store.insert(new_key, Box::new(new_stream));
        store.remove(old_key);

        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Quic, 100)
                .is_some()
        );
        assert!(
            store
                .claim_for_syn(flow, ProxyTransport::Quic, 100)
                .is_none()
        );
    }

    #[test]
    fn transport_health_degrades_and_recovers_through_half_open_probes() {
        let start = Instant::now();
        let mut health = TransportHealth::default();

        assert_eq!(health.record_result(start, false), HealthTransition::None);
        assert_eq!(health.record_result(start, false), HealthTransition::None);
        assert_eq!(
            health.record_result(start, false),
            HealthTransition::Degraded
        );
        assert!(!health.allows_attempt(start));
        assert!(!health.allows_attempt(start + HALF_OPEN_INTERVAL - Duration::from_millis(1)));

        for probe in 1..=2 {
            let now = start + HALF_OPEN_INTERVAL * probe;
            assert!(health.allows_attempt(now));
            assert_eq!(health.record_result(now, true), HealthTransition::None);
            assert!(!health.allows_attempt(now));
        }

        let recovered_at = start + HALF_OPEN_INTERVAL * 3;
        assert!(health.allows_attempt(recovered_at));
        assert_eq!(
            health.record_result(recovered_at, true),
            HealthTransition::Recovered
        );
        assert!(health.allows_attempt(recovered_at));
        assert!(!health.degraded);
    }

    #[test]
    fn failed_half_open_probe_resets_recovery_progress() {
        let start = Instant::now();
        let mut health = TransportHealth::default();
        for _ in 0..3 {
            health.record_result(start, false);
        }

        let first_probe = start + HALF_OPEN_INTERVAL;
        assert!(health.allows_attempt(first_probe));
        health.record_result(first_probe, true);
        assert_eq!(health.consecutive_successes, 1);

        let second_probe = first_probe + HALF_OPEN_INTERVAL;
        assert!(health.allows_attempt(second_probe));
        health.record_result(second_probe, false);
        assert_eq!(health.consecutive_successes, 0);
        assert!(health.degraded);
    }

    #[tokio::test]
    async fn stale_transport_health_is_evicted() {
        let runtime = Arc::new(FakeRuntime::default());
        let selector = fake_selector(runtime, Vec::new());
        let key = (9, ProxyTransport::Quic);
        selector.health.insert(
            key,
            Arc::new(Mutex::new(TransportHealth {
                updated_at: Instant::now() - HEALTH_TTL,
                ..TransportHealth::default()
            })),
        );

        selector.cleanup_health().await;

        assert!(!selector.health.contains_key(&key));
    }

    #[tokio::test]
    async fn pending_retransmit_is_merged_and_proxy_dispatch_preserves_context_and_tfo() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::WaitThenSuccess(release.clone())],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: true,
        };
        let (mut first, original) = tcp_syn(100);
        let (mut retransmit, _) = tcp_syn(100);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut first, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut retransmit, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        assert_eq!(transport.calls.lock().await.len(), 1);

        release.notify_one();
        runtime.wait_for_sent(1).await;
        let sent = runtime.sent.lock().await;
        let (packet, sent_context) = &sent[0];
        let header = packet.peer_manager_header().unwrap();
        assert!(header.is_quic_src_modified());
        assert!(header.is_deferred_proxy());
        assert_eq!(header.from_peer_id.get(), 77);
        assert_eq!(header.to_peer_id.get(), 77);
        assert_eq!(*sent_context, context);
        assert_eq!(packet.payload(), original);
        drop(sent);

        let (mut decided_retransmit, _) = tcp_syn(100);
        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut decided_retransmit, &context)
                .await,
            NicPacketFilterAction::StopAndSend
        );
        assert!(
            decided_retransmit
                .peer_manager_header()
                .unwrap()
                .is_quic_src_modified()
        );
        assert!(
            decided_retransmit
                .peer_manager_header()
                .unwrap()
                .is_deferred_proxy()
        );
        assert_eq!(transport.calls.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn failed_proxy_prepare_dispatches_unmodified_native_syn() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::Failure(
                ProxyPrepareFailureClass::Transport,
            )],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: false,
        };
        let (mut packet, original) = tcp_syn(101);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        runtime.wait_for_sent(1).await;
        let sent = runtime.sent.lock().await;
        let (packet, sent_context) = &sent[0];
        let header = packet.peer_manager_header().unwrap();
        assert!(!header.is_quic_src_modified());
        assert!(!header.is_kcp_src_modified());
        assert!(!header.is_no_proxy());
        assert_eq!(header.to_peer_id.get(), 0);
        assert_eq!(packet.payload(), original);
        assert_eq!(*sent_context, context);
        drop(sent);

        let statuses = selector.list_statuses().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].requested_transport, "quic,native");
        assert_eq!(statuses[0].selected_transport, "native");
        assert_eq!(statuses[0].fallback_reason, "quic_prepare_failed");
        assert_eq!(statuses[0].consecutive_failures, 1);
    }

    #[tokio::test]
    async fn local_bypass_skips_the_deferred_selector_without_consuming_the_marker() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: true,
                prepare_ack_version: 1,
            },
        );
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::Success],
        ));
        let selector = fake_selector(runtime, vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(102);
        packet.set_bypass_proxy_interception(true);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Continue
        );
        assert!(packet.bypass_proxy_interception());
        assert!(transport.calls.lock().await.is_empty());
        assert!(selector.list_statuses().await.is_empty());
    }

    #[tokio::test]
    async fn cleanup_converts_expired_pending_flow_to_cached_native_decision() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::WaitThenSuccess(release)],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: true,
        };
        let (mut packet, original) = tcp_syn(105);
        let (flow, _) = DeferredProxySelector::parse_syn(&packet).unwrap();

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        let entry = selector.flows.get(&flow).unwrap().clone();
        entry.lock().await.created_at = Instant::now() - PENDING_TTL;

        selector.cleanup_flows().await;
        runtime.wait_for_sent(1).await;
        assert_eq!(runtime.sent.lock().await[0].0.payload(), original);
        assert_eq!(runtime.sent.lock().await[0].1, context);
        assert_eq!(
            entry.lock().await.decision,
            FlowDecision::Native,
            "expired pending decision must remain cached"
        );

        let (mut retransmit, _) = tcp_syn(105);
        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut retransmit, &context)
                .await,
            NicPacketFilterAction::StopAndSend
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(runtime.sent.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn policy_failure_does_not_degrade_transport_health() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::Failure(ProxyPrepareFailureClass::Policy)],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(104);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        runtime.wait_for_sent(1).await;
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].selected_transport, "native");
        assert_eq!(statuses[0].consecutive_failures, 0);
        assert!(!statuses[0].transport_degraded);
    }

    #[tokio::test]
    async fn quic_failure_falls_back_to_kcp_and_caches_kcp_decision() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: true,
                prepare_ack_version: 0,
            },
        );
        let quic = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [FakePrepareResult::Failure(
                ProxyPrepareFailureClass::Transport,
            )],
        ));
        let kcp = Arc::new(FakeTransport::new(
            ProxyTransport::Kcp,
            [FakePrepareResult::Success],
        ));
        let selector = fake_selector(runtime.clone(), vec![kcp.clone(), quic.clone()]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(150);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        runtime.wait_for_sent(1).await;
        assert_eq!(&*quic.calls.lock().await, &[9]);
        assert_eq!(&*kcp.calls.lock().await, &[9]);
        assert!(
            runtime.sent.lock().await[0]
                .0
                .peer_manager_header()
                .unwrap()
                .is_kcp_src_modified()
        );

        let (mut retransmit, _) = tcp_syn(150);
        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut retransmit, &context)
                .await,
            NicPacketFilterAction::StopAndSend
        );
        assert!(
            retransmit
                .peer_manager_header()
                .unwrap()
                .is_kcp_src_modified()
        );
        assert_eq!(quic.calls.lock().await.len(), 1);
        assert_eq!(kcp.calls.lock().await.len(), 1);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].requested_transport, "quic,kcp,native");
        assert_eq!(statuses[0].selected_transport, "kcp");
        assert_eq!(statuses[0].fallback_reason, "quic_prepare_failed");
    }

    #[tokio::test]
    async fn new_syn_sequence_cancels_old_generation_without_late_dispatch() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.set_route(
            "10.44.0.3".parse().unwrap(),
            9,
            CapabilitySnapshot {
                quic: true,
                kcp: false,
                prepare_ack_version: 0,
            },
        );
        let stale_release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [
                FakePrepareResult::WaitThenSuccess(stale_release.clone()),
                FakePrepareResult::Success,
            ],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: "10.44.0.3".parse().unwrap(),
            not_send_to_self: true,
        };
        let (mut old_syn, _) = tcp_syn(200);
        let (mut new_syn, _) = tcp_syn(201);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut old_syn, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut new_syn, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(2).await;
        stale_release.notify_one();
        runtime.wait_for_sent(1).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let sent = runtime.sent.lock().await;
        assert_eq!(sent.len(), 1);
        assert_eq!(DeferredProxySelector::parse_syn(&sent[0].0).unwrap().1, 201);
        drop(sent);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].selected_transport, "quic");
    }

    #[tokio::test]
    async fn route_change_after_ready_discards_stale_success() {
        let runtime = Arc::new(FakeRuntime::default());
        let dst = "10.44.0.3".parse().unwrap();
        let capabilities = CapabilitySnapshot {
            quic: true,
            kcp: false,
            prepare_ack_version: 0,
        };
        runtime.set_route(dst, 9, capabilities);
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [
                FakePrepareResult::WaitThenSuccess(release.clone()),
                FakePrepareResult::Success,
            ],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: IpAddr::V4(dst),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(102);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        runtime.set_route(dst, 10, capabilities);
        release.notify_one();
        transport.wait_for_calls(2).await;
        runtime.wait_for_sent(1).await;

        assert_eq!(&*transport.calls.lock().await, &[9, 10]);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].dst_peer_id, 10);
        assert_eq!(statuses[0].selected_transport, "quic");
        assert!(statuses[0].generation >= 2);
    }

    #[tokio::test]
    async fn route_change_while_waiting_for_ready_wins_over_ambiguous_timeout() {
        let runtime = Arc::new(FakeRuntime::default());
        let dst = "10.44.0.3".parse().unwrap();
        let capabilities = CapabilitySnapshot {
            quic: true,
            kcp: false,
            prepare_ack_version: 1,
        };
        runtime.set_route(dst, 9, capabilities);
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [
                FakePrepareResult::WaitThenFailure(
                    release.clone(),
                    ProxyPrepareFailureClass::AmbiguousTimeout,
                ),
                FakePrepareResult::Success,
            ],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: IpAddr::V4(dst),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(106);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        runtime.set_route(dst, 10, capabilities);
        release.notify_one();
        transport.wait_for_calls(2).await;
        runtime.wait_for_sent(1).await;

        let stale_health = selector
            .health
            .get(&(9, ProxyTransport::Quic))
            .unwrap()
            .clone();
        let stale_health = stale_health.lock().await;
        assert_eq!(stale_health.consecutive_failures, 0);
        assert_eq!(stale_health.ambiguous_timeout_strikes, 0);
        drop(stale_health);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].dst_peer_id, 10);
        assert_eq!(statuses[0].selected_transport, "quic");
    }

    #[tokio::test]
    async fn route_change_while_waiting_for_accepted_wins_over_transport_failure() {
        let runtime = Arc::new(FakeRuntime::default());
        let dst = "10.44.0.3".parse().unwrap();
        let capabilities = CapabilitySnapshot {
            quic: true,
            kcp: false,
            prepare_ack_version: 1,
        };
        runtime.set_route(dst, 9, capabilities);
        let release = Arc::new(Notify::new());
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            [
                FakePrepareResult::WaitThenFailure(
                    release.clone(),
                    ProxyPrepareFailureClass::Transport,
                ),
                FakePrepareResult::Success,
            ],
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: IpAddr::V4(dst),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(107);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        transport.wait_for_calls(1).await;
        runtime.set_route(dst, 10, capabilities);
        release.notify_one();
        transport.wait_for_calls(2).await;
        runtime.wait_for_sent(1).await;

        let stale_health = selector
            .health
            .get(&(9, ProxyTransport::Quic))
            .unwrap()
            .clone();
        let stale_health = stale_health.lock().await;
        assert_eq!(stale_health.consecutive_failures, 0);
        assert_eq!(stale_health.ambiguous_timeout_strikes, 0);
        drop(stale_health);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].dst_peer_id, 10);
        assert_eq!(statuses[0].selected_transport, "quic");
    }

    #[tokio::test]
    async fn third_route_change_falls_back_to_native_on_current_peer() {
        let runtime = Arc::new(FakeRuntime::default());
        let dst = "10.44.0.3".parse().unwrap();
        let capabilities = CapabilitySnapshot {
            quic: true,
            kcp: false,
            prepare_ack_version: 0,
        };
        runtime.set_route(dst, 9, capabilities);
        let releases = [
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        ];
        let transport = Arc::new(FakeTransport::new(
            ProxyTransport::Quic,
            releases
                .iter()
                .cloned()
                .map(FakePrepareResult::WaitThenSuccess),
        ));
        let selector = fake_selector(runtime.clone(), vec![transport.clone()]);
        let context = NicPacketContext {
            ip_addr: IpAddr::V4(dst),
            not_send_to_self: false,
        };
        let (mut packet, _) = tcp_syn(103);

        assert_eq!(
            selector
                .try_process_packet_from_nic(&mut packet, &context)
                .await,
            NicPacketFilterAction::Consume
        );
        for (index, peer) in [10, 11, 12].into_iter().enumerate() {
            transport.wait_for_calls(index + 1).await;
            runtime.set_route(dst, peer, capabilities);
            releases[index].notify_one();
        }
        runtime.wait_for_sent(1).await;

        assert_eq!(&*transport.calls.lock().await, &[9, 10, 11]);
        let statuses = selector.list_statuses().await;
        assert_eq!(statuses[0].selected_transport, "native");
        assert_eq!(statuses[0].fallback_reason, "route_restart_limit");
        assert_eq!(statuses[0].dst_peer_id, 12);
    }

    #[test]
    fn parses_only_initial_tcp_syn_and_preserves_flow_key() {
        use pnet::packet::{MutablePacket, ipv4::MutableIpv4Packet, tcp::MutableTcpPacket};

        let mut bytes = vec![0u8; 40];
        let mut ip = MutableIpv4Packet::new(&mut bytes).unwrap();
        ip.set_version(4);
        ip.set_header_length(5);
        ip.set_total_length(40);
        ip.set_next_level_protocol(IpNextHeaderProtocols::Tcp);
        ip.set_source("10.44.0.2".parse().unwrap());
        ip.set_destination("10.44.0.3".parse().unwrap());
        let mut tcp = MutableTcpPacket::new(ip.payload_mut()).unwrap();
        tcp.set_source(32000);
        tcp.set_destination(443);
        tcp.set_sequence(99);
        tcp.set_data_offset(5);
        tcp.set_flags(TcpFlags::SYN);
        let packet = ZCPacket::new_with_payload(&bytes);

        assert_eq!(
            DeferredProxySelector::parse_syn(&packet),
            Some((
                FlowKey {
                    src: "10.44.0.2:32000".parse().unwrap(),
                    dst: "10.44.0.3:443".parse().unwrap(),
                },
                99
            ))
        );
    }
}
