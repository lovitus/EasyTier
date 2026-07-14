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
    config::RuleSet,
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
        port: u16,
    ) -> Option<ResolvedMeshServer>;
}

impl<F> MeshServerResolver for F
where
    F: Fn(&str, Option<Uuid>, Option<std::net::IpAddr>, u16) -> Option<ResolvedMeshServer>
        + Send
        + Sync,
{
    fn resolve(
        &self,
        proxy_name: &str,
        instance_id: Option<Uuid>,
        virtual_ip: Option<std::net::IpAddr>,
        port: u16,
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
}

pub fn compile_leaf_config(
    revision: &PolicyRevision,
    tun_fd: i32,
    base_dir: &Path,
    resolver: &dyn MeshServerResolver,
    dns_servers: &[IpAddr],
) -> Result<String, LeafConfigError> {
    if dns_servers.is_empty() {
        return Err(LeafConfigError::NoDnsServers);
    }
    let document = &revision.document;
    let mut outbounds = vec![
        serde_json::json!({ "tag": "DIRECT", "protocol": "direct" }),
        serde_json::json!({ "tag": "REJECT", "protocol": "drop" }),
    ];
    for (name, proxy) in &document.proxies {
        let (address, port, credentials) = match &proxy.server {
            ProxyServer::Address(address) => (
                address.clone(),
                proxy.port,
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
            ProxyKind::Http => unreachable!("policy validation rejects HTTP outbound in v1"),
        }
    }
    for name in revision.group_order.iter() {
        let group = &document.groups[name];
        let (protocol, settings) = match group.kind {
            ChainKind::Chain => ("chain", serde_json::json!({ "actors": group.members })),
            ChainKind::Fallback => (
                "failover",
                // Disable Leaf's active probes against hard-coded public targets. v1 intentionally
                // uses passive per-connection failover only.
                serde_json::json!({
                    "actors": group.members,
                    "healthCheck": false,
                    "failover": true,
                }),
            ),
        };
        outbounds.push(serde_json::json!({
            "tag": name,
            "protocol": protocol,
            "settings": settings,
        }));
    }

    let config = serde_json::json!({
        "log": { "level": "warn" },
        "dns": {
            "servers": dns_servers
                .iter()
                .map(|server| format!("direct:{server}"))
                .collect::<Vec<_>>(),
        },
        "inbounds": [{
            "tag": "tun",
            "protocol": "tun",
            "settings": {
                "fd": tun_fd,
                "fakeDnsInclude": ["*"],
                "tun2socks": "smoltcp",
            },
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

        match rule_type.as_str() {
            "IP-CIDR" => {
                rule.ip = Some(vec![parts[1].to_owned()]);
                merge_family = Some(RuleMergeFamily::Ip);
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
            }
            "COUNTRY" => {
                let rule_set = find_single_rule_set(document.rule_sets.values(), RuleSetKind::Mmdb)
                    .ok_or(LeafConfigError::MissingRuleSet {
                        index,
                        kind: "mmdb",
                    })?;
                rule.external = Some(vec![external_rule("mmdb", rule_set, parts[1], base_dir)?]);
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
                    }
                    "geoip" | "geoip-dat" => {
                        compiled_geoip_cidrs = reserve_geoip_cidrs(
                            compiled_geoip_cidrs,
                            apply_geoip_rule(&mut rule, code, &geoip_categories)?,
                        )?;
                        merge_family = Some(RuleMergeFamily::Ip);
                    }
                    _ => {
                        return Err(LeafConfigError::MissingRuleSet {
                            index,
                            kind: "recognized external",
                        });
                    }
                }
            }
            "PORT-RANGE" => rule.port_range = Some(vec![parts[1].to_owned()]),
            "NETWORK" => rule.network = Some(vec![parts[1].to_ascii_lowercase()]),
            "INBOUND-TAG" => rule.inbound_tag = Some(vec![parts[1].to_owned()]),
            // A network matcher over both supported session kinds is Leaf's non-special-cased,
            // order-preserving representation of an unconditional MATCH/FINAL rule.
            "MATCH" | "FINAL" => {
                rule.network = Some(vec!["tcp".to_owned(), "udp".to_owned()]);
            }
            _ => unreachable!("policy validation rejects unsupported rule types"),
        }

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

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::PolicyRevision;

    use super::*;

    struct Unresolved;

    impl MeshServerResolver for Unresolved {
        fn resolve(
            &self,
            _proxy_name: &str,
            _instance_id: Option<Uuid>,
            _virtual_ip: Option<IpAddr>,
            _port: u16,
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
            port: u16,
        ) -> Option<ResolvedMeshServer> {
            assert_eq!(proxy_name, "mesh");
            assert_eq!(port, 1080);
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
        assert_eq!(config["dns"]["servers"][0], "direct:1.1.1.1");
        assert_eq!(config["outbounds"][2]["tag"], "native");
        assert_eq!(config["outbounds"][2]["protocol"], "socks");
        assert_eq!(config["outbounds"][3]["tag"], "final");
        assert_eq!(config["outbounds"][3]["protocol"], "failover");
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
