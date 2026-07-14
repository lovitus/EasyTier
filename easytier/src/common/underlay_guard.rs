use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::Context;

use crate::{
    common::{
        config::Flags,
        error::Error,
        global_ctx::{
            ArcGlobalCtx, UnderlayBreakerKey, UnderlayBreakerScope, UnderlayBreakerStrikeKind,
            UnderlayBreakerTrace, UnderlayPreflightGuard,
        },
        network::IPCollector,
    },
    tunnel::IpScheme,
};

pub const DEFAULT_UNDERLAY_EXCLUDE_CIDRS: &str =
    "198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,192.19.0.0/24";
pub const BUILTIN_UNDERLAY_GUARD_CIDRS: &str = DEFAULT_UNDERLAY_EXCLUDE_CIDRS;

type ParsedCidrCache = Option<(String, Arc<Vec<cidr::IpCidr>>)>;

static PARSED_CIDR_CACHE: LazyLock<Mutex<ParsedCidrCache>> = LazyLock::new(|| Mutex::new(None));
static BUILTIN_CIDRS: LazyLock<Vec<cidr::IpCidr>> = LazyLock::new(|| {
    parse_exclude_cidrs(BUILTIN_UNDERLAY_GUARD_CIDRS)
        .expect("built-in underlay guard CIDRs must be valid")
});

fn parse_one_cidr(item: &str) -> anyhow::Result<cidr::IpCidr> {
    if let Ok(cidr) = item.parse::<cidr::IpCidr>() {
        return Ok(cidr);
    }

    let inet = item
        .parse::<cidr::IpInet>()
        .with_context(|| format!("invalid underlay exclude CIDR: {item}"))?;
    Ok(inet.network())
}

pub fn parse_exclude_cidrs(input: &str) -> anyhow::Result<Vec<cidr::IpCidr>> {
    input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(parse_one_cidr)
        .collect()
}

pub fn validate_exclude_cidrs(input: &str) -> anyhow::Result<()> {
    parse_exclude_cidrs(input).map(|_| ())
}

fn configured_excludes(flags: &Flags) -> anyhow::Result<Arc<Vec<cidr::IpCidr>>> {
    let key = flags.underlay_exclude_cidrs.clone();
    let mut cache = PARSED_CIDR_CACHE.lock().unwrap();
    if let Some((cached_key, parsed)) = cache.as_ref()
        && cached_key == &key
    {
        return Ok(parsed.clone());
    }

    let parsed = Arc::new(parse_exclude_cidrs(&key)?);
    *cache = Some((key, parsed.clone()));
    Ok(parsed)
}

fn configured_excludes_match(flags: &Flags, ip: IpAddr) -> bool {
    match configured_excludes(flags) {
        Ok(excludes) => excludes.iter().any(|cidr| cidr.contains(&ip)),
        Err(error) => {
            tracing::warn!(
                ?error,
                "underlay exclude CIDR list is invalid; skipping CIDR guard"
            );
            false
        }
    }
}

fn builtin_excludes_match(ip: IpAddr) -> bool {
    BUILTIN_CIDRS.iter().any(|cidr| cidr.contains(&ip))
}

pub fn is_runtime_guarded_ip(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_virtual_ipv4(global_ctx, v4),
        IpAddr::V6(v6) => global_ctx.is_ip_easytier_managed_ipv6(&v6),
    }
}

pub fn should_block_underlay_ip(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> bool {
    let flags = global_ctx.get_flags();
    flags.underlay_candidate_guard
        && (is_runtime_guarded_ip(global_ctx, ip)
            || builtin_excludes_match(ip)
            || configured_excludes_match(&flags, ip))
}

fn historical_guarded_ip(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V6(ipv6) if global_ctx.is_ip_easytier_managed_ipv6(&ipv6))
}

fn wildcard_udp_bind_addr(remote_addr: SocketAddr) -> SocketAddr {
    if remote_addr.is_ipv4() {
        SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, 0, 0, 0).into()
    }
}

fn suspicious_interface_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utun")
        || name.starts_with("tun")
        || name.starts_with("tap")
        || name.contains("wintun")
}

async fn source_interface_signal(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> Option<(String, bool)> {
    for iface in IPCollector::collect_interfaces(global_ctx.net_ns.clone(), false).await {
        if !iface.ips.iter().any(|network| network.ip() == ip) {
            continue;
        }

        let suspicious =
            iface.is_point_to_point() || suspicious_interface_name(iface.name.as_str());
        return Some((iface.name, suspicious));
    }

    None
}

fn bind_device_source_filter_active() -> bool {
    !cfg!(any(
        target_os = "android",
        any(
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne")
        ),
        target_env = "ohos"
    ))
}

async fn bind_device_sources(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
) -> Vec<SocketAddr> {
    if !global_ctx.get_flags().bind_device || !bind_device_source_filter_active() {
        return Vec::new();
    }

    // A cached empty or removed source can otherwise block every reconnect
    // for the advertisement cache lifetime after a network transition.
    let ips = global_ctx
        .get_ip_collector()
        .collect_local_ip_addrs_now()
        .await;
    if remote_addr.is_ipv4() {
        ips.interface_ipv4s
            .into_iter()
            .filter_map(|ip| {
                let ip = Ipv4Addr::from(ip);
                let ip_addr = IpAddr::V4(ip);
                (!should_block_underlay_ip(global_ctx, ip_addr))
                    .then_some(SocketAddrV4::new(ip, 0).into())
            })
            .collect()
    } else {
        ips.interface_ipv6s
            .into_iter()
            .chain(ips.public_ipv6)
            .filter_map(|ip| {
                let ip = std::net::Ipv6Addr::from(ip);
                let ip_addr = IpAddr::V6(ip);
                (!historical_guarded_ip(global_ctx, ip_addr)
                    && !should_block_underlay_ip(global_ctx, ip_addr))
                .then_some(SocketAddrV6::new(ip, 0, 0, 0).into())
            })
            .collect()
    }
}

async fn validate_connected_udp_source(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    bind_addr: SocketAddr,
    key: UnderlayBreakerKey,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
) -> Result<(), Error> {
    let socket = crate::tunnel::common::bind::<tokio::net::UdpSocket>()
        .addr(bind_addr)
        .net_ns(global_ctx.net_ns.clone())
        .only_v6(remote_addr.is_ipv6())
        .maybe_socket_mark(global_ctx.get_flags().socket_mark)
        .call()?;
    socket.connect(remote_addr).await?;

    let local_ip = socket.local_addr()?.ip();
    if historical_guarded_ip(global_ctx, local_ip) || should_block_underlay_ip(global_ctx, local_ip)
    {
        global_ctx.record_underlay_breaker_strike(
            key,
            UnderlayBreakerStrikeKind::Hard,
            "guarded_source_ip",
            Some(UnderlayBreakerTrace {
                local_ip: Some(local_ip),
                ..Default::default()
            }),
        );
        return Err(Error::InvalidUrl(format!(
            "underlay candidate {remote_addr} would use guarded local source {local_ip}"
        )));
    }

    match source_interface_signal(global_ctx, local_ip).await {
        Some((ifname, true)) => {
            global_ctx.record_underlay_breaker_strike(
                key,
                UnderlayBreakerStrikeKind::Soft,
                "suspicious_source_interface",
                Some(UnderlayBreakerTrace {
                    local_ip: Some(local_ip),
                    ifname: Some(ifname),
                    ..Default::default()
                }),
            );
        }
        Some((ifname, false)) => {
            tracing::trace!(
                ?remote_addr,
                ?scheme,
                ?scope,
                ?bind_addr,
                ?local_ip,
                ?ifname,
                "underlay validation source interface accepted"
            );
        }
        None => {
            tracing::debug!(
                ?remote_addr,
                ?scheme,
                ?scope,
                ?bind_addr,
                ?local_ip,
                "underlay validation could not map source IP to an interface"
            );
        }
    }

    Ok(())
}

pub async fn sanitize_underlay_candidate(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
) -> Result<(), Error> {
    if historical_guarded_ip(global_ctx, remote_addr.ip()) {
        return Err(Error::InvalidUrl(format!(
            "underlay candidate {remote_addr} is EasyTier-managed IPv6"
        )));
    }

    if !global_ctx.get_flags().underlay_candidate_guard {
        return Ok(());
    }

    let key = UnderlayBreakerKey::endpoint(remote_addr, scheme, scope);

    if should_block_underlay_ip(global_ctx, remote_addr.ip()) {
        global_ctx.record_underlay_breaker_strike(
            key,
            UnderlayBreakerStrikeKind::Hard,
            "guarded_remote_ip",
            None,
        );
        return Err(Error::InvalidUrl(format!(
            "underlay candidate {remote_addr} resolves to guarded address {}",
            remote_addr.ip()
        )));
    }

    let bind_sources = bind_device_sources(global_ctx, remote_addr).await;
    if !bind_sources.is_empty() {
        let mut last_error = None;
        for bind_addr in bind_sources {
            match validate_connected_udp_source(
                global_ctx,
                remote_addr,
                bind_addr,
                key.clone(),
                scheme,
                scope,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }

        return Err(last_error.unwrap_or_else(|| {
            Error::InvalidUrl(format!(
                "underlay candidate {remote_addr} has no usable bind-device source"
            ))
        }));
    }

    if global_ctx.get_flags().bind_device && bind_device_source_filter_active() {
        let reason = if remote_addr.is_ipv4() {
            "no usable IPv4 bind-device source"
        } else {
            "no usable IPv6 bind-device source"
        };
        return Err(Error::InvalidUrl(format!(
            "underlay candidate {remote_addr} refused: {reason}"
        )));
    }

    validate_connected_udp_source(
        global_ctx,
        remote_addr,
        wildcard_udp_bind_addr(remote_addr),
        key,
        scheme,
        scope,
    )
    .await
}

pub async fn prepare_underlay_attempt(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
    expected_peer_id: Option<crate::common::PeerId>,
) -> Result<UnderlayPreflightGuard, Error> {
    let endpoint_key = UnderlayBreakerKey::endpoint(remote_addr, scheme, scope);
    let mut keys = Vec::with_capacity(2);
    if let Some(peer_id) = expected_peer_id {
        keys.push(UnderlayBreakerKey::peer(peer_id, scheme, scope));
    }
    keys.push(endpoint_key);

    let guard = global_ctx
        .try_begin_underlay_attempt(&keys)
        .map_err(|error| {
            Error::InvalidUrl(format!(
                "underlay candidate {remote_addr} is temporarily gated: {error}"
            ))
        })?;
    sanitize_underlay_candidate(global_ctx, remote_addr, scheme, scope).await?;
    Ok(guard)
}

pub async fn validate_underlay_candidate(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
) -> Result<(), Error> {
    sanitize_underlay_candidate(global_ctx, remote_addr, scheme, scope).await
}

fn is_local_virtual_ipv4(global_ctx: &ArcGlobalCtx, ip: Ipv4Addr) -> bool {
    global_ctx
        .get_ipv4()
        .map(|inet| inet.address() == ip)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;
    use crate::common::global_ctx::{UnderlayBreakerScope, tests::get_mock_global_ctx};
    use crate::tunnel::IpScheme;

    #[test]
    fn parse_exclude_cidrs_accepts_cidrs_and_host_prefixes() {
        let parsed =
            parse_exclude_cidrs("198.18.0.0/15, 192.19.0.1/24, fc00::1/18, fdfe:dcba:9876::1/48")
                .unwrap();
        let rendered = parsed.iter().map(ToString::to_string).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "198.18.0.0/15",
                "192.19.0.0/24",
                "fc00::/18",
                "fdfe:dcba:9876::/48"
            ]
        );
    }

    #[test]
    fn parse_exclude_cidrs_rejects_invalid_items() {
        assert!(parse_exclude_cidrs("198.18.0.0/15,bad-cidr").is_err());
    }

    #[tokio::test]
    async fn should_block_configured_and_runtime_addresses_when_enabled() {
        let global_ctx = get_mock_global_ctx();
        global_ctx.set_ipv4(Some("10.44.0.9/16".parse().unwrap()));
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = true;
        flags.underlay_exclude_cidrs = DEFAULT_UNDERLAY_EXCLUDE_CIDRS.to_string();
        global_ctx.set_flags(flags);

        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V6("fdfe:dcba:9876::1".parse::<Ipv6Addr>().unwrap())
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V6("fc00::1".parse::<Ipv6Addr>().unwrap())
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(10, 44, 0, 9))
        ));
        assert!(!should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(192, 168, 2, 160))
        ));
    }

    #[tokio::test]
    async fn should_block_builtin_fake_ip_ranges_even_when_config_is_empty() {
        let global_ctx = get_mock_global_ctx();
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = true;
        flags.underlay_exclude_cidrs.clear();
        global_ctx.set_flags(flags);

        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(192, 19, 0, 1))
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V6("fdfe:dcba:9876::1".parse::<Ipv6Addr>().unwrap())
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V6("fc00::1".parse::<Ipv6Addr>().unwrap())
        ));
    }

    #[tokio::test]
    async fn configured_excludes_remain_additive_to_builtin_ranges() {
        let global_ctx = get_mock_global_ctx();
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = true;
        flags.underlay_exclude_cidrs = "203.0.113.0/24".to_string();
        global_ctx.set_flags(flags);

        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))
        ));
        assert!(should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))
        ));
    }

    #[tokio::test]
    async fn disabled_guard_keeps_new_filters_inactive() {
        let global_ctx = get_mock_global_ctx();
        global_ctx.set_ipv4(Some("10.44.0.9/16".parse().unwrap()));
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = false;
        flags.underlay_exclude_cidrs = DEFAULT_UNDERLAY_EXCLUDE_CIDRS.to_string();
        global_ctx.set_flags(flags);

        assert!(!should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))
        ));
        assert!(!should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(10, 44, 0, 9))
        ));
    }

    #[tokio::test]
    async fn validation_blocks_builtin_target_before_connect_probe() {
        let global_ctx = get_mock_global_ctx();
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = true;
        flags.underlay_exclude_cidrs.clear();
        global_ctx.set_flags(flags);

        let ret = validate_underlay_candidate(
            &global_ctx,
            "198.18.0.1:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        )
        .await;

        assert!(matches!(ret, Err(Error::InvalidUrl(_))));
    }

    #[tokio::test]
    async fn validation_guard_false_allows_builtin_target() {
        let global_ctx = get_mock_global_ctx();
        let mut flags = global_ctx.get_flags();
        flags.underlay_candidate_guard = false;
        flags.underlay_exclude_cidrs = DEFAULT_UNDERLAY_EXCLUDE_CIDRS.to_string();
        global_ctx.set_flags(flags);

        let ret = validate_underlay_candidate(
            &global_ctx,
            "198.18.0.1:11010".parse().unwrap(),
            IpScheme::Tcp,
            UnderlayBreakerScope::Generic,
        )
        .await;

        assert!(ret.is_ok());
    }
}
