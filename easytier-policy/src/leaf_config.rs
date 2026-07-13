use std::{
    fmt::Write as _,
    net::{IpAddr, SocketAddr},
    path::Path,
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    ChainKind, PolicyRevision, ProxyKind, ProxyServer, ProxyVia, RuleSetKind, config::RuleSet,
};

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
    let mut output = String::new();
    writeln!(output, "[General]").unwrap();
    writeln!(output, "tun-fd = {tun_fd}").unwrap();
    writeln!(output, "tun2socks-backend = smoltcp").unwrap();
    writeln!(output, "always-fake-ip = *").unwrap();
    writeln!(
        output,
        "dns-server = {}",
        dns_servers
            .iter()
            .map(|server| format!("direct:{server}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(output, "loglevel = warn\n").unwrap();

    writeln!(output, "[Proxy]").unwrap();
    writeln!(output, "DIRECT = direct").unwrap();
    writeln!(output, "REJECT = drop").unwrap();
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
        let protocol = match proxy.kind {
            ProxyKind::Socks5 => "socks",
            ProxyKind::Http => "http",
        };
        write!(output, "{name} = {protocol}, {address}, {port}").unwrap();
        if let Some((username, password)) = credentials {
            write!(output, ", username={username}, password={password}").unwrap();
        }
        writeln!(output).unwrap();
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
            write!(output, "{name} = {protocol}, {}", group.members.join(", ")).unwrap();
            if group.kind == ChainKind::Fallback {
                // Leaf otherwise starts active probes against hard-coded public targets after the
                // group is used. v1 intentionally uses passive per-connection fallback only.
                write!(output, ", health-check=false").unwrap();
            }
            writeln!(output).unwrap();
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
  - EXTERNAL,site:cn,DIRECT
  - GEOIP,US,final
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
        assert!(config.contains("tun-fd = 7"));
        assert!(config.contains("always-fake-ip = *"));
        assert!(config.contains("dns-server = direct:1.1.1.1"));
        assert!(
            config.contains("native = socks, 127.0.0.1, 1080, username=alice, password=secret")
        );
        assert!(config.contains("final = fallback, native, DIRECT, health-check=false"));
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
        assert!(
            config.contains("mesh = socks, 127.0.0.1, 32100, username=easytier, password=secret")
        );
        assert!(!config.contains("10.44.0.8"));
    }
}
