use std::{
    collections::{BTreeSet, HashMap, hash_map::DefaultHasher},
    hash::Hasher,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use arc_swap::ArcSwap;
use dashmap::DashMap;

use super::{
    PeerId,
    config::{
        ConfigLoader, Flags, NicBackend, is_effective_secure_mode_enabled, process_secure_mode_cfg,
    },
    netns::NetNS,
    network::IPCollector,
    stun::{StunInfoCollector, StunInfoCollectorTrait},
};
use crate::{
    common::{
        config::ProxyNetworkConfig, shrink_dashmap, stats_manager::StatsManager,
        token_bucket::TokenBucketManager,
    },
    peers::{acl_filter::AclFilter, credential_manager::CredentialManager},
    proto::{
        acl::GroupIdentity,
        api::{config::InstanceConfigPatch, instance::PeerConnInfo},
        common::{PeerFeatureFlag, PortForwardConfigPb, SecureModeConfig},
        peer_rpc::PeerGroupInfo,
    },
    rpc_service::protected_port,
    tunnel::{IpScheme, matches_protocol},
};
use crossbeam::atomic::AtomicCell;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use socket2::Protocol;

pub type NetworkIdentity = crate::common::config::NetworkIdentity;

const PROTOCOL_LOOP_TRACKED_SCHEMES: usize = 8;
const PROTOCOL_LOOP_SCOPE_COUNT: usize = 2;
const PROTOCOL_LOOP_STATE_SLOTS: usize = PROTOCOL_LOOP_TRACKED_SCHEMES * PROTOCOL_LOOP_SCOPE_COUNT;
const UNDERLAY_BREAKER_CAPACITY: usize = 4096;
const UNDERLAY_BREAKER_STRIKE_THRESHOLD: u8 = 100;
const UNDERLAY_BREAKER_STRIKE_WINDOW_SECS: u64 = 10;
const UNDERLAY_BREAKER_INITIAL_TTL_SECS: u64 = 30;
const UNDERLAY_BREAKER_MAX_TTL_SECS: u64 = 300;
const UNDERLAY_BREAKER_SOFT_TTL_SECS: u64 = 30;
const UNDERLAY_BREAKER_HALF_OPEN_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolLoopScope {
    Direct,
    HolePunch,
}

impl ProtocolLoopScope {
    const fn index(self) -> usize {
        match self {
            Self::Direct => 0,
            Self::HolePunch => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ProtocolLoopSuppressionSlot {
    strike_count: u8,
    first_hit_at_secs: u64,
    suppressed_until_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnderlayBreakerScope {
    Direct,
    HolePunch,
    Generic,
    ProxyPrepare,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnderlayBreakerKey {
    Endpoint {
        remote_addr: SocketAddr,
        scheme: IpScheme,
        scope: UnderlayBreakerScope,
    },
    Peer {
        peer_id: PeerId,
        scheme: IpScheme,
        scope: UnderlayBreakerScope,
    },
}

impl UnderlayBreakerKey {
    pub fn endpoint(
        remote_addr: SocketAddr,
        scheme: IpScheme,
        scope: UnderlayBreakerScope,
    ) -> Self {
        Self::Endpoint {
            remote_addr,
            scheme,
            scope,
        }
    }

    pub fn peer(peer_id: PeerId, scheme: IpScheme, scope: UnderlayBreakerScope) -> Self {
        Self::Peer {
            peer_id,
            scheme,
            scope,
        }
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        match self {
            Self::Endpoint { remote_addr, .. } => Some(*remote_addr),
            Self::Peer { .. } => None,
        }
    }

    fn peer_id(&self) -> Option<PeerId> {
        match self {
            Self::Endpoint { .. } => None,
            Self::Peer { peer_id, .. } => Some(*peer_id),
        }
    }

    fn scheme(&self) -> IpScheme {
        match self {
            Self::Endpoint { scheme, .. } | Self::Peer { scheme, .. } => *scheme,
        }
    }

    fn scope(&self) -> UnderlayBreakerScope {
        match self {
            Self::Endpoint { scope, .. } | Self::Peer { scope, .. } => *scope,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlayBreakerStrikeKind {
    Hard,
    Soft,
}

#[derive(Debug, Default, Clone)]
pub struct UnderlayBreakerTrace {
    pub expected_peer_id: Option<PeerId>,
    pub actual_peer_id: Option<PeerId>,
    pub local_ip: Option<IpAddr>,
    pub ifname: Option<String>,
}

struct UnderlayBreakerLog {
    warn: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Default)]
struct UnderlayBreakerEntry {
    hard_strikes: u8,
    soft_strikes: u16,
    first_hard_at_secs: u64,
    blocked_until_secs: u64,
    backoff_secs: u64,
    half_open: bool,
    half_open_at_secs: u64,
    half_open_lease_id: Option<u64>,
    updated_at_secs: u64,
}

#[derive(Debug, Default)]
struct UnderlayBreakerState {
    entries: HashMap<UnderlayBreakerKey, UnderlayBreakerEntry>,
    next_lease_id: u64,
}

#[derive(Debug, Clone)]
pub struct UnderlayBreakerGateError {
    pub key: UnderlayBreakerKey,
}

impl std::fmt::Display for UnderlayBreakerGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "underlay breaker gated key {:?}", self.key)
    }
}

impl std::error::Error for UnderlayBreakerGateError {}

#[derive(Debug, Clone)]
pub struct UnderlayAttemptContext {
    endpoint_key: UnderlayBreakerKey,
    peer_key: Option<UnderlayBreakerKey>,
}

impl UnderlayAttemptContext {
    pub fn new(
        remote_addr: SocketAddr,
        scheme: IpScheme,
        scope: UnderlayBreakerScope,
        peer_id_hint: Option<PeerId>,
    ) -> Self {
        Self {
            endpoint_key: UnderlayBreakerKey::endpoint(remote_addr, scheme, scope),
            peer_key: peer_id_hint.map(|peer_id| UnderlayBreakerKey::peer(peer_id, scheme, scope)),
        }
    }

    pub fn set_peer_id_if_missing(&mut self, peer_id: PeerId) {
        if self.peer_key.is_none() {
            self.peer_key = Some(UnderlayBreakerKey::peer(
                peer_id,
                self.endpoint_key.scheme(),
                self.endpoint_key.scope(),
            ));
        }
    }

    pub fn endpoint_key(&self) -> &UnderlayBreakerKey {
        &self.endpoint_key
    }

    pub fn peer_key(&self) -> Option<&UnderlayBreakerKey> {
        self.peer_key.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GlobalCtxEvent {
    TunDeviceReady(String),
    TunDeviceError(String),

    PeerAdded(PeerId),
    PeerRemoved(PeerId),
    PeerConnAdded(PeerConnInfo),
    PeerConnRemoved(PeerConnInfo),

    ListenerAdded(url::Url),
    ListenerAddFailed(url::Url, String), // (url, error message)
    ListenerAcceptFailed(url::Url, String), // (url, error message)
    ConnectionAccepted(String, String),  // (local url, remote url)
    ConnectionError(String, String, String), // (local url, remote url, error message)
    ListenerPortMappingEstablished {
        local_listener: url::Url,
        mapped_listener: url::Url,
        backend: String,
    },

    Connecting(url::Url),
    ConnectError(String, String, String), // (dst, ip version, error message)

    VpnPortalStarted(String),                    // (portal)
    VpnPortalClientConnected(String, String),    // (portal, client ip)
    VpnPortalClientDisconnected(String, String), // (portal, client ip)

    DhcpIpv4Changed(Option<cidr::Ipv4Inet>, Option<cidr::Ipv4Inet>), // (old, new)
    DhcpIpv4Conflicted(Option<cidr::Ipv4Inet>),
    PublicIpv6Changed(Option<cidr::Ipv6Inet>, Option<cidr::Ipv6Inet>), // (old, new)
    PublicIpv6RoutesUpdated(Vec<cidr::Ipv6Inet>, Vec<cidr::Ipv6Inet>), // (added, removed)

    PortForwardAdded(PortForwardConfigPb),

    ConfigPatched(InstanceConfigPatch),

    ProxyCidrsUpdated(Vec<cidr::Ipv4Cidr>, Vec<cidr::Ipv4Cidr>), // (added, removed)

    UdpBroadcastRelayStartResult {
        capture_backend: Option<String>,
        error: Option<String>,
    },

    CredentialChanged,
}

pub type EventBus = tokio::sync::broadcast::Sender<GlobalCtxEvent>;
pub type EventBusSubscriber = tokio::sync::broadcast::Receiver<GlobalCtxEvent>;

/// Source of a trusted public key from OSPF route propagation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedKeySource {
    /// Peer node's noise static pubkey
    OspfNode,
    /// Admin-declared trusted credential pubkey
    OspfCredential,
}

/// Metadata for a trusted public key
#[derive(Debug, Clone)]
pub struct TrustedKeyMetadata {
    pub source: TrustedKeySource,
    /// Expiry time in Unix seconds. None means never expires.
    pub expiry_unix: Option<i64>,
}

impl TrustedKeyMetadata {
    pub fn is_expired(&self) -> bool {
        if let Some(expiry) = self.expiry_unix {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            return now >= expiry;
        }
        false
    }
}

// key is (pubkey, network-name)
pub type TrustedKeyMap = HashMap<Vec<u8>, TrustedKeyMetadata>;

struct TrustedKeyMapManager {
    network_trusted_keys: DashMap<String, ArcSwap<TrustedKeyMap>>,
}

impl TrustedKeyMapManager {
    pub fn new() -> Self {
        Self {
            network_trusted_keys: DashMap::new(),
        }
    }

    pub fn update_trusted_keys(&self, network_name: &str, trusted_keys: TrustedKeyMap) {
        match self.network_trusted_keys.entry(network_name.to_string()) {
            dashmap::Entry::Vacant(entry) => {
                entry.insert(ArcSwap::new(Arc::new(trusted_keys)));
            }
            dashmap::Entry::Occupied(entry) => {
                entry.get().store(Arc::new(trusted_keys));
            }
        }
    }

    pub fn remove_trusted_keys(&self, network_name: &str) {
        self.network_trusted_keys.remove(network_name);
        shrink_dashmap(&self.network_trusted_keys, None);
    }

    pub fn verify_trusted_key(&self, pubkey: &[u8], network_name: &str) -> bool {
        self.verify_trusted_key_with_source(pubkey, network_name, None)
    }

    pub fn verify_trusted_key_with_source(
        &self,
        pubkey: &[u8],
        network_name: &str,
        source: Option<TrustedKeySource>,
    ) -> bool {
        let Some(trusted_keys) = self
            .network_trusted_keys
            .get(network_name)
            .map(|v| v.load_full())
        else {
            return false;
        };

        let Some(metadata) = trusted_keys.get(&pubkey.to_vec()) else {
            return false;
        };

        if let Some(source) = source {
            metadata.source == source && !metadata.is_expired()
        } else {
            !metadata.is_expired()
        }
    }

    pub fn list_trusted_keys(&self, network_name: &str) -> Vec<(Vec<u8>, TrustedKeyMetadata)> {
        let Some(trusted_keys) = self
            .network_trusted_keys
            .get(network_name)
            .map(|v| v.load_full())
        else {
            return Vec::new();
        };

        let mut items = trusted_keys
            .iter()
            .filter(|(_, metadata)| !metadata.is_expired())
            .map(|(pubkey, metadata)| (pubkey.clone(), metadata.clone()))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        items
    }
}

pub struct GlobalCtx {
    pub inst_name: String,
    pub id: uuid::Uuid,
    pub config: Box<dyn ConfigLoader>,
    pub net_ns: NetNS,
    pub network: NetworkIdentity,
    resolved_nic_backend: std::sync::OnceLock<NicBackend>,
    derived_secure_mode: OnceLock<SecureModeConfig>,

    event_bus: EventBus,

    cached_ipv4: AtomicCell<Option<cidr::Ipv4Inet>>,
    cached_ipv6: AtomicCell<Option<cidr::Ipv6Inet>>,
    public_ipv6_lease: AtomicCell<Option<cidr::Ipv6Inet>>,
    public_ipv6_routes: Mutex<BTreeSet<std::net::Ipv6Addr>>,
    cached_proxy_cidrs: AtomicCell<Option<Vec<ProxyNetworkConfig>>>,

    ip_collector: Mutex<Option<Arc<IPCollector>>>,

    hostname: Mutex<String>,

    stun_info_collection: Mutex<Arc<dyn StunInfoCollectorTrait>>,

    running_listeners: Mutex<Vec<url::Url>>,
    advertised_ipv6_public_addr_prefix: Mutex<Option<cidr::Ipv6Cidr>>,

    flags: ArcSwap<Flags>,
    protocol_loop_suppression: Mutex<[ProtocolLoopSuppressionSlot; PROTOCOL_LOOP_STATE_SLOTS]>,
    underlay_breaker: Mutex<UnderlayBreakerState>,

    // Runtime/base advertised feature flags before config-owned fields are
    // overlaid by set_flags. Keep this separate so config patches do not erase
    // runtime state such as public-server role, IPv6 provider status, or the
    // non-whitelist avoid-relay preference.
    base_feature_flags: ArcSwap<PeerFeatureFlag>,

    feature_flags: ArcSwap<PeerFeatureFlag>,

    token_bucket_manager: TokenBucketManager,

    stats_manager: Arc<StatsManager>,

    acl_filter: Arc<AclFilter>,

    credential_manager: Arc<CredentialManager>,

    /// OSPF propagated trusted keys (peer pubkeys and admin credentials)
    /// Stored in ArcSwap for lock-free reads and atomic batch updates
    trusted_keys: Arc<TrustedKeyMapManager>,
}

impl std::fmt::Debug for GlobalCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalCtx")
            .field("inst_name", &self.inst_name)
            .field("id", &self.id)
            .field("net_ns", &self.net_ns.name())
            .field("event_bus", &"EventBus")
            .field("ipv4", &self.cached_ipv4)
            .finish()
    }
}

pub type ArcGlobalCtx = std::sync::Arc<GlobalCtx>;

#[derive(Debug)]
pub struct UnderlayPreflightGuard {
    global_ctx: ArcGlobalCtx,
    lease_id: u64,
    acquired_half_open_keys: Vec<UnderlayBreakerKey>,
    committed: bool,
}

impl UnderlayPreflightGuard {
    fn new(
        global_ctx: ArcGlobalCtx,
        lease_id: u64,
        acquired_half_open_keys: Vec<UnderlayBreakerKey>,
    ) -> Self {
        Self {
            global_ctx,
            lease_id,
            acquired_half_open_keys,
            committed: false,
        }
    }

    pub fn commit(&mut self) {
        debug_assert!(!self.committed, "underlay preflight guard committed twice");
        self.committed = true;
    }

    #[cfg(test)]
    fn lease_id(&self) -> u64 {
        self.lease_id
    }
}

impl Drop for UnderlayPreflightGuard {
    fn drop(&mut self) {
        if !self.committed && self.lease_id != 0 {
            self.global_ctx
                .rollback_underlay_preflight(self.lease_id, &self.acquired_half_open_keys);
        }
    }
}

impl GlobalCtx {
    const PROTOCOL_LOOP_STRIKE_THRESHOLD: u8 = 2;
    const PROTOCOL_LOOP_STRIKE_WINDOW_SECS: u64 = 30;
    const PROTOCOL_LOOP_SUPPRESS_SECS: u64 = 300;

    fn stealth_enabled_for_config(
        flags: &Flags,
        secure_mode: bool,
        network_secret: Option<&str>,
    ) -> bool {
        crate::tunnel::stealth::is_stealth_effectively_enabled(
            network_secret,
            flags.stealth_mode,
            secure_mode,
        )
    }

    fn apply_disable_relay_data_flag(
        flags: &Flags,
        mut feature_flags: PeerFeatureFlag,
    ) -> PeerFeatureFlag {
        if flags.disable_relay_data {
            feature_flags.avoid_relay_data = true;
        }
        feature_flags
    }

    fn derive_feature_flags(
        flags: &Flags,
        mut feature_flags: PeerFeatureFlag,
        stealth_enabled: bool,
    ) -> PeerFeatureFlag {
        feature_flags.kcp_input = cfg!(feature = "kcp") && !flags.disable_kcp_input;
        feature_flags.no_relay_kcp = flags.disable_relay_kcp;
        feature_flags.support_conn_list_sync = true;
        feature_flags.quic_input = cfg!(feature = "quic") && !flags.disable_quic_input;
        feature_flags.no_relay_quic = flags.disable_relay_quic;
        feature_flags.proxy_prepare_ack_version = if cfg!(any(feature = "kcp", feature = "quic")) {
            crate::common::constants::PROXY_PREPARE_ACK_VERSION
        } else {
            0
        };
        feature_flags.need_p2p = flags.need_p2p;
        feature_flags.disable_p2p = flags.disable_p2p;
        let protocols =
            crate::common::stealth_registry::StealthProtocolSet::parse(&flags.stealth_protocols)
                .expect("stealth_protocols is validated while loading configuration");
        feature_flags.stealth_supported = stealth_enabled
            && protocols.contains(
                flags.stealth_mode,
                crate::common::stealth_registry::StealthProtocol::Udp,
            );
        feature_flags.stealth_capabilities = if stealth_enabled {
            protocols.capabilities(flags.stealth_mode)
        } else {
            Vec::new()
        };
        Self::apply_disable_relay_data_flag(flags, feature_flags)
    }

    pub fn new(config_fs: impl ConfigLoader + 'static) -> Self {
        let id = config_fs.get_id();
        let network = config_fs.get_network_identity();
        let net_ns = NetNS::new(config_fs.get_netns());
        let hostname = config_fs.get_hostname();

        let (event_bus, _) = tokio::sync::broadcast::channel(16);

        let stun_info_collector = StunInfoCollector::new_with_default_servers();

        if let Some(stun_servers) = config_fs.get_stun_servers() {
            stun_info_collector.set_stun_servers(stun_servers);
        } else {
            stun_info_collector.set_stun_servers(StunInfoCollector::get_default_servers());
        }

        if let Some(stun_servers) = config_fs.get_stun_servers_v6() {
            stun_info_collector.set_stun_servers_v6(stun_servers);
        } else {
            stun_info_collector.set_stun_servers_v6(StunInfoCollector::get_default_servers_v6());
        }

        let stun_info_collector = Arc::new(stun_info_collector);

        let flags = config_fs.get_flags();
        let explicit_secure_mode = config_fs.get_secure_mode();
        let stealth_protocols =
            crate::common::stealth_registry::StealthProtocolSet::parse(&flags.stealth_protocols)
                .expect("stealth_protocols is validated while loading configuration");
        crate::common::config::TomlConfigLoader::warn_stealth_configuration(
            &flags,
            explicit_secure_mode.as_ref(),
            network.network_secret.as_deref(),
            &stealth_protocols,
        );

        let base_feature_flags = PeerFeatureFlag::default();
        let feature_flags = Self::derive_feature_flags(
            &flags,
            base_feature_flags.clone(),
            Self::stealth_enabled_for_config(
                &flags,
                is_effective_secure_mode_enabled(
                    explicit_secure_mode.as_ref(),
                    flags.stealth_mode,
                    network.network_secret.as_deref(),
                ),
                network.network_secret.as_deref(),
            ),
        );

        let credential_storage_path = config_fs.get_credential_file();
        let credential_manager = Arc::new(CredentialManager::new(credential_storage_path));

        GlobalCtx {
            inst_name: config_fs.get_inst_name(),
            id,
            config: Box::new(config_fs),
            net_ns: net_ns.clone(),
            network,
            resolved_nic_backend: std::sync::OnceLock::new(),
            derived_secure_mode: OnceLock::new(),

            event_bus,
            cached_ipv4: AtomicCell::new(None),
            cached_ipv6: AtomicCell::new(None),
            public_ipv6_lease: AtomicCell::new(None),
            public_ipv6_routes: Mutex::new(BTreeSet::new()),
            cached_proxy_cidrs: AtomicCell::new(None),

            ip_collector: Mutex::new(Some(Arc::new(IPCollector::new(
                net_ns,
                stun_info_collector.clone(),
            )))),

            hostname: Mutex::new(hostname),

            stun_info_collection: Mutex::new(stun_info_collector),

            running_listeners: Mutex::new(Vec::new()),
            advertised_ipv6_public_addr_prefix: Mutex::new(None),

            flags: ArcSwap::new(Arc::new(flags)),
            protocol_loop_suppression: Mutex::new(
                [ProtocolLoopSuppressionSlot::default(); PROTOCOL_LOOP_STATE_SLOTS],
            ),
            underlay_breaker: Mutex::new(UnderlayBreakerState::default()),

            base_feature_flags: ArcSwap::new(Arc::new(base_feature_flags)),

            feature_flags: ArcSwap::new(Arc::new(feature_flags)),

            token_bucket_manager: TokenBucketManager::new(),

            stats_manager: Arc::new(StatsManager::new()),

            acl_filter: Arc::new(AclFilter::new()),

            credential_manager,

            trusted_keys: Arc::new(TrustedKeyMapManager::new()),
        }
    }

    pub fn requested_nic_backend(&self) -> NicBackend {
        self.config.get_nic_backend()
    }

    pub fn resolved_nic_backend(&self) -> Option<NicBackend> {
        self.resolved_nic_backend.get().copied()
    }

    pub fn commit_nic_backend(&self, backend: NicBackend) -> Result<(), NicBackend> {
        self.resolved_nic_backend.set(backend)
    }

    pub fn subscribe(&self) -> EventBusSubscriber {
        self.event_bus.subscribe()
    }

    pub fn issue_event(&self, event: GlobalCtxEvent) {
        if let Err(e) = self.event_bus.send(event.clone()) {
            tracing::warn!(
                "Failed to send event: {:?}, error: {:?}, receiver count: {}",
                event,
                e,
                self.event_bus.receiver_count()
            );
        }
    }

    pub fn check_network_in_whitelist(&self, network_name: &str) -> Result<(), anyhow::Error> {
        if self
            .get_flags()
            .relay_network_whitelist
            .split(" ")
            .map(wildmatch::WildMatch::new)
            .any(|wl| wl.matches(network_name))
        {
            Ok(())
        } else {
            Err(anyhow::anyhow!("network {} not in whitelist", network_name))
        }
    }

    pub fn get_ipv4(&self) -> Option<cidr::Ipv4Inet> {
        if let Some(ret) = self.cached_ipv4.load() {
            return Some(ret);
        }
        let addr = self.config.get_ipv4();
        self.cached_ipv4.store(addr);
        addr
    }

    pub fn set_ipv4(&self, addr: Option<cidr::Ipv4Inet>) {
        self.config.set_ipv4(addr);
        self.cached_ipv4.store(None);
    }

    pub fn get_ipv6(&self) -> Option<cidr::Ipv6Inet> {
        if let Some(ret) = self.cached_ipv6.load() {
            return Some(ret);
        }
        let addr = self.config.get_ipv6();
        self.cached_ipv6.store(addr);
        addr
    }

    pub fn set_ipv6(&self, addr: Option<cidr::Ipv6Inet>) {
        self.config.set_ipv6(addr);
        self.cached_ipv6.store(None);
    }

    pub fn get_public_ipv6_lease(&self) -> Option<cidr::Ipv6Inet> {
        self.public_ipv6_lease.load()
    }

    pub fn set_public_ipv6_lease(&self, addr: Option<cidr::Ipv6Inet>) {
        self.public_ipv6_lease.store(addr);
    }

    pub fn set_public_ipv6_routes(&self, routes: BTreeSet<cidr::Ipv6Inet>) {
        *self.public_ipv6_routes.lock().unwrap() =
            routes.into_iter().map(|route| route.address()).collect();
    }

    pub fn is_ip_local_ipv6(&self, ip: &std::net::Ipv6Addr) -> bool {
        self.get_ipv6().map(|x| x.address() == *ip).unwrap_or(false)
            || self
                .get_public_ipv6_lease()
                .map(|x| x.address() == *ip)
                .unwrap_or(false)
    }

    pub fn is_ip_easytier_managed_ipv6(&self, ip: &std::net::Ipv6Addr) -> bool {
        self.is_ip_local_ipv6(ip) || self.public_ipv6_routes.lock().unwrap().contains(ip)
    }

    pub fn get_advertised_ipv6_public_addr_prefix(&self) -> Option<cidr::Ipv6Cidr> {
        *self.advertised_ipv6_public_addr_prefix.lock().unwrap()
    }

    pub fn set_advertised_ipv6_public_addr_prefix(&self, prefix: Option<cidr::Ipv6Cidr>) -> bool {
        let mut guard = self.advertised_ipv6_public_addr_prefix.lock().unwrap();
        if *guard == prefix {
            return false;
        }

        *guard = prefix;
        true
    }

    pub fn get_id(&self) -> uuid::Uuid {
        self.config.get_id()
    }

    pub fn is_ip_in_same_network(&self, ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => self.get_ipv4().map(|x| x.contains(v4)).unwrap_or(false),
            IpAddr::V6(v6) => self.get_ipv6().map(|x| x.contains(v6)).unwrap_or(false),
        }
    }

    pub fn is_ip_local_virtual_ip(&self, ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => self.get_ipv4().map(|x| x.address() == *v4).unwrap_or(false),
            IpAddr::V6(v6) => self.is_ip_local_ipv6(v6),
        }
    }

    pub fn get_network_identity(&self) -> NetworkIdentity {
        self.config.get_network_identity()
    }

    pub fn get_secret_proof(&self, challenge: &[u8]) -> Option<Hmac<Sha256>> {
        let network_secret = self.get_network_identity().network_secret?;
        let key = network_secret.as_bytes();
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(b"easytier secret proof");
        mac.update(challenge);
        Some(mac)
    }

    pub fn get_network_name(&self) -> String {
        self.get_network_identity().network_name
    }

    pub fn get_ip_collector(&self) -> Arc<IPCollector> {
        self.ip_collector.lock().unwrap().as_ref().unwrap().clone()
    }

    pub fn get_hostname(&self) -> String {
        return self.hostname.lock().unwrap().clone();
    }

    pub fn set_hostname(&self, hostname: String) {
        *self.hostname.lock().unwrap() = hostname;
    }

    pub fn get_stun_info_collector(&self) -> Arc<dyn StunInfoCollectorTrait> {
        self.stun_info_collection.lock().unwrap().clone()
    }

    pub fn replace_stun_info_collector(&self, collector: Box<dyn StunInfoCollectorTrait>) {
        let arc_collector: Arc<dyn StunInfoCollectorTrait> = Arc::new(collector);
        *self.stun_info_collection.lock().unwrap() = arc_collector.clone();

        // rebuild the ip collector
        *self.ip_collector.lock().unwrap() = Some(Arc::new(IPCollector::new(
            self.net_ns.clone(),
            arc_collector,
        )));
    }

    pub fn get_running_listeners(&self) -> Vec<url::Url> {
        self.running_listeners.lock().unwrap().clone()
    }

    pub fn add_running_listener(&self, url: url::Url) {
        let mut l = self.running_listeners.lock().unwrap();
        if !l.contains(&url) {
            l.push(url);
        }
    }

    pub fn get_vpn_portal_cidr(&self) -> Option<cidr::Ipv4Cidr> {
        self.config.get_vpn_portal_config().map(|x| x.client_cidr)
    }

    pub fn get_flags(&self) -> Flags {
        self.flags.load().as_ref().clone()
    }

    pub fn get_effective_secure_mode(&self) -> Option<SecureModeConfig> {
        if let Some(explicit) = self.config.get_secure_mode() {
            return explicit.enabled.then_some(explicit);
        }

        let flags = self.get_flags();
        let secret = self.get_network_identity().network_secret;
        if !flags.stealth_mode || secret.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return None;
        }

        Some(
            self.derived_secure_mode
                .get_or_init(|| {
                    process_secure_mode_cfg(SecureModeConfig {
                        enabled: true,
                        local_private_key: None,
                        local_public_key: None,
                    })
                    .expect("generated secure mode keypair must be valid")
                })
                .clone(),
        )
    }

    pub fn get_secure_mode_for_tunnel(&self, stealth_protected: bool) -> Option<SecureModeConfig> {
        if let Some(explicit) = self.config.get_secure_mode() {
            return explicit.enabled.then_some(explicit);
        }
        stealth_protected
            .then(|| self.get_effective_secure_mode())
            .flatten()
    }

    pub fn is_explicit_secure_mode_enabled(&self) -> bool {
        self.config
            .get_secure_mode()
            .is_some_and(|secure_mode| secure_mode.enabled)
    }

    pub fn is_secure_mode_enabled(&self) -> bool {
        self.get_effective_secure_mode()
            .is_some_and(|secure_mode| secure_mode.enabled)
    }

    pub fn set_flags(&self, flags: Flags) {
        self.config.set_flags(flags);
        let flags = self.config.get_flags();
        self.feature_flags
            .store(Arc::new(Self::derive_feature_flags(
                &flags,
                self.base_feature_flags.load().as_ref().clone(),
                Self::stealth_enabled_for_config(
                    &flags,
                    is_effective_secure_mode_enabled(
                        self.config.get_secure_mode().as_ref(),
                        flags.stealth_mode,
                        self.get_network_identity().network_secret.as_deref(),
                    ),
                    self.get_network_identity().network_secret.as_deref(),
                ),
            )));
        self.flags.store(Arc::new(flags));
    }

    fn protocol_loop_slot_index(scheme: IpScheme, scope: ProtocolLoopScope) -> usize {
        let idx = (scheme.loop_avoidance_bit().trailing_zeros() as usize)
            * PROTOCOL_LOOP_SCOPE_COUNT
            + scope.index();
        debug_assert!(idx < PROTOCOL_LOOP_STATE_SLOTS);
        idx
    }

    fn clear_expired_protocol_loop_slot(slot: &mut ProtocolLoopSuppressionSlot, now_secs: u64) {
        if slot.suppressed_until_secs != 0 && now_secs >= slot.suppressed_until_secs {
            *slot = ProtocolLoopSuppressionSlot::default();
        }
    }

    fn record_protocol_self_loop_at(
        &self,
        scheme: IpScheme,
        scope: ProtocolLoopScope,
        now_secs: u64,
    ) -> bool {
        let idx = Self::protocol_loop_slot_index(scheme, scope);
        let mut slots = self.protocol_loop_suppression.lock().unwrap();
        let slot = &mut slots[idx];
        Self::clear_expired_protocol_loop_slot(slot, now_secs);

        if slot.suppressed_until_secs != 0 {
            slot.suppressed_until_secs = now_secs + Self::PROTOCOL_LOOP_SUPPRESS_SECS;
            tracing::warn!(
                ?scheme,
                ?scope,
                suppress_until_secs = slot.suppressed_until_secs,
                "extend underlay protocol suppression after repeated self-loop detection"
            );
            return true;
        }

        if slot.first_hit_at_secs == 0
            || now_secs.saturating_sub(slot.first_hit_at_secs)
                > Self::PROTOCOL_LOOP_STRIKE_WINDOW_SECS
        {
            slot.first_hit_at_secs = now_secs;
            slot.strike_count = 1;
        } else {
            slot.strike_count = slot.strike_count.saturating_add(1);
        }

        if slot.strike_count >= Self::PROTOCOL_LOOP_STRIKE_THRESHOLD {
            slot.suppressed_until_secs = now_secs + Self::PROTOCOL_LOOP_SUPPRESS_SECS;
            tracing::warn!(
                ?scheme,
                ?scope,
                strikes = slot.strike_count,
                suppress_for_secs = Self::PROTOCOL_LOOP_SUPPRESS_SECS,
                "suppress underlay protocol for this runtime after repeated self-loop detection"
            );
            true
        } else {
            tracing::info!(
                ?scheme,
                ?scope,
                strikes = slot.strike_count,
                threshold = Self::PROTOCOL_LOOP_STRIKE_THRESHOLD,
                "recorded underlay self-loop signal without suppressing protocol yet"
            );
            false
        }
    }

    fn is_protocol_loop_suppressed_at(
        &self,
        scheme: IpScheme,
        scope: ProtocolLoopScope,
        now_secs: u64,
    ) -> bool {
        let idx = Self::protocol_loop_slot_index(scheme, scope);
        let mut slots = self.protocol_loop_suppression.lock().unwrap();
        let slot = &mut slots[idx];
        Self::clear_expired_protocol_loop_slot(slot, now_secs);
        slot.suppressed_until_secs != 0
    }

    pub fn record_protocol_self_loop(&self, scheme: IpScheme, scope: ProtocolLoopScope) -> bool {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.record_protocol_self_loop_at(scheme, scope, now_secs)
    }

    pub fn is_protocol_loop_suppressed(&self, scheme: IpScheme, scope: ProtocolLoopScope) -> bool {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.is_protocol_loop_suppressed_at(scheme, scope, now_secs)
    }

    fn underlay_breaker_enabled(&self) -> bool {
        self.get_flags().underlay_candidate_guard
    }

    fn prune_underlay_breaker_locked(
        entries: &mut HashMap<UnderlayBreakerKey, UnderlayBreakerEntry>,
        now_secs: u64,
    ) {
        entries.retain(|_, entry| {
            if entry.blocked_until_secs != 0 {
                if now_secs <= entry.blocked_until_secs {
                    return true;
                }
                if now_secs.saturating_sub(entry.blocked_until_secs)
                    <= UNDERLAY_BREAKER_MAX_TTL_SECS
                {
                    return true;
                }
                if entry.half_open
                    && now_secs.saturating_sub(entry.half_open_at_secs)
                        <= UNDERLAY_BREAKER_HALF_OPEN_TIMEOUT_SECS
                {
                    return true;
                }
            }

            if entry.first_hard_at_secs != 0
                && now_secs.saturating_sub(entry.first_hard_at_secs)
                    <= UNDERLAY_BREAKER_STRIKE_WINDOW_SECS
            {
                return true;
            }

            entry.soft_strikes != 0
                && now_secs.saturating_sub(entry.updated_at_secs) <= UNDERLAY_BREAKER_SOFT_TTL_SECS
        });
    }

    fn evict_underlay_breaker_if_full(
        entries: &mut HashMap<UnderlayBreakerKey, UnderlayBreakerEntry>,
    ) {
        if entries.len() < UNDERLAY_BREAKER_CAPACITY {
            return;
        }

        let Some(oldest_key) = entries
            .iter()
            .min_by_key(|(_, entry)| entry.updated_at_secs)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        entries.remove(&oldest_key);
    }

    fn block_underlay_breaker_entry(entry: &mut UnderlayBreakerEntry, now_secs: u64) -> u64 {
        let ttl = if entry.backoff_secs == 0 {
            UNDERLAY_BREAKER_INITIAL_TTL_SECS
        } else {
            entry
                .backoff_secs
                .saturating_mul(2)
                .min(UNDERLAY_BREAKER_MAX_TTL_SECS)
        };
        entry.backoff_secs = ttl;
        entry.blocked_until_secs = now_secs.saturating_add(ttl);
        entry.hard_strikes = 0;
        entry.first_hard_at_secs = 0;
        entry.half_open = false;
        entry.half_open_at_secs = 0;
        entry.half_open_lease_id = None;
        entry.updated_at_secs = now_secs;
        ttl
    }

    fn log_underlay_breaker_event(
        key: &UnderlayBreakerKey,
        reason: &str,
        strike_kind: Option<UnderlayBreakerStrikeKind>,
        ttl: Option<u64>,
        half_open: bool,
        trace: Option<&UnderlayBreakerTrace>,
        log: UnderlayBreakerLog,
    ) {
        let trace = trace.cloned().unwrap_or_default();
        if log.warn {
            tracing::warn!(
                ?key,
                peer_id = ?key.peer_id(),
                expected_peer_id = ?trace.expected_peer_id,
                actual_peer_id = ?trace.actual_peer_id,
                remote_addr = ?key.remote_addr(),
                scheme = ?key.scheme(),
                scope = ?key.scope(),
                reason,
                ?strike_kind,
                ?ttl,
                local_ip = ?trace.local_ip,
                ifname = ?trace.ifname,
                half_open,
                event = log.message,
                "underlay breaker event"
            );
        } else {
            tracing::debug!(
                ?key,
                peer_id = ?key.peer_id(),
                expected_peer_id = ?trace.expected_peer_id,
                actual_peer_id = ?trace.actual_peer_id,
                remote_addr = ?key.remote_addr(),
                scheme = ?key.scheme(),
                scope = ?key.scope(),
                reason,
                ?strike_kind,
                ?ttl,
                local_ip = ?trace.local_ip,
                ifname = ?trace.ifname,
                half_open,
                event = log.message,
                "underlay breaker event"
            );
        }
    }

    fn record_underlay_breaker_strike_at(
        &self,
        key: UnderlayBreakerKey,
        kind: UnderlayBreakerStrikeKind,
        reason: &'static str,
        trace: Option<UnderlayBreakerTrace>,
        now_secs: u64,
    ) -> bool {
        if !self.underlay_breaker_enabled() {
            return false;
        }

        let mut state = self.underlay_breaker.lock().unwrap();
        Self::prune_underlay_breaker_locked(&mut state.entries, now_secs);
        if !state.entries.contains_key(&key) {
            Self::evict_underlay_breaker_if_full(&mut state.entries);
        }
        let entry = state.entries.entry(key.clone()).or_default();
        entry.updated_at_secs = now_secs;

        match kind {
            UnderlayBreakerStrikeKind::Soft => {
                entry.soft_strikes = entry.soft_strikes.saturating_add(1);
                Self::log_underlay_breaker_event(
                    &key,
                    reason,
                    Some(kind),
                    None,
                    entry.half_open,
                    trace.as_ref(),
                    UnderlayBreakerLog {
                        warn: false,
                        message: "recorded soft underlay loopback signal",
                    },
                );
                false
            }
            UnderlayBreakerStrikeKind::Hard => {
                if entry.half_open {
                    let ttl = Self::block_underlay_breaker_entry(entry, now_secs);
                    Self::log_underlay_breaker_event(
                        &key,
                        reason,
                        Some(kind),
                        Some(ttl),
                        false,
                        trace.as_ref(),
                        UnderlayBreakerLog {
                            warn: true,
                            message: "underlay breaker half-open attempt failed; re-blocking key",
                        },
                    );
                    return true;
                }

                if entry.first_hard_at_secs == 0
                    || now_secs.saturating_sub(entry.first_hard_at_secs)
                        > UNDERLAY_BREAKER_STRIKE_WINDOW_SECS
                {
                    entry.first_hard_at_secs = now_secs;
                    entry.hard_strikes = 1;
                } else {
                    entry.hard_strikes = entry.hard_strikes.saturating_add(1);
                }

                if entry.hard_strikes >= UNDERLAY_BREAKER_STRIKE_THRESHOLD {
                    let ttl = Self::block_underlay_breaker_entry(entry, now_secs);
                    Self::log_underlay_breaker_event(
                        &key,
                        reason,
                        Some(kind),
                        Some(ttl),
                        false,
                        trace.as_ref(),
                        UnderlayBreakerLog {
                            warn: true,
                            message: "underlay breaker blocked key after repeated hard signals",
                        },
                    );
                    true
                } else {
                    Self::log_underlay_breaker_event(
                        &key,
                        reason,
                        Some(kind),
                        None,
                        false,
                        trace.as_ref(),
                        UnderlayBreakerLog {
                            warn: false,
                            message: "recorded hard underlay loopback signal",
                        },
                    );
                    false
                }
            }
        }
    }

    pub fn record_underlay_breaker_strike(
        &self,
        key: UnderlayBreakerKey,
        kind: UnderlayBreakerStrikeKind,
        reason: &'static str,
        trace: Option<UnderlayBreakerTrace>,
    ) -> bool {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.record_underlay_breaker_strike_at(key, kind, reason, trace, now_secs)
    }

    fn next_underlay_lease_id(state: &mut UnderlayBreakerState) -> u64 {
        state.next_lease_id = state.next_lease_id.wrapping_add(1);
        if state.next_lease_id == 0 {
            state.next_lease_id = 1;
        }
        state.next_lease_id
    }

    fn settle_underlay_half_open_timeout(
        key: &UnderlayBreakerKey,
        entry: &mut UnderlayBreakerEntry,
        now_secs: u64,
    ) {
        if !entry.half_open
            || now_secs.saturating_sub(entry.half_open_at_secs)
                <= UNDERLAY_BREAKER_HALF_OPEN_TIMEOUT_SECS
        {
            return;
        }

        let ttl = Self::block_underlay_breaker_entry(entry, now_secs);
        Self::log_underlay_breaker_event(
            key,
            "half_open_timeout",
            None,
            Some(ttl),
            false,
            None,
            UnderlayBreakerLog {
                warn: true,
                message: "underlay breaker half-open attempt timed out; re-blocking key",
            },
        );
    }

    fn try_begin_underlay_attempt_at(
        self: &Arc<Self>,
        keys: &[UnderlayBreakerKey],
        now_secs: u64,
    ) -> Result<UnderlayPreflightGuard, UnderlayBreakerGateError> {
        if !self.underlay_breaker_enabled() {
            return Ok(UnderlayPreflightGuard::new(self.clone(), 0, Vec::new()));
        }

        let mut unique_keys = Vec::with_capacity(keys.len());
        for key in keys {
            if !unique_keys.contains(key) {
                unique_keys.push(key.clone());
            }
        }

        let mut state = self.underlay_breaker.lock().unwrap();
        for key in &unique_keys {
            if let Some(entry) = state.entries.get_mut(key) {
                Self::settle_underlay_half_open_timeout(key, entry, now_secs);
            }
        }
        Self::prune_underlay_breaker_locked(&mut state.entries, now_secs);

        for key in &unique_keys {
            let Some(entry) = state.entries.get(key) else {
                continue;
            };
            if entry.blocked_until_secs == 0 {
                continue;
            }
            if now_secs < entry.blocked_until_secs || entry.half_open {
                let reason = if entry.half_open {
                    "half_open_in_flight"
                } else {
                    "breaker_ttl_active"
                };
                Self::log_underlay_breaker_event(
                    key,
                    reason,
                    None,
                    (now_secs < entry.blocked_until_secs)
                        .then_some(entry.blocked_until_secs.saturating_sub(now_secs)),
                    entry.half_open,
                    None,
                    UnderlayBreakerLog {
                        warn: false,
                        message: "underlay breaker gated atomic connection attempt",
                    },
                );
                return Err(UnderlayBreakerGateError { key: key.clone() });
            }
        }

        let lease_id = Self::next_underlay_lease_id(&mut state);
        let mut acquired = Vec::new();
        for key in &unique_keys {
            let Some(entry) = state.entries.get_mut(key) else {
                continue;
            };
            if entry.blocked_until_secs == 0 {
                continue;
            }
            entry.half_open = true;
            entry.half_open_at_secs = now_secs;
            entry.half_open_lease_id = Some(lease_id);
            entry.updated_at_secs = now_secs;
            acquired.push(key.clone());
            Self::log_underlay_breaker_event(
                key,
                "half_open_release",
                None,
                None,
                true,
                None,
                UnderlayBreakerLog {
                    warn: false,
                    message: "underlay breaker released key in atomic half-open attempt",
                },
            );
        }
        drop(state);

        Ok(UnderlayPreflightGuard::new(
            self.clone(),
            lease_id,
            acquired,
        ))
    }

    pub fn try_begin_underlay_attempt(
        self: &Arc<Self>,
        keys: &[UnderlayBreakerKey],
    ) -> Result<UnderlayPreflightGuard, UnderlayBreakerGateError> {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.try_begin_underlay_attempt_at(keys, now_secs)
    }

    #[cfg(test)]
    fn is_underlay_breaker_gated_at(
        self: &Arc<Self>,
        key: &UnderlayBreakerKey,
        now_secs: u64,
    ) -> bool {
        match self.try_begin_underlay_attempt_at(std::slice::from_ref(key), now_secs) {
            Ok(mut guard) => {
                guard.commit();
                false
            }
            Err(_) => true,
        }
    }

    pub fn is_underlay_attempt_blocked(&self, keys: &[UnderlayBreakerKey]) -> bool {
        if !self.underlay_breaker_enabled() {
            return false;
        }
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut state = self.underlay_breaker.lock().unwrap();
        for key in keys {
            if let Some(entry) = state.entries.get_mut(key) {
                Self::settle_underlay_half_open_timeout(key, entry, now_secs);
            }
        }
        Self::prune_underlay_breaker_locked(&mut state.entries, now_secs);
        keys.iter().any(|key| {
            state.entries.get(key).is_some_and(|entry| {
                entry.blocked_until_secs != 0
                    && (now_secs < entry.blocked_until_secs || entry.half_open)
            })
        })
    }

    fn rollback_underlay_preflight(&self, lease_id: u64, keys: &[UnderlayBreakerKey]) {
        let mut state = self.underlay_breaker.lock().unwrap();
        for key in keys {
            let Some(entry) = state.entries.get_mut(key) else {
                continue;
            };
            if entry.half_open_lease_id != Some(lease_id) {
                continue;
            }
            entry.half_open = false;
            entry.half_open_at_secs = 0;
            entry.half_open_lease_id = None;
            Self::log_underlay_breaker_event(
                key,
                "preflight_cancelled",
                None,
                None,
                false,
                None,
                UnderlayBreakerLog {
                    warn: false,
                    message: "underlay breaker rolled back cancelled preflight lease",
                },
            );
        }
    }

    pub fn clear_underlay_breaker(&self, key: &UnderlayBreakerKey, reason: &'static str) {
        if !self.underlay_breaker_enabled() {
            return;
        }

        let mut state = self.underlay_breaker.lock().unwrap();
        if state.entries.remove(key).is_some() {
            Self::log_underlay_breaker_event(
                key,
                reason,
                None,
                None,
                false,
                None,
                UnderlayBreakerLog {
                    warn: false,
                    message: "underlay breaker cleared key after successful connection",
                },
            );
        }
    }

    pub fn flags_arc(&self) -> Arc<Flags> {
        self.flags.load_full()
    }

    pub fn get_128_key(&self) -> [u8; 16] {
        let mut key = [0u8; 16];
        let secret = self
            .config
            .get_network_identity()
            .network_secret
            .unwrap_or_default();
        // fill key according to network secret
        let mut hasher = DefaultHasher::new();
        hasher.write(secret.as_bytes());
        key[0..8].copy_from_slice(&hasher.finish().to_be_bytes());
        hasher.write(&key[0..8]);
        key[8..16].copy_from_slice(&hasher.finish().to_be_bytes());
        hasher.write(&key[0..16]);
        key
    }

    pub fn get_256_key(&self) -> [u8; 32] {
        let mut key = [0u8; 32];
        let secret = self
            .config
            .get_network_identity()
            .network_secret
            .unwrap_or_default();
        // fill key according to network secret
        let mut hasher = DefaultHasher::new();
        hasher.write(secret.as_bytes());
        hasher.write(b"easytier-256bit-key"); // 添加固定盐值以区分128位和256位密钥

        // 生成32字节密钥
        for i in 0..4 {
            let chunk_start = i * 8;
            let chunk_end = chunk_start + 8;
            hasher.write(&key[0..chunk_start]);
            hasher.write(&[i as u8]); // 添加索引以确保每个8字节块都不同
            key[chunk_start..chunk_end].copy_from_slice(&hasher.finish().to_be_bytes());
        }
        key
    }

    pub fn enable_exit_node(&self) -> bool {
        self.flags.load().enable_exit_node || cfg!(target_env = "ohos")
    }

    pub fn proxy_forward_by_system(&self) -> bool {
        self.flags.load().proxy_forward_by_system
    }

    pub fn no_tun(&self) -> bool {
        self.flags.load().no_tun
    }

    pub fn get_feature_flags(&self) -> PeerFeatureFlag {
        let mut feature_flags = self.feature_flags.load().as_ref().clone();
        let flags = self.flags.load();
        let stealth_enabled = Self::stealth_enabled_for_config(
            flags.as_ref(),
            self.is_secure_mode_enabled(),
            self.get_network_identity().network_secret.as_deref(),
        );
        let protocols =
            crate::common::stealth_registry::StealthProtocolSet::parse(&flags.stealth_protocols)
                .expect("stealth_protocols is validated while loading configuration");
        feature_flags.stealth_supported = stealth_enabled
            && protocols.contains(
                flags.stealth_mode,
                crate::common::stealth_registry::StealthProtocol::Udp,
            );
        feature_flags.stealth_capabilities = if stealth_enabled {
            protocols.capabilities(flags.stealth_mode)
        } else {
            Vec::new()
        };
        feature_flags
    }

    /// Replace the runtime/base advertised flags as a complete snapshot.
    ///
    /// This is intended for foreign scoped contexts that inherit an already
    /// computed feature-flag snapshot from their parent. Most callers should use
    /// a narrower setter so they do not accidentally overwrite unrelated runtime
    /// state.
    pub fn set_base_advertised_feature_flags(&self, feature_flags: PeerFeatureFlag) {
        self.base_feature_flags
            .store(Arc::new(feature_flags.clone()));
        let flags = self.flags.load();
        self.feature_flags
            .store(Arc::new(Self::apply_disable_relay_data_flag(
                flags.as_ref(),
                feature_flags,
            )));
    }

    /// Set the avoid-relay preference that is independent of disable_relay_data.
    ///
    /// disable_relay_data still forces the effective advertised flag to true,
    /// but this base preference is preserved when that config flag is toggled.
    pub fn set_avoid_relay_data_preference(&self, avoid_relay_data: bool) -> bool {
        let mut base_feature_flags = self.base_feature_flags.load().as_ref().clone();
        base_feature_flags.avoid_relay_data = avoid_relay_data;
        self.base_feature_flags.store(Arc::new(base_feature_flags));

        let mut feature_flags = self.feature_flags.load().as_ref().clone();
        let previous = feature_flags.avoid_relay_data;
        feature_flags.avoid_relay_data = avoid_relay_data || self.flags.load().disable_relay_data;
        self.feature_flags.store(Arc::new(feature_flags.clone()));
        previous != feature_flags.avoid_relay_data
    }

    /// Set the runtime IPv6-provider advertised bit without touching
    /// config-derived feature flags.
    pub fn set_ipv6_public_addr_provider_feature_flag(&self, enabled: bool) -> bool {
        let mut base_feature_flags = self.base_feature_flags.load().as_ref().clone();
        base_feature_flags.ipv6_public_addr_provider = enabled;
        self.base_feature_flags.store(Arc::new(base_feature_flags));

        let mut feature_flags = self.feature_flags.load().as_ref().clone();
        if feature_flags.ipv6_public_addr_provider == enabled {
            return false;
        }

        feature_flags.ipv6_public_addr_provider = enabled;
        self.feature_flags.store(Arc::new(feature_flags));
        true
    }

    pub fn token_bucket_manager(&self) -> &TokenBucketManager {
        &self.token_bucket_manager
    }

    pub fn stats_manager(&self) -> &Arc<StatsManager> {
        &self.stats_manager
    }

    pub fn get_acl_filter(&self) -> &Arc<AclFilter> {
        &self.acl_filter
    }

    pub fn get_credential_manager(&self) -> &Arc<CredentialManager> {
        &self.credential_manager
    }

    /// Check if a public key is trusted using two-level lookup:
    /// 1. OSPF propagated trusted_keys (lock-free)
    /// 2. Local credential_manager
    pub fn is_pubkey_trusted(&self, pubkey: &[u8], network_name: &str) -> bool {
        // First level: check OSPF propagated keys (lock-free)
        if self.trusted_keys.verify_trusted_key(pubkey, network_name) {
            return true;
        }

        // Second level: check local credential_manager if in the same network
        if network_name == self.get_network_name() {
            return self.credential_manager.is_pubkey_trusted(pubkey);
        }

        false
    }

    pub fn is_pubkey_trusted_with_source(
        &self,
        pubkey: &[u8],
        network_name: &str,
        source: TrustedKeySource,
    ) -> bool {
        self.trusted_keys
            .verify_trusted_key_with_source(pubkey, network_name, Some(source))
    }

    /// Atomically replace all OSPF trusted keys with a new set
    /// Called by OSPF route layer after each route update
    pub fn update_trusted_keys(&self, keys: TrustedKeyMap, network_name: &str) {
        self.trusted_keys.update_trusted_keys(network_name, keys);
    }

    pub fn remove_trusted_keys(&self, network_name: &str) {
        self.trusted_keys.remove_trusted_keys(network_name);
    }

    pub fn list_trusted_keys(&self, network_name: &str) -> Vec<(Vec<u8>, TrustedKeyMetadata)> {
        self.trusted_keys.list_trusted_keys(network_name)
    }

    pub fn get_acl_groups(&self, peer_id: PeerId) -> Vec<PeerGroupInfo> {
        use std::collections::HashSet;
        self.config
            .get_acl()
            .and_then(|acl| acl.acl_v1)
            .and_then(|acl_v1| acl_v1.group)
            .map_or_else(Vec::new, |group| {
                let memberships: HashSet<_> = group.members.iter().collect();
                group
                    .declares
                    .iter()
                    .filter(|g| memberships.contains(&g.group_name))
                    .map(|g| {
                        PeerGroupInfo::generate_with_proof(
                            g.group_name.clone(),
                            g.group_secret.clone(),
                            peer_id,
                        )
                    })
                    .collect()
            })
    }

    pub fn get_acl_group_declarations(&self) -> Vec<GroupIdentity> {
        self.config
            .get_acl()
            .and_then(|acl| acl.acl_v1)
            .and_then(|acl_v1| acl_v1.group)
            .map_or_else(Vec::new, |group| group.declares.to_vec())
    }

    pub fn p2p_only(&self) -> bool {
        self.flags.load().p2p_only
    }

    pub fn latency_first(&self) -> bool {
        // NOTICE: p2p only is conflict with latency first
        let flags = self.flags.load();
        flags.latency_first && !flags.p2p_only
    }

    fn is_port_in_running_listeners(&self, port: u16, is_udp: bool) -> bool {
        self.running_listeners
            .lock()
            .unwrap()
            .iter()
            .any(|x| x.port() == Some(port) && matches_protocol!(x, Protocol::UDP) == is_udp)
    }

    #[tracing::instrument(ret, skip(self))]
    pub fn should_deny_proxy(&self, dst_addr: &SocketAddr, is_udp: bool) -> bool {
        self.should_deny_proxy_with_local_virtual_occupied_guard(dst_addr, is_udp, true)
    }

    #[tracing::instrument(ret, skip(self))]
    pub fn should_deny_proxy_with_local_virtual_occupied_guard(
        &self,
        dst_addr: &SocketAddr,
        is_udp: bool,
        deny_local_virtual_occupied: bool,
    ) -> bool {
        let _g = self.net_ns.guard();
        let ip = dst_addr.ip();
        // first check if ip is an EasyTier-managed local address
        // then try bind this ip, if succ means it is local ip
        let dst_is_local_et_ip = self.is_ip_local_virtual_ip(&ip);
        // this is an expensive operation, should be called sparingly
        // 1. tcp/kcp/quic call this only after proxy conn is established
        // 2. udp cache the result in nat entry
        let dst_is_local_phy_ip = std::net::UdpSocket::bind(format!("{}:0", ip)).is_ok();

        tracing::trace!(
            "check should_deny_proxy: dst_addr={}, dst_is_local_et_ip={}, dst_is_local_phy_ip={}, is_udp={}",
            dst_addr,
            dst_is_local_et_ip,
            dst_is_local_phy_ip,
            is_udp
        );

        if !dst_is_local_et_ip && !dst_is_local_phy_ip {
            return false;
        }

        // Always block our own internal listeners/RPC port to avoid proxy
        // loops back into EasyTier's own control plane.
        if self.is_port_in_running_listeners(dst_addr.port(), is_udp)
            || (!is_udp && protected_port::is_protected_tcp_port(dst_addr.port()))
        {
            return true;
        }

        // A destination that equals our own EasyTier virtual/leased address
        // (not just any local physical interface) can only be a legitimate
        // proxy target when it points at one of our own advertised listeners,
        // which is already handled above. Any other locally-bound port at
        // that exact address belongs to an unrelated process on this host
        // (for example a system-wide TUN/proxy tool such as Mihomo/Clash that
        // accepts connections on every local address via a wildcard bind).
        // Proxying into it would leak tunneled traffic into a foreign local
        // service and can create a feedback loop between the two processes,
        // so fail closed instead of silently connecting. This intentionally
        // does not apply to `dst_is_local_phy_ip`, since proxying to a real
        // service bound on this host's physical LAN address is the normal
        // `proxy_cidrs` exit-node use case.
        deny_local_virtual_occupied
            && dst_is_local_et_ip
            && is_local_port_occupied(dst_addr, is_udp)
    }
}

/// Returns true if some socket on this host is already bound to `dst_addr`,
/// detected by attempting to bind the exact address ourselves. This works
/// identically across platforms and does not require enumerating other
/// processes' sockets.
fn is_local_port_occupied(dst_addr: &SocketAddr, is_udp: bool) -> bool {
    let bind_result = if is_udp {
        std::net::UdpSocket::bind(dst_addr).map(|_| ())
    } else {
        std::net::TcpListener::bind(dst_addr).map(|_| ())
    };
    match bind_result {
        Ok(()) => false,
        Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => false,
        Err(_) => true,
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        common::{
            config::{NetworkIdentity, TomlConfigLoader},
            new_peer_id,
            stun::MockStunInfoCollector,
        },
        proto::common::NatType,
    };

    use super::*;

    #[tokio::test]
    async fn test_global_ctx() {
        let config = TomlConfigLoader::default();
        let global_ctx = Arc::new(GlobalCtx::new(config));

        let mut subscriber = global_ctx.subscribe();
        let peer_id = new_peer_id();
        global_ctx.issue_event(GlobalCtxEvent::PeerAdded(peer_id));
        global_ctx.issue_event(GlobalCtxEvent::PeerRemoved(peer_id));
        global_ctx.issue_event(GlobalCtxEvent::PeerConnAdded(PeerConnInfo::default()));
        global_ctx.issue_event(GlobalCtxEvent::PeerConnRemoved(PeerConnInfo::default()));

        assert_eq!(
            subscriber.recv().await.unwrap(),
            GlobalCtxEvent::PeerAdded(peer_id)
        );
        assert_eq!(
            subscriber.recv().await.unwrap(),
            GlobalCtxEvent::PeerRemoved(peer_id)
        );
        assert_eq!(
            subscriber.recv().await.unwrap(),
            GlobalCtxEvent::PeerConnAdded(PeerConnInfo::default())
        );
        assert_eq!(
            subscriber.recv().await.unwrap(),
            GlobalCtxEvent::PeerConnRemoved(PeerConnInfo::default())
        );
    }

    #[tokio::test]
    async fn trusted_key_source_lookup_is_precise() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        let network_name = "net1";
        let pubkey = vec![1; 32];

        global_ctx.update_trusted_keys(
            HashMap::from([(
                pubkey.clone(),
                TrustedKeyMetadata {
                    source: TrustedKeySource::OspfCredential,
                    expiry_unix: None,
                },
            )]),
            network_name,
        );

        assert!(global_ctx.is_pubkey_trusted(&pubkey, network_name));
        assert!(!global_ctx.is_pubkey_trusted_with_source(
            &pubkey,
            network_name,
            TrustedKeySource::OspfNode,
        ));
        assert!(global_ctx.is_pubkey_trusted_with_source(
            &pubkey,
            network_name,
            TrustedKeySource::OspfCredential,
        ));
    }

    #[tokio::test]
    async fn set_flags_keeps_derived_feature_flags_in_sync() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        let mut feature_flags = global_ctx.get_feature_flags();
        feature_flags.avoid_relay_data = true;
        feature_flags.is_public_server = true;
        global_ctx.set_base_advertised_feature_flags(feature_flags.clone());

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_kcp_input = true;
        flags.disable_relay_kcp = true;
        flags.disable_quic_input = true;
        flags.disable_relay_quic = true;
        flags.need_p2p = true;
        flags.disable_p2p = true;
        global_ctx.set_flags(flags);

        let feature_flags = global_ctx.get_feature_flags();
        assert!(!feature_flags.kcp_input);
        assert!(feature_flags.no_relay_kcp);
        assert!(!feature_flags.quic_input);
        assert!(feature_flags.no_relay_quic);
        assert!(feature_flags.need_p2p);
        assert!(feature_flags.disable_p2p);
        assert!(feature_flags.support_conn_list_sync);
        assert!(feature_flags.avoid_relay_data);
        assert!(feature_flags.is_public_server);
        assert!(!feature_flags.ipv6_public_addr_provider);
    }

    #[tokio::test]
    async fn proxy_capabilities_follow_compiled_features() {
        let global_ctx = Arc::new(GlobalCtx::new(TomlConfigLoader::default()));
        let feature_flags = global_ctx.get_feature_flags();

        assert_eq!(feature_flags.kcp_input, cfg!(feature = "kcp"));
        assert_eq!(feature_flags.quic_input, cfg!(feature = "quic"));
        assert_eq!(
            feature_flags.proxy_prepare_ack_version,
            if cfg!(any(feature = "kcp", feature = "quic")) {
                crate::common::constants::PROXY_PREPARE_ACK_VERSION
            } else {
                0
            }
        );
    }

    #[tokio::test]
    async fn set_base_advertised_feature_flags_applies_current_values() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        let feature_flags = PeerFeatureFlag {
            kcp_input: false,
            no_relay_kcp: true,
            quic_input: false,
            no_relay_quic: true,
            is_public_server: true,
            ..Default::default()
        };
        global_ctx.set_base_advertised_feature_flags(feature_flags.clone());

        assert_eq!(global_ctx.get_feature_flags(), feature_flags);
    }

    #[tokio::test]
    async fn set_base_advertised_feature_flags_keeps_disable_relay_data_effective() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = true;
        global_ctx.set_flags(flags);

        let mut feature_flags = global_ctx.get_feature_flags();
        feature_flags.avoid_relay_data = false;
        feature_flags.is_public_server = true;
        global_ctx.set_base_advertised_feature_flags(feature_flags);

        let advertised_feature_flags = global_ctx.get_feature_flags();
        assert!(advertised_feature_flags.avoid_relay_data);
        assert!(advertised_feature_flags.is_public_server);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = false;
        global_ctx.set_flags(flags);

        let advertised_feature_flags = global_ctx.get_feature_flags();
        assert!(!advertised_feature_flags.avoid_relay_data);
        assert!(advertised_feature_flags.is_public_server);
    }

    #[tokio::test]
    async fn disable_relay_data_sets_avoid_relay_feature_flag() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = true;
        global_ctx.set_flags(flags);

        assert!(global_ctx.get_feature_flags().avoid_relay_data);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = false;
        global_ctx.set_flags(flags);

        assert!(!global_ctx.get_feature_flags().avoid_relay_data);

        global_ctx.set_avoid_relay_data_preference(true);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = true;
        global_ctx.set_flags(flags);

        assert!(global_ctx.get_feature_flags().avoid_relay_data);

        let mut flags = global_ctx.get_flags().clone();
        flags.disable_relay_data = false;
        global_ctx.set_flags(flags);

        assert!(global_ctx.get_feature_flags().avoid_relay_data);
    }

    #[tokio::test]
    async fn should_deny_proxy_for_process_wide_rpc_port() {
        let _guard = protected_port::PROTECTED_TCP_PORTS_TEST_LOCK
            .lock()
            .unwrap();
        protected_port::clear_protected_tcp_ports_for_test();
        protected_port::register_protected_tcp_port(15888);

        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        let rpc_addr = SocketAddr::from(([127, 0, 0, 1], 15888));
        let other_tcp_addr = SocketAddr::from(([127, 0, 0, 1], 15889));

        assert!(global_ctx.should_deny_proxy(&rpc_addr, false));
        assert!(!global_ctx.should_deny_proxy(&rpc_addr, true));
        assert!(!global_ctx.should_deny_proxy(&other_tcp_addr, false));

        protected_port::clear_protected_tcp_ports_for_test();
    }

    #[tokio::test]
    async fn should_deny_proxy_for_third_party_listener_on_own_virtual_ip() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        global_ctx.set_ipv4(Some("127.0.0.1/8".parse().unwrap()));

        // Simulate an unrelated local process (e.g. a system-wide TUN/proxy
        // tool such as Mihomo/Clash) holding a listener on our own virtual
        // IP via a wildcard bind. EasyTier never registered this port as one
        // of its own listeners.
        let foreign_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let foreign_addr = SocketAddr::from((
            [127, 0, 0, 1],
            foreign_listener.local_addr().unwrap().port(),
        ));

        assert!(global_ctx.should_deny_proxy(&foreign_addr, false));
        assert!(
            !global_ctx.should_deny_proxy_with_local_virtual_occupied_guard(
                &foreign_addr,
                false,
                false
            )
        );

        drop(foreign_listener);
    }

    #[tokio::test]
    async fn should_allow_proxy_for_free_port_on_own_virtual_ip() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        global_ctx.set_ipv4(Some("127.0.0.1/8".parse().unwrap()));

        // Find an ephemeral port that is currently free, then confirm an
        // unoccupied port on our own virtual IP is still allowed (harmless;
        // the connect attempt would simply be refused).
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let free_port = probe.local_addr().unwrap().port();
        drop(probe);
        let free_addr = SocketAddr::from(([127, 0, 0, 1], free_port));

        assert!(!global_ctx.should_deny_proxy(&free_addr, false));
    }

    #[tokio::test]
    async fn virtual_ipv6_and_public_ipv6_lease_are_stored_separately() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        let virtual_ipv6 = "fd00::1/64".parse().unwrap();
        let public_ipv6 = "2001:db8::2/64".parse().unwrap();

        global_ctx.set_ipv6(Some(virtual_ipv6));
        global_ctx.set_public_ipv6_lease(Some(public_ipv6));

        assert_eq!(global_ctx.get_ipv6(), Some(virtual_ipv6));
        assert_eq!(global_ctx.get_public_ipv6_lease(), Some(public_ipv6));
    }

    #[tokio::test]
    async fn public_ipv6_lease_is_treated_as_local_ip() {
        let _guard = protected_port::PROTECTED_TCP_PORTS_TEST_LOCK
            .lock()
            .unwrap();
        protected_port::clear_protected_tcp_ports_for_test();

        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);
        let public_ipv6 = "2001:db8::2/64".parse().unwrap();
        let listener: url::Url = "tcp://[2001:db8::2]:11010".parse().unwrap();
        global_ctx.set_public_ipv6_lease(Some(public_ipv6));
        global_ctx.add_running_listener(listener);

        let ip = std::net::IpAddr::V6(public_ipv6.address());
        let socket = SocketAddr::from((public_ipv6.address(), 11010));

        assert!(global_ctx.is_ip_local_virtual_ip(&ip));
        assert!(global_ctx.should_deny_proxy(&socket, false));

        protected_port::clear_protected_tcp_ports_for_test();
    }

    #[tokio::test]
    async fn protocol_loop_suppression_requires_repeated_hits_and_expires() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        assert!(!global_ctx.record_protocol_self_loop_at(
            IpScheme::Udp,
            ProtocolLoopScope::Direct,
            100
        ));
        assert!(!global_ctx.is_protocol_loop_suppressed_at(
            IpScheme::Udp,
            ProtocolLoopScope::Direct,
            100
        ));

        assert!(global_ctx.record_protocol_self_loop_at(
            IpScheme::Udp,
            ProtocolLoopScope::Direct,
            110
        ));
        assert!(global_ctx.is_protocol_loop_suppressed_at(
            IpScheme::Udp,
            ProtocolLoopScope::Direct,
            111
        ));

        assert!(!global_ctx.is_protocol_loop_suppressed_at(
            IpScheme::Udp,
            ProtocolLoopScope::Direct,
            110 + GlobalCtx::PROTOCOL_LOOP_SUPPRESS_SECS + 1,
        ));
    }

    #[tokio::test]
    async fn protocol_loop_suppression_is_scope_isolated() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        assert!(!global_ctx.record_protocol_self_loop_at(
            IpScheme::Tcp,
            ProtocolLoopScope::Direct,
            200
        ));
        assert!(global_ctx.record_protocol_self_loop_at(
            IpScheme::Tcp,
            ProtocolLoopScope::Direct,
            201
        ));
        assert!(global_ctx.is_protocol_loop_suppressed_at(
            IpScheme::Tcp,
            ProtocolLoopScope::Direct,
            202
        ));
        assert!(!global_ctx.is_protocol_loop_suppressed_at(
            IpScheme::Tcp,
            ProtocolLoopScope::HolePunch,
            202,
        ));
    }

    #[tokio::test]
    async fn underlay_breaker_requires_repeated_hard_strikes_and_half_open_is_single_flight() {
        let config = TomlConfigLoader::default();
        let global_ctx = Arc::new(GlobalCtx::new(config));
        let key = UnderlayBreakerKey::endpoint(
            "198.51.100.1:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        );

        // Fire threshold-1 strikes at the same instant; none should trigger yet.
        for _ in 0..(UNDERLAY_BREAKER_STRIKE_THRESHOLD - 1) {
            assert!(!global_ctx.record_underlay_breaker_strike_at(
                key.clone(),
                UnderlayBreakerStrikeKind::Hard,
                "test",
                None,
                100
            ));
        }
        assert!(!global_ctx.is_underlay_breaker_gated_at(&key, 100));

        // The threshold-th strike triggers the breaker.
        let trigger_time = 100;
        assert!(global_ctx.record_underlay_breaker_strike_at(
            key.clone(),
            UnderlayBreakerStrikeKind::Hard,
            "test",
            None,
            trigger_time
        ));
        assert!(global_ctx.is_underlay_breaker_gated_at(&key, trigger_time + 1));

        // After the initial TTL expires the half-open probe is allowed.
        assert!(!global_ctx.is_underlay_breaker_gated_at(
            &key,
            trigger_time + UNDERLAY_BREAKER_INITIAL_TTL_SECS + 1
        ));
        // The half-open probe itself gates further attempts.
        assert!(global_ctx.is_underlay_breaker_gated_at(
            &key,
            trigger_time + UNDERLAY_BREAKER_INITIAL_TTL_SECS + 2
        ));

        // A failure during half-open re-blocks the key.
        assert!(global_ctx.record_underlay_breaker_strike_at(
            key.clone(),
            UnderlayBreakerStrikeKind::Hard,
            "test_half_open_failed",
            None,
            trigger_time + UNDERLAY_BREAKER_INITIAL_TTL_SECS + 3
        ));
        assert!(global_ctx.is_underlay_breaker_gated_at(
            &key,
            trigger_time + UNDERLAY_BREAKER_INITIAL_TTL_SECS + 4
        ));
    }

    fn block_test_underlay_key(
        global_ctx: &ArcGlobalCtx,
        key: &UnderlayBreakerKey,
        first_strike_at: u64,
    ) {
        // All strikes at the same instant — mirrors a real loopback storm.
        for _ in 0..u64::from(UNDERLAY_BREAKER_STRIKE_THRESHOLD) {
            global_ctx.record_underlay_breaker_strike_at(
                key.clone(),
                UnderlayBreakerStrikeKind::Hard,
                "test_block",
                None,
                first_strike_at,
            );
        }
    }

    #[tokio::test]
    async fn underlay_breaker_batch_gate_is_atomic_for_misaligned_ttls() {
        let global_ctx = Arc::new(GlobalCtx::new(TomlConfigLoader::default()));
        let peer_key = UnderlayBreakerKey::peer(42, IpScheme::Udp, UnderlayBreakerScope::Direct);
        let endpoint_key = UnderlayBreakerKey::endpoint(
            "198.51.100.42:11010".parse().unwrap(),
            IpScheme::Udp,
            UnderlayBreakerScope::Direct,
        );
        block_test_underlay_key(&global_ctx, &peer_key, 100);
        // Stagger the endpoint block so its TTL expires later than the peer key.
        let endpoint_block_start = 100 + UNDERLAY_BREAKER_INITIAL_TTL_SECS / 2;
        block_test_underlay_key(&global_ctx, &endpoint_key, endpoint_block_start);

        // Pick a time when peer_key TTL has expired but endpoint_key TTL has not.
        let peer_blocked_until = 100 + UNDERLAY_BREAKER_INITIAL_TTL_SECS;
        let endpoint_blocked_until = endpoint_block_start + UNDERLAY_BREAKER_INITIAL_TTL_SECS;
        let between_time = peer_blocked_until + 1;
        assert!(
            between_time < endpoint_blocked_until,
            "test requires staggered TTLs"
        );

        assert!(
            global_ctx
                .try_begin_underlay_attempt_at(
                    &[peer_key.clone(), endpoint_key.clone()],
                    between_time
                )
                .is_err()
        );
        assert!(
            !global_ctx
                .underlay_breaker
                .lock()
                .unwrap()
                .entries
                .get(&peer_key)
                .unwrap()
                .half_open
        );

        // After both TTLs expire, the atomic half-open probe should succeed.
        let both_expired = endpoint_blocked_until + 1;
        let guard = global_ctx
            .try_begin_underlay_attempt_at(&[peer_key.clone(), endpoint_key.clone()], both_expired)
            .unwrap();
        let lease_id = guard.lease_id();
        let state = global_ctx.underlay_breaker.lock().unwrap();
        assert_eq!(
            state.entries.get(&peer_key).unwrap().half_open_lease_id,
            Some(lease_id)
        );
        assert_eq!(
            state.entries.get(&endpoint_key).unwrap().half_open_lease_id,
            Some(lease_id)
        );
        drop(state);
        drop(guard);
    }

    #[tokio::test]
    async fn stale_underlay_preflight_guard_cannot_rollback_new_lease() {
        let global_ctx = Arc::new(GlobalCtx::new(TomlConfigLoader::default()));
        let key = UnderlayBreakerKey::endpoint(
            "198.51.100.43:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        );
        block_test_underlay_key(&global_ctx, &key, 100);
        let blocked_until = 100 + UNDERLAY_BREAKER_INITIAL_TTL_SECS;
        let old_guard = global_ctx
            .try_begin_underlay_attempt_at(std::slice::from_ref(&key), blocked_until + 1)
            .unwrap();
        global_ctx.record_underlay_breaker_strike_at(
            key.clone(),
            UnderlayBreakerStrikeKind::Hard,
            "test_reblock",
            None,
            blocked_until + 2,
        );
        let reblocked_until = blocked_until + 2 + UNDERLAY_BREAKER_INITIAL_TTL_SECS * 2;
        let new_guard = global_ctx
            .try_begin_underlay_attempt_at(std::slice::from_ref(&key), reblocked_until + 1)
            .unwrap();
        let new_lease_id = new_guard.lease_id();

        drop(old_guard);
        assert_eq!(
            global_ctx
                .underlay_breaker
                .lock()
                .unwrap()
                .entries
                .get(&key)
                .unwrap()
                .half_open_lease_id,
            Some(new_lease_id)
        );
        drop(new_guard);
    }

    #[tokio::test]
    async fn cancelled_preflight_rolls_back_without_backoff_but_committed_timeout_reblocks() {
        let global_ctx = Arc::new(GlobalCtx::new(TomlConfigLoader::default()));
        let key = UnderlayBreakerKey::endpoint(
            "198.51.100.44:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        );
        block_test_underlay_key(&global_ctx, &key, 100);
        let blocked_until = 100 + UNDERLAY_BREAKER_INITIAL_TTL_SECS;

        let cancelled = global_ctx
            .try_begin_underlay_attempt_at(std::slice::from_ref(&key), blocked_until + 1)
            .unwrap();
        drop(cancelled);
        {
            let state = global_ctx.underlay_breaker.lock().unwrap();
            let entry = state.entries.get(&key).unwrap();
            assert!(!entry.half_open);
            assert_eq!(entry.backoff_secs, UNDERLAY_BREAKER_INITIAL_TTL_SECS);
        }

        let mut committed = global_ctx
            .try_begin_underlay_attempt_at(std::slice::from_ref(&key), blocked_until + 1)
            .unwrap();
        committed.commit();
        drop(committed);
        assert!(
            global_ctx
                .try_begin_underlay_attempt_at(std::slice::from_ref(&key), blocked_until + 2)
                .is_err()
        );
        {
            let state = global_ctx.underlay_breaker.lock().unwrap();
            let entry = state.entries.get(&key).unwrap();
            assert_eq!(entry.backoff_secs, UNDERLAY_BREAKER_INITIAL_TTL_SECS);
            assert!(entry.half_open);
        }
        assert!(
            global_ctx
                .try_begin_underlay_attempt_at(
                    std::slice::from_ref(&key),
                    blocked_until + UNDERLAY_BREAKER_HALF_OPEN_TIMEOUT_SECS + 2
                )
                .is_err()
        );
        let state = global_ctx.underlay_breaker.lock().unwrap();
        let entry = state.entries.get(&key).unwrap();
        assert_eq!(entry.backoff_secs, UNDERLAY_BREAKER_INITIAL_TTL_SECS * 2);
        assert!(!entry.half_open);
    }

    #[tokio::test]
    async fn underlay_breaker_soft_strike_does_not_gate() {
        let config = TomlConfigLoader::default();
        let global_ctx = Arc::new(GlobalCtx::new(config));
        let key = UnderlayBreakerKey::endpoint(
            "198.51.100.2:11010".parse().unwrap(),
            IpScheme::Udp,
            UnderlayBreakerScope::Direct,
        );

        for now in 100..110 {
            assert!(!global_ctx.record_underlay_breaker_strike_at(
                key.clone(),
                UnderlayBreakerStrikeKind::Soft,
                "test_soft",
                None,
                now
            ));
        }
        assert!(!global_ctx.is_underlay_breaker_gated_at(&key, 111));
    }

    #[tokio::test]
    async fn underlay_breaker_is_disabled_by_guard_flag() {
        let config = TomlConfigLoader::default();
        let global_ctx = Arc::new(GlobalCtx::new(config));
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = false;
        global_ctx.set_flags(flags);
        let key = UnderlayBreakerKey::endpoint(
            "198.51.100.3:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        );

        assert!(!global_ctx.record_underlay_breaker_strike_at(
            key.clone(),
            UnderlayBreakerStrikeKind::Hard,
            "test_disabled",
            None,
            100
        ));
        assert!(!global_ctx.is_underlay_breaker_gated_at(&key, 101));
        assert!(
            global_ctx
                .underlay_breaker
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[tokio::test]
    async fn underlay_breaker_capacity_is_bounded() {
        let config = TomlConfigLoader::default();
        let global_ctx = Arc::new(GlobalCtx::new(config));

        for idx in 0..(UNDERLAY_BREAKER_CAPACITY + 16) {
            let key = UnderlayBreakerKey::endpoint(
                SocketAddr::from(([198, 51, (idx / 256) as u8, (idx % 256) as u8], 11010)),
                IpScheme::Tcp,
                UnderlayBreakerScope::Generic,
            );
            global_ctx.record_underlay_breaker_strike_at(
                key,
                UnderlayBreakerStrikeKind::Soft,
                "test_capacity",
                None,
                idx as u64,
            );
        }

        assert!(
            global_ctx.underlay_breaker.lock().unwrap().entries.len() <= UNDERLAY_BREAKER_CAPACITY
        );
    }

    #[tokio::test]
    async fn derived_secure_mode_is_lazy_and_tracks_stealth_conditions() {
        let config = TomlConfigLoader::default();
        let global_ctx = GlobalCtx::new(config);

        assert!(global_ctx.get_effective_secure_mode().is_none());
        assert!(global_ctx.derived_secure_mode.get().is_none());
        assert!(!global_ctx.get_feature_flags().stealth_supported);

        global_ctx.config.set_network_identity(NetworkIdentity {
            network_name: "test".to_owned(),
            network_secret: Some("secret".to_owned()),
            network_secret_digest: None,
        });
        let derived = global_ctx.get_effective_secure_mode().unwrap();
        assert!(derived.enabled);
        assert!(global_ctx.config.get_secure_mode().is_none());
        assert!(global_ctx.derived_secure_mode.get().is_some());
        assert!(global_ctx.get_feature_flags().stealth_supported);

        let mut flags = global_ctx.get_flags();
        flags.stealth_mode = false;
        global_ctx.set_flags(flags.clone());
        assert!(global_ctx.get_effective_secure_mode().is_none());
        assert!(!global_ctx.get_feature_flags().stealth_supported);

        flags.stealth_mode = true;
        global_ctx.set_flags(flags);
        let restored = global_ctx.get_effective_secure_mode().unwrap();
        assert_eq!(derived.local_private_key, restored.local_private_key);
    }

    #[tokio::test]
    async fn explicit_disabled_secure_mode_suppresses_effective_secure_mode() {
        let config = TomlConfigLoader::default();
        config.set_network_identity(NetworkIdentity {
            network_name: "test".to_owned(),
            network_secret: Some("secret".to_owned()),
            network_secret_digest: None,
        });
        let enabled_secure = process_secure_mode_cfg(SecureModeConfig {
            enabled: true,
            local_private_key: None,
            local_public_key: None,
        })
        .unwrap();
        config.set_secure_mode(Some(SecureModeConfig {
            enabled: false,
            ..enabled_secure
        }));
        config.set_stealth_mode_explicit(true);
        let mut flags = config.get_flags();
        flags.stealth_mode = true;
        config.set_flags(flags);

        let global_ctx = GlobalCtx::new(config);

        assert!(global_ctx.get_effective_secure_mode().is_none());
        assert!(global_ctx.get_secure_mode_for_tunnel(true).is_none());
        assert!(!global_ctx.get_feature_flags().stealth_supported);
    }

    #[tokio::test]
    async fn set_flags_marks_runtime_stealth_change_explicit() {
        let config = TomlConfigLoader::default();
        config.set_secure_mode(Some(SecureModeConfig {
            enabled: true,
            ..Default::default()
        }));
        let global_ctx = GlobalCtx::new(config);
        global_ctx.config.set_network_identity(NetworkIdentity {
            network_name: "test".to_owned(),
            network_secret: Some("secret".to_owned()),
            network_secret_digest: None,
        });

        let mut flags = global_ctx.get_flags();
        flags.stealth_mode = true;
        global_ctx.set_flags(flags);

        assert!(global_ctx.get_flags().stealth_mode);
        assert!(global_ctx.get_feature_flags().stealth_supported);
    }

    pub fn get_mock_global_ctx_with_network(
        network_identy: Option<NetworkIdentity>,
    ) -> ArcGlobalCtx {
        let config_fs = TomlConfigLoader::default();
        config_fs.set_inst_name(format!("test_{}", config_fs.get_id()));
        config_fs.set_network_identity(network_identy.unwrap_or_default());

        let ctx = Arc::new(GlobalCtx::new(config_fs));
        ctx.replace_stun_info_collector(Box::new(MockStunInfoCollector {
            udp_nat_type: NatType::Unknown,
        }));
        ctx
    }

    pub fn get_mock_global_ctx() -> ArcGlobalCtx {
        get_mock_global_ctx_with_network(None)
    }
}
