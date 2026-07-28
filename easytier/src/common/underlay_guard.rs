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
        network::UnderlayInterfaceSnapshot,
    },
    tunnel::IpScheme,
};

pub const DEFAULT_UNDERLAY_EXCLUDE_CIDRS: &str =
    "198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,fd65:6173:7974::/48,192.19.0.0/24";
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

#[derive(Debug, PartialEq, Eq)]
enum PreflightUdpBindTarget {
    Wildcard,
    Resolved {
        interface_name: String,
        interface_index: std::num::NonZeroU32,
    },
}

fn select_preflight_udp_bind_target(
    bind_addr: SocketAddr,
    snapshot: Option<&UnderlayInterfaceSnapshot>,
) -> Result<PreflightUdpBindTarget, Error> {
    if bind_addr.ip().is_unspecified() {
        return Ok(PreflightUdpBindTarget::Wildcard);
    }

    let interface = snapshot
        .and_then(|snapshot| snapshot.interface_for(&bind_addr.ip()))
        .ok_or_else(|| {
            Error::InvalidUrl(format!(
                "underlay preflight source {bind_addr} has no interface identity"
            ))
        })?;
    let interface_index = std::num::NonZeroU32::new(interface.index).ok_or_else(|| {
        Error::InvalidUrl(format!(
            "underlay preflight source {bind_addr} has invalid interface index 0"
        ))
    })?;

    Ok(PreflightUdpBindTarget::Resolved {
        interface_name: interface.name.clone(),
        interface_index,
    })
}

fn bind_preflight_udp_source(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    bind_addr: SocketAddr,
    snapshot: Option<&UnderlayInterfaceSnapshot>,
) -> Result<tokio::net::UdpSocket, Error> {
    let socket = match select_preflight_udp_bind_target(bind_addr, snapshot)? {
        PreflightUdpBindTarget::Wildcard => crate::tunnel::common::bind::<tokio::net::UdpSocket>()
            .addr(bind_addr)
            .dev(crate::tunnel::common::BindDev::Disabled)
            .net_ns(global_ctx.net_ns.clone())
            .only_v6(remote_addr.is_ipv6())
            .maybe_socket_mark(global_ctx.get_flags().socket_mark)
            .call(),
        PreflightUdpBindTarget::Resolved {
            interface_name,
            interface_index,
        } => crate::tunnel::common::bind_resolved::<tokio::net::UdpSocket>(
            bind_addr,
            interface_name,
            Some(interface_index),
            Some(global_ctx.net_ns.clone()),
            remote_addr.is_ipv6(),
            global_ctx.get_flags().socket_mark,
        ),
    }
    .map_err(crate::tunnel::mark_local_bind_error)?;

    Ok(socket)
}

fn suspicious_interface_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utun")
        || name.starts_with("tun")
        || name.starts_with("tap")
        || name.contains("wintun")
}

fn native_interface_inspection_active() -> bool {
    !cfg!(any(
        target_os = "android",
        target_os = "ios",
        all(target_os = "macos", feature = "macos-ne"),
        target_env = "ohos"
    ))
}

fn source_interface_signal(
    snapshot: Option<&UnderlayInterfaceSnapshot>,
    ip: IpAddr,
) -> Option<(String, bool)> {
    // Mihomo/sing-tun invalidates its interface cache from the platform network
    // monitor instead of enumerating interfaces on every dial. Mobile VPN hosts
    // likewise own network changes and socket protection. Besides duplicating
    // that ownership, pnet enumeration is denied by Android SELinux and turns
    // every reconnect preflight into packet-socket and /proc/sysfs retries.
    // The hard managed-IP guards above remain active on every platform; this
    // optional interface-name signal only contributes a soft breaker strike.
    let interface = snapshot?.interface_for(&ip)?;
    let suspicious =
        interface.is_point_to_point || suspicious_interface_name(interface.name.as_str());
    Some((interface.name.clone(), suspicious))
}

fn bind_device_sources(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    snapshot: Option<&UnderlayInterfaceSnapshot>,
) -> Result<Vec<SocketAddr>, Error> {
    if !global_ctx.get_flags().bind_device || !native_interface_inspection_active() {
        return Ok(Vec::new());
    }

    let snapshot = snapshot.ok_or_else(|| {
        Error::InvalidUrl("underlay interface snapshot is unavailable".to_owned())
    })?;
    if snapshot.has_unmapped_fallback_for(remote_addr.is_ipv4()) {
        return Err(Error::InvalidUrl(format!(
            "underlay {} fallback has no interface identity",
            if remote_addr.is_ipv4() {
                "IPv4"
            } else {
                "IPv6"
            }
        )));
    }

    let ips = &snapshot.ip_list;
    if remote_addr.is_ipv4() {
        Ok(ips
            .interface_ipv4s
            .iter()
            .copied()
            .filter_map(|ip| {
                let ip = Ipv4Addr::from(ip);
                let ip_addr = IpAddr::V4(ip);
                (!should_block_underlay_ip(global_ctx, ip_addr))
                    .then_some(SocketAddrV4::new(ip, 0).into())
            })
            .collect())
    } else {
        Ok(ips
            .interface_ipv6s
            .iter()
            .copied()
            .filter_map(|ip| {
                let ip = std::net::Ipv6Addr::from(ip);
                let ip_addr = IpAddr::V6(ip);
                (!historical_guarded_ip(global_ctx, ip_addr)
                    && !should_block_underlay_ip(global_ctx, ip_addr))
                .then_some(SocketAddrV6::new(ip, 0, 0, 0).into())
            })
            .collect())
    }
}

async fn validate_connected_udp_source(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    bind_addr: SocketAddr,
    key: UnderlayBreakerKey,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
    snapshot: Option<&UnderlayInterfaceSnapshot>,
) -> Result<(), Error> {
    let socket = bind_preflight_udp_source(global_ctx, remote_addr, bind_addr, snapshot)?;
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

    match source_interface_signal(snapshot, local_ip) {
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

fn stale_interface_error(error: &Error) -> bool {
    let io_error = match error {
        Error::TunnelError(error) => crate::tunnel::local_bind_io_error(error),
        _ => return false,
    };
    io_error.is_some_and(crate::tunnel::is_stale_interface_io_error)
}

async fn run_bounded_stale_preflight_recovery<
    S,
    E,
    Validate,
    ValidateFuture,
    Refresh,
    RefreshFuture,
>(
    mut snapshot: S,
    mut validate: Validate,
    mut refresh: Refresh,
    mut should_refresh: impl FnMut(&E, &S) -> bool,
) -> Result<S, E>
where
    S: Clone,
    Validate: FnMut(S) -> ValidateFuture,
    ValidateFuture: std::future::Future<Output = Result<(), E>>,
    Refresh: FnMut(S) -> RefreshFuture,
    RefreshFuture: std::future::Future<Output = Result<S, E>>,
{
    for retry in 0..=1 {
        match validate(snapshot.clone()).await {
            Ok(()) => return Ok(snapshot),
            Err(error) if retry == 0 && should_refresh(&error, &snapshot) => {
                snapshot = refresh(snapshot).await?;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded stale preflight loop always returns")
}

async fn collect_attempt_snapshot(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
) -> Result<Option<Arc<UnderlayInterfaceSnapshot>>, Error> {
    if !native_interface_inspection_active()
        || (!global_ctx.get_flags().bind_device && !global_ctx.get_flags().underlay_candidate_guard)
    {
        return Ok(None);
    }

    let collector = global_ctx.get_ip_collector();
    let mut snapshot = collector.collect_underlay_snapshot().await?;
    if global_ctx.get_flags().bind_device
        && snapshot.has_unmapped_fallback_for(remote_addr.is_ipv4())
    {
        collector.invalidate_underlay_snapshot_generation(snapshot.generation);
        snapshot = collector.collect_underlay_snapshot().await?;
    }
    Ok(Some(snapshot))
}

async fn sanitize_underlay_candidate_with_snapshot(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
    snapshot: Option<&UnderlayInterfaceSnapshot>,
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

    let bind_sources = bind_device_sources(global_ctx, remote_addr, snapshot)?;
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
                snapshot,
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

    if global_ctx.get_flags().bind_device && native_interface_inspection_active() {
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
        snapshot,
    )
    .await
}

async fn sanitize_underlay_candidate_with_recovery(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
) -> Result<Option<Arc<UnderlayInterfaceSnapshot>>, Error> {
    let snapshot = collect_attempt_snapshot(global_ctx, remote_addr).await?;
    run_bounded_stale_preflight_recovery(
        snapshot,
        |snapshot| async move {
            sanitize_underlay_candidate_with_snapshot(
                global_ctx,
                remote_addr,
                scheme,
                scope,
                snapshot.as_deref(),
            )
            .await
        },
        |snapshot| async move {
            let stale_snapshot = snapshot
                .as_ref()
                .expect("stale preflight refresh requires an owned snapshot");
            global_ctx
                .get_ip_collector()
                .invalidate_underlay_snapshot_generation(stale_snapshot.generation);
            collect_attempt_snapshot(global_ctx, remote_addr).await
        },
        |error, snapshot| stale_interface_error(error) && snapshot.is_some(),
    )
    .await
}

pub async fn sanitize_underlay_candidate(
    global_ctx: &ArcGlobalCtx,
    remote_addr: SocketAddr,
    scheme: IpScheme,
    scope: UnderlayBreakerScope,
) -> Result<(), Error> {
    sanitize_underlay_candidate_with_recovery(global_ctx, remote_addr, scheme, scope)
        .await
        .map(|_| ())
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

    let mut guard = global_ctx
        .try_begin_underlay_attempt(&keys)
        .map_err(|error| {
            Error::InvalidUrl(format!(
                "underlay candidate {remote_addr} is temporarily gated: {error}"
            ))
        })?;
    let snapshot =
        sanitize_underlay_candidate_with_recovery(global_ctx, remote_addr, scheme, scope).await?;
    guard.set_underlay_snapshot(snapshot);
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
    use std::{
        io,
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;
    use crate::tunnel::{TunnelError, mark_local_bind_error};

    #[test]
    fn native_interface_inspection_matches_platform_ownership() {
        assert_eq!(
            native_interface_inspection_active(),
            !cfg!(any(
                target_os = "android",
                target_os = "ios",
                all(target_os = "macos", feature = "macos-ne"),
                target_env = "ohos"
            ))
        );
    }
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

    fn stale_local_bind_error() -> Error {
        Error::TunnelError(mark_local_bind_error(TunnelError::IOError(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "synthetic stale local bind",
        ))))
    }

    #[tokio::test]
    async fn stale_preflight_refreshes_and_revalidates_at_most_once() {
        let validation_calls = Arc::new(AtomicUsize::new(0));
        let refresh_calls = Arc::new(AtomicUsize::new(0));
        let result = run_bounded_stale_preflight_recovery(
            Some(1_u64),
            {
                let validation_calls = validation_calls.clone();
                move |snapshot| {
                    let validation_calls = validation_calls.clone();
                    async move {
                        let call = validation_calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(snapshot, Some(if call == 0 { 1 } else { 2 }));
                        Err(stale_local_bind_error())
                    }
                }
            },
            {
                let refresh_calls = refresh_calls.clone();
                move |snapshot| {
                    let refresh_calls = refresh_calls.clone();
                    async move {
                        assert_eq!(snapshot, Some(1));
                        refresh_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(Some(2))
                    }
                }
            },
            |error, snapshot| stale_interface_error(error) && snapshot.is_some(),
        )
        .await;

        assert!(stale_interface_error(&result.unwrap_err()));
        assert_eq!(validation_calls.load(Ordering::SeqCst), 2);
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);

        let ordinary_validation_calls = Arc::new(AtomicUsize::new(0));
        let ordinary_refresh_calls = Arc::new(AtomicUsize::new(0));
        let ordinary_result = run_bounded_stale_preflight_recovery(
            Some(1_u64),
            {
                let validation_calls = ordinary_validation_calls.clone();
                move |_| {
                    let validation_calls = validation_calls.clone();
                    async move {
                        validation_calls.fetch_add(1, Ordering::SeqCst);
                        Err(Error::TunnelError(TunnelError::IOError(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            "ordinary connect error",
                        ))))
                    }
                }
            },
            {
                let refresh_calls = ordinary_refresh_calls.clone();
                move |_| {
                    let refresh_calls = refresh_calls.clone();
                    async move {
                        refresh_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(Some(2))
                    }
                }
            },
            |error, snapshot| stale_interface_error(error) && snapshot.is_some(),
        )
        .await;

        assert!(!stale_interface_error(&ordinary_result.unwrap_err()));
        assert_eq!(ordinary_validation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(ordinary_refresh_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn preflight_bind_target_uses_snapshot_identity_or_explicit_wildcard() {
        let interface = pnet::datalink::NetworkInterface {
            name: "en-test0".to_owned(),
            description: String::new(),
            index: 4,
            mac: None,
            ips: vec![
                "192.0.2.10/24".parse().unwrap(),
                "2001:db8::10/64".parse().unwrap(),
            ],
            flags: 0,
        };
        let snapshot = crate::common::network::IPCollector::build_underlay_snapshot(
            &[interface.clone()],
            &[interface],
            None,
            None,
        );

        assert_eq!(
            select_preflight_udp_bind_target("192.0.2.10:0".parse().unwrap(), Some(&snapshot))
                .unwrap(),
            PreflightUdpBindTarget::Resolved {
                interface_name: "en-test0".to_owned(),
                interface_index: std::num::NonZeroU32::new(4).unwrap(),
            }
        );
        assert_eq!(
            select_preflight_udp_bind_target("[2001:db8::10]:0".parse().unwrap(), Some(&snapshot))
                .unwrap(),
            PreflightUdpBindTarget::Resolved {
                interface_name: "en-test0".to_owned(),
                interface_index: std::num::NonZeroU32::new(4).unwrap(),
            }
        );
        assert_eq!(
            select_preflight_udp_bind_target("0.0.0.0:0".parse().unwrap(), None).unwrap(),
            PreflightUdpBindTarget::Wildcard
        );
        assert_eq!(
            select_preflight_udp_bind_target("[::]:0".parse().unwrap(), None).unwrap(),
            PreflightUdpBindTarget::Wildcard
        );
        assert!(
            select_preflight_udp_bind_target("198.51.100.10:0".parse().unwrap(), Some(&snapshot))
                .is_err()
        );

        let zero_index_interface = pnet::datalink::NetworkInterface {
            name: "en-zero".to_owned(),
            description: String::new(),
            index: 0,
            mac: None,
            ips: vec!["198.51.100.10/24".parse().unwrap()],
            flags: 0,
        };
        let zero_index_snapshot = crate::common::network::IPCollector::build_underlay_snapshot(
            &[zero_index_interface.clone()],
            &[zero_index_interface],
            None,
            None,
        );
        assert!(
            select_preflight_udp_bind_target(
                "198.51.100.10:0".parse().unwrap(),
                Some(&zero_index_snapshot)
            )
            .is_err()
        );
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
            IpAddr::V6("fd65:6173:7974::4".parse::<Ipv6Addr>().unwrap())
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
