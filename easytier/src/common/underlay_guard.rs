use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::Context;

use crate::common::{config::Flags, global_ctx::ArcGlobalCtx};

pub const DEFAULT_UNDERLAY_EXCLUDE_CIDRS: &str = "198.18.0.0/15,fdfe:dcba:9876::/48,192.19.0.0/24";

static PARSED_CIDR_CACHE: LazyLock<Mutex<Option<(String, Arc<Vec<cidr::IpCidr>>)>>> =
    LazyLock::new(|| Mutex::new(None));

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

pub fn is_runtime_guarded_ip(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_virtual_ipv4(global_ctx, v4),
        IpAddr::V6(v6) => global_ctx.is_ip_easytier_managed_ipv6(&v6),
    }
}

pub fn should_block_underlay_ip(global_ctx: &ArcGlobalCtx, ip: IpAddr) -> bool {
    let flags = global_ctx.get_flags();
    flags.underlay_candidate_guard
        && (is_runtime_guarded_ip(global_ctx, ip) || configured_excludes_match(&flags, ip))
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
    use crate::common::global_ctx::tests::get_mock_global_ctx;

    #[test]
    fn parse_exclude_cidrs_accepts_cidrs_and_host_prefixes() {
        let parsed =
            parse_exclude_cidrs("198.18.0.0/15, 192.19.0.1/24, fdfe:dcba:9876::1/48").unwrap();
        let rendered = parsed.iter().map(ToString::to_string).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["198.18.0.0/15", "192.19.0.0/24", "fdfe:dcba:9876::/48"]
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
            IpAddr::V4(Ipv4Addr::new(10, 44, 0, 9))
        ));
        assert!(!should_block_underlay_ip(
            &global_ctx,
            IpAddr::V4(Ipv4Addr::new(192, 168, 2, 160))
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
}
