use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read as _,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RULE_SET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_ACTORS: usize = 1_024;
const MAX_EXPANDED_CHAIN_ACTORS: usize = 32;
const MAX_GROUP_REFERENCES: usize = 64;

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("failed to read policy document {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("policy document exceeds {MAX_DOCUMENT_BYTES} bytes")]
    TooLarge,
    #[error("invalid policy YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unsupported policy document version {0}")]
    UnsupportedVersion(u32),
    #[error("policy document has no rule")]
    NoRules,
    #[error("policy document exceeds the {kind} limit ({limit})")]
    Limit { kind: &'static str, limit: usize },
    #[error("duplicate reserved actor name {0}")]
    ReservedName(String),
    #[error("invalid actor name {0}")]
    InvalidActorName(String),
    #[error("unknown actor reference {reference} in {owner}")]
    UnknownReference { owner: String, reference: String },
    #[error("proxy group cycle: {0}")]
    Cycle(String),
    #[error("chain {0} expands beyond the actor limit")]
    ChainTooDeep(String),
    #[error("proxy {name} has invalid server selector: {reason}")]
    InvalidServer { name: String, reason: String },
    #[error("rule {index} is invalid: {reason}")]
    InvalidRule { index: usize, reason: String },
    #[error("rule-set {name} is invalid: {reason}")]
    InvalidRuleSet { name: String, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyMode {
    #[default]
    Rule,
    Global,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleSetKind {
    Geosite,
    Mmdb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    #[serde(rename = "type")]
    pub kind: RuleSetKind,
    pub path: PathBuf,
    #[serde(default = "manual_update")]
    pub update: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

fn manual_update() -> String {
    "manual".to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyKind {
    Socks5,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyVia {
    Mesh,
    #[default]
    Native,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProxyServer {
    Address(String),
    Mesh {
        #[serde(rename = "instance-id")]
        instance_id: Option<Uuid>,
        #[serde(rename = "virtual-ip")]
        virtual_ip: Option<IpAddr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    #[serde(rename = "type")]
    pub kind: ProxyKind,
    pub server: ProxyServer,
    pub port: u16,
    #[serde(default)]
    pub via: ProxyVia,
    #[serde(default)]
    pub udp: bool,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

impl Proxy {
    pub fn credentials(&self) -> Option<(&str, &str)> {
        self.username.as_deref().zip(self.password.as_deref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChainKind {
    Chain,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    #[serde(rename = "type")]
    pub kind: ChainKind,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDocument {
    pub version: u32,
    #[serde(default, rename = "rule-sets")]
    pub rule_sets: BTreeMap<String, RuleSet>,
    #[serde(default)]
    pub proxies: BTreeMap<String, Proxy>,
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PolicyRevision {
    pub id: String,
    pub digest: [u8; 32],
    pub source: Arc<str>,
    pub document: Arc<PolicyDocument>,
    pub group_order: Arc<[String]>,
}

impl PolicyRevision {
    pub fn parse(source: impl Into<Arc<str>>, base_dir: &Path) -> Result<Self, PolicyError> {
        let source = source.into();
        if source.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(PolicyError::TooLarge);
        }
        let document: PolicyDocument = serde_yaml::from_str(&source)?;
        let group_order = document.validate(base_dir)?;
        let digest: [u8; 32] = Sha256::digest(source.as_bytes()).into();
        let id = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            id,
            digest,
            source,
            document: Arc::new(document),
            group_order: group_order.into(),
        })
    }
}

pub fn validate_policy_file(path: &Path) -> Result<PolicyRevision, PolicyError> {
    let metadata = fs::metadata(path).map_err(|source| PolicyError::Read {
        path: path.to_owned(),
        source,
    })?;
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(PolicyError::TooLarge);
    }
    let source = fs::read_to_string(path).map_err(|source| PolicyError::Read {
        path: path.to_owned(),
        source,
    })?;
    PolicyRevision::parse(source, path.parent().unwrap_or_else(|| Path::new(".")))
}

impl PolicyDocument {
    fn validate(&self, base_dir: &Path) -> Result<Vec<String>, PolicyError> {
        if self.version != 1 {
            return Err(PolicyError::UnsupportedVersion(self.version));
        }
        if self.rules.is_empty() {
            return Err(PolicyError::NoRules);
        }
        if self.rules.len() > MAX_RULES {
            return Err(PolicyError::Limit {
                kind: "rule",
                limit: MAX_RULES,
            });
        }
        if self.proxies.len() + self.groups.len() > MAX_ACTORS {
            return Err(PolicyError::Limit {
                kind: "actor",
                limit: MAX_ACTORS,
            });
        }

        self.validate_rule_sets(base_dir)?;
        self.validate_names()?;
        self.validate_proxies()?;
        let order = self.topological_group_order()?;
        self.validate_rules()?;
        Ok(order)
    }

    fn validate_rule_sets(&self, base_dir: &Path) -> Result<(), PolicyError> {
        for (name, rule_set) in &self.rule_sets {
            if rule_set.update != "manual" {
                return Err(PolicyError::InvalidRuleSet {
                    name: name.clone(),
                    reason: "v1 only permits update: manual".to_owned(),
                });
            }
            let path = if rule_set.path.is_absolute() {
                rule_set.path.clone()
            } else {
                base_dir.join(&rule_set.path)
            };
            if path
                .to_string_lossy()
                .chars()
                .any(|character| matches!(character, ':' | ',' | '\r' | '\n' | '=' | '#' | ';'))
            {
                return Err(PolicyError::InvalidRuleSet {
                    name: name.clone(),
                    reason: "path contains a delimiter unsupported by Leaf".to_owned(),
                });
            }
            let metadata = fs::metadata(&path).map_err(|error| PolicyError::InvalidRuleSet {
                name: name.clone(),
                reason: format!("{}: {error}", path.display()),
            })?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(PolicyError::InvalidRuleSet {
                    name: name.clone(),
                    reason: "rule data must be a non-empty regular file".to_owned(),
                });
            }
            if metadata.len() > MAX_RULE_SET_BYTES {
                return Err(PolicyError::InvalidRuleSet {
                    name: name.clone(),
                    reason: format!("rule data exceeds {MAX_RULE_SET_BYTES} bytes"),
                });
            }
            if let Some(expected) = &rule_set.sha256 {
                if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(PolicyError::InvalidRuleSet {
                        name: name.clone(),
                        reason: "sha256 must contain exactly 64 hexadecimal characters".to_owned(),
                    });
                }
                let mut file =
                    fs::File::open(&path).map_err(|error| PolicyError::InvalidRuleSet {
                        name: name.clone(),
                        reason: error.to_string(),
                    })?;
                let mut digest = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let length =
                        file.read(&mut buffer)
                            .map_err(|error| PolicyError::InvalidRuleSet {
                                name: name.clone(),
                                reason: error.to_string(),
                            })?;
                    if length == 0 {
                        break;
                    }
                    digest.update(&buffer[..length]);
                }
                let actual = format!("{:x}", digest.finalize());
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err(PolicyError::InvalidRuleSet {
                        name: name.clone(),
                        reason: "sha256 mismatch".to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_names(&self) -> Result<(), PolicyError> {
        for name in self.proxies.keys().chain(self.groups.keys()) {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return Err(PolicyError::InvalidActorName(name.clone()));
            }
            if matches!(name.as_str(), "DIRECT" | "REJECT") {
                return Err(PolicyError::ReservedName(name.clone()));
            }
        }
        for name in self.proxies.keys() {
            if self.groups.contains_key(name) {
                return Err(PolicyError::ReservedName(name.clone()));
            }
        }
        Ok(())
    }

    fn validate_proxies(&self) -> Result<(), PolicyError> {
        for (name, proxy) in &self.proxies {
            if proxy.port == 0 {
                return Err(PolicyError::InvalidServer {
                    name: name.clone(),
                    reason: "port 0 is not allowed".to_owned(),
                });
            }
            match (&proxy.via, &proxy.server) {
                (
                    ProxyVia::Mesh,
                    ProxyServer::Mesh {
                        instance_id: None,
                        virtual_ip: None,
                    },
                ) => {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "mesh server requires instance-id or virtual-ip".to_owned(),
                    });
                }
                (ProxyVia::Mesh, ProxyServer::Address(_)) => {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "via: mesh requires a structured server selector".to_owned(),
                    });
                }
                (ProxyVia::Native, ProxyServer::Mesh { .. }) => {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "structured mesh selector requires via: mesh".to_owned(),
                    });
                }
                (_, ProxyServer::Address(address))
                    if address.is_empty()
                        || address.len() > 253
                        || address.bytes().any(|byte| {
                            byte.is_ascii_whitespace() || matches!(byte, b',' | b'=' | b'#' | b';')
                        }) =>
                {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "address contains unsupported characters".to_owned(),
                    });
                }
                _ => {}
            }
            match (&proxy.username, &proxy.password) {
                (None, None) => {}
                (Some(username), Some(password))
                    if valid_proxy_credential(username) && valid_proxy_credential(password) => {}
                (Some(_), Some(_)) => {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "username/password must be 1..=128 safe ASCII characters"
                            .to_owned(),
                    });
                }
                _ => {
                    return Err(PolicyError::InvalidServer {
                        name: name.clone(),
                        reason: "username and password must be configured together".to_owned(),
                    });
                }
            }
            if proxy.kind == ProxyKind::Http && proxy.udp {
                return Err(PolicyError::InvalidServer {
                    name: name.clone(),
                    reason: "HTTP CONNECT is TCP-only".to_owned(),
                });
            }
            if proxy.kind == ProxyKind::Http {
                return Err(PolicyError::InvalidServer {
                    name: name.clone(),
                    reason:
                        "the pinned Leaf runtime has no HTTP CONNECT outbound; v1 requires SOCKS5"
                            .to_owned(),
                });
            }
            if proxy.via == ProxyVia::Mesh && proxy.kind != ProxyKind::Socks5 {
                return Err(PolicyError::InvalidServer {
                    name: name.clone(),
                    reason: "v1 mesh transport requires a SOCKS5 actor".to_owned(),
                });
            }
        }
        Ok(())
    }

    fn topological_group_order(&self) -> Result<Vec<String>, PolicyError> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Mark {
            Visiting,
            Done,
        }

        fn visit(
            document: &PolicyDocument,
            name: &str,
            marks: &mut BTreeMap<String, Mark>,
            stack: &mut Vec<String>,
            order: &mut Vec<String>,
            references: &mut usize,
        ) -> Result<(), PolicyError> {
            if marks.get(name) == Some(&Mark::Done) {
                return Ok(());
            }
            if marks.get(name) == Some(&Mark::Visiting) {
                stack.push(name.to_owned());
                return Err(PolicyError::Cycle(stack.join(" -> ")));
            }
            marks.insert(name.to_owned(), Mark::Visiting);
            stack.push(name.to_owned());
            let group = &document.groups[name];
            if group.members.is_empty() {
                return Err(PolicyError::UnknownReference {
                    owner: name.to_owned(),
                    reference: "<empty group>".to_owned(),
                });
            }
            for member in &group.members {
                *references += 1;
                if *references > MAX_GROUP_REFERENCES {
                    return Err(PolicyError::Limit {
                        kind: "group reference",
                        limit: MAX_GROUP_REFERENCES,
                    });
                }
                if document.groups.contains_key(member) {
                    visit(document, member, marks, stack, order, references)?;
                } else if !document.proxies.contains_key(member)
                    && !matches!(member.as_str(), "DIRECT" | "REJECT")
                {
                    return Err(PolicyError::UnknownReference {
                        owner: name.to_owned(),
                        reference: member.clone(),
                    });
                }
            }
            stack.pop();
            marks.insert(name.to_owned(), Mark::Done);
            order.push(name.to_owned());
            Ok(())
        }

        let mut marks = BTreeMap::new();
        let mut stack = Vec::new();
        let mut order = Vec::new();
        let mut references = 0;
        for name in self.groups.keys() {
            visit(
                self,
                name,
                &mut marks,
                &mut stack,
                &mut order,
                &mut references,
            )?;
            if self.expanded_actor_count(name, &mut BTreeSet::new())? > MAX_EXPANDED_CHAIN_ACTORS {
                return Err(PolicyError::ChainTooDeep(name.clone()));
            }
        }
        Ok(order)
    }

    fn expanded_actor_count(
        &self,
        actor: &str,
        path: &mut BTreeSet<String>,
    ) -> Result<usize, PolicyError> {
        let Some(group) = self.groups.get(actor) else {
            return Ok(1);
        };
        if !path.insert(actor.to_owned()) {
            return Err(PolicyError::Cycle(actor.to_owned()));
        }
        let mut count = 0usize;
        for member in &group.members {
            count = count.saturating_add(self.expanded_actor_count(member, path)?);
        }
        path.remove(actor);
        Ok(count)
    }

    fn validate_rules(&self) -> Result<(), PolicyError> {
        for (index, rule) in self.rules.iter().enumerate() {
            let parts: Vec<_> = rule.split(',').map(str::trim).collect();
            if parts.len() < 2
                || parts.iter().any(|part| {
                    part.is_empty()
                        || part
                            .chars()
                            .any(|character| matches!(character, '\r' | '\n' | '=' | '#' | ';'))
                })
            {
                return Err(PolicyError::InvalidRule {
                    index,
                    reason: "expected comma-separated rule and target".to_owned(),
                });
            }
            let rule_type = parts[0].to_ascii_uppercase();
            let expected_parts = match rule_type.as_str() {
                "MATCH" | "FINAL" => 2,
                "IP-CIDR" | "DOMAIN" | "DOMAIN-SUFFIX" | "DOMAIN-KEYWORD" | "GEOIP" | "GEOSITE"
                | "EXTERNAL" | "PORT-RANGE" | "NETWORK" | "INBOUND-TAG" => 3,
                _ => {
                    return Err(PolicyError::InvalidRule {
                        index,
                        reason: format!("unsupported rule type {}", parts[0]),
                    });
                }
            };
            if parts.len() != expected_parts {
                return Err(PolicyError::InvalidRule {
                    index,
                    reason: format!("{rule_type} requires {expected_parts} fields"),
                });
            }
            if rule_type == "NETWORK" {
                let network = parts[1].to_ascii_lowercase();
                if !matches!(network.as_str(), "tcp" | "udp") {
                    return Err(PolicyError::InvalidRule {
                        index,
                        reason: "NETWORK requires tcp or udp".to_owned(),
                    });
                }
            }
            match rule_type.as_str() {
                "GEOIP" => self.require_single_rule_set(index, RuleSetKind::Mmdb, "mmdb")?,
                "GEOSITE" => {
                    self.require_single_rule_set(index, RuleSetKind::Geosite, "geosite")?
                }
                "EXTERNAL" => {
                    let (kind, code) = parts[1].split_once(':').unwrap_or(("site", parts[1]));
                    if code.is_empty() {
                        return Err(PolicyError::InvalidRule {
                            index,
                            reason: "EXTERNAL category cannot be empty".to_owned(),
                        });
                    }
                    match kind.to_ascii_lowercase().as_str() {
                        "site" | "geosite" => {
                            self.require_single_rule_set(index, RuleSetKind::Geosite, "geosite")?
                        }
                        "mmdb" | "geoip" => {
                            self.require_single_rule_set(index, RuleSetKind::Mmdb, "mmdb")?
                        }
                        _ => {
                            return Err(PolicyError::InvalidRule {
                                index,
                                reason: format!("unsupported EXTERNAL data kind {kind}"),
                            });
                        }
                    }
                }
                _ => {}
            }
            let target = parts.last().copied().unwrap_or_default();
            if !self.actor_exists(target) {
                return Err(PolicyError::UnknownReference {
                    owner: format!("rule {index}"),
                    reference: target.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn require_single_rule_set(
        &self,
        index: usize,
        kind: RuleSetKind,
        label: &str,
    ) -> Result<(), PolicyError> {
        if self
            .rule_sets
            .values()
            .filter(|rule_set| rule_set.kind == kind)
            .count()
            == 1
        {
            return Ok(());
        }
        Err(PolicyError::InvalidRule {
            index,
            reason: format!("rule requires exactly one {label} rule-set"),
        })
    }

    fn actor_exists(&self, name: &str) -> bool {
        matches!(name, "DIRECT" | "REJECT")
            || self.proxies.contains_key(name)
            || self.groups.contains_key(name)
    }

    pub(crate) fn actor_supports_udp(
        &self,
        actor: &str,
        visited: &mut BTreeSet<String>,
    ) -> Result<bool, PolicyError> {
        if matches!(actor, "DIRECT" | "REJECT") {
            return Ok(true);
        }
        if let Some(proxy) = self.proxies.get(actor) {
            return Ok(proxy.udp && proxy.kind == ProxyKind::Socks5);
        }
        if !visited.insert(actor.to_owned()) {
            return Err(PolicyError::Cycle(actor.to_owned()));
        }
        let group = &self.groups[actor];
        let support = match group.kind {
            ChainKind::Chain => {
                let mut all = true;
                for member in &group.members {
                    all &= self.actor_supports_udp(member, visited)?;
                }
                all
            }
            ChainKind::Fallback => {
                let mut any = false;
                for member in &group.members {
                    any |= self.actor_supports_udp(member, visited)?;
                }
                any
            }
        };
        visited.remove(actor);
        Ok(support)
    }
}

fn valid_proxy_credential(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b',' | b'=' | b'#' | b';'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
version: 1
proxies:
  mesh:
    type: socks5
    server:
      instance-id: 11111111-1111-1111-1111-111111111111
      virtual-ip: 10.44.0.8
    port: 1080
    via: mesh
    udp: true
  firewall:
    type: socks5
    server: 192.168.1.1
    port: 1080
groups:
  chain:
    type: chain
    members: [mesh, firewall]
  final-tcp:
    type: fallback
    members: [chain, mesh]
  final-udp:
    type: fallback
    members: [mesh, DIRECT]
rules:
  - NETWORK,udp,final-udp
  - MATCH,final-tcp
"#;

    #[test]
    fn validates_document_and_stable_digest() {
        let first = PolicyRevision::parse(VALID, Path::new(".")).unwrap();
        let second = PolicyRevision::parse(VALID, Path::new(".")).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(
            first.group_order.as_ref(),
            ["chain", "final-tcp", "final-udp"]
        );
    }

    #[test]
    fn rejects_cycles() {
        let source = r#"
version: 1
groups:
  a: { type: chain, members: [b] }
  b: { type: fallback, members: [a] }
rules: ["MATCH,a"]
"#;
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::Cycle(_))
        ));
    }

    #[test]
    fn accepts_tcp_only_udp_target_for_ordered_runtime_fallthrough() {
        let source = r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules: ["NETWORK,udp,tcp-only"]
"#;
        PolicyRevision::parse(source, Path::new(".")).unwrap();
    }

    #[test]
    fn accepts_tcp_only_domain_rule_before_udp_fallback() {
        let source = r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules:
  - DOMAIN-SUFFIX,example.com,tcp-only
  - NETWORK,udp,REJECT
  - MATCH,tcp-only
"#;
        PolicyRevision::parse(source, Path::new(".")).unwrap();
    }

    #[test]
    fn allows_tcp_only_rules_after_an_unconditional_udp_rule() {
        let source = r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules:
  - NETWORK,udp,REJECT
  - DOMAIN-SUFFIX,example.com,tcp-only
  - MATCH,tcp-only
"#;
        PolicyRevision::parse(source, Path::new(".")).unwrap();
    }

    #[test]
    fn rejects_unknown_network_protocol() {
        let source = "version: 1\nrules: [\"NETWORK,quic,DIRECT\", \"MATCH,DIRECT\"]\n";
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidRule { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_external_rule_data() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("first.mmdb"), b"first").unwrap();
        std::fs::write(directory.path().join("second.mmdb"), b"second").unwrap();
        let source = r#"
version: 1
rule-sets:
  first: { type: mmdb, path: first.mmdb }
  second: { type: mmdb, path: second.mmdb }
rules: ["GEOIP,CN,DIRECT"]
"#;
        assert!(matches!(
            PolicyRevision::parse(source, directory.path()),
            Err(PolicyError::InvalidRule { .. })
        ));
    }

    #[test]
    fn rejects_leaf_delimiters_in_rule_set_paths_during_preflight() {
        let source = r#"
version: 1
rule-sets:
  site: { type: geosite, path: "bad:name.dat" }
rules: ["EXTERNAL,site:cn,DIRECT"]
"#;
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidRuleSet { .. })
        ));
    }

    #[test]
    fn rejects_unknown_external_data_kind() {
        let source = "version: 1\nrules: [\"EXTERNAL,unknown:cn,DIRECT\"]\n";
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidRule { .. })
        ));
    }

    #[test]
    fn validates_proxy_credentials_as_an_atomic_safe_pair() {
        let valid = r#"
version: 1
proxies:
  authenticated:
    type: socks5
    server: 127.0.0.1
    port: 1080
    username: alice
    password: secret
rules:
  - NETWORK,udp,REJECT
  - FINAL,authenticated
"#;
        let revision = PolicyRevision::parse(valid, Path::new(".")).unwrap();
        assert_eq!(
            revision.document.proxies["authenticated"].credentials(),
            Some(("alice", "secret"))
        );

        for invalid in [
            valid.replace("    password: secret\n", ""),
            valid.replace("alice", "bad,name"),
            valid.replace("secret", "\"\""),
        ] {
            assert!(matches!(
                PolicyRevision::parse(invalid, Path::new(".")),
                Err(PolicyError::InvalidServer { .. })
            ));
        }
    }

    #[test]
    fn rejects_http_actor_over_mesh_bridge() {
        let source = r#"
version: 1
proxies:
  http:
    type: http
    server: { virtual-ip: 10.44.0.8 }
    port: 8080
    via: mesh
rules: ["FINAL,http"]
"#;
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidServer { .. })
        ));
    }

    #[test]
    fn rejects_native_http_actor_until_runtime_support_exists() {
        let source = r#"
version: 1
proxies:
  http:
    type: http
    server: 192.0.2.10
    port: 8080
    via: native
rules: ["FINAL,http"]
"#;
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidServer { .. })
        ));
    }

    #[test]
    fn rejects_rules_leaf_would_silently_ignore() {
        let source = "version: 1\nrules: [\"UNKNOWN,value,DIRECT\"]\n";
        assert!(matches!(
            PolicyRevision::parse(source, Path::new(".")),
            Err(PolicyError::InvalidRule { .. })
        ));
    }

    #[test]
    fn rejects_leaf_config_injection_fields() {
        let actor = r#"
version: 1
proxies:
  "bad\n[Rule]": { type: http, server: 127.0.0.1, port: 80 }
rules: ["FINAL,REJECT"]
"#;
        assert!(matches!(
            PolicyRevision::parse(actor, Path::new(".")),
            Err(PolicyError::InvalidActorName(_))
        ));

        let address = r#"
version: 1
proxies:
  bad: { type: http, server: "127.0.0.1 # DIRECT", port: 80 }
rules: ["FINAL,bad"]
"#;
        assert!(matches!(
            PolicyRevision::parse(address, Path::new(".")),
            Err(PolicyError::InvalidServer { .. })
        ));

        let rule = "version: 1\nrules: [\"FINAL,DIRECT\\n[Proxy]\"]\n";
        assert!(matches!(
            PolicyRevision::parse(rule, Path::new(".")),
            Err(PolicyError::InvalidRule { .. })
        ));
    }
}
