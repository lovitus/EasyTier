use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::{
    common::{
        PeerId,
        dns::socket_addrs,
        error::Error,
        global_ctx::{ArcGlobalCtx, UnderlayBreakerScope, UnderlayPreflightGuard},
        idn, underlay_guard,
    },
    connector::dns_connector::DnsTunnelConnector,
    proto::common::PeerFeatureFlag,
    tunnel::{
        self, IpScheme, IpVersion, TunnelConnector, TunnelError, TunnelScheme,
        ring::RingTunnelConnector, tcp::TcpTunnelConnector, udp::UdpTunnelConnector,
    },
    utils::BoxExt,
};
use http_connector::HttpTunnelConnector;
use rand::seq::SliceRandom;

pub mod direct;
pub mod manual;
pub mod tcp_hole_punch;
pub mod udp_hole_punch;

pub mod dns_connector;
pub mod http_connector;

pub(crate) fn should_try_p2p_with_peer(
    feature_flag: Option<&PeerFeatureFlag>,
    allow_public_server: bool,
    local_disable_p2p: bool,
    local_need_p2p: bool,
) -> bool {
    feature_flag
        .map(|flag| {
            (allow_public_server || !flag.is_public_server)
                && (!local_disable_p2p || flag.need_p2p)
                && (!flag.disable_p2p || local_need_p2p)
        })
        .unwrap_or(!local_disable_p2p)
}

pub(crate) fn should_background_p2p_with_peer(
    feature_flag: Option<&PeerFeatureFlag>,
    allow_public_server: bool,
    lazy_p2p: bool,
    local_disable_p2p: bool,
    local_need_p2p: bool,
) -> bool {
    should_try_p2p_with_peer(
        feature_flag,
        allow_public_server,
        local_disable_p2p,
        local_need_p2p,
    ) && (!lazy_p2p || feature_flag.map(|flag| flag.need_p2p).unwrap_or(false))
}

pub(crate) fn should_downgrade_udp_stealth(
    local_stealth_mode: bool,
    remote_feature_flag: Option<&PeerFeatureFlag>,
) -> bool {
    local_stealth_mode && remote_feature_flag.is_some_and(|flag| !flag.stealth_supported)
}

pub(crate) fn should_attempt_ranked_hole_punch(
    has_peer: bool,
    priority_enabled: bool,
    has_same_transport: bool,
    candidate_improves: bool,
) -> bool {
    !has_peer || (priority_enabled && !has_same_transport && candidate_improves)
}

async fn set_bind_addr_for_peer_connector(
    connector: &mut (impl TunnelConnector + ?Sized),
    is_ipv4: bool,
    global_ctx: &ArcGlobalCtx,
) {
    if cfg!(any(
        target_os = "android",
        any(
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne")
        ),
        target_env = "ohos"
    )) {
        return;
    }

    let ips = global_ctx
        .get_ip_collector()
        .collect_local_ip_addrs_now()
        .await;
    if is_ipv4 {
        let mut bind_addrs = vec![];
        for ipv4 in ips.interface_ipv4s {
            let ip = IpAddr::V4(ipv4.into());
            if underlay_guard::should_block_underlay_ip(global_ctx, ip) {
                continue;
            }
            let socket_addr = SocketAddrV4::new(ipv4.into(), 0).into();
            bind_addrs.push(socket_addr);
        }
        connector.set_bind_addrs(bind_addrs);
    } else {
        let mut bind_addrs = vec![];
        for ipv6 in ips.interface_ipv6s.iter().chain(ips.public_ipv6.iter()) {
            let ipv6 = std::net::Ipv6Addr::from(*ipv6);
            if global_ctx.is_ip_easytier_managed_ipv6(&ipv6)
                || underlay_guard::should_block_underlay_ip(global_ctx, IpAddr::V6(ipv6))
            {
                continue;
            }
            let socket_addr = SocketAddrV6::new(ipv6, 0, 0, 0).into();
            bind_addrs.push(socket_addr);
        }
        connector.set_bind_addrs(bind_addrs);
    }
    let _ = connector;
}

struct ResolvedConnectorAddr {
    addr: SocketAddr,
    ip_version: IpVersion,
    preflight: UnderlayPreflightGuard,
}

pub(crate) struct PreparedUnderlayConnector {
    inner: Box<dyn TunnelConnector + 'static>,
    preflight: Option<UnderlayPreflightGuard>,
    resolved_addr: Option<SocketAddr>,
}

impl std::fmt::Debug for PreparedUnderlayConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedUnderlayConnector")
            .field("remote_url", &self.inner.remote_url())
            .field("has_preflight", &self.preflight.is_some())
            .finish()
    }
}

impl PreparedUnderlayConnector {
    pub(crate) fn commit_preflight(&mut self) {
        if let Some(mut preflight) = self.preflight.take() {
            preflight.commit();
        }
    }

    /// The exact address that was resolved and validated by the underlay
    /// guard for this connector. Callers that bypass `connect()` and build
    /// their own transport (e.g. direct UDP hole-punch) must reuse this
    /// address instead of re-resolving the remote URL, otherwise the
    /// re-resolved address (DNS/multi-addr URLs are resolved randomly, see
    /// `SocketAddr::from_url`) may never have been checked by the guard.
    pub(crate) fn resolved_addr(&self) -> Option<SocketAddr> {
        self.resolved_addr
    }
}

#[async_trait::async_trait]
impl TunnelConnector for PreparedUnderlayConnector {
    async fn connect(&mut self) -> Result<Box<dyn tunnel::Tunnel>, TunnelError> {
        self.commit_preflight();
        self.inner.connect().await
    }

    fn remote_url(&self) -> url::Url {
        self.inner.remote_url()
    }

    fn set_bind_addrs(&mut self, addrs: Vec<SocketAddr>) {
        self.inner.set_bind_addrs(addrs);
    }

    fn set_ip_version(&mut self, ip_version: IpVersion) {
        self.inner.set_ip_version(ip_version);
    }

    fn set_resolved_addr(&mut self, addr: SocketAddr) {
        self.inner.set_resolved_addr(addr);
    }

    fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.inner.set_socket_mark(socket_mark);
    }

    fn disable_stealth(&mut self) {
        self.inner.disable_stealth();
    }

    fn require_stealth(&mut self) {
        self.inner.require_stealth();
    }
}

fn connector_default_port(url: &url::Url) -> Option<u16> {
    url.try_into()
        .ok()
        .and_then(|s: TunnelScheme| s.try_into().ok())
        .map(IpScheme::default_port)
}

fn addr_matches_ip_version(addr: &SocketAddr, ip_version: IpVersion) -> bool {
    match ip_version {
        IpVersion::V4 => addr.is_ipv4(),
        IpVersion::V6 => addr.is_ipv6(),
        IpVersion::Both => true,
    }
}

fn infer_effective_ip_version(addrs: &[SocketAddr], requested_ip_version: IpVersion) -> IpVersion {
    match requested_ip_version {
        IpVersion::Both if addrs.iter().all(SocketAddr::is_ipv4) => IpVersion::V4,
        IpVersion::Both if addrs.iter().all(SocketAddr::is_ipv6) => IpVersion::V6,
        _ => requested_ip_version,
    }
}

async fn resolve_connector_socket_addr(
    url: &url::Url,
    global_ctx: &ArcGlobalCtx,
    scheme: IpScheme,
    ip_version: IpVersion,
    scope: UnderlayBreakerScope,
    expected_peer_id: Option<PeerId>,
) -> Result<ResolvedConnectorAddr, Error> {
    let mut addrs = socket_addrs(url, || connector_default_port(url))
        .await
        .map_err(|e| {
            TunnelError::InvalidAddr(format!(
                "failed to resolve socket addr, url: {}, error: {}",
                url, e
            ))
        })?;

    addrs.retain(|addr| addr_matches_ip_version(addr, ip_version));
    let effective_ip_version = infer_effective_ip_version(&addrs, ip_version);
    addrs.shuffle(&mut rand::thread_rng());

    let mut rejected_reason = None;
    let skip_source_validation_errors = ip_version == IpVersion::Both;
    for addr in addrs {
        match underlay_guard::prepare_underlay_attempt(
            global_ctx,
            addr,
            scheme,
            scope,
            expected_peer_id,
        )
        .await
        {
            Ok(preflight) => {
                return Ok(ResolvedConnectorAddr {
                    addr,
                    ip_version: effective_ip_version,
                    preflight,
                });
            }
            Err(err) if skip_source_validation_errors => {
                rejected_reason = Some(format!(
                    "{} candidate {} could not be validated: {}",
                    url, addr, err
                ));
            }
            Err(err) => return Err(err),
        }
    }

    if let Some(reason) = rejected_reason {
        return Err(Error::InvalidUrl(format!(
            "{}, refusing guarded underlay connection",
            reason
        )));
    }

    Err(Error::TunnelError(TunnelError::NoDnsRecordFound(
        ip_version,
    )))
}

pub async fn create_connector_by_url(
    url: &str,
    global_ctx: &ArcGlobalCtx,
    ip_version: IpVersion,
) -> Result<Box<dyn TunnelConnector + 'static>, Error> {
    Ok(Box::new(
        create_connector_by_url_with_scope(
            url,
            global_ctx,
            ip_version,
            UnderlayBreakerScope::Generic,
            None,
        )
        .await?,
    ))
}

pub(crate) async fn create_connector_by_url_with_scope(
    url: &str,
    global_ctx: &ArcGlobalCtx,
    ip_version: IpVersion,
    scope: UnderlayBreakerScope,
    expected_peer_id: Option<PeerId>,
) -> Result<PreparedUnderlayConnector, Error> {
    let url = url::Url::parse(url).map_err(|_| Error::InvalidUrl(url.to_owned()))?;
    let url = idn::convert_idn_to_ascii(url)?;
    let scheme = (&url)
        .try_into()
        .map_err(|_| TunnelError::InvalidProtocol(url.scheme().to_owned()))?;
    let mut effective_connector_ip_version = ip_version;
    let mut preflight = None;
    let mut resolved_socket_addr = None;
    let mut connector: Box<dyn TunnelConnector + 'static> = match scheme {
        TunnelScheme::Ip(scheme) => {
            let resolved_addr = resolve_connector_socket_addr(
                &url,
                global_ctx,
                scheme,
                ip_version,
                scope,
                expected_peer_id,
            )
            .await?;
            effective_connector_ip_version = resolved_addr.ip_version;
            resolved_socket_addr = Some(resolved_addr.addr);
            preflight = Some(resolved_addr.preflight);
            let mut connector: Box<dyn TunnelConnector> = match scheme {
                IpScheme::Tcp => {
                    let mut connector = TcpTunnelConnector::new(url);
                    let flags = global_ctx.get_flags();
                    let tcp_stealth = crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::Tcp,
                    );
                    connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
                        global_ctx.get_network_identity().network_secret.as_deref(),
                        tcp_stealth,
                        global_ctx.is_secure_mode_enabled(),
                        flags.stealth_window_secs,
                    ));
                    connector.boxed()
                }
                IpScheme::Udp => {
                    let mut c = UdpTunnelConnector::new(url);
                    let flags = global_ctx.get_flags();
                    let secure_mode = global_ctx.is_secure_mode_enabled();
                    let udp_stealth = crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::Udp,
                    );
                    c.prefer_stealth_with_legacy_fallback(
                        crate::tunnel::stealth::build_outer_session(
                            global_ctx.get_network_identity().network_secret.as_deref(),
                            udp_stealth,
                            secure_mode,
                            flags.stealth_window_secs,
                        ),
                    );
                    c.boxed()
                }
                #[cfg(feature = "quic")]
                IpScheme::Quic => {
                    let mut connector =
                        tunnel::quic::QuicTunnelConnector::new(url, global_ctx.clone());
                    let flags = global_ctx.get_flags();
                    let enabled = crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::Quic,
                    );
                    connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
                        global_ctx.get_network_identity().network_secret.as_deref(),
                        enabled,
                        global_ctx.is_secure_mode_enabled(),
                        flags.stealth_window_secs,
                    ));
                    connector.boxed()
                }
                #[cfg(feature = "wireguard")]
                IpScheme::Wg => {
                    use crate::tunnel::wireguard::{WgConfig, WgTunnelConnector};
                    let nid = global_ctx.get_network_identity();
                    let wg_config = WgConfig::new_from_network_identity(
                        &nid.network_name,
                        &nid.network_secret.unwrap_or_default(),
                    );
                    let mut connector = WgTunnelConnector::new(url, wg_config);
                    let flags = global_ctx.get_flags();
                    let enabled = crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::Wg,
                    );
                    connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
                        global_ctx.get_network_identity().network_secret.as_deref(),
                        enabled,
                        global_ctx.is_secure_mode_enabled(),
                        flags.stealth_window_secs,
                    ));
                    connector.boxed()
                }
                #[cfg(feature = "websocket")]
                IpScheme::Ws | IpScheme::Wss => {
                    let mut connector = tunnel::websocket::WsTunnelConnector::new(url);
                    let flags = global_ctx.get_flags();
                    let protocol = if matches!(scheme, IpScheme::Wss) {
                        crate::common::stealth_registry::StealthProtocol::Wss
                    } else {
                        crate::common::stealth_registry::StealthProtocol::Ws
                    };
                    let enabled =
                        crate::common::stealth_registry::protocol_enabled(&flags, protocol);
                    connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
                        global_ctx.get_network_identity().network_secret.as_deref(),
                        enabled,
                        global_ctx.is_secure_mode_enabled(),
                        flags.stealth_window_secs,
                    ));
                    connector.boxed()
                }
                #[cfg(feature = "faketcp")]
                IpScheme::FakeTcp => {
                    let mut connector = tunnel::fake_tcp::FakeTcpTunnelConnector::new(url);
                    let flags = global_ctx.get_flags();
                    let enabled = crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::FakeTcp,
                    );
                    connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
                        global_ctx.get_network_identity().network_secret.as_deref(),
                        enabled,
                        global_ctx.is_secure_mode_enabled(),
                        flags.stealth_window_secs,
                    ));
                    connector.boxed()
                }
            };
            connector.set_resolved_addr(resolved_addr.addr);
            connector.set_socket_mark(global_ctx.config.get_flags().socket_mark);
            if global_ctx.config.get_flags().bind_device {
                set_bind_addr_for_peer_connector(
                    &mut connector,
                    resolved_addr.addr.is_ipv4(),
                    global_ctx,
                )
                .await;
            }
            connector
        }
        #[cfg(unix)]
        TunnelScheme::Unix => tunnel::unix::UnixSocketTunnelConnector::new(url).boxed(),
        TunnelScheme::Http | TunnelScheme::Https => {
            HttpTunnelConnector::new(url, global_ctx.clone()).boxed()
        }
        TunnelScheme::Ring => RingTunnelConnector::new(url).boxed(),
        TunnelScheme::Txt | TunnelScheme::Srv => {
            if url.host_str().is_none() {
                return Err(Error::InvalidUrl(format!(
                    "host should not be empty in txt or srv url: {}",
                    url
                )));
            }
            DnsTunnelConnector::new(url, global_ctx.clone()).boxed()
        }
    };
    connector.set_ip_version(effective_connector_ip_version);

    Ok(PreparedUnderlayConnector {
        inner: connector,
        preflight,
        resolved_addr: resolved_socket_addr,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        common::global_ctx::tests::get_mock_global_ctx,
        proto::common::PeerFeatureFlag,
        tunnel::{IpScheme, IpVersion},
    };

    use super::{
        create_connector_by_url, should_attempt_ranked_hole_punch, should_background_p2p_with_peer,
        should_try_p2p_with_peer,
    };

    #[test]
    fn ranked_hole_punch_only_upgrades_existing_peer() {
        assert!(should_attempt_ranked_hole_punch(false, false, false, false));
        assert!(!should_attempt_ranked_hole_punch(true, false, false, true));
        assert!(!should_attempt_ranked_hole_punch(true, true, true, true));
        assert!(!should_attempt_ranked_hole_punch(true, true, false, false));
        assert!(should_attempt_ranked_hole_punch(true, true, false, true));
    }

    #[tokio::test]
    async fn connector_rejects_easytier_managed_ipv6_destination() {
        let global_ctx = get_mock_global_ctx();
        let public_route: cidr::Ipv6Inet = "2001:db8::2/128".parse().unwrap();
        global_ctx.set_public_ipv6_routes(BTreeSet::from([public_route]));

        let ret =
            create_connector_by_url("tcp://[2001:db8::2]:11010", &global_ctx, IpVersion::V6).await;

        assert!(matches!(
            ret,
            Err(crate::common::error::Error::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn connector_rejects_configured_underlay_destination() {
        let global_ctx = get_mock_global_ctx();
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = true;
        flags.underlay_exclude_cidrs = "203.0.113.0/24".to_string();
        global_ctx.set_flags(flags);

        let ipv4_ret =
            create_connector_by_url("tcp://203.0.113.10:11010", &global_ctx, IpVersion::V4).await;
        let ipv6_ret = create_connector_by_url(
            "tcp://[fdfe:dcba:9876::2]:11010",
            &global_ctx,
            IpVersion::V6,
        )
        .await;

        assert!(matches!(
            ipv4_ret,
            Err(crate::common::error::Error::InvalidUrl(_))
        ));
        assert!(matches!(
            ipv6_ret,
            Err(crate::common::error::Error::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn connector_factory_ignores_runtime_loop_suppression_for_generic_clients() {
        let global_ctx = get_mock_global_ctx();
        global_ctx.record_protocol_self_loop(
            IpScheme::Tcp,
            crate::common::global_ctx::ProtocolLoopScope::Direct,
        );
        global_ctx.record_protocol_self_loop(
            IpScheme::Tcp,
            crate::common::global_ctx::ProtocolLoopScope::Direct,
        );
        let ret =
            create_connector_by_url("tcp://127.0.0.1:11010", &global_ctx, IpVersion::V4).await;
        assert!(ret.is_ok());
    }

    #[test]
    fn lazy_background_p2p_requires_need_p2p() {
        let no_need_p2p = PeerFeatureFlag {
            need_p2p: false,
            ..Default::default()
        };
        let need_p2p = PeerFeatureFlag {
            need_p2p: true,
            ..Default::default()
        };

        assert!(should_background_p2p_with_peer(
            Some(&no_need_p2p),
            false,
            false,
            false,
            false
        ));
        assert!(!should_background_p2p_with_peer(
            Some(&no_need_p2p),
            false,
            true,
            false,
            false
        ));
        assert!(should_background_p2p_with_peer(
            Some(&need_p2p),
            false,
            true,
            false,
            false
        ));
    }

    #[test]
    fn p2p_policy_respects_public_server_setting() {
        let public_server = PeerFeatureFlag {
            is_public_server: true,
            ..Default::default()
        };

        assert!(!should_try_p2p_with_peer(
            Some(&public_server),
            false,
            false,
            false
        ));
        assert!(should_try_p2p_with_peer(
            Some(&public_server),
            true,
            false,
            false
        ));
        assert!(!should_background_p2p_with_peer(
            Some(&public_server),
            false,
            false,
            false,
            false
        ));
        assert!(should_background_p2p_with_peer(
            Some(&public_server),
            true,
            false,
            false,
            false
        ));
    }

    #[test]
    fn disable_p2p_only_allows_need_p2p_exceptions() {
        let normal_peer = PeerFeatureFlag::default();
        let need_peer = PeerFeatureFlag {
            need_p2p: true,
            ..Default::default()
        };
        let disable_peer = PeerFeatureFlag {
            disable_p2p: true,
            ..Default::default()
        };
        let disable_need_peer = PeerFeatureFlag {
            disable_p2p: true,
            need_p2p: true,
            ..Default::default()
        };

        assert!(should_try_p2p_with_peer(
            Some(&normal_peer),
            false,
            false,
            false
        ));
        assert!(should_try_p2p_with_peer(None, false, false, false));
        assert!(!should_try_p2p_with_peer(None, false, true, false));
        assert!(!should_try_p2p_with_peer(
            Some(&normal_peer),
            false,
            true,
            false
        ));
        assert!(should_try_p2p_with_peer(
            Some(&need_peer),
            false,
            true,
            false
        ));
        assert!(!should_try_p2p_with_peer(
            Some(&disable_peer),
            false,
            false,
            false
        ));
        assert!(should_try_p2p_with_peer(
            Some(&disable_peer),
            false,
            false,
            true
        ));
        assert!(should_try_p2p_with_peer(
            Some(&disable_need_peer),
            false,
            true,
            true
        ));
        assert!(!should_try_p2p_with_peer(
            Some(&disable_need_peer),
            false,
            true,
            false
        ));
    }
}
