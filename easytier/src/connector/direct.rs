// try connect peers directly, with either its public ip or lan ip

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use hotpath::instant::Instant;
use pnet::ipnetwork::IpNetwork;

use crate::{
    common::{
        PeerId,
        dns::socket_addrs,
        error::Error,
        global_ctx::{
            ArcGlobalCtx, GlobalCtxEvent, UnderlayBreakerKey, UnderlayBreakerScope,
            UnderlayBreakerStrikeKind, UnderlayBreakerTrace,
        },
        network::IPCollector,
        stun::StunInfoCollectorTrait,
        transport_priority::{
            PreferenceKey, TransportPathClass, TransportPriority, protocol_is_compiled,
        },
        underlay_guard,
    },
    connector::udp_hole_punch::handle_rpc_result,
    peers::{
        peer_conn::PeerConnId,
        peer_manager::PeerManager,
        peer_rpc::PeerRpcManager,
        peer_rpc_service::DirectConnectorManagerRpcServer,
        peer_task::{PeerTaskLauncher, PeerTaskManager},
    },
    proto::{
        peer_rpc::{
            DirectConnectorRpc, DirectConnectorRpcClientFactory, DirectConnectorRpcServer,
            GetIpListRequest, GetIpListResponse, SendUdpHolePunchPacketRequest,
        },
        rpc_types::controller::BaseController,
    },
    tunnel::{IpVersion, TunnelConnector, common::bind, matches_protocol, udp::UdpTunnelConnector},
    use_global_var,
};

use super::{
    create_connector_by_url_with_scope, should_background_p2p_with_peer, should_try_p2p_with_peer,
    udp_hole_punch,
};
use crate::tunnel::{FromUrl, IpScheme, TunnelScheme, matches_scheme};
use anyhow::Context;
use rand::Rng;
use socket2::Protocol;
use tokio::{net::UdpSocket, task::JoinSet, time::timeout};
use url::Host;

pub const DIRECT_CONNECTOR_SERVICE_ID: u32 = 1;
pub const DIRECT_CONNECTOR_BLACKLIST_TIMEOUT_SEC: u64 = 300;
const DIRECT_CONNECTOR_FAILURE_COOLDOWN_SEC: u64 = 30;
const MAX_IPV6_HOLE_PUNCH_CONNECTOR_ADDRS: usize = 16;

static TESTING: AtomicBool = AtomicBool::new(false);
static DEFAULT_PROTOCOL_DEPRECATION_LOGGED: AtomicBool = AtomicBool::new(false);
static UNCOMPILED_PRIORITY_WARNINGS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone)]
struct DirectCandidate {
    url: url::Url,
    is_lan: bool,
}

impl DirectCandidate {
    fn preference_key(&self, lan_order: &[String], wan_order: &[String]) -> PreferenceKey {
        let path = if self.is_lan {
            TransportPathClass::Lan
        } else {
            TransportPathClass::Wan
        };
        PreferenceKey::new(
            path,
            if self.is_lan { lan_order } else { wan_order },
            self.url.scheme(),
        )
    }
}

fn retain_higher_priority_candidates(
    candidates: Vec<DirectCandidate>,
    lan_order: &[String],
    wan_order: &[String],
    existing_key: Option<PreferenceKey>,
) -> Vec<DirectCandidate> {
    let Some(existing_key) = existing_key else {
        return candidates;
    };
    candidates
        .into_iter()
        .filter(|candidate| candidate.preference_key(lan_order, wan_order) < existing_key)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectAttemptOutcome {
    Connected(Option<PreferenceKey>),
    AlreadySatisfied,
}

#[derive(Debug)]
enum DirectConnectAttemptError {
    Guarded(Error),
    Failed(Error),
}

impl DirectConnectAttemptError {
    fn into_error(self) -> Error {
        match self {
            Self::Guarded(err) | Self::Failed(err) => err,
        }
    }

    fn is_self_loop_signal(&self) -> bool {
        match self {
            Self::Guarded(_) => false,
            Self::Failed(err) => err.is_self_loop_signal(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DirectStealthMode {
    #[default]
    Disabled,
    Required,
    PreferLegacyFallback,
}

fn direct_stealth_mode(
    protocol: crate::common::stealth_registry::StealthProtocol,
    remote_feature: Option<&crate::proto::common::PeerFeatureFlag>,
) -> DirectStealthMode {
    match remote_feature {
        Some(feature)
            if crate::common::stealth_registry::peer_supports_protocol(feature, protocol) =>
        {
            DirectStealthMode::Required
        }
        Some(_) => DirectStealthMode::Disabled,
        None => DirectStealthMode::PreferLegacyFallback,
    }
}

fn stealth_protocol_key_for_scheme(scheme: &str) -> &str {
    match scheme {
        "quic-brutal" => crate::common::stealth_registry::StealthProtocol::Quic.as_str(),
        _ => scheme,
    }
}

fn mapped_listener_port(url: &url::Url) -> Option<u16> {
    url.port().or_else(|| {
        TunnelScheme::try_from(url)
            .ok()
            .and_then(|scheme| IpScheme::try_from(scheme).ok())
            .map(IpScheme::default_port)
    })
}

fn direct_ip_scheme_from_url(url: &url::Url) -> Option<IpScheme> {
    TunnelScheme::try_from(url)
        .ok()
        .and_then(|scheme| IpScheme::try_from(scheme).ok())
}

async fn resolve_mapped_listener_addrs(listener: &url::Url) -> Result<Vec<SocketAddr>, Error> {
    socket_addrs(listener, || mapped_listener_port(listener)).await
}

fn is_usable_public_ipv6_candidate(ip: &Ipv6Addr, global_ctx: &ArcGlobalCtx) -> bool {
    is_usable_public_ipv6_candidate_with_mode(ip, global_ctx, TESTING.load(Ordering::Relaxed))
}

fn is_usable_public_ipv6_candidate_with_mode(
    ip: &Ipv6Addr,
    global_ctx: &ArcGlobalCtx,
    testing: bool,
) -> bool {
    !global_ctx.is_ip_easytier_managed_ipv6(ip)
        && (testing
            || (!ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && !ip.is_multicast()))
}

fn push_ipv6_hole_punch_candidate(
    candidates: &mut Vec<Ipv6Addr>,
    ip: Ipv6Addr,
    global_ctx: &ArcGlobalCtx,
    limit: usize,
) {
    if candidates.len() >= limit
        || !is_usable_public_ipv6_candidate(&ip, global_ctx)
        || underlay_guard::should_block_underlay_ip(global_ctx, IpAddr::V6(ip))
        || candidates.contains(&ip)
    {
        return;
    }
    candidates.push(ip);
}

async fn collect_ipv6_hole_punch_candidates(global_ctx: &ArcGlobalCtx) -> Vec<Ipv6Addr> {
    let mut candidates = Vec::new();
    for ip in global_ctx
        .get_stun_info_collector()
        .get_stun_info()
        .public_ip
        .iter()
        .filter_map(|ip| ip.parse::<Ipv6Addr>().ok())
    {
        push_ipv6_hole_punch_candidate(
            &mut candidates,
            ip,
            global_ctx,
            MAX_IPV6_HOLE_PUNCH_CONNECTOR_ADDRS,
        );
    }

    let ip_list = global_ctx.get_ip_collector().collect_ip_addrs().await;
    for ip in ip_list
        .interface_ipv6s
        .iter()
        .chain(ip_list.public_ipv6.iter())
        .map(|ip| Ipv6Addr::from(*ip))
    {
        push_ipv6_hole_punch_candidate(
            &mut candidates,
            ip,
            global_ctx,
            MAX_IPV6_HOLE_PUNCH_CONNECTOR_ADDRS,
        );
    }

    candidates
}

fn wildcard_udp_bind_addr(remote_addr: SocketAddr) -> SocketAddr {
    if remote_addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    }
}

fn bind_direct_udp_socket(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
) -> Result<UdpSocket, Error> {
    Ok(bind::<UdpSocket>()
        .addr(wildcard_udp_bind_addr(remote_addr))
        .net_ns(global_ctx.net_ns.clone())
        .only_v6(remote_addr.is_ipv6())
        .maybe_socket_mark(global_ctx.get_flags().socket_mark)
        .call()?)
}

async fn resolve_public_ipv4_connect_result<T, F, Fut>(
    remote_url: &url::Url,
    attempt: Result<T, DirectConnectAttemptError>,
    fallback: F,
) -> Result<T, DirectConnectAttemptError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, DirectConnectAttemptError>>,
{
    match attempt {
        Ok(ret) => Ok(ret),
        Err(DirectConnectAttemptError::Guarded(err)) => {
            Err(DirectConnectAttemptError::Guarded(err))
        }
        Err(DirectConnectAttemptError::Failed(err)) => {
            tracing::debug!(
                ?err,
                %remote_url,
                "udp public ipv4 listener punch failed, falling back to direct connect"
            );
            fallback().await
        }
    }
}

#[async_trait::async_trait]
pub trait PeerManagerForDirectConnector {
    async fn list_peers(&self) -> Vec<PeerId>;
    fn get_peer_rpc_mgr(&self) -> Arc<PeerRpcManager>;
}

#[async_trait::async_trait]
impl PeerManagerForDirectConnector for PeerManager {
    async fn list_peers(&self) -> Vec<PeerId> {
        let mut ret = vec![];
        let allow_public_server = use_global_var!(DIRECT_CONNECT_TO_PUBLIC_SERVER);
        let flags = self.get_global_ctx().get_flags();
        let lazy_p2p = flags.lazy_p2p;
        let priority_enabled = !TransportPriority::parse(&flags.transport_priority)
            .expect("transport_priority is validated while loading configuration")
            .is_empty();
        let now = Instant::now();

        let routes = self.list_routes().await;
        for route in routes.iter() {
            let static_allowed = should_background_p2p_with_peer(
                route.feature_flag.as_ref(),
                allow_public_server,
                lazy_p2p,
                flags.disable_p2p,
                flags.need_p2p,
            );
            let dynamic_allowed = should_try_p2p_with_peer(
                route.feature_flag.as_ref(),
                allow_public_server,
                flags.disable_p2p,
                flags.need_p2p,
            ) && self.has_recent_traffic(route.peer_id, now);
            let priority_upgrade_allowed = priority_enabled
                && self.get_peer_map().has_peer(route.peer_id)
                && should_try_p2p_with_peer(
                    route.feature_flag.as_ref(),
                    allow_public_server,
                    flags.disable_p2p,
                    flags.need_p2p,
                );
            if static_allowed || dynamic_allowed || priority_upgrade_allowed {
                ret.push(route.peer_id);
            }
        }

        ret
    }

    fn get_peer_rpc_mgr(&self) -> Arc<PeerRpcManager> {
        self.get_peer_rpc_mgr()
    }
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct DstListenerUrlBlackListItem(PeerId, String);

struct DirectConnectorManagerData {
    global_ctx: ArcGlobalCtx,
    peer_manager: Arc<PeerManager>,
    dst_listener_blacklist: timedmap::TimedMap<DstListenerUrlBlackListItem, ()>,
    peer_black_list: timedmap::TimedMap<PeerId, ()>,
    preferred_direct_targets: timedmap::TimedMap<PeerId, PreferenceKey>,
}

impl DirectConnectorManagerData {
    pub fn new(global_ctx: ArcGlobalCtx, peer_manager: Arc<PeerManager>) -> Self {
        Self {
            global_ctx,
            peer_manager,
            dst_listener_blacklist: timedmap::TimedMap::new(),
            peer_black_list: timedmap::TimedMap::new(),
            preferred_direct_targets: timedmap::TimedMap::new(),
        }
    }

    fn build_udp_stealth_for_peer(
        &self,
        mode: DirectStealthMode,
    ) -> std::sync::Arc<crate::tunnel::stealth::OuterSessionState> {
        if matches!(mode, DirectStealthMode::Disabled) {
            std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled())
        } else {
            let flags = self.global_ctx.get_flags();
            let secure_mode = self.global_ctx.is_secure_mode_enabled();
            let udp_stealth = crate::common::stealth_registry::protocol_enabled(
                &flags,
                crate::common::stealth_registry::StealthProtocol::Udp,
            );
            crate::tunnel::stealth::build_outer_session(
                self.global_ctx
                    .get_network_identity()
                    .network_secret
                    .as_deref(),
                udp_stealth,
                secure_mode,
                flags.stealth_window_secs,
            )
        }
    }

    fn blacklist_direct_target_for(&self, dst_peer_id: PeerId, addr: &str, timeout_secs: u64) {
        self.dst_listener_blacklist.insert(
            DstListenerUrlBlackListItem(dst_peer_id, addr.to_owned()),
            (),
            std::time::Duration::from_secs(timeout_secs),
        );
    }

    fn blacklist_loop_target(&self, dst_peer_id: PeerId, addr: &str) {
        self.blacklist_direct_target_for(dst_peer_id, addr, DIRECT_CONNECTOR_BLACKLIST_TIMEOUT_SEC);
    }

    fn preference_satisfied(&self, dst_peer_id: PeerId, target: PreferenceKey) -> bool {
        self.peer_manager
            .best_transport_preference_key(dst_peer_id)
            .is_some_and(|current| current <= target)
    }

    fn direct_target_satisfied(&self, dst_peer_id: PeerId) -> bool {
        self.preferred_direct_targets
            .get(&dst_peer_id)
            .is_some_and(|target| self.preference_satisfied(dst_peer_id, target))
    }

    fn clear_preferred_direct_targets(&self) {
        let keys = self
            .preferred_direct_targets
            .snapshot::<Vec<_>>()
            .into_iter()
            .map(|(peer_id, _)| peer_id)
            .collect::<Vec<_>>();
        for peer_id in keys {
            self.preferred_direct_targets.remove(&peer_id);
        }
    }

    async fn remote_send_udp_hole_punch_packet(
        &self,
        dst_peer_id: PeerId,
        connector_addrs: Vec<SocketAddr>,
        preferred_src_ipv6: Option<Ipv6Addr>,
        remote_url: &url::Url,
    ) -> Result<(), Error> {
        if !matches_scheme!(remote_url, TunnelScheme::Ip(IpScheme::Udp)) {
            return Err(anyhow::anyhow!(
                "udp hole punch packet only applies to udp listener: {}",
                remote_url
            )
            .into());
        }

        let global_ctx = self.peer_manager.get_global_ctx();
        let listener_port = mapped_listener_port(remote_url).ok_or(anyhow::anyhow!(
            "failed to parse port from remote url: {}",
            remote_url
        ))?;

        let rpc_stub = self
            .peer_manager
            .get_peer_rpc_mgr()
            .rpc_client()
            .scoped_client::<DirectConnectorRpcClientFactory<BaseController>>(
            self.peer_manager.my_peer_id(),
            dst_peer_id,
            global_ctx.get_network_name(),
        );

        rpc_stub
            .send_udp_hole_punch_packet(
                BaseController::default(),
                SendUdpHolePunchPacketRequest {
                    connector_addr: connector_addrs.first().copied().map(Into::into),
                    listener_port: listener_port as u32,
                    preferred_src_ipv6: preferred_src_ipv6.map(Into::into),
                    connector_addrs: connector_addrs.into_iter().map(Into::into).collect(),
                },
            )
            .await
            .with_context(|| {
                format!(
                    "do rpc, send udp hole punch packet to peer {} at {} with preferred source {:?}",
                    dst_peer_id, remote_url, preferred_src_ipv6
                )
            })?;

        Ok(())
    }

    async fn connect_to_public_ipv6(
        &self,
        dst_peer_id: PeerId,
        remote_url: &url::Url,
        remote_addr: SocketAddr,
        stealth_mode: DirectStealthMode,
    ) -> Result<(PeerId, PeerConnId), Error> {
        let local_socket = Arc::new(
            bind_direct_udp_socket(&self.global_ctx, remote_addr)
                .with_context(|| format!("failed to bind local socket for {}", remote_url))?,
        );
        let connector_ips = collect_ipv6_hole_punch_candidates(&self.global_ctx).await;

        // ask remote to send v6 hole punch packet
        // and no matter what the result is, continue to connect
        if !connector_ips.is_empty() {
            let local_port = local_socket.local_addr()?.port();
            let connector_addrs = connector_ips
                .into_iter()
                .map(|ip| SocketAddr::new(IpAddr::V6(ip), local_port))
                .collect::<Vec<_>>();
            let preferred_src_ipv6 = match remote_url.host() {
                Some(Host::Ipv6(ip)) => Some(ip),
                _ => None,
            };
            tracing::debug!(
                ?connector_addrs,
                ?preferred_src_ipv6,
                ?remote_url,
                "request remote IPv6 hole-punch packets"
            );
            if let Err(err) = self
                .remote_send_udp_hole_punch_packet(
                    dst_peer_id,
                    connector_addrs,
                    preferred_src_ipv6,
                    remote_url,
                )
                .await
            {
                tracing::debug!(
                    ?err,
                    ?remote_url,
                    "remote IPv6 hole-punch packet request failed"
                );
            }
        } else {
            tracing::debug!(
                ?remote_url,
                "skip remote IPv6 hole-punch packet; no non-EasyTier public IPv6 in STUN info"
            );
        }

        let mut udp_connector = UdpTunnelConnector::new(remote_url.clone());
        let stealth = self.build_udp_stealth_for_peer(stealth_mode);
        match stealth_mode {
            DirectStealthMode::Disabled | DirectStealthMode::Required => {
                udp_connector.set_stealth(stealth);
            }
            DirectStealthMode::PreferLegacyFallback => {
                udp_connector.prefer_stealth_with_legacy_fallback(stealth);
            }
        }
        let ret = udp_connector
            .try_connect_with_socket(local_socket, remote_addr)
            .await?;

        // NOTICE: must add as directly connected tunnel
        self.peer_manager
            .add_client_tunnel_with_peer_id_hint_scoped(
                ret,
                true,
                Some(dst_peer_id),
                UnderlayBreakerScope::Direct,
            )
            .await
    }

    async fn connect_to_public_ipv4(
        &self,
        dst_peer_id: PeerId,
        remote_url: &url::Url,
        remote_addr: SocketAddr,
        stealth_mode: DirectStealthMode,
    ) -> Result<(PeerId, PeerConnId), DirectConnectAttemptError> {
        let local_socket = Arc::new(
            bind_direct_udp_socket(&self.global_ctx, remote_addr)
                .with_context(|| format!("failed to bind local socket for {}", remote_url))
                .map_err(|err| DirectConnectAttemptError::Failed(err.into()))?,
        );
        let connector_addr = self
            .peer_manager
            .get_global_ctx()
            .get_stun_info_collector()
            .get_udp_port_mapping_with_socket(local_socket.clone())
            .await
            .with_context(|| format!("failed to get udp port mapping for {}", remote_url))
            .map_err(|err| DirectConnectAttemptError::Failed(err.into()))?;

        let _ = self
            .remote_send_udp_hole_punch_packet(dst_peer_id, vec![connector_addr], None, remote_url)
            .await;

        let mut udp_connector = UdpTunnelConnector::new(remote_url.clone());
        let stealth = self.build_udp_stealth_for_peer(stealth_mode);
        match stealth_mode {
            DirectStealthMode::Disabled | DirectStealthMode::Required => {
                udp_connector.set_stealth(stealth);
            }
            DirectStealthMode::PreferLegacyFallback => {
                udp_connector.prefer_stealth_with_legacy_fallback(stealth);
            }
        }
        let ret = udp_connector
            .try_connect_with_socket(local_socket, remote_addr)
            .await
            .map_err(|err| DirectConnectAttemptError::Failed(err.into()))?;

        self.peer_manager
            .add_client_tunnel_with_peer_id_hint_scoped(
                ret,
                true,
                Some(dst_peer_id),
                UnderlayBreakerScope::Direct,
            )
            .await
            .map_err(DirectConnectAttemptError::Failed)
    }

    async fn try_direct_connect_with_peer_id_hint_timeout<C>(
        &self,
        connector: C,
        dst_peer_id: PeerId,
    ) -> Result<(PeerId, PeerConnId), DirectConnectAttemptError>
    where
        C: crate::tunnel::TunnelConnector + std::fmt::Debug,
    {
        timeout(
            std::time::Duration::from_secs(3),
            self.peer_manager
                .try_direct_connect_with_peer_id_hint_scoped(
                    connector,
                    Some(dst_peer_id),
                    UnderlayBreakerScope::Direct,
                ),
        )
        .await
        .map_err(Error::from)
        .map_err(DirectConnectAttemptError::Failed)?
        .map_err(DirectConnectAttemptError::Failed)
    }

    async fn do_try_connect_to_ip(
        &self,
        dst_peer_id: PeerId,
        addr: String,
        stealth_mode: DirectStealthMode,
    ) -> Result<(), DirectConnectAttemptError> {
        let mut connector = create_connector_by_url_with_scope(
            &addr,
            &self.global_ctx,
            IpVersion::Both,
            UnderlayBreakerScope::Direct,
            Some(dst_peer_id),
        )
        .await
        .map_err(DirectConnectAttemptError::Failed)?;
        match stealth_mode {
            DirectStealthMode::Disabled => connector.disable_stealth(),
            DirectStealthMode::Required => connector.require_stealth(),
            DirectStealthMode::PreferLegacyFallback => {}
        }
        let remote_url = connector.remote_url();
        let (peer_id, conn_id) = if matches_scheme!(remote_url, TunnelScheme::Ip(IpScheme::Udp)) {
            match remote_url.host() {
                Some(Host::Ipv6(_)) => {
                    // Reuse the exact address the underlay guard already
                    // validated instead of re-resolving `remote_url`, which
                    // picks a random address on every call and could pick a
                    // different, unvalidated address for multi-addr URLs.
                    let remote_addr = match connector.resolved_addr() {
                        Some(addr) => addr,
                        None => SocketAddr::from_url(remote_url.clone(), IpVersion::V6)
                            .await
                            .map_err(|err| DirectConnectAttemptError::Failed(err.into()))?,
                    };
                    // Keep this commit immediately before entering the public UDP helper.
                    // Awaiting the helper polls it in the current task, and that helper
                    // synchronously binds the real UDP socket before its first await. This
                    // preserves the preflight boundary: cancellation before this point rolls
                    // back, while cancellation after this point happens after the real socket
                    // side effect has started. Do not move this into the helper unless that
                    // ordering is preserved.
                    connector.commit_preflight();
                    self.connect_to_public_ipv6(dst_peer_id, &remote_url, remote_addr, stealth_mode)
                        .await
                        .map_err(DirectConnectAttemptError::Failed)?
                }
                Some(Host::Ipv4(ip)) if is_public_ipv4(ip) => {
                    let remote_addr = match connector.resolved_addr() {
                        Some(addr) => addr,
                        None => SocketAddr::from_url(remote_url.clone(), IpVersion::V4)
                            .await
                            .map_err(|err| DirectConnectAttemptError::Failed(err.into()))?,
                    };
                    // Same boundary as the IPv6 branch: the awaited helper is immediately
                    // polled and synchronously creates the real UDP socket before its first
                    // await, then proceeds to STUN/hole-punch. Keeping the commit here avoids
                    // a second breaker lease while still rolling back candidates dropped before
                    // the public UDP path starts.
                    connector.commit_preflight();
                    resolve_public_ipv4_connect_result(
                        &remote_url,
                        self.connect_to_public_ipv4(
                            dst_peer_id,
                            &remote_url,
                            remote_addr,
                            stealth_mode,
                        )
                        .await,
                        || {
                            self.try_direct_connect_with_peer_id_hint_timeout(
                                connector,
                                dst_peer_id,
                            )
                        },
                    )
                    .await?
                }
                _ => {
                    self.try_direct_connect_with_peer_id_hint_timeout(connector, dst_peer_id)
                        .await?
                }
            }
        } else {
            self.try_direct_connect_with_peer_id_hint_timeout(connector, dst_peer_id)
                .await?
        };

        if peer_id != dst_peer_id && !TESTING.load(Ordering::Relaxed) {
            tracing::info!(
                "connect to ip succ: {}, but peer id mismatch, expect: {}, actual: {}",
                addr,
                dst_peer_id,
                peer_id
            );
            if let Some(scheme) = direct_ip_scheme_from_url(&remote_url) {
                let blocked = self.global_ctx.record_underlay_breaker_strike(
                    UnderlayBreakerKey::peer(dst_peer_id, scheme, UnderlayBreakerScope::Direct),
                    UnderlayBreakerStrikeKind::Hard,
                    "handshake_peer_mismatch",
                    Some(UnderlayBreakerTrace {
                        expected_peer_id: Some(dst_peer_id),
                        actual_peer_id: Some(peer_id),
                        ..Default::default()
                    }),
                );
                if blocked {
                    self.peer_manager.invalidate_peer_default_conn(dst_peer_id);
                }
            }
            self.peer_manager
                .close_peer_conn(peer_id, &conn_id)
                .await
                .map_err(DirectConnectAttemptError::Failed)?;
            return Err(DirectConnectAttemptError::Failed(Error::InvalidUrl(addr)));
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn try_connect_to_ip(
        self: Arc<DirectConnectorManagerData>,
        dst_peer_id: PeerId,
        addr: String,
        stealth_mode: DirectStealthMode,
        preference_key: Option<PreferenceKey>,
    ) -> Result<DirectAttemptOutcome, Error> {
        let mut rand_gen = rand::rngs::OsRng;
        let backoff_ms = [1000, 2000, 4000];
        let mut backoff_idx = 0;

        tracing::debug!(?dst_peer_id, ?addr, "try_connect_to_ip start");

        self.dst_listener_blacklist.cleanup();
        let blacklist_item = DstListenerUrlBlackListItem(dst_peer_id, addr.clone());

        if self.dst_listener_blacklist.contains(&blacklist_item) {
            return Err(Error::UrlInBlacklist);
        }

        loop {
            if preference_key.is_some_and(|target| self.preference_satisfied(dst_peer_id, target))
                || (preference_key.is_none()
                    && self.peer_manager.has_directly_connected_conn(dst_peer_id))
            {
                return Ok(DirectAttemptOutcome::AlreadySatisfied);
            }
            if self.dst_listener_blacklist.contains(&blacklist_item) {
                return Err(Error::UrlInBlacklist);
            }

            tracing::debug!(?dst_peer_id, ?addr, "try_connect_to_ip start one round");
            let ret = self
                .do_try_connect_to_ip(dst_peer_id, addr.clone(), stealth_mode)
                .await;
            tracing::debug!(?ret, ?dst_peer_id, ?addr, "try_connect_to_ip return");
            if ret.is_ok() {
                return Ok(DirectAttemptOutcome::Connected(preference_key));
            }
            if matches!(ret, Err(DirectConnectAttemptError::Guarded(_))) {
                return Err(ret.unwrap_err().into_error());
            }
            if ret
                .as_ref()
                .is_err_and(DirectConnectAttemptError::is_self_loop_signal)
            {
                self.blacklist_loop_target(dst_peer_id, &addr);
                return Err(ret.unwrap_err().into_error());
            }

            if preference_key.is_some_and(|target| self.preference_satisfied(dst_peer_id, target))
                || (preference_key.is_none()
                    && self.peer_manager.has_directly_connected_conn(dst_peer_id))
            {
                return Ok(DirectAttemptOutcome::AlreadySatisfied);
            }

            if backoff_idx < backoff_ms.len() {
                let delta = backoff_ms[backoff_idx] >> 1;
                assert!(delta > 0);
                assert!(delta < backoff_ms[backoff_idx]);

                tokio::time::sleep(Duration::from_millis(
                    (backoff_ms[backoff_idx] + rand_gen.gen_range(-delta..delta)) as u64,
                ))
                .await;

                backoff_idx += 1;
                continue;
            } else {
                // Ordinary reachability failures must be retried soon enough to restore a
                // preferred transport. Self-loop signals retain the longer safety timeout.
                self.blacklist_direct_target_for(
                    dst_peer_id,
                    &addr,
                    DIRECT_CONNECTOR_FAILURE_COOLDOWN_SEC,
                );
                return Err(ret.unwrap_err().into_error());
            }
        }
    }

    fn is_lan_candidate(ip: IpAddr, local_networks: &[IpNetwork]) -> bool {
        match ip {
            IpAddr::V4(ip) if ip.is_link_local() => local_networks
                .iter()
                .any(|network| network.contains(IpAddr::V4(ip))),
            IpAddr::V6(ip) if ip.is_unicast_link_local() => local_networks
                .iter()
                .any(|network| network.contains(IpAddr::V6(ip))),
            // Public on-link prefixes are common on VPS providers (for example
            // two hosts in the same /48). Treating those as LAN lets a public
            // IPv6 TCP/WS candidate suppress WAN QUIC/FakeTCP, which violates
            // the user's global transport preference. LAN here means local
            // non-public address space only.
            IpAddr::V4(ip) if ip.is_private() => local_networks
                .iter()
                .any(|network| network.contains(IpAddr::V4(ip))),
            IpAddr::V6(ip) if ip.is_unique_local() => local_networks
                .iter()
                .any(|network| network.contains(IpAddr::V6(ip))),
            _ => false,
        }
    }

    fn listener_url_for_addr(listener: &url::Url, addr: SocketAddr) -> Option<url::Url> {
        let mut url = listener.clone();
        // quic-brutal's tx_mbps (or legacy tx_bps) is local sender state. Advertising a listener's
        // value to the peer would make an asymmetric receiver reuse the wrong
        // direction's rate. A discovered connector therefore uses the safe BBR
        // fallback unless that node has an explicit manual peer URL.
        if url.scheme() == "quic-brutal" {
            url.set_query(None);
        }
        let host = match addr.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        url.set_host(Some(&host)).ok()?;
        url.set_port(Some(addr.port())).ok()?;
        Some(url)
    }

    async fn expand_direct_candidates(
        &self,
        ip_list: &GetIpListResponse,
        listeners: &[url::Url],
    ) -> Vec<DirectCandidate> {
        let local_networks = IPCollector::collect_interfaces(self.global_ctx.net_ns.clone(), false)
            .await
            .into_iter()
            .flat_map(|interface| interface.ips)
            .collect::<Vec<_>>();
        let local_listeners = self.global_ctx.get_running_listeners();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for listener in listeners {
            let Ok(resolved) = resolve_mapped_listener_addrs(listener).await else {
                tracing::warn!(?listener, "failed to resolve direct listener");
                continue;
            };
            let is_udp = matches_protocol!(listener, Protocol::UDP);
            let mut expanded = Vec::new();
            for addr in resolved {
                if addr.ip().is_unspecified() {
                    match addr {
                        SocketAddr::V4(addr) => expanded.extend(
                            ip_list
                                .interface_ipv4s
                                .iter()
                                .chain(ip_list.public_ipv4.iter())
                                .map(|ip| {
                                    SocketAddr::new(
                                        IpAddr::V4(Ipv4Addr::from(ip.addr)),
                                        addr.port(),
                                    )
                                }),
                        ),
                        SocketAddr::V6(addr) => expanded.extend(
                            ip_list
                                .interface_ipv6s
                                .iter()
                                .chain(ip_list.public_ipv6.iter())
                                .map(|ip| {
                                    SocketAddr::new(IpAddr::V6(Ipv6Addr::from(*ip)), addr.port())
                                }),
                        ),
                    }
                } else {
                    expanded.push(addr);
                }
            }

            for addr in expanded {
                if addr.ip().is_loopback() && !TESTING.load(Ordering::Relaxed) {
                    continue;
                }
                if underlay_guard::should_block_underlay_ip(&self.global_ctx, addr.ip()) {
                    tracing::debug!(?listener, ?addr, "skip guarded underlay direct candidate");
                    continue;
                }
                if let IpAddr::V6(ip) = addr.ip()
                    && self.global_ctx.is_ip_easytier_managed_ipv6(&ip)
                {
                    tracing::debug!(?listener, ?addr, "skip EasyTier-managed IPv6 target");
                    continue;
                }
                let check_self = local_listeners.iter().any(|local| {
                    local.port() == Some(addr.port())
                        && matches_protocol!(local, Protocol::UDP) == is_udp
                });
                if check_self && self.global_ctx.should_deny_proxy(&addr, is_udp) {
                    tracing::debug!(?listener, ?addr, "skip self-connection candidate");
                    continue;
                }
                let Some(url) = Self::listener_url_for_addr(listener, addr) else {
                    tracing::warn!(?listener, ?addr, "failed to build direct candidate URL");
                    continue;
                };
                if !seen.insert(url.to_string()) {
                    continue;
                }
                candidates.push(DirectCandidate {
                    url,
                    is_lan: Self::is_lan_candidate(addr.ip(), &local_networks),
                });
            }
        }
        candidates
    }

    async fn try_direct_candidate_bucket(
        self: &Arc<Self>,
        dst_peer_id: PeerId,
        candidates: &[DirectCandidate],
        protocol_order: &[String],
        stealth_modes: &HashMap<String, DirectStealthMode>,
    ) -> bool {
        let mut tasks = JoinSet::new();
        let priority_enabled =
            !TransportPriority::parse(&self.global_ctx.get_flags().transport_priority)
                .expect("transport_priority is validated while loading configuration")
                .is_empty();
        let target_key = priority_enabled.then(|| {
            candidates
                .iter()
                .map(|candidate| {
                    PreferenceKey::new(
                        if candidate.is_lan {
                            TransportPathClass::Lan
                        } else {
                            TransportPathClass::Wan
                        },
                        protocol_order,
                        candidate.url.scheme(),
                    )
                })
                .min()
                .expect("candidate bucket is not empty")
        });
        let mut group_index = 0u32;
        for protocol in protocol_order {
            let group = candidates
                .iter()
                .filter(|candidate| candidate.url.scheme() == protocol)
                .cloned()
                .collect::<Vec<_>>();
            if group.is_empty() {
                continue;
            }
            let delay = Duration::from_millis(u64::from(group_index) * 300);
            group_index += 1;
            for candidate in group {
                let this = self.clone();
                let stealth_mode = stealth_modes
                    .get(stealth_protocol_key_for_scheme(candidate.url.scheme()))
                    .copied()
                    .unwrap_or_default();
                let preference_key = target_key.map(|_| {
                    PreferenceKey::new(
                        if candidate.is_lan {
                            TransportPathClass::Lan
                        } else {
                            TransportPathClass::Wan
                        },
                        protocol_order,
                        candidate.url.scheme(),
                    )
                });
                tasks.spawn(async move {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    Self::try_connect_to_ip(
                        this,
                        dst_peer_id,
                        candidate.url.to_string(),
                        stealth_mode,
                        preference_key,
                    )
                    .await
                });
            }
        }

        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(DirectAttemptOutcome::Connected(None))) => {
                    tasks.abort_all();
                    return true;
                }
                Ok(Ok(DirectAttemptOutcome::Connected(Some(connected_key)))) => {
                    if Some(connected_key) == target_key {
                        tasks.abort_all();
                        return true;
                    }
                }
                Ok(Ok(DirectAttemptOutcome::AlreadySatisfied)) => {
                    if target_key
                        .is_some_and(|target| self.preference_satisfied(dst_peer_id, target))
                        || (target_key.is_none()
                            && self.peer_manager.has_directly_connected_conn(dst_peer_id))
                    {
                        tasks.abort_all();
                        return true;
                    }
                }
                Ok(Err(error)) => tracing::debug!(?error, ?dst_peer_id, "direct candidate failed"),
                Err(error) if !error.is_cancelled() => {
                    tracing::warn!(?error, ?dst_peer_id, "direct candidate task failed")
                }
                Err(_) => {}
            }
        }
        target_key.is_some_and(|target| self.preference_satisfied(dst_peer_id, target))
            || (target_key.is_none() && self.peer_manager.has_directly_connected_conn(dst_peer_id))
    }

    #[tracing::instrument(skip(self))]
    async fn do_try_direct_connect_internal(
        self: &Arc<DirectConnectorManagerData>,
        dst_peer_id: PeerId,
        ip_list: GetIpListResponse,
    ) -> Result<(), Error> {
        let route = self.peer_manager.get_route();
        let remote_peer_info = route.get_peer_info(dst_peer_id).await;
        let remote_feature_flag = remote_peer_info
            .as_ref()
            .and_then(|info| info.feature_flag.clone());
        let flags = self.global_ctx.get_flags();
        let enable_ipv6 = self.global_ctx.get_flags().enable_ipv6;
        let available_listeners = ip_list
            .listeners
            .clone()
            .into_iter()
            .map(Into::<url::Url>::into)
            .filter_map(|l| if l.scheme() != "ring" { Some(l) } else { None })
            .filter(|l| mapped_listener_port(l).is_some() && l.host().is_some())
            .filter(|l| enable_ipv6 || !matches!(l.host().unwrap().to_owned(), Host::Ipv6(_)))
            .collect::<Vec<_>>();

        tracing::debug!(?available_listeners, "got available listeners");

        if available_listeners.is_empty() {
            return Err(anyhow::anyhow!("peer {} have no valid listener", dst_peer_id).into());
        }

        let priority = TransportPriority::parse(&flags.transport_priority)
            .expect("transport_priority is validated while loading configuration");
        let configured_stealth =
            crate::common::stealth_registry::StealthProtocolSet::parse(&flags.stealth_protocols)
                .expect("stealth_protocols is validated while loading configuration");
        let stealth_modes = configured_stealth
            .effective_protocols(flags.stealth_mode)
            .into_iter()
            .map(|protocol| {
                let mode = direct_stealth_mode(protocol, remote_feature_flag.as_ref());
                (protocol.as_str().to_string(), mode)
            })
            .collect::<HashMap<_, _>>();
        let virtual_ip = remote_peer_info.as_ref().and_then(|info| {
            let ipv4 = info.ipv4_addr.map(|ip| IpAddr::V4(ip.into()));
            if ipv4.is_some_and(|ip| priority.has_virtual_ip_rule(ip)) {
                return ipv4;
            }
            info.ipv6_addr
                .as_ref()
                .and_then(|inet| inet.address)
                .map(|ip| IpAddr::V6(ip.into()))
        });
        let peer_ipv4 = remote_peer_info
            .as_ref()
            .and_then(|info| info.ipv4_addr)
            .map(|ip| IpAddr::V4(ip.into()));
        let peer_ipv6 = remote_peer_info
            .as_ref()
            .and_then(|info| info.ipv6_addr.as_ref())
            .and_then(|inet| inet.address)
            .map(|ip| IpAddr::V6(ip.into()));
        self.peer_manager
            .set_peer_transport_virtual_ips(dst_peer_id, peer_ipv4, peer_ipv6);

        let (lan_order, wan_order) = if priority.is_empty() {
            let mut order = crate::common::transport_priority::BUILTIN_TRANSPORT_ORDER
                .map(str::to_owned)
                .to_vec();
            let default_protocol = flags.default_protocol.to_ascii_lowercase();
            order.retain(|protocol| protocol != &default_protocol && protocol != "udp");
            let mut compatible = vec![default_protocol.clone()];
            if default_protocol != "udp" {
                compatible.push("udp".to_string());
            }
            compatible.extend(order);
            (compatible.clone(), compatible)
        } else {
            if !flags.default_protocol.is_empty()
                && !DEFAULT_PROTOCOL_DEPRECATION_LOGGED.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    default_protocol = %flags.default_protocol,
                    "transport_priority is set; default_protocol is ignored for direct-connect"
                );
            }
            for protocol in priority.configured_protocols() {
                if !protocol_is_compiled(protocol) {
                    let mut warned = UNCOMPILED_PRIORITY_WARNINGS.lock().unwrap();
                    if warned.insert(protocol.to_string()) {
                        tracing::warn!(?protocol, "configured transport is not compiled; skipping");
                    }
                }
            }
            (
                priority.order_for(true, virtual_ip),
                priority.order_for(false, virtual_ip),
            )
        };

        let candidates = self
            .expand_direct_candidates(&ip_list, &available_listeners)
            .await;
        if !priority.is_empty()
            && let Some(target) = candidates
                .iter()
                .map(|candidate| candidate.preference_key(&lan_order, &wan_order))
                .min()
        {
            self.preferred_direct_targets.insert(
                dst_peer_id,
                target,
                Duration::from_secs(DIRECT_CONNECTOR_BLACKLIST_TIMEOUT_SEC),
            );
        }
        let mut lan_candidates = candidates
            .iter()
            .filter(|candidate| candidate.is_lan)
            .cloned()
            .collect::<Vec<_>>();
        let mut wan_candidates = candidates
            .iter()
            .filter(|candidate| !candidate.is_lan)
            .cloned()
            .collect::<Vec<_>>();

        let existing_key = (!priority.is_empty())
            .then(|| self.peer_manager.best_transport_preference_key(dst_peer_id))
            .flatten();
        if !priority.is_empty() {
            lan_candidates = retain_higher_priority_candidates(
                lan_candidates,
                &lan_order,
                &wan_order,
                existing_key,
            );
            wan_candidates = retain_higher_priority_candidates(
                wan_candidates,
                &lan_order,
                &wan_order,
                existing_key,
            );
        }

        tracing::debug!(
            ?dst_peer_id,
            ?lan_candidates,
            ?wan_candidates,
            ?lan_order,
            ?wan_order,
            ?existing_key,
            "scheduled direct-connect candidates"
        );

        if !lan_candidates.is_empty()
            && self
                .try_direct_candidate_bucket(
                    dst_peer_id,
                    &lan_candidates,
                    &lan_order,
                    &stealth_modes,
                )
                .await
        {
            return Ok(());
        }
        if !wan_candidates.is_empty() {
            self.try_direct_candidate_bucket(
                dst_peer_id,
                &wan_candidates,
                &wan_order,
                &stealth_modes,
            )
            .await;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn do_try_direct_connect(
        self: Arc<DirectConnectorManagerData>,
        dst_peer_id: PeerId,
    ) -> Result<(), Error> {
        let mut backoff =
            udp_hole_punch::BackOff::new(vec![1000, 2000, 2000, 5000, 5000, 10000, 30000, 60000]);
        let mut attempt = 0;
        loop {
            if self.peer_black_list.contains(&dst_peer_id) {
                return Err(anyhow::anyhow!("peer {} is blacklisted", dst_peer_id).into());
            }

            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(backoff.next_backoff())).await;
            }
            attempt += 1;

            let peer_manager = self.peer_manager.clone();
            tracing::debug!("try direct connect to peer: {}", dst_peer_id);

            let rpc_stub = peer_manager
                .get_peer_rpc_mgr()
                .rpc_client()
                .scoped_client::<DirectConnectorRpcClientFactory<BaseController>>(
                peer_manager.my_peer_id(),
                dst_peer_id,
                self.global_ctx.get_network_name(),
            );

            let ip_list = rpc_stub
                .get_ip_list(BaseController::default(), GetIpListRequest {})
                .await;
            let ip_list = handle_rpc_result(ip_list, dst_peer_id, &self.peer_black_list)
                .with_context(|| format!("get ip list from peer {}", dst_peer_id))?;

            tracing::info!(ip_list = ?ip_list, dst_peer_id = ?dst_peer_id, "got ip list");

            let ret = self
                .do_try_direct_connect_internal(dst_peer_id, ip_list)
                .await;
            tracing::info!(?ret, ?dst_peer_id, "do_try_direct_connect return");

            let priority_enabled =
                !TransportPriority::parse(&self.global_ctx.get_flags().transport_priority)
                    .expect("transport_priority is validated while loading configuration")
                    .is_empty();
            if (!priority_enabled && peer_manager.has_directly_connected_conn(dst_peer_id))
                || (priority_enabled && self.direct_target_satisfied(dst_peer_id))
            {
                tracing::info!(
                    "direct connect to peer {} success, has direct conn",
                    dst_peer_id
                );
                return Ok(());
            }
        }
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_unspecified()
}

impl std::fmt::Debug for DirectConnectorManagerData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectConnectorManagerData")
            .field("peer_manager", &self.peer_manager)
            .finish()
    }
}

pub struct DirectConnectorManager {
    global_ctx: ArcGlobalCtx,
    data: Arc<DirectConnectorManagerData>,
    client: PeerTaskManager<DirectConnectorLauncher>,
    tasks: JoinSet<()>,
}

#[derive(Clone)]
struct DirectConnectorLauncher(Arc<DirectConnectorManagerData>);

#[async_trait::async_trait]
impl PeerTaskLauncher for DirectConnectorLauncher {
    type Data = Arc<DirectConnectorManagerData>;
    type CollectPeerItem = PeerId;
    type TaskRet = ();

    fn new_data(&self, _peer_mgr: Arc<PeerManager>) -> Self::Data {
        self.0.clone()
    }

    async fn collect_peers_need_task(&self, data: &Self::Data) -> Vec<Self::CollectPeerItem> {
        data.peer_black_list.cleanup();
        data.preferred_direct_targets.cleanup();
        let my_peer_id = data.peer_manager.my_peer_id();
        let flags = data.peer_manager.get_global_ctx().get_flags();
        let priority = TransportPriority::parse(&flags.transport_priority)
            .expect("transport_priority is validated while loading configuration");
        data.peer_manager
            .list_peers()
            .await
            .into_iter()
            .filter(|peer_id| {
                let direct_conn_satisfies_priority = if !priority.is_empty() {
                    data.direct_target_satisfied(*peer_id)
                } else {
                    data.peer_manager.has_directly_connected_conn(*peer_id)
                };
                *peer_id != my_peer_id
                    && !direct_conn_satisfies_priority
                    && !data.peer_black_list.contains(peer_id)
            })
            .collect()
    }

    async fn launch_task(
        &self,
        data: &Self::Data,
        item: Self::CollectPeerItem,
    ) -> tokio::task::JoinHandle<Result<Self::TaskRet, anyhow::Error>> {
        let data = data.clone();
        tokio::spawn(async move { data.do_try_direct_connect(item).await.map_err(Into::into) })
    }

    async fn all_task_done(&self, _data: &Self::Data) {}

    fn loop_interval_ms(&self) -> u64 {
        5000
    }
}

impl DirectConnectorManager {
    pub fn new(global_ctx: ArcGlobalCtx, peer_manager: Arc<PeerManager>) -> Self {
        let data = Arc::new(DirectConnectorManagerData::new(
            global_ctx.clone(),
            peer_manager.clone(),
        ));
        let client = PeerTaskManager::new_with_external_signal(
            DirectConnectorLauncher(data.clone()),
            peer_manager.clone(),
            Some(peer_manager.p2p_demand_notify()),
        );
        Self {
            global_ctx,
            data,
            client,
            tasks: JoinSet::new(),
        }
    }

    pub fn run(&mut self) {
        self.run_as_server();
        self.run_as_client();
        let mut events = self.global_ctx.subscribe();
        let data = self.data.clone();
        self.tasks.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(GlobalCtxEvent::ConfigPatched(_)) => {
                        data.clear_preferred_direct_targets();
                        data.peer_manager.p2p_demand_notify().notify();
                    }
                    Ok(GlobalCtxEvent::PeerRemoved(peer_id)) => {
                        data.preferred_direct_targets.remove(&peer_id);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        data.clear_preferred_direct_targets();
                        data.peer_manager.p2p_demand_notify().notify();
                        events = events.resubscribe();
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub fn run_as_server(&mut self) {
        self.data
            .peer_manager
            .get_peer_rpc_mgr()
            .rpc_server()
            .registry()
            .register(
                DirectConnectorRpcServer::new(DirectConnectorManagerRpcServer::new(
                    self.global_ctx.clone(),
                )),
                &self.data.global_ctx.get_network_name(),
            );
    }

    pub fn run_as_client(&mut self) {
        self.client.start();
    }

    #[cfg(test)]
    pub(crate) async fn try_direct_connect_with_ip_list(
        &self,
        dst_peer_id: PeerId,
        ip_list: GetIpListResponse,
    ) -> Result<(), Error> {
        self.data
            .do_try_direct_connect_internal(dst_peer_id, ip_list)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering as AtomicOrdering},
        },
    };

    use crate::{
        common::{error::Error, global_ctx::tests::get_mock_global_ctx},
        connector::direct::{
            DirectConnectorManager, DirectConnectorManagerData, DstListenerUrlBlackListItem,
        },
        connector::should_downgrade_udp_stealth,
        instance::listeners::ListenerManager,
        peers::tests::{
            connect_peer_manager, create_mock_peer_manager, wait_route_appear,
            wait_route_appear_with_cost,
        },
        proto::common::PeerFeatureFlag,
        proto::peer_rpc::GetIpListResponse,
        tunnel::{IpScheme, TunnelScheme, matches_scheme},
    };

    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{
        DirectCandidate, DirectConnectAttemptError, TESTING, mapped_listener_port,
        resolve_mapped_listener_addrs, retain_higher_priority_candidates,
    };

    #[tokio::test]
    async fn public_ipv6_candidate_rejects_easytier_managed_addr_even_in_tests() {
        let global_ctx = get_mock_global_ctx();
        let managed_ipv6: cidr::Ipv6Inet = "2001:db8::2/128".parse().unwrap();
        global_ctx.set_public_ipv6_routes(BTreeSet::from([managed_ipv6]));

        assert!(!super::is_usable_public_ipv6_candidate_with_mode(
            &"2001:db8::2".parse().unwrap(),
            &global_ctx,
            true,
        ));
        assert!(super::is_usable_public_ipv6_candidate_with_mode(
            &"::1".parse().unwrap(),
            &global_ctx,
            true,
        ));
    }

    #[tokio::test]
    async fn ipv6_hole_punch_candidates_are_deduped_filtered_and_capped() {
        let global_ctx = get_mock_global_ctx();
        let managed_ipv6: cidr::Ipv6Inet = "2001:db8::2/128".parse().unwrap();
        global_ctx.set_public_ipv6_routes(BTreeSet::from([managed_ipv6]));

        let first: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let managed = managed_ipv6.address();
        let second: Ipv6Addr = "2001:db8::3".parse().unwrap();
        let third: Ipv6Addr = "2001:db8::4".parse().unwrap();
        let mut candidates = Vec::new();

        super::push_ipv6_hole_punch_candidate(&mut candidates, first, &global_ctx, 2);
        super::push_ipv6_hole_punch_candidate(&mut candidates, first, &global_ctx, 2);
        super::push_ipv6_hole_punch_candidate(&mut candidates, managed, &global_ctx, 2);
        super::push_ipv6_hole_punch_candidate(&mut candidates, second, &global_ctx, 2);
        super::push_ipv6_hole_punch_candidate(&mut candidates, third, &global_ctx, 2);

        assert_eq!(candidates, vec![first, second]);
    }

    #[test]
    fn udp_ipv6_url_matches_hole_punch_branch_condition() {
        let remote_url: url::Url = "udp://[2001:db8::1]:11010".parse().unwrap();
        let takes_udp_ipv6_hole_punch_branch =
            matches_scheme!(remote_url, TunnelScheme::Ip(IpScheme::Udp))
                && matches!(remote_url.host(), Some(url::Host::Ipv6(_)));

        assert!(takes_udp_ipv6_hole_punch_branch);
    }

    #[test]
    fn mapped_listener_port_uses_ip_scheme_defaults() {
        assert_eq!(
            mapped_listener_port(&"ws://example.com".parse().unwrap()),
            Some(80)
        );
        assert_eq!(
            mapped_listener_port(&"wss://example.com".parse().unwrap()),
            Some(443)
        );
        assert_eq!(
            mapped_listener_port(&"tcp://127.0.0.1".parse().unwrap()),
            Some(11010)
        );
        assert_eq!(
            mapped_listener_port(&"udp://127.0.0.1".parse().unwrap()),
            Some(11010)
        );
    }

    #[test]
    fn priority_upgrade_keeps_only_better_candidates_than_existing_direct_conn() {
        let order = ["quic", "faketcp", "ws", "wg", "udp", "tcp", "wss"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let candidates = [
            "tcp://127.0.0.1:11010",
            "udp://127.0.0.1:11010",
            "quic://127.0.0.1:11012",
        ]
        .into_iter()
        .map(|url| DirectCandidate {
            url: url.parse().unwrap(),
            is_lan: true,
        })
        .collect::<Vec<_>>();

        let existing_tcp_key = crate::common::transport_priority::PreferenceKey::new(
            crate::common::transport_priority::TransportPathClass::Lan,
            &order,
            "tcp",
        );
        let upgraded = retain_higher_priority_candidates(
            candidates.clone(),
            &order,
            &order,
            Some(existing_tcp_key),
        );
        assert_eq!(
            upgraded
                .iter()
                .map(|candidate| candidate.url.scheme())
                .collect::<Vec<_>>(),
            ["udp", "quic"]
        );

        let existing_quic_key = crate::common::transport_priority::PreferenceKey::new(
            crate::common::transport_priority::TransportPathClass::Lan,
            &order,
            "quic",
        );
        let already_best = retain_higher_priority_candidates(
            candidates.clone(),
            &order,
            &order,
            Some(existing_quic_key),
        );
        assert!(already_best.is_empty());

        let no_existing =
            retain_higher_priority_candidates(candidates.clone(), &order, &order, None);
        assert_eq!(no_existing.len(), candidates.len());
    }

    #[test]
    fn priority_upgrade_compares_path_before_protocol() {
        let order = ["quic", "tcp"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let wan_quic = DirectCandidate {
            url: "quic://198.51.100.2:11012".parse().unwrap(),
            is_lan: false,
        };
        let lan_tcp = crate::common::transport_priority::PreferenceKey::new(
            crate::common::transport_priority::TransportPathClass::Lan,
            &order,
            "tcp",
        );

        assert!(
            retain_higher_priority_candidates(vec![wan_quic], &order, &order, Some(lan_tcp),)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn resolve_mapped_listener_addrs_uses_default_ports() {
        let wss_addrs = resolve_mapped_listener_addrs(&"wss://127.0.0.1".parse().unwrap())
            .await
            .unwrap();
        assert_eq!(
            wss_addrs,
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 443)]
        );

        let tcp_addrs = resolve_mapped_listener_addrs(&"tcp://127.0.0.1".parse().unwrap())
            .await
            .unwrap();
        assert_eq!(
            tcp_addrs,
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 11010)]
        );
    }

    async fn run_direct_connector_mapped_listener_test(
        mapped_listener: &str,
        target_listener: &str,
    ) {
        TESTING.store(true, std::sync::atomic::Ordering::Relaxed);
        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;
        let p_x = create_mock_peer_manager().await;
        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        connect_peer_manager(p_c.clone(), p_x.clone()).await;

        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();
        wait_route_appear(p_a.clone(), p_x.clone()).await.unwrap();

        let mut f = p_a.get_global_ctx().get_flags();
        f.bind_device = false;
        p_a.get_global_ctx().set_flags(f);

        p_c.get_global_ctx()
            .config
            .set_mapped_listeners(Some(vec![mapped_listener.parse().unwrap()]));

        p_x.get_global_ctx()
            .config
            .set_listeners(vec![target_listener.parse().unwrap()]);
        let mut lis_x = ListenerManager::new(p_x.get_global_ctx(), p_x.clone());
        lis_x.prepare_listeners().await.unwrap();
        lis_x.run().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let mut dm_a = DirectConnectorManager::new(p_a.get_global_ctx(), p_a.clone());
        let mut dm_c = DirectConnectorManager::new(p_c.get_global_ctx(), p_c.clone());
        dm_a.run_as_client();
        dm_c.run_as_server();
        // p_c's mapped listener is p_x's listener, so p_a should connect to p_x directly

        wait_route_appear_with_cost(p_a.clone(), p_x.my_peer_id(), Some(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn direct_connector_mapped_listener() {
        run_direct_connector_mapped_listener_test("tcp://127.0.0.1:11334", "tcp://0.0.0.0:11334")
            .await;
    }

    #[rstest::rstest]
    #[tokio::test]
    async fn direct_connector_basic_test(
        #[values("tcp", "udp", "wg")] proto: &str,
        #[values("true", "false")] ipv6: bool,
    ) {
        TESTING.store(true, std::sync::atomic::Ordering::Relaxed);

        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;
        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;

        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        p_c.get_global_ctx()
            .get_ip_collector()
            .collect_ip_addrs()
            .await;

        tokio::time::sleep(std::time::Duration::from_secs(4)).await;

        let mut dm_a = DirectConnectorManager::new(p_a.get_global_ctx(), p_a.clone());
        let mut dm_c = DirectConnectorManager::new(p_c.get_global_ctx(), p_c.clone());

        dm_a.run_as_client();
        dm_c.run_as_server();

        let port = if proto == "wg" { 11040 } else { 11041 };
        if !ipv6 {
            p_c.get_global_ctx().config.set_listeners(vec![
                format!("{}://0.0.0.0:{}", proto, port).parse().unwrap(),
            ]);
        } else {
            p_c.get_global_ctx()
                .config
                .set_listeners(vec![format!("{}://[::]:{}", proto, port).parse().unwrap()]);
        }
        let mut f = p_c.get_global_ctx().config.get_flags();
        f.enable_ipv6 = ipv6;
        p_c.get_global_ctx().set_flags(f);
        let mut lis_c = ListenerManager::new(p_c.get_global_ctx(), p_c.clone());
        lis_c.prepare_listeners().await.unwrap();

        lis_c.run().await.unwrap();

        wait_route_appear_with_cost(p_a.clone(), p_c.my_peer_id(), Some(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn direct_connector_scheme_blacklist() {
        TESTING.store(true, std::sync::atomic::Ordering::Relaxed);
        let p_a = create_mock_peer_manager().await;
        let data = Arc::new(DirectConnectorManagerData::new(
            p_a.get_global_ctx(),
            p_a.clone(),
        ));
        let mut ip_list = GetIpListResponse::default();
        ip_list
            .listeners
            .push("tcp://127.0.0.1:10222".parse().unwrap());

        ip_list
            .interface_ipv4s
            .push("127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap().into());

        data.do_try_direct_connect_internal(1, ip_list.clone())
            .await
            .unwrap();

        assert!(
            data.dst_listener_blacklist
                .contains(&DstListenerUrlBlackListItem(
                    1,
                    "tcp://127.0.0.1:10222".parse().unwrap()
                ))
        );
    }

    #[test]
    fn direct_connector_downgrades_udp_stealth_only_when_peer_disables_it() {
        assert!(!should_downgrade_udp_stealth(
            false,
            Some(&PeerFeatureFlag {
                stealth_supported: false,
                ..Default::default()
            }),
        ));
        assert!(!should_downgrade_udp_stealth(
            true,
            Some(&PeerFeatureFlag {
                stealth_supported: true,
                ..Default::default()
            }),
        ));
        assert!(!should_downgrade_udp_stealth(true, None,));
        assert!(should_downgrade_udp_stealth(
            true,
            Some(&PeerFeatureFlag {
                stealth_supported: false,
                ..Default::default()
            }),
        ));
    }

    #[test]
    fn direct_transport_stealth_preserves_unknown_capability_fallback() {
        use crate::{
            common::stealth_registry::{STEALTH_LEVEL_AUTHENTICATED, StealthProtocol},
            proto::common::{StealthTransportProtocol, TransportStealthCapability},
        };

        assert_eq!(
            super::direct_stealth_mode(StealthProtocol::Tcp, None),
            super::DirectStealthMode::PreferLegacyFallback
        );
        assert_eq!(
            super::direct_stealth_mode(StealthProtocol::Tcp, Some(&PeerFeatureFlag::default())),
            super::DirectStealthMode::Disabled
        );
        let feature = PeerFeatureFlag {
            stealth_capabilities: vec![TransportStealthCapability {
                protocol: StealthTransportProtocol::Tcp.into(),
                wire_version: 1,
                level_mask: STEALTH_LEVEL_AUTHENTICATED,
            }],
            ..Default::default()
        };
        assert_eq!(
            super::direct_stealth_mode(StealthProtocol::Tcp, Some(&feature)),
            super::DirectStealthMode::Required
        );
    }

    #[test]
    fn quic_brutal_uses_quic_stealth_capability_key() {
        assert_eq!(super::stealth_protocol_key_for_scheme("quic"), "quic");
        assert_eq!(
            super::stealth_protocol_key_for_scheme("quic-brutal"),
            "quic"
        );
        assert_eq!(super::stealth_protocol_key_for_scheme("tcp"), "tcp");
    }

    #[tokio::test]
    async fn guarded_public_ipv4_attempt_does_not_trigger_direct_fallback() {
        let remote_url: url::Url = "udp://198.51.100.10:11010".parse().unwrap();
        let fallback_called = Arc::new(AtomicBool::new(false));
        let fallback_called_clone = fallback_called.clone();
        let attempt: Result<(u32, uuid::Uuid), DirectConnectAttemptError> = Err(
            DirectConnectAttemptError::Guarded(Error::InvalidUrl("guarded".to_owned())),
        );

        let ret = super::resolve_public_ipv4_connect_result(&remote_url, attempt, move || {
            let fallback_called = fallback_called_clone.clone();
            async move {
                fallback_called.store(true, AtomicOrdering::Relaxed);
                Err(DirectConnectAttemptError::Failed(Error::NotFound))
            }
        })
        .await;

        assert!(matches!(ret, Err(DirectConnectAttemptError::Guarded(_))));
        assert!(!fallback_called.load(AtomicOrdering::Relaxed));
    }

    #[tokio::test]
    async fn failed_public_ipv4_attempt_still_uses_direct_fallback() {
        let remote_url: url::Url = "udp://198.51.100.10:11010".parse().unwrap();
        let fallback_called = Arc::new(AtomicBool::new(false));
        let fallback_called_clone = fallback_called.clone();
        let attempt: Result<(u32, uuid::Uuid), DirectConnectAttemptError> =
            Err(DirectConnectAttemptError::Failed(Error::NotFound));

        let ret = super::resolve_public_ipv4_connect_result(&remote_url, attempt, move || {
            let fallback_called = fallback_called_clone.clone();
            async move {
                fallback_called.store(true, AtomicOrdering::Relaxed);
                Ok((7, uuid::Uuid::nil()))
            }
        })
        .await;

        assert_eq!(ret.unwrap().0, 7);
        assert!(fallback_called.load(AtomicOrdering::Relaxed));
    }

    #[test]
    fn direct_candidates_are_classified_after_address_resolution() {
        let local_networks = [
            "192.168.50.10/24".parse().unwrap(),
            "169.254.10.1/24".parse().unwrap(),
            "fd00:50::10/64".parse().unwrap(),
            "fe80::1/64".parse().unwrap(),
            "203.0.113.10/24".parse().unwrap(),
            "2001:db8:50::10/64".parse().unwrap(),
        ];

        assert!(DirectConnectorManagerData::is_lan_candidate(
            "192.168.50.99".parse().unwrap(),
            &local_networks
        ));
        assert!(DirectConnectorManagerData::is_lan_candidate(
            "fd00:50::99".parse().unwrap(),
            &local_networks
        ));
        assert!(DirectConnectorManagerData::is_lan_candidate(
            "169.254.10.2".parse().unwrap(),
            &local_networks
        ));
        assert!(DirectConnectorManagerData::is_lan_candidate(
            "fe80::2".parse().unwrap(),
            &local_networks
        ));
        assert!(!DirectConnectorManagerData::is_lan_candidate(
            "169.254.200.2".parse().unwrap(),
            &local_networks
        ));
        assert!(!DirectConnectorManagerData::is_lan_candidate(
            "fe80:1::2".parse().unwrap(),
            &local_networks
        ));
        assert!(!DirectConnectorManagerData::is_lan_candidate(
            "198.51.100.7".parse().unwrap(),
            &local_networks
        ));
        assert!(!DirectConnectorManagerData::is_lan_candidate(
            "203.0.113.99".parse().unwrap(),
            &local_networks
        ));
        assert!(!DirectConnectorManagerData::is_lan_candidate(
            "2001:db8:50::99".parse().unwrap(),
            &local_networks
        ));
    }

    #[cfg(feature = "quic")]
    #[test]
    fn direct_candidate_does_not_copy_remote_quic_brutal_send_rate() {
        let listener: url::Url = "quic-brutal://0.0.0.0:21013?tx_bps=500000000"
            .parse()
            .unwrap();
        let candidate = DirectConnectorManagerData::listener_url_for_addr(
            &listener,
            "192.0.2.10:21013".parse().unwrap(),
        )
        .unwrap();

        assert_eq!(candidate.as_str(), "quic-brutal://192.0.2.10:21013");
    }
}
