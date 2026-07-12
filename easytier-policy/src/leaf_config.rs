use std::{fmt::Write as _, net::IpAddr, path::Path};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    ChainKind, PolicyRevision, ProxyKind, ProxyServer, ProxyVia, RuleSetKind, config::RuleSet,
};

pub trait MeshServerResolver: Send + Sync {
    fn resolve(&self, instance_id: Option<Uuid>, virtual_ip: Option<IpAddr>) -> Option<IpAddr>;
}

impl<F> MeshServerResolver for F
where
    F: Fn(Option<Uuid>, Option<IpAddr>) -> Option<IpAddr> + Send + Sync,
{
    fn resolve(&self, instance_id: Option<Uuid>, virtual_ip: Option<IpAddr>) -> Option<IpAddr> {
        self(instance_id, virtual_ip)
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
}

pub fn compile_leaf_config(
    revision: &PolicyRevision,
    tun_fd: i32,
    base_dir: &Path,
    resolver: &dyn MeshServerResolver,
) -> Result<String, LeafConfigError> {
    let document = &revision.document;
    let mut output = String::new();
    writeln!(output, "[General]").unwrap();
    writeln!(output, "tun-fd = {tun_fd}").unwrap();
    writeln!(output, "tun2socks-backend = smoltcp").unwrap();
    writeln!(output, "loglevel = warn\n").unwrap();

    writeln!(output, "[Proxy]").unwrap();
    writeln!(output, "DIRECT = direct").unwrap();
    writeln!(output, "REJECT = drop").unwrap();
    for (name, proxy) in &document.proxies {
        let address = match &proxy.server {
            ProxyServer::Address(address) => address.clone(),
            ProxyServer::Mesh {
                instance_id,
                virtual_ip,
            } => resolver
                .resolve(*instance_id, *virtual_ip)
                .ok_or_else(|| LeafConfigError::UnresolvedMeshProxy(name.clone()))?
                .to_string(),
        };
        let protocol = match proxy.kind {
            ProxyKind::Socks5 => "socks",
            ProxyKind::Http => "http",
        };
        writeln!(output, "{name} = {protocol}, {address}, {}", proxy.port).unwrap();
        if proxy.via == ProxyVia::Mesh {
            writeln!(
                output,
                "# easytier-via-mesh = {name}; resolved endpoint is consumed by the mesh dial adapter"
            )
            .unwrap();
        }
    }
    writeln!(output).unwrap();

    if !document.groups.is_empty() {
        writeln!(output, "[Proxy Group]").unwrap();
        for name in revision.group_order.iter() {
            let group = &document.groups[name];
            let protocol = match group.kind {
                ChainKind::Chain => "chain",
                ChainKind::Fallback => "fallback",
            };
            writeln!(output, "{name} = {protocol}, {}", group.members.join(", ")).unwrap();
        }
        writeln!(output).unwrap();
    }

    writeln!(output, "[Rule]").unwrap();
    for (index, rule) in document.rules.iter().enumerate() {
        let mut parts: Vec<String> = rule.split(',').map(|part| part.trim().to_owned()).collect();
        if parts[0].eq_ignore_ascii_case("MATCH") {
            parts[0] = "FINAL".to_owned();
        }
        if parts[0].eq_ignore_ascii_case("GEOIP") {
            let rule_set = find_single_rule_set(document.rule_sets.values(), RuleSetKind::Mmdb)
                .ok_or(LeafConfigError::MissingRuleSet {
                    index,
                    kind: "mmdb",
                })?;
            parts[0] = "EXTERNAL".to_owned();
            parts[1] = external_rule("mmdb", rule_set, &parts[1], base_dir)?;
        } else if parts[0].eq_ignore_ascii_case("EXTERNAL") {
            let (kind, code) = parts[1].split_once(':').unwrap_or(("site", &parts[1]));
            let (rule_set_kind, leaf_kind) = match kind.to_ascii_lowercase().as_str() {
                "site" | "geosite" => (RuleSetKind::Geosite, "site"),
                "mmdb" | "geoip" => (RuleSetKind::Mmdb, "mmdb"),
                _ => {
                    return Err(LeafConfigError::MissingRuleSet {
                        index,
                        kind: "recognized external",
                    });
                }
            };
            let rule_set = find_single_rule_set(document.rule_sets.values(), rule_set_kind).ok_or(
                LeafConfigError::MissingRuleSet {
                    index,
                    kind: leaf_kind,
                },
            )?;
            parts[1] = external_rule(leaf_kind, rule_set, code, base_dir)?;
        }
        writeln!(output, "{}", parts.join(", ")).unwrap();
    }

    Ok(output)
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
    if path.contains(':') || path.contains(',') || path.contains('\n') {
        return Err(LeafConfigError::InvalidRuleSetPath(path.into_owned()));
    }
    Ok(format!("{kind}:{path}:{code}"))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::PolicyRevision;

    use super::*;

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
groups:
  final:
    type: fallback
    members: [native, DIRECT]
rules:
  - EXTERNAL,site:cn,DIRECT
  - GEOIP,US,final
  - MATCH,final
"#;
        let revision = PolicyRevision::parse(source, dir.path()).unwrap();
        let config = compile_leaf_config(&revision, 7, dir.path(), &|_, _| None).unwrap();
        assert!(config.contains("tun-fd = 7"));
        assert!(config.contains("final = fallback, native, DIRECT"));
        assert!(config.contains(&format!(
            "EXTERNAL, site:{}/site.dat:cn, DIRECT",
            dir.path().display()
        )));
        assert!(config.contains(&format!(
            "EXTERNAL, mmdb:{}/geo.mmdb:US, final",
            dir.path().display()
        )));
        assert!(config.contains("FINAL, final"));
        assert!(!config.contains("MATCH"));
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
            compile_leaf_config(&revision, 7, Path::new("."), &|_, _| None),
            Err(LeafConfigError::UnresolvedMeshProxy("mesh".to_owned()))
        );
    }
}
