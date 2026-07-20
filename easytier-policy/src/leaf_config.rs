use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    path::Path,
};

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ChainKind, PolicyRevision, ProxyKind, ProxyServer, RuleSetKind,
    config::{RuleSet, verify_rule_set_file},
    geodata::{lan_cidrs, load_geoip_categories},
};

const MAX_COMPILED_GEOIP_CIDRS: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMeshServer {
    pub endpoint: SocketAddr,
    pub username: String,
    pub password: String,
}

pub trait MeshServerResolver: Send + Sync {
    fn resolve(
        &self,
        proxy_name: &str,
        instance_id: Option<Uuid>,
        virtual_ip: Option<std::net::IpAddr>,
        port: Option<u16>,
    ) -> Option<ResolvedMeshServer>;
}

impl<F> MeshServerResolver for F
where
    F: Fn(&str, Option<Uuid>, Option<std::net::IpAddr>, Option<u16>) -> Option<ResolvedMeshServer>
        + Send
        + Sync,
{
    fn resolve(
        &self,
        proxy_name: &str,
        instance_id: Option<Uuid>,
        virtual_ip: Option<std::net::IpAddr>,
        port: Option<u16>,
    ) -> Option<ResolvedMeshServer> {
        self(proxy_name, instance_id, virtual_ip, port)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LeafConfigError {
    #[error("mesh proxy {0} cannot be resolved")]
    UnresolvedMeshProxy(String),
    #[error("rule {index} references {kind} data but no matching rule-set is configured")]
    MissingRuleSet { index: usize, kind: &'static str },
    #[error("rule-set path contains a delimiter unsupported by Leaf: {0}")]
    InvalidRuleSetPath(String),
    #[error("mesh bridge credentials for {0} contain unsupported characters")]
    InvalidBridgeCredentials(String),
    #[error("no safe system DNS server is available")]
    NoDnsServers,
    #[error("failed to serialize Leaf JSON config: {0}")]
    Serialize(String),
    #[error("failed to determine actor capability for rule {index}: {reason}")]
    ActorCapability { index: usize, reason: String },
    #[error("failed to load GeoIP rule data: {0}")]
    GeoipData(String),
    #[error("rule-set {name} failed compile-time integrity check: {reason}")]
    RuleSetIntegrity { name: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafOwnedTunConfig {
    pub name: String,
    pub address: String,
    pub gateway: String,
    pub netmask: String,
    pub mtu: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeafConfigOptions {
    pub fake_dns_ipv6: bool,
    pub leaf_owned_tun: Option<LeafOwnedTunConfig>,
}

pub fn compile_leaf_config(
    revision: &PolicyRevision,
    tun_fd: i32,
    base_dir: &Path,
    resolver: &dyn MeshServerResolver,
    dns_servers: &[IpAddr],
) -> Result<String, LeafConfigError> {
    compile_leaf_config_with_options(
        revision,
        tun_fd,
        base_dir,
        resolver,
        dns_servers,
        LeafConfigOptions::default(),
    )
}

pub fn compile_leaf_config_with_options(
    revision: &PolicyRevision,
    tun_fd: i32,
    base_dir: &Path,
    resolver: &dyn MeshServerResolver,
    dns_servers: &[IpAddr],
    options: LeafConfigOptions,
) -> Result<String, LeafConfigError> {
    if dns_servers.is_empty() {
        return Err(LeafConfigError::NoDnsServers);
    }
    verify_revision_rule_sets(revision, base_dir)?;
    let document = &revision.document;
    let dns_servers = compile_dns_servers(document, dns_servers);
    let mut outbounds = vec![
        serde_json::json!({ "tag": "DIRECT", "protocol": "direct" }),
        serde_json::json!({ "tag": "REJECT", "protocol": "drop" }),
    ];
    for (name, proxy) in &document.proxies {
        let (address, port, credentials) = match &proxy.server {
            ProxyServer::Address(address) => (
                address.clone(),
                proxy
                    .port
                    .expect("validated native proxy has an explicit port"),
                proxy
                    .credentials()
                    .map(|(username, password)| (username.to_owned(), password.to_owned())),
            ),
            ProxyServer::Mesh {
                instance_id,
                virtual_ip,
            } => {
                let address = resolver
                    .resolve(name, *instance_id, *virtual_ip, proxy.port)
                    .ok_or_else(|| LeafConfigError::UnresolvedMeshProxy(name.clone()))?;
                if !valid_bridge_credential(&address.username)
                    || !valid_bridge_credential(&address.password)
                {
                    return Err(LeafConfigError::InvalidBridgeCredentials(name.clone()));
                }
                (
                    address.endpoint.ip().to_string(),
                    address.endpoint.port(),
                    Some((address.username, address.password)),
                )
            }
        };
        let mut settings = serde_json::Map::new();
        settings.insert("address".to_owned(), address.into());
        settings.insert("port".to_owned(), port.into());
        if let Some((username, password)) = credentials {
            settings.insert("username".to_owned(), username.into());
            settings.insert("password".to_owned(), password.into());
        }
        match proxy.kind {
            ProxyKind::Socks5 => outbounds.push(serde_json::json!({
                "tag": name,
                "protocol": "socks",
                "settings": settings,
            })),
            ProxyKind::Shadowsocks => outbounds.push(crate::shadowsocks::compile_outbound(
                name,
                settings["address"]
                    .as_str()
                    .expect("validated Shadowsocks address"),
                settings["port"]
                    .as_u64()
                    .expect("validated Shadowsocks port") as u16,
                proxy,
            )),
            ProxyKind::Trojan => outbounds.extend(crate::trojan::compile_outbounds(
                name,
                settings["address"]
                    .as_str()
                    .expect("validated Trojan address"),
                settings["port"].as_u64().expect("validated Trojan port") as u16,
                proxy,
            )),
            ProxyKind::Vmess => outbounds.extend(crate::vmess::compile_outbounds(
                name,
                settings["address"]
                    .as_str()
                    .expect("validated VMess address"),
                settings["port"].as_u64().expect("validated VMess port") as u16,
                proxy,
            )),
            ProxyKind::Vless => outbounds.extend(crate::vless::compile_outbounds(
                name,
                settings["address"]
                    .as_str()
                    .expect("validated VLESS address"),
                settings["port"].as_u64().expect("validated VLESS port") as u16,
                proxy,
            )),
            ProxyKind::Http => unreachable!("policy validation rejects HTTP outbound in v1"),
        }
    }
    for name in revision.group_order.iter() {
        let group = &document.groups[name];
        let (protocol, settings) = match group.kind {
            ChainKind::Chain => ("chain", serde_json::json!({ "actors": group.members })),
            ChainKind::Fallback => (
                "failover",
                // Mihomo adapter/outboundgroup/fallback.go selects one actor before dialing;
                // user traffic never retries group members. EasyTier intentionally differs from
                // Mihomo's periodic scheduler by enabling Leaf's traffic-triggered URL checker.
                serde_json::json!({
                    "actors": group.members,
                    "healthCheck": false,
                    "failover": true,
                    "stableFailover": true,
                    "healthCheckUrl": group.url.as_deref().unwrap_or("https://www.gstatic.com/generate_204"),
                    "healthCheckTimeout": 5,
                }),
            ),
        };
        outbounds.push(serde_json::json!({
            "tag": name,
            "protocol": protocol,
            "settings": settings,
        }));
    }

    let tun_settings = if let Some(tun) = options.leaf_owned_tun.as_ref() {
        serde_json::json!({
            "fd": -1,
            // EasyTier, not Leaf, remains the routing owner. `auto=false` is
            // therefore part of the compatibility contract for this fast path.
            "auto": false,
            "name": tun.name,
            "address": tun.address,
            "gateway": tun.gateway,
            "netmask": tun.netmask,
            "mtu": tun.mtu,
            "fakeDnsInclude": ["*"],
            "fakeDnsRange": document.dns.fake_ip_range,
            "fakeDnsIpv6": options.fake_dns_ipv6,
            "fakeDnsIpv6Range": document.dns.fake_ip_range6,
            "tun2socks": "smoltcp",
        })
    } else {
        serde_json::json!({
            "fd": tun_fd,
            "fakeDnsInclude": ["*"],
            "fakeDnsRange": document.dns.fake_ip_range,
            "fakeDnsIpv6": options.fake_dns_ipv6,
            "fakeDnsIpv6Range": document.dns.fake_ip_range6,
            "tun2socks": "smoltcp",
        })
    };
    let config = serde_json::json!({
        "log": { "level": "warn" },
        "dns": {
            "servers": dns_servers,
        },
        "inbounds": [{
            "tag": "tun",
            "protocol": "tun",
            "settings": tun_settings,
        }],
        "outbounds": outbounds,
        "router": {
            "domainResolve": false,
            "rules": compile_leaf_rules(revision, base_dir)?,
        },
    });
    serde_json::to_string_pretty(&config)
        .map_err(|error| LeafConfigError::Serialize(error.to_string()))
}

fn compile_dns_servers(
    document: &crate::PolicyDocument,
    platform_servers: &[IpAddr],
) -> Vec<String> {
    let direct = if document.dns.direct.is_empty() {
        platform_servers
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        document.dns.direct.clone()
    };
    let mut seen = BTreeSet::new();
    let mut servers = Vec::new();
    {
        let mut push_direct = |server: String| {
            let server = format!("direct:{server}");
            if seen.insert(server.clone()) {
                servers.push(server);
            }
        };
        for server in direct {
            if server.eq_ignore_ascii_case("system") {
                // Proxy endpoints must be resolved outside the policy TUN. Expanding
                // `system` to the DNS addresses captured before TUN ownership avoids
                // feeding a proxy hostname back into Leaf FakeDNS. This follows Mihomo
                // hub/executor/executor.go::updateDNS, which gives proxy endpoints a
                // dedicated ProxyServerHostResolver instead of its FakeIP service.
                for platform_server in platform_servers {
                    push_direct(platform_server.to_string());
                }
            } else {
                push_direct(server);
            }
        }
    }
    for server in &document.dns.proxy {
        if seen.insert(server.clone()) {
            servers.push(server.clone());
        }
    }
    servers
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompiledLeafRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain_keyword: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain_suffix: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port_range: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inbound_tag: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolve_domain: Option<bool>,
    target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleMergeFamily {
    Domain,
    Ip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuleMergeKey {
    family: RuleMergeFamily,
    no_resolve: bool,
}

fn compile_leaf_rules(
    revision: &PolicyRevision,
    base_dir: &Path,
) -> Result<Vec<CompiledLeafRule>, LeafConfigError> {
    let document = &revision.document;
    let geoip_rule_set = find_single_rule_set(document.rule_sets.values(), RuleSetKind::Geoip);
    let requested_geoip_categories = document
        .rules
        .iter()
        .filter_map(|source| {
            let parts = source.split(',').map(str::trim).collect::<Vec<_>>();
            match parts.first()?.to_ascii_uppercase().as_str() {
                "GEOIP" => Some(parts.get(1)?.to_ascii_uppercase()),
                "EXTERNAL" => {
                    let (kind, code) = parts.get(1)?.split_once(':')?;
                    matches!(kind.to_ascii_lowercase().as_str(), "geoip" | "geoip-dat")
                        .then(|| code.to_ascii_uppercase())
                }
                _ => None,
            }
        })
        .filter(|code| code != "LAN")
        .collect::<BTreeSet<_>>();
    let geoip_categories = if let Some(rule_set) = geoip_rule_set
        && !requested_geoip_categories.is_empty()
    {
        load_geoip_categories(
            &resolved_rule_set_path(rule_set, base_dir),
            &requested_geoip_categories,
        )
        .map_err(|error| LeafConfigError::GeoipData(error.to_string()))?
    } else {
        BTreeMap::new()
    };
    let mut compiled = Vec::with_capacity(document.rules.len());
    let mut last_merge_key = None;
    let mut compiled_geoip_cidrs = 0usize;
    for (index, source) in document.rules.iter().enumerate() {
        let parts: Vec<&str> = source.split(',').map(str::trim).collect();
        let rule_type = parts[0].to_ascii_uppercase();
        let has_no_resolve = parts
            .last()
            .is_some_and(|part| part.eq_ignore_ascii_case("no-resolve"));
        let target = parts[parts.len() - 1 - usize::from(has_no_resolve)].to_owned();
        let mut rule = empty_leaf_rule(target.clone());
        let mut merge_family = None;
        let mut resolve_domain = false;

        match rule_type.as_str() {
            "IP-CIDR" => {
                rule.ip = Some(vec![parts[1].to_owned()]);
                merge_family = Some(RuleMergeFamily::Ip);
                resolve_domain = !has_no_resolve;
            }
            "DOMAIN" => {
                rule.domain = Some(vec![parts[1].to_owned()]);
                merge_family = Some(RuleMergeFamily::Domain);
            }
            "DOMAIN-SUFFIX" => {
                rule.domain_suffix = Some(vec![parts[1].to_owned()]);
                merge_family = Some(RuleMergeFamily::Domain);
            }
            "DOMAIN-KEYWORD" => {
                rule.domain_keyword = Some(vec![parts[1].to_owned()]);
                merge_family = Some(RuleMergeFamily::Domain);
            }
            "GEOIP" => {
                compiled_geoip_cidrs = reserve_geoip_cidrs(
                    compiled_geoip_cidrs,
                    apply_geoip_rule(&mut rule, parts[1], &geoip_categories)?,
                )?;
                merge_family = Some(RuleMergeFamily::Ip);
                resolve_domain = !has_no_resolve;
            }
            "COUNTRY" => {
                let rule_set = find_single_rule_set(document.rule_sets.values(), RuleSetKind::Mmdb)
                    .ok_or(LeafConfigError::MissingRuleSet {
                        index,
                        kind: "mmdb",
                    })?;
                rule.external = Some(vec![external_rule("mmdb", rule_set, parts[1], base_dir)?]);
                resolve_domain = !has_no_resolve;
            }
            "GEOSITE" => {
                let rule_set =
                    find_single_rule_set(document.rule_sets.values(), RuleSetKind::Geosite).ok_or(
                        LeafConfigError::MissingRuleSet {
                            index,
                            kind: "geosite",
                        },
                    )?;
                rule.external = Some(vec![external_rule("site", rule_set, parts[1], base_dir)?]);
                merge_family = Some(RuleMergeFamily::Domain);
            }
            "EXTERNAL" => {
                let (kind, code) = parts[1].split_once(':').unwrap_or(("site", parts[1]));
                match kind.to_ascii_lowercase().as_str() {
                    "site" | "geosite" => {
                        let rule_set =
                            find_single_rule_set(document.rule_sets.values(), RuleSetKind::Geosite)
                                .ok_or(LeafConfigError::MissingRuleSet {
                                    index,
                                    kind: "geosite",
                                })?;
                        rule.external =
                            Some(vec![external_rule("site", rule_set, code, base_dir)?]);
                        merge_family = Some(RuleMergeFamily::Domain);
                    }
                    "mmdb" => {
                        let rule_set =
                            find_single_rule_set(document.rule_sets.values(), RuleSetKind::Mmdb)
                                .ok_or(LeafConfigError::MissingRuleSet {
                                    index,
                                    kind: "mmdb",
                                })?;
                        rule.external =
                            Some(vec![external_rule("mmdb", rule_set, code, base_dir)?]);
                        resolve_domain = !has_no_resolve;
                    }
                    "geoip" | "geoip-dat" => {
                        compiled_geoip_cidrs = reserve_geoip_cidrs(
                            compiled_geoip_cidrs,
                            apply_geoip_rule(&mut rule, code, &geoip_categories)?,
                        )?;
                        merge_family = Some(RuleMergeFamily::Ip);
                        resolve_domain = !has_no_resolve;
                    }
                    _ => {
                        return Err(LeafConfigError::MissingRuleSet {
                            index,
                            kind: "recognized external",
                        });
                    }
                }
            }
            "PORT-RANGE" => {
                rule.port_range =
                    Some(vec![crate::config::normalize_port_range(parts[1]).expect(
                        "validated PORT-RANGE has a Leaf-compatible representation",
                    )])
            }
            "NETWORK" => rule.network = Some(vec![parts[1].to_ascii_lowercase()]),
            "INBOUND-TAG" => rule.inbound_tag = Some(vec![parts[1].to_owned()]),
            // A network matcher over both supported session kinds is Leaf's non-special-cased,
            // order-preserving representation of an unconditional MATCH/FINAL rule.
            "MATCH" | "FINAL" => {
                rule.network = Some(vec!["tcp".to_owned(), "udp".to_owned()]);
            }
            _ => unreachable!("policy validation rejects unsupported rule types"),
        }
        rule.resolve_domain = resolve_domain.then_some(true);

        let supports_udp = document
            .actor_supports_udp(&target, &mut BTreeSet::new())
            .map_err(|error| LeafConfigError::ActorCapability {
                index,
                reason: error.to_string(),
            })?;
        if !supports_udp {
            match rule.network.as_mut() {
                Some(networks) if networks.iter().all(|network| network == "udp") => {
                    // Mihomo would match this rule, skip the unsupported actor, and continue. An
                    // impossible Leaf rule has the same result, so omit it from the runtime list.
                    continue;
                }
                Some(networks) => networks.retain(|network| network == "tcp"),
                None => rule.network = Some(vec!["tcp".to_owned()]),
            }
        }
        push_compiled_rule(
            &mut compiled,
            &mut last_merge_key,
            rule,
            merge_family.map(|family| RuleMergeKey {
                family,
                no_resolve: has_no_resolve,
            }),
        );
    }
    Ok(compiled)
}

fn push_compiled_rule(
    compiled: &mut Vec<CompiledLeafRule>,
    last_merge_key: &mut Option<RuleMergeKey>,
    rule: CompiledLeafRule,
    merge_key: Option<RuleMergeKey>,
) {
    if let Some(merge_key) = merge_key
        && *last_merge_key == Some(merge_key)
        && let Some(previous) = compiled.last_mut()
        && previous.target == rule.target
        && previous.network == rule.network
        && previous.resolve_domain == rule.resolve_domain
        && can_merge_rule_values(previous, &rule, merge_key)
    {
        merge_rule_values(previous, rule, merge_key);
        return;
    }

    compiled.push(rule);
    *last_merge_key = merge_key;
}

fn can_merge_rule_values(
    previous: &CompiledLeafRule,
    next: &CompiledLeafRule,
    key: RuleMergeKey,
) -> bool {
    if previous.port_range.is_some()
        || previous.inbound_tag.is_some()
        || next.port_range.is_some()
        || next.inbound_tag.is_some()
    {
        return false;
    }

    match key.family {
        RuleMergeFamily::Domain => previous.ip.is_none() && next.ip.is_none(),
        RuleMergeFamily::Ip => {
            previous.domain.is_none()
                && previous.domain_keyword.is_none()
                && previous.domain_suffix.is_none()
                && previous.external.is_none()
                && next.domain.is_none()
                && next.domain_keyword.is_none()
                && next.domain_suffix.is_none()
                && next.external.is_none()
        }
    }
}

fn merge_rule_values(
    previous: &mut CompiledLeafRule,
    mut next: CompiledLeafRule,
    key: RuleMergeKey,
) {
    match key.family {
        RuleMergeFamily::Domain => {
            append_rule_values(&mut previous.domain, next.domain.take());
            append_rule_values(&mut previous.domain_keyword, next.domain_keyword.take());
            append_rule_values(&mut previous.domain_suffix, next.domain_suffix.take());
            append_rule_values(&mut previous.external, next.external.take());
        }
        RuleMergeFamily::Ip => append_rule_values(&mut previous.ip, next.ip.take()),
    }
}

fn append_rule_values(destination: &mut Option<Vec<String>>, source: Option<Vec<String>>) {
    let Some(mut source) = source else {
        return;
    };
    destination.get_or_insert_with(Vec::new).append(&mut source);
}

fn apply_geoip_rule(
    rule: &mut CompiledLeafRule,
    code: &str,
    geoip_categories: &BTreeMap<String, Vec<String>>,
) -> Result<usize, LeafConfigError> {
    if code.eq_ignore_ascii_case("lan") {
        rule.ip = Some(lan_cidrs());
    } else {
        let code = code.to_ascii_uppercase();
        rule.ip =
            Some(geoip_categories.get(&code).cloned().ok_or_else(|| {
                LeafConfigError::GeoipData(format!("category {code} is missing"))
            })?);
    }
    Ok(rule.ip.as_ref().map_or(0, Vec::len))
}

fn reserve_geoip_cidrs(current: usize, additional: usize) -> Result<usize, LeafConfigError> {
    let total = current.saturating_add(additional);
    if total > MAX_COMPILED_GEOIP_CIDRS {
        return Err(LeafConfigError::GeoipData(format!(
            "compiled rules exceed the {MAX_COMPILED_GEOIP_CIDRS} CIDR limit"
        )));
    }
    Ok(total)
}

fn empty_leaf_rule(target: String) -> CompiledLeafRule {
    CompiledLeafRule {
        ip: None,
        domain: None,
        domain_keyword: None,
        domain_suffix: None,
        external: None,
        port_range: None,
        network: None,
        inbound_tag: None,
        resolve_domain: None,
        target,
    }
}

fn valid_bridge_credential(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn find_single_rule_set<'a>(
    rule_sets: impl Iterator<Item = &'a RuleSet>,
    kind: RuleSetKind,
) -> Option<&'a RuleSet> {
    let mut matching = rule_sets.filter(|rule_set| rule_set.kind == kind);
    let first = matching.next()?;
    matching.next().is_none().then_some(first)
}

fn external_rule(
    kind: &str,
    rule_set: &RuleSet,
    code: &str,
    base_dir: &Path,
) -> Result<String, LeafConfigError> {
    let path = if rule_set.path.is_absolute() {
        rule_set.path.clone()
    } else {
        base_dir.join(&rule_set.path)
    };
    let path = path.to_string_lossy();
    if path
        .chars()
        .any(|character| matches!(character, ':' | ',' | '\r' | '\n' | '=' | '#' | ';'))
    {
        return Err(LeafConfigError::InvalidRuleSetPath(path.into_owned()));
    }
    Ok(format!("{kind}:{path}:{code}"))
}

fn resolved_rule_set_path(rule_set: &RuleSet, base_dir: &Path) -> std::path::PathBuf {
    if rule_set.path.is_absolute() {
        rule_set.path.clone()
    } else {
        base_dir.join(&rule_set.path)
    }
}

fn verify_revision_rule_sets(
    revision: &PolicyRevision,
    base_dir: &Path,
) -> Result<(), LeafConfigError> {
    for (name, rule_set) in &revision.document.rule_sets {
        let path = resolved_rule_set_path(rule_set, base_dir);
        verify_rule_set_file(&path, rule_set.sha256.as_deref()).map_err(|reason| {
            LeafConfigError::RuleSetIntegrity {
                name: name.clone(),
                reason,
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::PolicyRevision;
    use sha2::{Digest, Sha256};

    use super::*;

    struct Unresolved;

    impl MeshServerResolver for Unresolved {
        fn resolve(
            &self,
            _proxy_name: &str,
            _instance_id: Option<Uuid>,
            _virtual_ip: Option<IpAddr>,
            _port: Option<u16>,
        ) -> Option<ResolvedMeshServer> {
            None
        }
    }

    struct LoopbackMesh;

    impl MeshServerResolver for LoopbackMesh {
        fn resolve(
            &self,
            proxy_name: &str,
            _instance_id: Option<Uuid>,
            _virtual_ip: Option<IpAddr>,
            port: Option<u16>,
        ) -> Option<ResolvedMeshServer> {
            assert_eq!(proxy_name, "mesh");
            assert_eq!(port, Some(1080));
            Some(ResolvedMeshServer {
                endpoint: "127.0.0.1:32100".parse().unwrap(),
                username: "easytier".to_owned(),
                password: "secret".to_owned(),
            })
        }
    }

    fn compiled_rules(source: &str) -> Vec<serde_json::Value> {
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        serde_json::from_str::<serde_json::Value>(&config).unwrap()["router"]["rules"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn leaf_owned_tun_is_explicit_and_legacy_fd_mode_remains_unchanged() {
        let revision =
            PolicyRevision::parse("version: 1\nrules: [\"FINAL,DIRECT\"]\n", Path::new("."))
                .unwrap();
        let legacy = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let legacy: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(legacy["inbounds"][0]["settings"]["fd"], 7);
        assert!(legacy["inbounds"][0]["settings"].get("auto").is_none());
        assert!(legacy["inbounds"][0]["settings"].get("name").is_none());

        let owned = compile_leaf_config_with_options(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
            LeafConfigOptions {
                leaf_owned_tun: Some(LeafOwnedTunConfig {
                    name: "etp00010001".to_owned(),
                    address: "198.18.0.6".to_owned(),
                    gateway: "198.18.0.5".to_owned(),
                    netmask: "255.255.255.252".to_owned(),
                    mtu: 1_500,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let owned: serde_json::Value = serde_json::from_str(&owned).unwrap();
        let settings = &owned["inbounds"][0]["settings"];
        assert_eq!(settings["fd"], -1);
        assert_eq!(settings["auto"], false);
        assert_eq!(settings["name"], "etp00010001");
        assert_eq!(settings["address"], "198.18.0.6");
        assert_eq!(settings["gateway"], "198.18.0.5");
        assert_eq!(settings["netmask"], "255.255.255.252");
        assert_eq!(settings["mtu"], 1_500);
    }

    #[test]
    fn compiles_trojan_vmess_and_vless_as_private_transport_chains() {
        let source = r#"
version: 1
proxies:
  trojan:
    type: trojan
    server: edge.example
    port: 443
    password: secret
    tls: { server-name: trojan.example }
  vmess:
    type: vmess
    server: edge.example
    port: 80
    uuid: 00000000-0000-0000-0000-000000000001
    alter-id: 0
    cipher: auto
    transport: { type: websocket, path: /vmess, headers: { Host: vmess.example } }
  vless:
    type: vless
    server: edge.example
    port: 443
    uuid: 00000000-0000-0000-0000-000000000002
    transport: { type: websocket, path: /vless }
    tls: { server-name: vless.example }
rules: ["MATCH,trojan"]
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        for name in ["trojan", "vmess", "vless"] {
            let public = outbounds
                .iter()
                .find(|outbound| outbound["tag"] == name)
                .unwrap();
            assert_eq!(public["protocol"], "chain");
        }
        assert!(outbounds.iter().any(|outbound| {
            outbound["tag"] == "@et:vmess:protocol"
                && outbound["settings"]["security"]
                    == if cfg!(any(
                        target_arch = "x86_64",
                        target_arch = "aarch64",
                        target_arch = "s390x"
                    )) {
                        "aes-128-gcm"
                    } else {
                        "chacha20-poly1305"
                    }
        }));
        let vless_tls = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "@et:vless:tls")
            .unwrap();
        assert_eq!(vless_tls["settings"]["serverName"], "vless.example");
        assert_eq!(
            vless_tls["settings"]["alpn"],
            serde_json::json!(["http/1.1"])
        );
        let trojan_tls = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "@et:trojan:tls")
            .unwrap();
        assert!(trojan_tls["settings"].get("alpn").is_none());
    }

    #[test]
    fn compiles_stable_yaml_to_strict_leaf_config() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("site.dat"), b"test").unwrap();
        fs::write(dir.path().join("geo.mmdb"), b"test").unwrap();
        let source = r#"
version: 1
rule-sets:
  site: { type: geosite, path: site.dat }
  geo: { type: mmdb, path: geo.mmdb }
proxies:
  native:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: true
    username: alice
    password: secret
groups:
  final:
    type: fallback
    members: [native, DIRECT]
    url: https://probe.example/ready
rules:
  - GEOSITE,cn,DIRECT
  - COUNTRY,US,final,no-resolve
  - MATCH,final
"#;
        let revision = PolicyRevision::parse(source, dir.path()).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            dir.path(),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        #[cfg(feature = "leaf-runtime")]
        leaf::config::from_string(&config).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["inbounds"][0]["protocol"], "tun");
        assert_eq!(config["inbounds"][0]["settings"]["fd"], 7);
        assert_eq!(
            config["dns"]["servers"],
            serde_json::json!([
                "direct:1.1.1.1",
                "direct:223.5.5.5",
                "direct:119.29.29.29",
                "direct:114.114.114.114",
                "doh:cloudflare-dns.com@1.1.1.1",
                "doh:dns.google@8.8.8.8",
                "doh:dns.quad9.net@9.9.9.9"
            ])
        );
        assert_eq!(config["outbounds"][2]["tag"], "native");
        assert_eq!(config["outbounds"][2]["protocol"], "socks");
        assert_eq!(config["outbounds"][3]["tag"], "final");
        assert_eq!(config["outbounds"][3]["protocol"], "failover");
        assert_eq!(config["outbounds"][3]["settings"]["stableFailover"], true);
        assert_eq!(config["outbounds"][3]["settings"]["healthCheck"], false);
        assert_eq!(
            config["outbounds"][3]["settings"]["healthCheckUrl"],
            "https://probe.example/ready"
        );
        assert_eq!(config["outbounds"][3]["settings"]["healthCheckTimeout"], 5);
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0]["target"], "DIRECT");
        assert_eq!(
            rules[0]["external"][0],
            format!("site:{}/site.dat:cn", dir.path().display())
        );
        assert_eq!(rules[1]["target"], "final");
        assert_eq!(
            rules[1]["external"][0],
            format!("mmdb:{}/geo.mmdb:US", dir.path().display())
        );
        assert_eq!(rules[2]["target"], "final");
        assert_eq!(rules[2]["network"], serde_json::json!(["tcp", "udp"]));
    }

    #[test]
    fn compiles_shadowsocks_native_udp_and_uot_chain_as_leaf_actors() {
        let source = r#"
version: 1
proxies:
  mesh-hop:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    via: mesh
    udp: true
  ss-native:
    type: shadowsocks
    server: ss.example
    port: 8388
    cipher: aes-128-gcm
    password: native-secret
    udp: native
  ss-uot:
    type: shadowsocks
    server: ss.example
    port: 8389
    cipher: chacha20-ietf-poly1305
    password: uot-secret
    udp: uot-v2
groups:
  mesh-ss-uot:
    type: chain
    members: [mesh-hop, ss-uot]
  preferred:
    type: fallback
    members: [mesh-ss-uot, ss-native]
rules:
  - NETWORK,udp,preferred
  - MATCH,preferred
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let resolver = |name: &str,
                        _instance_id: Option<Uuid>,
                        _virtual_ip: Option<IpAddr>,
                        _port: Option<u16>| {
            (name == "mesh-hop").then(|| ResolvedMeshServer {
                endpoint: "127.0.0.1:32100".parse().unwrap(),
                username: "easytier".to_owned(),
                password: "secret".to_owned(),
            })
        };
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &resolver,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        #[cfg(feature = "leaf-runtime")]
        leaf::config::from_string(&config).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let outbounds = config["outbounds"].as_array().unwrap();
        let native = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "ss-native")
            .unwrap();
        assert_eq!(native["protocol"], "shadowsocks");
        assert_eq!(native["settings"]["method"], "aes-128-gcm");
        assert_eq!(native["settings"]["uotV2"], false);
        let uot = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "ss-uot")
            .unwrap();
        assert_eq!(uot["settings"]["uotV2"], true);
        let chain = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "mesh-ss-uot")
            .unwrap();
        assert_eq!(
            chain["settings"]["actors"],
            serde_json::json!(["mesh-hop", "ss-uot"])
        );
        let fallback = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "preferred")
            .unwrap();
        assert_eq!(
            fallback["settings"]["actors"],
            serde_json::json!(["mesh-ss-uot", "ss-native"])
        );
    }

    #[test]
    fn revalidates_pinned_rule_data_before_compiling_leaf() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("geosite.dat");
        fs::write(&path, b"validated fixture").unwrap();
        let sha256 = format!("{:x}", Sha256::digest(b"validated fixture"));
        let source = format!(
            "version: 1\nrule-sets:\n  site: {{ type: geosite, path: geosite.dat, sha256: {sha256} }}\nrules: [\"GEOSITE,CN,DIRECT\", \"MATCH,REJECT\"]\n"
        );
        let revision = PolicyRevision::parse(source, directory.path()).unwrap();

        fs::write(path, b"changed after preflight").unwrap();
        let error = compile_leaf_config(
            &revision,
            7,
            directory.path(),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            LeafConfigError::RuleSetIntegrity { name, reason }
                if name == "site" && reason == "sha256 mismatch"
        ));
    }

    #[test]
    fn explicit_dns_sets_replace_platform_direct_and_keep_proxy_separate() {
        let source = r#"
version: 1
dns:
  direct: [223.5.5.5, "doh:dns.alidns.com@223.5.5.5"]
  proxy: ["doh:cloudflare-dns.com@1.1.1.1", "doh:dns.google@8.8.8.8"]
rules: ["MATCH,DIRECT"]
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config_with_options(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["192.0.2.53".parse().unwrap()],
            LeafConfigOptions {
                fake_dns_ipv6: true,
                ..Default::default()
            },
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(
            config["dns"]["servers"],
            serde_json::json!([
                "direct:223.5.5.5",
                "direct:doh:dns.alidns.com@223.5.5.5",
                "doh:cloudflare-dns.com@1.1.1.1",
                "doh:dns.google@8.8.8.8"
            ])
        );
        assert_eq!(config["inbounds"][0]["settings"]["fakeDnsIpv6"], true);
        assert_eq!(
            config["inbounds"][0]["settings"]["fakeDnsRange"],
            crate::DEFAULT_FAKE_DNS_IPV4_RANGE
        );
        assert_eq!(
            config["inbounds"][0]["settings"]["fakeDnsIpv6Range"],
            crate::DEFAULT_FAKE_DNS_IPV6_RANGE
        );
    }

    #[test]
    fn expands_system_dns_to_captured_platform_servers_for_proxy_bootstrap() {
        // Mihomo hub/executor/executor.go::updateDNS and
        // component/dialer/dialer.go::parseAddr resolve proxy server hostnames with
        // ProxyServerHostResolver, never through the FakeIP listener. EasyTier has a
        // narrower DNS surface, so `system` means the platform DNS snapshot supplied
        // by the host before Leaf takes ownership of the TUN.
        let source = r#"
version: 1
dns:
  direct: [system, 223.5.5.5, system]
  proxy: ["doh:cloudflare-dns.com@1.1.1.1"]
rules: ["MATCH,DIRECT"]
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &[
                "192.0.2.53".parse().unwrap(),
                "2001:db8::53".parse().unwrap(),
                "192.0.2.53".parse().unwrap(),
            ],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(
            config["dns"]["servers"],
            serde_json::json!([
                "direct:192.0.2.53",
                "direct:2001:db8::53",
                "direct:223.5.5.5",
                "doh:cloudflare-dns.com@1.1.1.1"
            ])
        );
        assert!(
            !config["dns"]["servers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|server| server == "direct:system")
        );
    }

    #[test]
    fn preserves_unresolved_domain_contract_for_direct_socks_and_fallback() {
        // Reference semantics:
        // - Mihomo adapter/outbound/direct.go::Direct::{DialContext,ResolveUDP} resolves a
        //   DIRECT destination with resolver::DirectHostResolver.
        // - Leaf b1e33b50 proxy/mod.rs::{connect_stream_outbound,connect_datagram_outbound}
        //   resolves DIRECT locally, while SOCKS receives the original destination domain;
        //   failover/{stream,datagram}.rs invokes the selected actor's connection path.
        // - At that exact pin, app/dns/client.rs::_lookup_inner forces DnsQueryRoute::direct
        //   for direct_lookup, so a DIRECT member inside a non-direct group still selects the
        //   direct-marked DNS subset and its resolver-scoped cache.
        // EasyTier therefore must not pre-resolve every domain before Leaf chooses an actor.
        let source = r#"
version: 1
dns:
  direct: [223.5.5.5]
  proxy: ["doh:cloudflare-dns.com@1.1.1.1"]
proxies:
  native:
    type: socks5
    server: proxy.example
    port: 1080
    udp: true
groups:
  route:
    type: fallback
    members: [native, DIRECT]
rules:
  - DOMAIN-SUFFIX,direct.example,DIRECT
  - DOMAIN-SUFFIX,proxy.example,route
  - MATCH,route
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["9.9.9.9".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();

        assert_eq!(config["router"]["domainResolve"], false);
        assert_eq!(
            config["dns"]["servers"],
            serde_json::json!(["direct:223.5.5.5", "doh:cloudflare-dns.com@1.1.1.1"])
        );

        let outbounds = config["outbounds"].as_array().unwrap();
        let native = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "native")
            .unwrap();
        assert_eq!(native["protocol"], "socks");
        assert_eq!(native["settings"]["address"], "proxy.example");
        let route = outbounds
            .iter()
            .find(|outbound| outbound["tag"] == "route")
            .unwrap();
        assert_eq!(route["protocol"], "failover");
        assert_eq!(
            route["settings"]["actors"],
            serde_json::json!(["native", "DIRECT"])
        );
        assert_eq!(
            route["settings"]["healthCheckUrl"],
            "https://www.gstatic.com/generate_204"
        );

        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["target"], "DIRECT");
        assert_eq!(rules[1]["target"], "route");
        assert!(rules[0].get("resolveDomain").is_none());
        assert!(rules[1].get("resolveDomain").is_none());
    }

    #[test]
    fn compiles_geoip_dat_categories_without_dns_or_mmdb() {
        let dir = tempfile::tempdir().unwrap();
        crate::geodata::write_test_geoip(
            &dir.path().join("geoip.dat"),
            "GOOGLE",
            vec![(vec![8, 8, 8, 0], 24)],
        );
        let source = r#"
version: 1
rule-sets:
  geoip: { type: geoip, path: geoip.dat }
rules:
  - GEOIP,google,DIRECT,no-resolve
  - GEOIP,lan,DIRECT,no-resolve
  - MATCH,REJECT
"#;
        let revision = PolicyRevision::parse(source, dir.path()).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            dir.path(),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert!(
            rules[0]["ip"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("8.8.8.0/24"))
        );
        assert!(
            rules[0]["ip"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("10.0.0.0/8"))
        );
    }

    #[test]
    fn compacts_contiguous_domain_rules_with_the_same_target() {
        let rules = compiled_rules(
            r#"
version: 1
rules:
  - DOMAIN,exact.example,DIRECT
  - DOMAIN-SUFFIX,suffix.example,DIRECT
  - DOMAIN-KEYWORD,keyword,DIRECT
  - MATCH,REJECT
"#,
        );
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["target"], "DIRECT");
        assert_eq!(rules[0]["domain"], serde_json::json!(["exact.example"]));
        assert_eq!(
            rules[0]["domainSuffix"],
            serde_json::json!(["suffix.example"])
        );
        assert_eq!(rules[0]["domainKeyword"], serde_json::json!(["keyword"]));
    }

    #[test]
    fn preserves_per_rule_domain_resolution_semantics() {
        let rules = compiled_rules(
            r#"
version: 1
rules:
  - IP-CIDR,203.0.113.0/24,DIRECT
  - IP-CIDR,198.51.100.0/24,DIRECT,no-resolve
  - MATCH,REJECT
"#,
        );

        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0]["resolveDomain"], true);
        assert!(rules[1].get("resolveDomain").is_none());
        assert!(rules[2].get("resolveDomain").is_none());
    }

    #[test]
    fn external_geoip_honors_no_resolve() {
        let directory = tempfile::tempdir().unwrap();
        crate::geodata::write_test_geoip(
            &directory.path().join("geoip.dat"),
            "GOOGLE",
            vec![(vec![8, 8, 8, 0], 24)],
        );
        let source = r#"
version: 1
rule-sets:
  geoip: { type: geoip, path: geoip.dat }
rules:
  - EXTERNAL,geoip:google,DIRECT
  - EXTERNAL,geoip:google,REJECT,no-resolve
  - MATCH,REJECT
"#;
        let revision = PolicyRevision::parse(source, directory.path()).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            directory.path(),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["resolveDomain"], true);
        assert!(rules[1].get("resolveDomain").is_none());
    }

    #[test]
    fn compacts_contiguous_geosite_categories_with_the_same_target() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("geosite.dat"), b"fixture").unwrap();
        let source = r#"
version: 1
rule-sets:
  geosite: { type: geosite, path: geosite.dat }
proxies:
  overseas:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: true
rules:
  - GEOSITE,github,overseas
  - GEOSITE,google,overseas
  - MATCH,DIRECT
"#;
        let revision = PolicyRevision::parse(source, directory.path()).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            directory.path(),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["target"], "overseas");
        assert_eq!(rules[0]["external"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn preserves_target_family_and_modifier_boundaries() {
        let rules = compiled_rules(
            r#"
version: 1
rules:
  - DOMAIN,a.example,DIRECT
  - DOMAIN-SUFFIX,b.example,DIRECT
  - DOMAIN,c.example,REJECT
  - DOMAIN,d.example,DIRECT
  - IP-CIDR,10.0.0.0/8,DIRECT,no-resolve
  - IP-CIDR,192.168.0.0/16,DIRECT
  - MATCH,REJECT
"#,
        );
        assert_eq!(rules.len(), 6);
        assert_eq!(rules[0]["target"], "DIRECT");
        assert_eq!(rules[1]["target"], "REJECT");
        assert_eq!(rules[2]["target"], "DIRECT");
        assert_eq!(rules[3]["ip"], serde_json::json!(["10.0.0.0/8"]));
        assert_eq!(rules[4]["ip"], serde_json::json!(["192.168.0.0/16"]));
        assert_eq!(rules[5]["target"], "REJECT");
    }

    #[test]
    fn compacts_large_same_target_rule_blocks() {
        let mut source = String::from("version: 1\nrules:\n");
        for index in 0..5_000 {
            source.push_str(&format!(
                "  - DOMAIN-SUFFIX,nonmatch-{index:05}.invalid,DIRECT\n"
            ));
        }
        source.push_str("  - MATCH,REJECT\n");

        let rules = compiled_rules(&source);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["domainSuffix"].as_array().unwrap().len(), 5_000);
    }

    #[test]
    fn bounds_repeated_geoip_rule_expansion() {
        assert_eq!(reserve_geoip_cidrs(100, 200).unwrap(), 300);
        assert!(reserve_geoip_cidrs(MAX_COMPILED_GEOIP_CIDRS, 1).is_err());
    }

    #[test]
    fn rejects_unresolved_mesh_proxy() {
        let source = r#"
version: 1
proxies:
  mesh:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    port: 1080
    via: mesh
    udp: true
rules: ["FINAL,mesh"]
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        assert_eq!(
            compile_leaf_config(
                &revision,
                7,
                Path::new("."),
                &Unresolved,
                &["1.1.1.1".parse().unwrap()],
            ),
            Err(LeafConfigError::UnresolvedMeshProxy("mesh".to_owned()))
        );
    }

    #[test]
    fn rewrites_mesh_proxy_to_private_bridge_endpoint() {
        let source = r#"
version: 1
proxies:
  mesh:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    port: 1080
    via: mesh
    udp: true
rules: ["FINAL,mesh"]
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &LoopbackMesh,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let mesh = config["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|outbound| outbound["tag"] == "mesh")
            .unwrap();
        assert_eq!(mesh["protocol"], "socks");
        assert_eq!(mesh["settings"]["address"], "127.0.0.1");
        assert_eq!(mesh["settings"]["port"], 32100);
        assert_eq!(mesh["settings"]["username"], "easytier");
        assert_eq!(mesh["settings"]["password"], "secret");
        assert!(!config.to_string().contains("10.44.0.8"));
    }

    #[test]
    fn preserves_rule_order_and_skips_tcp_only_actors_for_udp() {
        let source = r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
  udp-exit:
    type: socks5
    server: 127.0.0.1
    port: 1081
    udp: true
rules:
  - DOMAIN-SUFFIX,example.com,tcp-only
  - NETWORK,udp,udp-exit
  - DOMAIN-KEYWORD,internal,DIRECT
  - MATCH,tcp-only
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0]["domainSuffix"][0], "example.com");
        assert_eq!(rules[0]["target"], "tcp-only");
        assert_eq!(rules[0]["network"], serde_json::json!(["tcp"]));
        assert_eq!(rules[1]["network"], serde_json::json!(["udp"]));
        assert_eq!(rules[1]["target"], "udp-exit");
        assert_eq!(rules[2]["domainKeyword"][0], "internal");
        assert_eq!(rules[2]["target"], "DIRECT");
        assert_eq!(rules[3]["target"], "tcp-only");
        assert_eq!(rules[3]["network"], serde_json::json!(["tcp"]));
    }

    #[test]
    fn omits_impossible_udp_rule_and_keeps_the_next_rule() {
        let source = r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules:
  - NETWORK,udp,tcp-only
  - MATCH,DIRECT
"#;
        let revision = PolicyRevision::parse(source, Path::new(".")).unwrap();
        let config = compile_leaf_config(
            &revision,
            7,
            Path::new("."),
            &Unresolved,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        let rules = config["router"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["target"], "DIRECT");
        assert_eq!(rules[0]["network"], serde_json::json!(["tcp", "udp"]));
    }
}
