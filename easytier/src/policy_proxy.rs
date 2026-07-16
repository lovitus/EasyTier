use std::{
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Context as _;
use easytier_policy::PolicyRevision;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UdpSocket,
};

use crate::common::config::{ConfigLoader, PolicyProxyConfig};

#[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
mod macos_routing;
mod mesh_socks_bridge;
mod mesh_udp_relay;
#[cfg(target_os = "linux")]
mod policy_routing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyUnderlayTransition {
    Unchanged,
    RoutesChanged,
    IdentityChanged,
    Lost,
    Recovered,
}

#[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
pub(crate) use macos_routing::PolicyRoutingGuard;
pub(crate) use mesh_socks_bridge::{MeshProxyBridgeSet, MeshProxyTarget};
pub(crate) use mesh_udp_relay::{MeshSocksRelayService, RemoteUdpAssociation};
#[cfg(target_os = "linux")]
pub(crate) use policy_routing::PolicyRoutingGuard;

pub(crate) const POLICY_SOCKET_MARK: u32 = 0x4554_5001;
pub(crate) const BUILT_IN_MESH_SOCKS_PORT: u16 = easytier_socks_egress::DEFAULT_PORT_CANDIDATES[0];
const POLICY_UDP_SOCKET_BUFFER_SIZE: usize = 4 * 1_024 * 1_024;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PolicyProxyCredentials {
    pub(crate) username: String,
    pub(crate) password: String,
}

impl PolicyProxyCredentials {
    pub(crate) fn from_proxy(proxy: &easytier_policy::Proxy) -> Option<Self> {
        proxy.credentials().map(|(username, password)| Self {
            username: username.to_owned(),
            password: password.to_owned(),
        })
    }

    pub(crate) fn from_wire(username: String, password: String) -> anyhow::Result<Option<Self>> {
        if username.is_empty() && password.is_empty() {
            return Ok(None);
        }
        if !valid_policy_proxy_credential(&username) || !valid_policy_proxy_credential(&password) {
            anyhow::bail!(
                "policy proxy username and password must both contain 1..=128 safe ASCII characters"
            );
        }
        Ok(Some(Self { username, password }))
    }

    fn authentication_request(&self) -> Vec<u8> {
        let username = self.username.as_bytes();
        let password = self.password.as_bytes();
        let mut request = Vec::with_capacity(3 + username.len() + password.len());
        request.extend_from_slice(&[1, username.len() as u8]);
        request.extend_from_slice(username);
        request.push(password.len() as u8);
        request.extend_from_slice(password);
        request
    }
}

fn valid_policy_proxy_credential(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b',' | b'=' | b'#' | b';'))
}

pub(crate) async fn negotiate_policy_proxy_auth<S>(
    stream: &mut S,
    credentials: Option<&PolicyProxyCredentials>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let expected_method = if credentials.is_some() { 2 } else { 0 };
    stream.write_all(&[5, 1, expected_method]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    if method != [5, expected_method] {
        anyhow::bail!("SOCKS server rejected the configured authentication method");
    }
    if let Some(credentials) = credentials {
        stream
            .write_all(&credentials.authentication_request())
            .await?;
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).await?;
        if reply != [1, 0] {
            anyhow::bail!("SOCKS username/password authentication failed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn wire_credentials_preserve_legacy_empty_fields_and_reject_partial_values() {
        assert!(
            PolicyProxyCredentials::from_wire(String::new(), String::new())
                .unwrap()
                .is_none()
        );
        assert!(PolicyProxyCredentials::from_wire("user".to_owned(), String::new()).is_err());
        assert!(
            PolicyProxyCredentials::from_wire("bad,name".to_owned(), "secret".to_owned()).is_err()
        );
    }

    #[tokio::test]
    async fn negotiates_legacy_no_authentication() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let server = tokio::spawn(async move {
            let mut greeting = [0u8; 3];
            server.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            server.write_all(&[5, 0]).await.unwrap();
        });
        negotiate_policy_proxy_auth(&mut client, None)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn negotiates_rfc1929_username_and_password() {
        let credentials = PolicyProxyCredentials {
            username: "alice".to_owned(),
            password: "secret".to_owned(),
        };
        let (mut client, mut server) = tokio::io::duplex(64);
        let server = tokio::spawn(async move {
            let mut greeting = [0u8; 3];
            server.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 1, 2]);
            server.write_all(&[5, 2]).await.unwrap();
            let mut authentication = [0u8; 14];
            server.read_exact(&mut authentication).await.unwrap();
            assert_eq!(authentication, *b"\x01\x05alice\x06secret");
            server.write_all(&[1, 0]).await.unwrap();
        });
        negotiate_policy_proxy_auth(&mut client, Some(&credentials))
            .await
            .unwrap();
        server.await.unwrap();
    }
}

pub(crate) fn tune_policy_udp_socket(socket: &UdpSocket) {
    let socket = socket2::SockRef::from(socket);
    if let Err(error) = socket.set_recv_buffer_size(POLICY_UDP_SOCKET_BUFFER_SIZE) {
        tracing::warn!(?error, "failed to enlarge policy UDP receive buffer");
    }
    if let Err(error) = socket.set_send_buffer_size(POLICY_UDP_SOCKET_BUFFER_SIZE) {
        tracing::warn!(?error, "failed to enlarge policy UDP send buffer");
    }
    let recv_buffer_size = socket.recv_buffer_size().unwrap_or_default();
    let send_buffer_size = socket.send_buffer_size().unwrap_or_default();
    tracing::debug!(
        recv_buffer_size,
        send_buffer_size,
        "configured policy UDP socket buffers"
    );
}

#[derive(Debug, Clone)]
pub struct PolicyProcessConfig {
    pub revision: std::sync::Arc<PolicyRevision>,
    pub source_label: String,
    pub base_dir: PathBuf,
    pub leaf_executable: PathBuf,
    pub outbound_interface: String,
    source_file: Option<PathBuf>,
}

impl PolicyProcessConfig {
    pub fn reload_revision_if_changed(
        &self,
        current_digest: &[u8; 32],
    ) -> anyhow::Result<Option<std::sync::Arc<PolicyRevision>>> {
        let Some(source_file) = self.source_file.as_deref() else {
            return Ok(None);
        };
        easytier_policy::reload_policy_file_if_changed_with_rule_set_provider(
            source_file,
            current_digest,
            |kind| builtin_rule_set_default(self.base_dir.as_path(), kind),
        )
        .with_context(|| format!("invalid policy config {}", source_file.display()))
        .map(|revision| revision.map(std::sync::Arc::new))
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPolicyDocument {
    pub revision: std::sync::Arc<PolicyRevision>,
    pub source_label: String,
    pub base_dir: PathBuf,
}

static POLICY_CONFIG: OnceLock<PolicyProcessConfig> = OnceLock::new();
static POLICY_INSTANCE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub struct PolicyInstanceLease;

impl Drop for PolicyInstanceLease {
    fn drop(&mut self) {
        POLICY_INSTANCE_ACTIVE.store(false, Ordering::Release);
    }
}

pub fn configure(
    policy_file: PathBuf,
    leaf_executable: PathBuf,
    outbound_interface: String,
) -> anyhow::Result<()> {
    let (revision, leaf_executable) =
        resolve_process_inputs(&policy_file, &leaf_executable, outbound_interface.as_str())?;
    let source_label = policy_file.display().to_string();
    let base_dir = policy_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    POLICY_CONFIG
        .set(PolicyProcessConfig {
            revision: std::sync::Arc::new(revision),
            source_label,
            base_dir,
            leaf_executable,
            outbound_interface,
            source_file: Some(policy_file),
        })
        .map_err(|_| anyhow::anyhow!("policy process config was initialized more than once"))
}

pub(crate) fn validate_process_config(
    policy_file: &Path,
    leaf_executable: &Path,
    outbound_interface: &str,
) -> anyhow::Result<()> {
    resolve_process_inputs(policy_file, leaf_executable, outbound_interface).map(|_| ())
}

fn resolve_process_inputs(
    policy_file: &Path,
    leaf_executable: &Path,
    outbound_interface: &str,
) -> anyhow::Result<(PolicyRevision, PathBuf)> {
    if outbound_interface.trim().is_empty() {
        anyhow::bail!("policy mode requires a non-empty outbound interface");
    }
    let leaf_executable = resolve_executable(leaf_executable)?;
    let revision = validate_policy_file(policy_file)
        .with_context(|| format!("invalid policy config {}", policy_file.display()))?;
    Ok((revision, leaf_executable))
}

fn resolve_executable(executable: &Path) -> anyhow::Result<PathBuf> {
    if executable.components().count() > 1 || executable.is_absolute() {
        require_regular_file(executable, "Leaf executable")?;
        return Ok(executable.to_owned());
    }
    if let Some(directory) = std::env::current_exe()?.parent() {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "Leaf executable {} was not found in PATH",
        executable.display()
    )
}

pub fn configured_for(config: &dyn ConfigLoader) -> anyhow::Result<Option<PolicyProcessConfig>> {
    if let Some(override_config) = POLICY_CONFIG.get() {
        return Ok(Some(override_config.clone()));
    }
    let Some(config) = config.get_policy_proxy_config() else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }
    resolve_instance_config(config).map(Some)
}

/// Returns whether this instance requested policy mode without resolving
/// platform-specific worker paths or loading the policy document.
///
/// Runtime components that only need to reserve a shared capability (such as
/// the policy-only KCP endpoint) use this check; full validation remains owned
/// by `configured_for` at policy startup.
pub fn is_configured_for(config: &dyn ConfigLoader) -> bool {
    POLICY_CONFIG.get().is_some()
        || config
            .get_policy_proxy_config()
            .is_some_and(|policy| policy.enabled)
}

fn resolve_instance_config(config: PolicyProxyConfig) -> anyhow::Result<PolicyProcessConfig> {
    config.validate_envelope()?;
    let source_file = config.resolved_config_file();
    let outbound_interface = config
        .outbound_interface
        .clone()
        .ok_or_else(|| anyhow::anyhow!("policy_proxy requires outbound_interface"))?;
    let leaf_executable = resolve_executable(
        config
            .leaf_executable
            .as_deref()
            .unwrap_or_else(|| Path::new("easytier-leaf-worker")),
    )?;
    let document = resolve_document(&config)?;
    Ok(PolicyProcessConfig {
        revision: document.revision,
        source_label: document.source_label,
        base_dir: document.base_dir,
        leaf_executable,
        outbound_interface,
        source_file,
    })
}

pub fn resolve_document(config: &PolicyProxyConfig) -> anyhow::Result<ResolvedPolicyDocument> {
    config.validate_envelope()?;
    let (revision, source_label, base_dir) =
        if let Some(policy_file) = config.resolved_config_file() {
            let revision = validate_policy_file(&policy_file)
                .with_context(|| format!("invalid policy config {}", policy_file.display()))?;
            let source_label = policy_file.display().to_string();
            let base_dir = policy_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            (revision, source_label, base_dir)
        } else {
            let source = config
                .config_inline
                .as_deref()
                .expect("validated policy config has one source");
            let base_dir = config
                .source_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("."));
            let revision =
                parse_policy_source(source, &base_dir).context("invalid inline policy config")?;
            (revision, "inline policy config".to_owned(), base_dir)
        };
    Ok(ResolvedPolicyDocument {
        revision: std::sync::Arc::new(revision),
        source_label,
        base_dir,
    })
}

fn validate_policy_file(path: &Path) -> anyhow::Result<PolicyRevision> {
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    easytier_policy::validate_policy_file_with_rule_set_provider(path, |kind| {
        builtin_rule_set_default(base_dir, kind)
    })
    .map_err(Into::into)
}

fn parse_policy_source(
    source: impl Into<std::sync::Arc<str>>,
    base_dir: &Path,
) -> anyhow::Result<PolicyRevision> {
    PolicyRevision::parse_with_rule_set_provider(source, base_dir, |kind| {
        builtin_rule_set_default(base_dir, kind)
    })
    .map_err(Into::into)
}

fn builtin_rule_set_default(
    base_dir: &Path,
    kind: easytier_policy::RuleSetKind,
) -> Result<Option<(String, easytier_policy::RuleSet)>, easytier_policy::PolicyError> {
    crate::policy_rule_data::builtin_rule_set_default(base_dir, kind).map_err(|error| {
        easytier_policy::PolicyError::InvalidRuleSet {
            name: format!("builtin-{kind:?}").to_ascii_lowercase(),
            reason: error.to_string(),
        }
    })
}

pub fn acquire_instance() -> anyhow::Result<PolicyInstanceLease> {
    POLICY_INSTANCE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| PolicyInstanceLease)
        .map_err(|_| {
            anyhow::anyhow!("policy mode currently supports one network instance per process")
        })
}

fn require_regular_file(path: &Path, name: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect {name} {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{name} is not a regular file: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use easytier_policy::RuleSetKind;

    #[test]
    fn resolves_inline_instance_config_without_persisting_generated_state() {
        let directory = tempfile::tempdir().unwrap();
        let worker = directory.path().join("leaf-worker");
        std::fs::write(&worker, b"worker").unwrap();
        let config = PolicyProxyConfig {
            enabled: true,
            config_inline: Some("version: 1\nrules: [\"FINAL,DIRECT\"]\n".to_owned()),
            outbound_interface: Some("eth0".to_owned()),
            leaf_executable: Some(worker.clone()),
            source_dir: Some(directory.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = resolve_instance_config(config).unwrap();
        assert_eq!(resolved.revision.document.rules, ["FINAL,DIRECT"]);
        assert_eq!(resolved.source_label, "inline policy config");
        assert_eq!(resolved.base_dir, directory.path());
        assert_eq!(resolved.leaf_executable, worker);
        assert!(
            resolved
                .reload_revision_if_changed(&resolved.revision.digest)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn resolves_policy_file_relative_to_network_config_directory() {
        let directory = tempfile::tempdir().unwrap();
        let policy_dir = directory.path().join("policy");
        std::fs::create_dir(&policy_dir).unwrap();
        std::fs::write(
            policy_dir.join("default.yaml"),
            "version: 1\nrules: [\"FINAL,DIRECT\"]\n",
        )
        .unwrap();
        let worker = directory.path().join("leaf-worker");
        std::fs::write(&worker, b"worker").unwrap();
        let config = PolicyProxyConfig {
            enabled: true,
            config_file: Some("policy/default.yaml".into()),
            outbound_interface: Some("eth0".to_owned()),
            leaf_executable: Some(worker),
            source_dir: Some(directory.path().to_path_buf()),
            ..Default::default()
        };

        let resolved = resolve_instance_config(config).unwrap();
        assert_eq!(resolved.base_dir, policy_dir);
        assert!(resolved.source_label.ends_with("policy/default.yaml"));

        std::fs::write(
            policy_dir.join("default.yaml"),
            "version: 1\nrules: [\"FINAL,REJECT\"]\n",
        )
        .unwrap();
        let reloaded = resolved
            .reload_revision_if_changed(&resolved.revision.digest)
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.document.rules, ["FINAL,REJECT"]);
    }

    #[test]
    fn materializes_only_the_builtin_rule_data_used_by_the_policy() {
        let directory = tempfile::tempdir().unwrap();
        let revision = parse_policy_source(
            "version: 1\nrules: [\"GEOSITE,CN,DIRECT\", \"MATCH,DIRECT\"]\n",
            directory.path(),
        )
        .unwrap();

        let rule_set = revision
            .document
            .rule_sets
            .values()
            .find(|rule_set| rule_set.kind == RuleSetKind::Geosite)
            .unwrap();
        assert!(rule_set.path.is_file());
        assert!(rule_set.sha256.is_some());
        assert!(
            !revision
                .document
                .rule_sets
                .values()
                .any(|rule_set| rule_set.kind == RuleSetKind::Geoip)
        );
        assert!(!rule_set.path.with_file_name("geoip-lite.dat").exists());
    }

    #[test]
    fn ordinary_and_explicit_geo_policies_do_not_materialize_unused_builtins() {
        let directory = tempfile::tempdir().unwrap();
        parse_policy_source("version: 1\nrules: [\"MATCH,DIRECT\"]\n", directory.path()).unwrap();
        assert!(!directory.path().join(".easytier-policy-rule-data").exists());

        let custom = directory.path().join("custom-geosite.dat");
        std::fs::write(&custom, b"custom").unwrap();
        let source = format!(
            "version: 1\nrule-sets:\n  custom:\n    type: geosite\n    path: {}\nrules: [\"GEOSITE,CN,DIRECT\", \"MATCH,DIRECT\"]\n",
            custom.display()
        );
        let revision = parse_policy_source(source, directory.path()).unwrap();

        assert_eq!(revision.document.rule_sets.len(), 1);
        assert_eq!(revision.document.rule_sets["custom"].path, custom);
        assert!(!directory.path().join(".easytier-policy-rule-data").exists());
    }

    #[test]
    fn bundled_geosite_and_geoip_compile_into_leaf_rules() {
        let directory = tempfile::tempdir().unwrap();
        let revision = parse_policy_source(
            "version: 1\nrules: [\"GEOSITE,CN,DIRECT\", \"GEOIP,CN,DIRECT,no-resolve\", \"MATCH,DIRECT\"]\n",
            directory.path(),
        )
        .unwrap();
        let resolver = |_name: &str,
                        _instance_id: Option<uuid::Uuid>,
                        _virtual_ip: Option<std::net::IpAddr>,
                        _port: Option<u16>| { None };

        let compiled = easytier_policy::compile_leaf_config(
            &revision,
            7,
            directory.path(),
            &resolver,
            &["1.1.1.1".parse().unwrap()],
        )
        .unwrap();
        let compiled: serde_json::Value = serde_json::from_str(&compiled).unwrap();
        let rules = compiled["router"]["rules"].as_array().unwrap();

        assert!(
            rules[0]["external"][0]
                .as_str()
                .unwrap()
                .contains("geosite.dat:CN")
        );
        assert!(
            rules[1]["ip"]
                .as_array()
                .is_some_and(|cidrs| !cidrs.is_empty())
        );
    }
}
