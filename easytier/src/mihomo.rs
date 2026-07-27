//! Process-neutral Mihomo runtime overlay and Core-owned supervisor.
//!
//! This module never interprets or rewrites proxies, providers, groups,
//! subscriptions, or `dialer-proxy`. It owns only a generated runtime copy,
//! the policy TUN envelope, anti-loop exclusions/rules, a private controller,
//! and the Mihomo child process.
//!
//! Reference semantics were checked against the maintainer's Mihomo checkout
//! at `0a87b94845ef908c15f8495871e4cd8e33116328`:
//! - `config/config.go::{RawTun,RawConfig}` defines the overlaid YAML fields.
//! - `rules/parser.go::ParseRule` accepts PROCESS-NAME, PROCESS-NAME-REGEX,
//!   and PROCESS-NAME-WILDCARD.
//! - `hub/route/server.go::router` puts `/version` and `/configs/` behind
//!   Bearer authentication for the TCP controller.
//! - `hub/route/configs.go::getConfigs` returns the active `tun.enable` and
//!   `tun.device` values used by the readiness check.
//! - `hub/route/server.go::{startUnix,startPipe}` deliberately constructs its
//!   handler with an empty secret. Unix safety therefore comes from a socket
//!   inside EasyTier's mode-0700 runtime directory. Core uses authenticated
//!   loopback TCP instead of the unauthenticated named-pipe controller on
//!   Windows.
//! - `main.go` accepts `-f` and validates with `-t`.
//! - `constant/path.go::{SetHomeDir,path.Resolve}` resolves relative paths
//!   against `-d`. The generated YAML may live in the private runtime
//!   directory, but both validation and execution retain the source config
//!   directory as Mihomo home. Mihomo may consequently create its documented
//!   cache/database files there; EasyTier never changes the source YAML.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock, mpsc as std_mpsc},
    thread,
    time::{Duration, SystemTime},
};

use anyhow::{Context as _, ensure};
use rand::{RngCore as _, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    process::Command,
    sync::{RwLock, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::managed_child::{ManagedChild, configure_command};

use std::net::{Ipv4Addr, TcpListener};

const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CONTROLLER_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_VALIDATION_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_TAILSCALE_ROUTES: &[&str] = &["100.64.0.0/10", "fd7a:115c:a1e0::/48"];
const DEFAULT_AUTOGEN_CONFIG: &str = "mode: rule\nrules:\n  - MATCH,DIRECT\n";

const RESERVED_PROCESS_RULES: &[&str] = &[
    "PROCESS-NAME,io.tailscale.ipn.macsys.network-extension,DIRECT",
    "PROCESS-NAME,tailscaled,DIRECT",
    "PROCESS-NAME,tailscaled.exe,DIRECT",
    "PROCESS-NAME,tailscale,DIRECT",
    "PROCESS-NAME,tailscale.exe,DIRECT",
    "PROCESS-NAME,easytier-gui,DIRECT",
    "PROCESS-NAME,easytier-gui.exe,DIRECT",
    "PROCESS-NAME,easytier-core,DIRECT",
    "PROCESS-NAME,easytier-core.exe,DIRECT",
    "PROCESS-NAME,easytier-cli,DIRECT",
    "PROCESS-NAME,easytier-cli.exe,DIRECT",
    "PROCESS-NAME,easytier-hev-socks-egress,DIRECT",
    "PROCESS-NAME,easytier-hev-socks-egress.exe,DIRECT",
    "PROCESS-NAME,easytier-gost,DIRECT",
    "PROCESS-NAME,easytier-gost.exe,DIRECT",
    "PROCESS-NAME,easytier-leaf-worker,DIRECT",
    "PROCESS-NAME,easytier-leaf-worker.exe,DIRECT",
    "PROCESS-NAME-REGEX,(?i)^easytier(?:[-_.].*)?$,DIRECT",
    "PROCESS-NAME-WILDCARD,easytier-*,DIRECT",
];

const PROTECTED_PROXY_KEYS: &[&str] = &[
    "proxies",
    "proxy-providers",
    "rule-providers",
    "proxy-groups",
    "listeners",
];

const CONTROLLER_KEYS: &[&str] = &[
    "external-controller",
    "external-controller-tls",
    "external-controller-unix",
    "external-controller-pipe",
];

#[derive(Clone)]
struct ControllerSecret(Arc<str>);

impl ControllerSecret {
    fn random() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use fmt::Write as _;
            let _ = write!(encoded, "{byte:02x}");
        }
        Self(encoded.into())
    }

    #[cfg(test)]
    fn test_only(value: &str) -> Self {
        Self(Arc::from(value))
    }

    fn configured(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    fn expose_to_child(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ControllerSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControllerSecret([REDACTED])")
    }
}

#[derive(Clone)]
pub enum MihomoConfigSource {
    File(PathBuf),
    Inline { label: String, contents: Arc<str> },
}

impl fmt::Debug for MihomoConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => formatter.debug_tuple("File").field(path).finish(),
            Self::Inline { label, contents } => formatter
                .debug_struct("Inline")
                .field("label", label)
                .field("bytes", &contents.len())
                .field("contents", &"[REDACTED]")
                .finish(),
        }
    }
}

#[derive(Clone)]
pub struct LoadedMihomoConfig {
    pub label: String,
    contents: Arc<str>,
}

impl fmt::Debug for LoadedMihomoConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedMihomoConfig")
            .field("label", &self.label)
            .field("bytes", &self.contents.len())
            .field("contents", &"[REDACTED]")
            .finish()
    }
}

impl LoadedMihomoConfig {
    pub fn as_str(&self) -> &str {
        &self.contents
    }
}

impl MihomoConfigSource {
    pub fn load(&self) -> anyhow::Result<LoadedMihomoConfig> {
        match self {
            Self::Inline { label, contents } => {
                ensure!(
                    contents.len() as u64 <= MAX_SOURCE_BYTES,
                    "Mihomo inline config exceeds {MAX_SOURCE_BYTES} bytes"
                );
                Ok(LoadedMihomoConfig {
                    label: label.clone(),
                    contents: contents.clone(),
                })
            }
            Self::File(path) => {
                let metadata = fs::symlink_metadata(path).with_context(|| {
                    format!("failed to inspect Mihomo config {}", path.display())
                })?;
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "refusing symlink Mihomo source config {}",
                    path.display()
                );
                ensure!(
                    metadata.is_file(),
                    "Mihomo source config is not a regular file: {}",
                    path.display()
                );
                ensure!(
                    metadata.len() <= MAX_SOURCE_BYTES,
                    "Mihomo source config exceeds {MAX_SOURCE_BYTES} bytes"
                );
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("failed to read Mihomo config {}", path.display()))?;
                Ok(LoadedMihomoConfig {
                    label: path.display().to_string(),
                    contents: contents.into(),
                })
            }
        }
    }

    fn conflicts_with_runtime_path(&self, runtime_path: &Path) -> bool {
        matches!(self, Self::File(path) if path == runtime_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MihomoPrivateController {
    UnixSocket(PathBuf),
    Loopback(SocketAddr),
}

#[derive(Clone)]
pub struct MihomoOverlay {
    pub tun_device: String,
    pub route_exclude_addresses: Vec<String>,
    pub controller: MihomoPrivateController,
    controller_secret: ControllerSecret,
    controller_secret_override: Option<Arc<str>>,
}

impl fmt::Debug for MihomoOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MihomoOverlay")
            .field("tun_device", &self.tun_device)
            .field("route_exclude_addresses", &self.route_exclude_addresses)
            .field("controller", &self.controller)
            .field("controller_secret", &self.controller_secret)
            .finish()
    }
}

impl MihomoOverlay {
    pub fn new(
        tun_device: String,
        route_exclude_addresses: Vec<String>,
        controller: MihomoPrivateController,
        controller_secret_override: Option<String>,
    ) -> Self {
        Self {
            tun_device,
            route_exclude_addresses,
            controller,
            controller_secret: ControllerSecret::random(),
            controller_secret_override: controller_secret_override.map(Arc::from),
        }
    }

    #[cfg(test)]
    fn test_only(
        tun_device: String,
        route_exclude_addresses: Vec<String>,
        controller: MihomoPrivateController,
    ) -> Self {
        Self {
            tun_device,
            route_exclude_addresses,
            controller,
            controller_secret: ControllerSecret::test_only("test-only-secret"),
            controller_secret_override: None,
        }
    }

    fn resolve_controller_secret(&mut self, source: &LoadedMihomoConfig) -> anyhow::Result<()> {
        let document: Value = serde_yaml::from_str(source.as_str())
            .with_context(|| format!("failed to parse Mihomo source {}", source.label))?;
        let root = document
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Mihomo config root must be a YAML mapping"))?;
        if let Some(secret) = self.controller_secret_override.clone() {
            self.controller_secret = ControllerSecret::configured(secret);
            return Ok(());
        }
        if let Some(secret) = root.get(yaml_key("secret")) {
            let secret = secret
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Mihomo secret must be a string"))?;
            self.controller_secret = ControllerSecret::configured(Arc::<str>::from(secret));
        }
        Ok(())
    }

    fn dashboard_url(&self) -> anyhow::Result<String> {
        let MihomoPrivateController::Loopback(address) = self.controller else {
            anyhow::bail!("Mihomo dashboard requires a loopback TCP controller");
        };
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("hostname", "127.0.0.1")
            .append_pair("port", &address.port().to_string())
            .append_pair("secret", self.controller_secret.expose_to_child())
            .append_pair("http", "true")
            .append_pair("disableUpgradeCore", "1")
            .append_pair("disableTunMode", "1")
            .finish();
        Ok(format!("https://board.zash.run.place/#/setup?{query}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MihomoOverlayReport {
    pub changed_fields: Vec<String>,
    pub removed_controller_fields: Vec<String>,
    pub inserted_process_rules: usize,
    pub inserted_address_rules: usize,
}

#[derive(Debug, Clone)]
pub struct CompiledMihomoConfig {
    pub yaml: String,
    pub report: MihomoOverlayReport,
}

fn yaml_key(key: &str) -> Value {
    Value::String(key.to_owned())
}

fn root_mapping(document: &mut Value) -> anyhow::Result<&mut Mapping> {
    document
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Mihomo config root must be a YAML mapping"))
}

fn protected_snapshot(root: &Mapping) -> Vec<(&'static str, Option<Value>)> {
    PROTECTED_PROXY_KEYS
        .iter()
        .map(|key| (*key, root.get(yaml_key(key)).cloned()))
        .collect()
}

fn ensure_protected_unchanged(
    root: &Mapping,
    before: &[(&'static str, Option<Value>)],
) -> anyhow::Result<()> {
    for (key, expected) in before {
        ensure!(
            root.get(yaml_key(key)).cloned() == *expected,
            "internal error: Mihomo overlay changed protected field {key}"
        );
    }
    Ok(())
}

fn canonical_exclusions(overlay: &MihomoOverlay) -> anyhow::Result<Vec<String>> {
    let mut routes = BTreeSet::new();
    for address in overlay
        .route_exclude_addresses
        .iter()
        .map(String::as_str)
        .chain(DEFAULT_TAILSCALE_ROUTES.iter().copied())
    {
        let cidr = address
            .parse::<cidr::IpCidr>()
            .with_context(|| format!("invalid EasyTier route exclusion {address:?}"))?;
        routes.insert(cidr.to_string());
    }
    Ok(routes.into_iter().collect())
}

fn apply_tun_overlay(
    root: &mut Mapping,
    overlay: &MihomoOverlay,
    report: &mut MihomoOverlayReport,
) -> anyhow::Result<Vec<String>> {
    ensure!(
        !overlay.tun_device.trim().is_empty(),
        "Mihomo TUN device cannot be empty"
    );
    let requested_exclusions = canonical_exclusions(overlay)?;
    let tun = root
        .entry(yaml_key("tun"))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Mihomo tun must be a YAML mapping"))?;

    tun.insert(yaml_key("enable"), Value::Bool(true));
    tun.insert(yaml_key("auto-route"), Value::Bool(true));
    tun.insert(
        yaml_key("device"),
        Value::String(overlay.tun_device.clone()),
    );
    report.changed_fields.extend([
        "tun.enable".to_owned(),
        "tun.auto-route".to_owned(),
        "tun.device".to_owned(),
    ]);

    let excludes = tun
        .entry(yaml_key("route-exclude-address"))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("Mihomo tun.route-exclude-address must be a YAML sequence")
        })?;
    let mut seen = BTreeSet::new();
    for value in excludes.iter() {
        let address = value.as_str().ok_or_else(|| {
            anyhow::anyhow!("Mihomo tun.route-exclude-address entries must be strings")
        })?;
        let canonical = address
            .parse::<cidr::IpCidr>()
            .with_context(|| format!("invalid existing route-exclude-address {address:?}"))?
            .to_string();
        seen.insert(canonical);
    }
    for address in &requested_exclusions {
        if seen.insert(address.clone()) {
            excludes.push(Value::String(address.clone()));
        }
    }
    report
        .changed_fields
        .push("tun.route-exclude-address".to_owned());
    Ok(requested_exclusions)
}

fn address_rule(address: &str) -> anyhow::Result<String> {
    let cidr = address.parse::<cidr::IpCidr>()?;
    let kind = if matches!(cidr, cidr::IpCidr::V4(_)) {
        "IP-CIDR"
    } else {
        "IP-CIDR6"
    };
    Ok(format!("{kind},{cidr},DIRECT,no-resolve"))
}

fn apply_reserved_rules(
    root: &mut Mapping,
    exclusions: &[String],
    report: &mut MihomoOverlayReport,
) -> anyhow::Result<()> {
    let rules = root
        .entry(yaml_key("rules"))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| anyhow::anyhow!("Mihomo rules must be a YAML sequence"))?;

    let existing: BTreeSet<String> = rules
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let mut generated = Vec::new();
    for rule in RESERVED_PROCESS_RULES {
        if !existing.contains(*rule) {
            generated.push(Value::String((*rule).to_owned()));
            report.inserted_process_rules += 1;
        }
    }
    for exclusion in exclusions {
        let rule = address_rule(exclusion)?;
        if !existing.contains(&rule)
            && !generated
                .iter()
                .any(|value| value.as_str() == Some(rule.as_str()))
        {
            generated.push(Value::String(rule));
            report.inserted_address_rules += 1;
        }
    }
    generated.append(rules);
    *rules = generated;
    Ok(())
}

fn apply_private_controller(
    root: &mut Mapping,
    overlay: &MihomoOverlay,
    report: &mut MihomoOverlayReport,
) -> anyhow::Result<()> {
    for field in CONTROLLER_KEYS {
        if root.remove(yaml_key(field)).is_some() {
            report.removed_controller_fields.push((*field).to_owned());
        }
    }

    match &overlay.controller {
        MihomoPrivateController::UnixSocket(path) => {
            ensure!(
                path.is_absolute(),
                "Mihomo controller Unix socket path must be absolute"
            );
            root.insert(
                yaml_key("external-controller-unix"),
                Value::String(path.to_string_lossy().into_owned()),
            );
            report
                .changed_fields
                .push("external-controller-unix".to_owned());
        }
        MihomoPrivateController::Loopback(address) => {
            ensure!(
                address.ip().is_loopback(),
                "Mihomo TCP controller must be loopback-only"
            );
            root.insert(
                yaml_key("external-controller"),
                Value::String(address.to_string()),
            );
            report.changed_fields.push("external-controller".to_owned());

            // Mihomo's controller middleware requires the exact web origin and
            // Private Network Access opt-in for an HTTPS dashboard to reach a
            // loopback controller. Only the generated runtime copy is changed.
            let mut cors = Mapping::new();
            cors.insert(
                yaml_key("allow-origins"),
                Value::Sequence(vec![Value::String(
                    "https://board.zash.run.place".to_owned(),
                )]),
            );
            cors.insert(yaml_key("allow-private-network"), Value::Bool(true));
            root.insert(yaml_key("external-controller-cors"), Value::Mapping(cors));
            report
                .changed_fields
                .push("external-controller-cors".to_owned());
        }
    }
    root.insert(
        yaml_key("secret"),
        Value::String(overlay.controller_secret.expose_to_child().to_owned()),
    );
    report.changed_fields.push("secret".to_owned());
    Ok(())
}

pub fn compile_runtime_config(
    source: &LoadedMihomoConfig,
    overlay: &MihomoOverlay,
) -> anyhow::Result<CompiledMihomoConfig> {
    let mut document: Value = serde_yaml::from_str(source.as_str())
        .with_context(|| format!("failed to parse Mihomo source {}", source.label))?;
    let root = root_mapping(&mut document)?;
    let protected = protected_snapshot(root);
    let mut report = MihomoOverlayReport {
        changed_fields: Vec::new(),
        removed_controller_fields: Vec::new(),
        inserted_process_rules: 0,
        inserted_address_rules: 0,
    };

    let exclusions = apply_tun_overlay(root, overlay, &mut report)?;
    apply_reserved_rules(root, &exclusions, &mut report)?;
    apply_private_controller(root, overlay, &mut report)?;
    ensure_protected_unchanged(root, &protected)?;

    let yaml =
        serde_yaml::to_string(&document).context("failed to serialize Mihomo runtime copy")?;
    Ok(CompiledMihomoConfig { yaml, report })
}

fn random_suffix() -> String {
    let mut bytes = [0_u8; 8];
    OsRng.fill_bytes(&mut bytes);
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn cleanup_runtime_temps(path: &Path) -> anyhow::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!(".{file_name}.tmp-");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn publish_runtime_copy(temporary_path: &Path, path: &Path) -> anyhow::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(temporary_path, path)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        if !path.exists() {
            fs::rename(temporary_path, path)?;
            return Ok(());
        }
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "refusing to replace non-regular Mihomo runtime config {}",
            path.display()
        );
        let backup = path.with_extension(format!("bak-{}", random_suffix()));
        fs::rename(path, &backup)?;
        match fs::rename(temporary_path, path) {
            Ok(()) => {
                let _ = fs::remove_file(backup);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&backup, path);
                Err(error.into())
            }
        }
    }
}

fn write_runtime_copy(path: &Path, yaml: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Mihomo runtime config has no parent directory"))?;
    ensure!(
        parent.is_dir(),
        "Mihomo runtime config parent does not exist: {}",
        parent.display()
    );
    cleanup_runtime_temps(path)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Mihomo runtime config has no file name"))?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(format!(".tmp-{}", random_suffix()));
    let temporary_path = parent.join(temporary_name);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary_path).with_context(|| {
        format!(
            "failed to create Mihomo runtime config {}",
            temporary_path.display()
        )
    })?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(yaml.as_bytes())?;
        file.sync_all()?;
        drop(file);
        publish_runtime_copy(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.with_context(|| format!("failed to publish Mihomo runtime config {}", path.display()))
}

pub fn load_user_config(path: &Path) -> anyhow::Result<String> {
    Ok(MihomoConfigSource::File(path.to_owned())
        .load()?
        .contents
        .to_string())
}

pub fn save_user_config(path: &Path, contents: &str) -> anyhow::Result<()> {
    ensure!(
        contents.len() as u64 <= MAX_SOURCE_BYTES,
        "Mihomo config exceeds {MAX_SOURCE_BYTES} bytes"
    );
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "refusing non-regular Mihomo source config {}",
            path.display()
        );
    }
    write_runtime_copy(path, contents)
}

pub fn materialize_user_config(
    config_dir: &Path,
    instance_id: uuid::Uuid,
    inline: Option<&str>,
) -> anyhow::Result<(PathBuf, String, bool)> {
    let directory = config_dir
        .join("mihomo")
        .join(instance_id.simple().to_string());
    if !directory.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
    }
    let path = directory.join("autogen.yaml");
    if path.exists() {
        let contents = load_user_config(&path)?;
        return Ok((path, contents, false));
    }
    let contents = inline
        .filter(|contents| !contents.trim().is_empty())
        .unwrap_or(DEFAULT_AUTOGEN_CONFIG)
        .to_owned();
    save_user_config(&path, &contents)?;
    Ok((path, contents, true))
}

#[derive(Debug, Clone)]
pub struct MihomoRestartPolicy {
    pub max_restarts: u32,
    pub initial_backoff: Duration,
    pub maximum_backoff: Duration,
    pub validation_timeout: Duration,
    pub readiness_timeout: Duration,
    pub readiness_interval: Duration,
    pub stop_timeout: Duration,
    pub stable_uptime: Duration,
}

impl Default for MihomoRestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            initial_backoff: Duration::from_secs(1),
            maximum_backoff: Duration::from_secs(30),
            // Mihomo `-t` runs executor.Parse(), which may fetch missing Geo data.
            // Its per-file download timeout is 90s, so 20s rejects valid first starts.
            validation_timeout: Duration::from_secs(120),
            readiness_timeout: Duration::from_secs(20),
            readiness_interval: Duration::from_millis(200),
            stop_timeout: Duration::from_secs(5),
            stable_uptime: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MihomoSupervisorConfig {
    pub executable: PathBuf,
    pub source: MihomoConfigSource,
    pub home_dir: PathBuf,
    pub runtime_config: PathBuf,
    pub overlay: MihomoOverlay,
    pub restart: MihomoRestartPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MihomoProcessState {
    Stopped,
    Validating,
    Starting,
    Running,
    Backoff,
    Stopping,
    Failed,
}

impl MihomoProcessState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Validating => "validating",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Backoff => "backoff",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MihomoStatus {
    pub state: MihomoProcessState,
    pub pid: Option<u32>,
    pub restart_count: u32,
    pub started_at: Option<SystemTime>,
    pub last_exit: Option<String>,
    pub last_error: Option<String>,
    pub overlay_report: Option<MihomoOverlayReport>,
}

impl Default for MihomoStatus {
    fn default() -> Self {
        Self {
            state: MihomoProcessState::Stopped,
            pid: None,
            restart_count: 0,
            started_at: None,
            last_exit: None,
            last_error: None,
            overlay_report: None,
        }
    }
}

pub struct MihomoSupervisor {
    config: MihomoSupervisorConfig,
    status: Arc<RwLock<MihomoStatus>>,
    cancel: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

impl MihomoSupervisor {
    pub fn new(config: MihomoSupervisorConfig) -> Self {
        Self {
            config,
            status: Arc::new(RwLock::new(MihomoStatus::default())),
            cancel: None,
            task: None,
        }
    }

    pub async fn status(&self) -> MihomoStatus {
        self.status.read().await.clone()
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        ensure!(self.task.is_none(), "Mihomo supervisor is already started");
        ensure!(
            !self
                .config
                .source
                .conflicts_with_runtime_path(&self.config.runtime_config),
            "Mihomo source and runtime config paths must differ"
        );
        validate_executable(&self.config.executable)?;
        remove_stale_controller(&self.config.overlay.controller)?;

        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Validating;
            status.last_error = None;
        }
        let source = self.config.source.load()?;
        self.config.overlay.resolve_controller_secret(&source)?;
        let compiled = compile_runtime_config(&source, &self.config.overlay)?;
        write_runtime_copy(&self.config.runtime_config, &compiled.yaml)?;
        validate_runtime_config(&self.config).await?;

        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Starting;
            status.overlay_report = Some(compiled.report);
        }
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel();
        self.task = Some(tokio::spawn(run_supervisor(
            self.config.clone(),
            self.status.clone(),
            cancel.clone(),
            ready_tx,
        )));
        self.cancel = Some(cancel);

        match ready_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.cancel = None;
                if let Some(task) = self.task.take() {
                    let _ = task.await;
                }
                anyhow::bail!(error)
            }
            Err(_) => {
                self.cancel = None;
                self.task = None;
                anyhow::bail!("Mihomo supervisor exited before reporting readiness")
            }
        }
    }

    pub async fn stop(&mut self) {
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Stopping;
        }
        cancel.cancel();
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(
                self.config.restart.stop_timeout + Duration::from_secs(1),
                &mut task,
            )
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Stopped;
            status.pid = None;
            status.last_error = Some("Mihomo supervisor stop timed out".to_owned());
        }
        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Stopped;
            status.pid = None;
        }
        remove_stale_controller(&self.config.overlay.controller).ok();
    }
}

impl Drop for MihomoSupervisor {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn validate_executable(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect Mihomo executable {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "refusing symlink Mihomo executable {}",
        path.display()
    );
    ensure!(
        metadata.is_file(),
        "Mihomo executable is not a regular file: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        ensure!(
            metadata.mode() & 0o022 == 0,
            "Mihomo executable must not be group/world writable: {}",
            path.display()
        );
    }
    Ok(())
}

fn remove_stale_controller(controller: &MihomoPrivateController) -> anyhow::Result<()> {
    if let MihomoPrivateController::UnixSocket(path) = controller {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "refusing symlink Mihomo controller path {}",
                    path.display()
                );
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn validate_runtime_config(config: &MihomoSupervisorConfig) -> anyhow::Result<()> {
    ensure!(
        config.home_dir.is_dir(),
        "Mihomo source home does not exist: {}",
        config.home_dir.display()
    );
    let mut command = Command::new(&config.executable);
    command
        .args(mihomo_runtime_args(config, true))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    configure_command(&mut command);
    let child = command.spawn().with_context(|| {
        format!(
            "failed to execute Mihomo validator {}",
            config.executable.display()
        )
    })?;
    let mut child = ManagedChild::attach(child, &config.executable).await?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .context("Mihomo validator stderr pipe is unavailable")?;
    let diagnostics = tokio::spawn(read_bounded_diagnostics(stderr));
    let status = match tokio::time::timeout(config.restart.validation_timeout, child.wait()).await {
        Ok(status) => status.context("failed waiting for Mihomo config validation")?,
        Err(_) => {
            child.terminate(config.restart.stop_timeout).await;
            let _ = diagnostics.await;
            anyhow::bail!("Mihomo config validation timed out");
        }
    };
    let stderr = diagnostics
        .await
        .context("Mihomo validator diagnostic task failed")??;
    ensure!(
        status.success(),
        "Mihomo rejected generated config: {}",
        String::from_utf8_lossy(&stderr).trim()
    );
    Ok(())
}

async fn read_bounded_diagnostics(mut reader: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(output);
        }
        let retained = read.min(MAX_VALIDATION_DIAGNOSTIC_BYTES.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..retained]);
    }
}

async fn spawn_mihomo(config: &MihomoSupervisorConfig) -> anyhow::Result<ManagedChild> {
    remove_stale_controller(&config.overlay.controller)?;
    let mut command = Command::new(&config.executable);
    command
        .args(mihomo_runtime_args(config, false))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_command(&mut command);
    let child = command
        .spawn()
        .with_context(|| format!("failed to start Mihomo {}", config.executable.display()))?;
    ManagedChild::attach(child, &config.executable).await
}

fn mihomo_runtime_args(config: &MihomoSupervisorConfig, validate: bool) -> Vec<OsString> {
    let mut arguments = Vec::with_capacity(5);
    if validate {
        arguments.push(OsString::from("-t"));
    }
    arguments.push(OsString::from("-d"));
    arguments.push(config.home_dir.as_os_str().to_owned());
    arguments.push(OsString::from("-f"));
    arguments.push(config.runtime_config.as_os_str().to_owned());
    arguments
}

async fn stop_child(child: &mut ManagedChild, timeout: Duration) {
    child.terminate(timeout).await;
}

async fn controller_stream(
    controller: &MihomoPrivateController,
) -> anyhow::Result<Box<dyn AsyncReadWrite>> {
    match controller {
        MihomoPrivateController::Loopback(address) => {
            let stream = tokio::net::TcpStream::connect(address).await?;
            Ok(Box::new(stream))
        }
        MihomoPrivateController::UnixSocket(path) => {
            #[cfg(unix)]
            {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok(Box::new(stream))
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                anyhow::bail!("Unix Mihomo controller is unsupported on this platform")
            }
        }
    }
}

trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

async fn controller_get(config: &MihomoSupervisorConfig, path: &str) -> anyhow::Result<Vec<u8>> {
    let mut stream = controller_stream(&config.overlay.controller).await?;
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        config.overlay.controller_secret.expose_to_child()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.shutdown().await?;
    let mut response = Vec::new();
    stream
        .take(MAX_CONTROLLER_RESPONSE_BYTES)
        .read_to_end(&mut response)
        .await?;
    parse_controller_response(&response)
}

fn parse_controller_response(response: &[u8]) -> anyhow::Result<Vec<u8>> {
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("invalid Mihomo controller HTTP response"))?;
    let headers = std::str::from_utf8(&response[..separator])?;
    ensure!(
        headers
            .lines()
            .next()
            .is_some_and(|line| line.contains(" 200 ")),
        "Mihomo controller returned non-200 response"
    );
    Ok(response[separator + 4..].to_vec())
}

fn is_numbered_macos_utun(device: &str) -> bool {
    device.strip_prefix("utun").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn active_tun_device_matches(actual_device: &str, expected_device: &str, is_macos: bool) -> bool {
    if actual_device == expected_device {
        return true;
    }
    if !is_macos || is_numbered_macos_utun(expected_device) {
        return false;
    }

    // Mihomo listener/sing_tun/server.go::{checkTunName,New} rejects a
    // non-numbered macOS TUN name and replaces it with an available `utunN`.
    // Accept only that platform-owned rename; explicit utun names and every
    // other platform retain strict device identity.
    is_numbered_macos_utun(actual_device)
}

fn validate_active_tun(response: &[u8], expected_device: &str) -> anyhow::Result<()> {
    let active: serde_json::Value = serde_json::from_slice(response)?;
    ensure!(
        active
            .pointer("/tun/enable")
            .and_then(|value| value.as_bool())
            == Some(true),
        "Mihomo controller reports TUN disabled"
    );
    let actual_device = active
        .pointer("/tun/device")
        .and_then(|value| value.as_str())
        .context("Mihomo controller response is missing the active TUN device")?;
    ensure!(
        active_tun_device_matches(actual_device, expected_device, cfg!(target_os = "macos")),
        "Mihomo controller reports an unexpected TUN device"
    );
    Ok(())
}

async fn probe_readiness(config: &MihomoSupervisorConfig) -> anyhow::Result<()> {
    let version = controller_get(config, "/version").await?;
    let version: serde_json::Value = serde_json::from_slice(&version)?;
    ensure!(
        version
            .get("version")
            .and_then(|value| value.as_str())
            .is_some(),
        "Mihomo controller version response is missing version"
    );

    let active = controller_get(config, "/configs/").await?;
    validate_active_tun(&active, &config.overlay.tun_device)
}

async fn probe_readiness_until(
    config: &MihomoSupervisorConfig,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> anyhow::Result<bool> {
    tokio::select! {
        _ = cancel.cancelled() => anyhow::bail!("Mihomo start cancelled"),
        result = tokio::time::timeout_at(deadline, probe_readiness(config)) => {
            match result {
                Ok(Ok(())) => Ok(true),
                Ok(Err(_)) => Ok(false),
                Err(_) => anyhow::bail!("Mihomo private controller/TUN readiness timed out"),
            }
        }
    }
}

async fn wait_for_readiness(
    child: &mut ManagedChild,
    config: &MihomoSupervisorConfig,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + config.restart.readiness_timeout;
    loop {
        if let Some(exit) = child.try_wait()? {
            anyhow::bail!("Mihomo exited before readiness: {exit}");
        }
        if probe_readiness_until(config, cancel, deadline).await? {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Mihomo private controller/TUN readiness timed out"
        );
        tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("Mihomo start cancelled"),
            _ = tokio::time::sleep(config.restart.readiness_interval) => {}
        }
    }
}

async fn run_supervisor(
    config: MihomoSupervisorConfig,
    status: Arc<RwLock<MihomoStatus>>,
    cancel: CancellationToken,
    ready: oneshot::Sender<Result<(), String>>,
) {
    let mut ready = Some(ready);
    let mut consecutive_failures = 0_u32;
    let mut total_restarts = 0_u32;

    loop {
        {
            let mut current = status.write().await;
            current.state = MihomoProcessState::Starting;
            current.pid = None;
        }
        let mut child = match spawn_mihomo(&config).await {
            Ok(child) => child,
            Err(error) => {
                if fail_or_retry(
                    &config,
                    &status,
                    &cancel,
                    &mut ready,
                    &mut consecutive_failures,
                    &mut total_restarts,
                    format!("{error:#}"),
                )
                .await
                {
                    return;
                }
                continue;
            }
        };
        {
            let mut current = status.write().await;
            current.pid = child.id();
        }
        if let Err(error) = wait_for_readiness(&mut child, &config, &cancel).await {
            stop_child(&mut child, config.restart.stop_timeout).await;
            if cancel.is_cancelled() {
                let mut current = status.write().await;
                current.state = MihomoProcessState::Stopped;
                current.pid = None;
                return;
            }
            if fail_or_retry(
                &config,
                &status,
                &cancel,
                &mut ready,
                &mut consecutive_failures,
                &mut total_restarts,
                format!("{error:#}"),
            )
            .await
            {
                return;
            }
            continue;
        }

        {
            let mut current = status.write().await;
            current.state = MihomoProcessState::Running;
            current.started_at = Some(SystemTime::now());
            current.restart_count = total_restarts;
            current.last_error = None;
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }

        let running_since = tokio::time::Instant::now();
        let exit = tokio::select! {
            _ = cancel.cancelled() => {
                stop_child(&mut child, config.restart.stop_timeout).await;
                let mut current = status.write().await;
                current.state = MihomoProcessState::Stopped;
                current.pid = None;
                return;
            }
            result = child.wait() => result,
        };
        let exit_description = match exit {
            Ok(exit) => exit.to_string(),
            Err(error) => format!("wait failed: {error}"),
        };
        {
            let mut current = status.write().await;
            current.pid = None;
            current.last_exit = Some(exit_description.clone());
        }
        if running_since.elapsed() >= config.restart.stable_uptime {
            consecutive_failures = 0;
        }
        if fail_or_retry(
            &config,
            &status,
            &cancel,
            &mut ready,
            &mut consecutive_failures,
            &mut total_restarts,
            format!("Mihomo exited unexpectedly: {exit_description}"),
        )
        .await
        {
            return;
        }
    }
}

async fn fail_or_retry(
    config: &MihomoSupervisorConfig,
    status: &Arc<RwLock<MihomoStatus>>,
    cancel: &CancellationToken,
    ready: &mut Option<oneshot::Sender<Result<(), String>>>,
    consecutive_failures: &mut u32,
    total_restarts: &mut u32,
    error: String,
) -> bool {
    *consecutive_failures = consecutive_failures.saturating_add(1);
    *total_restarts = total_restarts.saturating_add(1);
    {
        let mut current = status.write().await;
        current.pid = None;
        current.restart_count = *total_restarts;
        current.last_error = Some(error.clone());
    }
    if *consecutive_failures > config.restart.max_restarts {
        let mut current = status.write().await;
        current.state = MihomoProcessState::Failed;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(error));
        }
        return true;
    }

    let factor = 1_u32 << (*consecutive_failures).saturating_sub(1).min(16);
    let backoff = config
        .restart
        .initial_backoff
        .saturating_mul(factor)
        .min(config.restart.maximum_backoff);
    {
        let mut current = status.write().await;
        current.state = MihomoProcessState::Backoff;
    }
    tokio::select! {
        _ = cancel.cancelled() => {
            let mut current = status.write().await;
            current.state = MihomoProcessState::Stopped;
            current.pid = None;
            true
        },
        _ = tokio::time::sleep(backoff) => false,
    }
}

struct OwnedRuntimeDirectory {
    path: PathBuf,
}

impl OwnedRuntimeDirectory {
    fn create(_instance_id: uuid::Uuid) -> anyhow::Result<Self> {
        // Keep generated runtime configuration and health state in a short,
        // private directory that is removed with the owned Mihomo process.
        #[cfg(unix)]
        let base = PathBuf::from("/tmp");
        #[cfg(not(unix))]
        let base = std::env::temp_dir();
        let path = base.join(format!("etm-{}-{}", std::process::id(), random_suffix()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for OwnedRuntimeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone)]
pub struct MihomoCoreStartRequest {
    pub instance_id: uuid::Uuid,
    pub executable: PathBuf,
    pub source: MihomoConfigSource,
    pub home_dir: PathBuf,
    pub tun_device: String,
    pub route_exclude_addresses: Vec<String>,
    pub controller_secret_override: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MihomoCoreStatus {
    pub owner_instance_id: Option<uuid::Uuid>,
    pub process: MihomoStatus,
}

enum CoreOwnerCommand {
    Start {
        request: MihomoCoreStartRequest,
        response: std_mpsc::SyncSender<anyhow::Result<()>>,
    },
    Stop {
        instance_id: uuid::Uuid,
        response: std_mpsc::SyncSender<anyhow::Result<()>>,
    },
    Status {
        response: oneshot::Sender<MihomoCoreStatus>,
    },
    DashboardUrl {
        instance_id: uuid::Uuid,
        response: oneshot::Sender<anyhow::Result<String>>,
    },
    Shutdown,
}

struct OwnedMihomo {
    instance_id: uuid::Uuid,
    supervisor: MihomoSupervisor,
    _runtime_directory: OwnedRuntimeDirectory,
}

pub struct MihomoCoreOwner {
    commands: mpsc::UnboundedSender<CoreOwnerCommand>,
    thread: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}

impl MihomoCoreOwner {
    pub fn global() -> Arc<Self> {
        static OWNER: OnceLock<Arc<MihomoCoreOwner>> = OnceLock::new();
        OWNER
            .get_or_init(|| Arc::new(MihomoCoreOwner::new()))
            .clone()
    }

    fn new() -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("easytier-mihomo-owner".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                runtime.block_on(core_owner_loop(receiver));
            })
            .ok();
        Self {
            commands,
            thread: std::sync::Mutex::new(thread),
        }
    }

    pub fn start(&self, request: MihomoCoreStartRequest) -> anyhow::Result<()> {
        let (response, result) = std_mpsc::sync_channel(1);
        self.commands
            .send(CoreOwnerCommand::Start { request, response })
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner is unavailable"))?;
        result
            .recv_timeout(OWNER_COMMAND_TIMEOUT)
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner start timed out"))?
    }

    pub fn stop(&self, instance_id: uuid::Uuid) -> anyhow::Result<()> {
        let (response, result) = std_mpsc::sync_channel(1);
        self.commands
            .send(CoreOwnerCommand::Stop {
                instance_id,
                response,
            })
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner is unavailable"))?;
        result
            .recv_timeout(OWNER_COMMAND_TIMEOUT)
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner stop timed out"))?
    }

    pub async fn status(&self) -> MihomoCoreStatus {
        let (response, result) = oneshot::channel();
        if self
            .commands
            .send(CoreOwnerCommand::Status { response })
            .is_err()
        {
            return MihomoCoreStatus {
                process: MihomoStatus {
                    state: MihomoProcessState::Failed,
                    last_error: Some("Mihomo Core owner is unavailable".to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            };
        }
        tokio::time::timeout(Duration::from_secs(2), result)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| MihomoCoreStatus {
                process: MihomoStatus {
                    state: MihomoProcessState::Failed,
                    last_error: Some("Mihomo Core owner status timed out".to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            })
    }

    pub async fn dashboard_url(&self, instance_id: uuid::Uuid) -> anyhow::Result<String> {
        let (response, result) = oneshot::channel();
        self.commands
            .send(CoreOwnerCommand::DashboardUrl {
                instance_id,
                response,
            })
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner is unavailable"))?;
        tokio::time::timeout(Duration::from_secs(2), result)
            .await
            .map_err(|_| anyhow::anyhow!("Mihomo dashboard request timed out"))??
    }

    /// Stop the process-wide supervisor before Core tears down its Tokio
    /// runtime. The owner lives in a `OnceLock`, so normal process shutdown
    /// cannot rely on `Drop` to reap the Mihomo child and its TUN.
    pub fn shutdown(&self) -> anyhow::Result<()> {
        let thread = self
            .thread
            .lock()
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner thread lock is poisoned"))?
            .take();
        let Some(thread) = thread else {
            return Ok(());
        };
        let send_result = self.commands.send(CoreOwnerCommand::Shutdown);
        thread
            .join()
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner thread panicked"))?;
        send_result.map_err(|_| anyhow::anyhow!("Mihomo Core owner was already unavailable"))
    }
}

impl Drop for MihomoCoreOwner {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

async fn core_owner_loop(mut commands: mpsc::UnboundedReceiver<CoreOwnerCommand>) {
    let mut owned: Option<OwnedMihomo> = None;
    while let Some(command) = commands.recv().await {
        match command {
            CoreOwnerCommand::Start { request, response } => {
                let result = start_owned_mihomo(&mut owned, request).await;
                let _ = response.send(result);
            }
            CoreOwnerCommand::Stop {
                instance_id,
                response,
            } => {
                let result = stop_owned_mihomo(&mut owned, instance_id).await;
                let _ = response.send(result);
            }
            CoreOwnerCommand::Status { response } => {
                let status = match owned.as_ref() {
                    Some(owned) => MihomoCoreStatus {
                        owner_instance_id: Some(owned.instance_id),
                        process: owned.supervisor.status().await,
                    },
                    None => MihomoCoreStatus::default(),
                };
                let _ = response.send(status);
            }
            CoreOwnerCommand::DashboardUrl {
                instance_id,
                response,
            } => {
                let result = match owned.as_ref() {
                    Some(owned) if owned.instance_id == instance_id => {
                        let status = owned.supervisor.status().await;
                        if status.state == MihomoProcessState::Running {
                            owned.supervisor.config.overlay.dashboard_url()
                        } else {
                            Err(anyhow::anyhow!("Mihomo is not running"))
                        }
                    }
                    _ => Err(anyhow::anyhow!(
                        "network instance does not own the Mihomo runtime"
                    )),
                };
                let _ = response.send(result);
            }
            CoreOwnerCommand::Shutdown => break,
        }
    }
    if let Some(mut owned) = owned {
        owned.supervisor.stop().await;
    }
}

async fn start_owned_mihomo(
    owned: &mut Option<OwnedMihomo>,
    request: MihomoCoreStartRequest,
) -> anyhow::Result<()> {
    ensure_owner_available(
        owned.as_ref().map(|current| current.instance_id),
        request.instance_id,
    )?;

    let runtime_directory = OwnedRuntimeDirectory::create(request.instance_id)?;
    let controller = core_private_controller(&runtime_directory)?;
    let config = MihomoSupervisorConfig {
        executable: request.executable,
        source: request.source,
        home_dir: request.home_dir,
        runtime_config: runtime_directory.path.join("mihomo-runtime.yaml"),
        overlay: MihomoOverlay::new(
            request.tun_device,
            request.route_exclude_addresses,
            controller,
            request.controller_secret_override,
        ),
        restart: MihomoRestartPolicy::default(),
    };
    let mut supervisor = MihomoSupervisor::new(config);
    supervisor.start().await?;
    *owned = Some(OwnedMihomo {
        instance_id: request.instance_id,
        supervisor,
        _runtime_directory: runtime_directory,
    });
    Ok(())
}

fn ensure_owner_available(
    current: Option<uuid::Uuid>,
    requested: uuid::Uuid,
) -> anyhow::Result<()> {
    if let Some(current) = current {
        if current == requested {
            anyhow::bail!("Mihomo is already owned by this network instance");
        }
        anyhow::bail!(
            "Mihomo is already owned by network instance {}; only one Mihomo policy network is allowed per easytier-core process",
            current
        );
    }
    Ok(())
}

async fn stop_owned_mihomo(
    owned: &mut Option<OwnedMihomo>,
    instance_id: uuid::Uuid,
) -> anyhow::Result<()> {
    let Some(current) = owned.as_ref() else {
        return Ok(());
    };
    if current.instance_id != instance_id {
        return Ok(());
    }
    if let Some(mut current) = owned.take() {
        current.supervisor.stop().await;
    }
    Ok(())
}

fn core_private_controller(
    runtime_directory: &OwnedRuntimeDirectory,
) -> anyhow::Result<MihomoPrivateController> {
    let _ = runtime_directory;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    Ok(MihomoPrivateController::Loopback(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay() -> MihomoOverlay {
        MihomoOverlay::test_only(
            "et-policy-test".to_owned(),
            vec!["10.44.0.0/16".to_owned(), "fd00:44::/48".to_owned()],
            MihomoPrivateController::UnixSocket("/tmp/easytier-mihomo-test.sock".into()),
        )
    }

    fn source(yaml: &str) -> LoadedMihomoConfig {
        LoadedMihomoConfig {
            label: "test".to_owned(),
            contents: Arc::from(yaml),
        }
    }

    #[test]
    fn overlay_preserves_all_proxy_semantics() {
        let original = source(
            r#"
proxies:
  - name: p1
    type: socks5
    server: 127.0.0.1
    port: 11080
  - name: peer
    type: socks5
    server: 10.44.0.8
    port: 24443
    dialer-proxy: p1
proxy-providers:
  subscription:
    type: http
    url: https://example.invalid/subscription
proxy-groups:
  - name: mesh
    type: fallback
    proxies: [peer, DIRECT]
rules:
  - MATCH,mesh
"#,
        );
        let before: Value = serde_yaml::from_str(original.as_str()).unwrap();
        let compiled = compile_runtime_config(&original, &overlay()).unwrap();
        let after: Value = serde_yaml::from_str(&compiled.yaml).unwrap();

        for key in PROTECTED_PROXY_KEYS {
            assert_eq!(
                before.get(key).cloned(),
                after.get(key).cloned(),
                "protected Mihomo field changed: {key}"
            );
        }
    }

    #[test]
    fn relative_provider_paths_and_source_home_are_preserved() {
        let original = source(
            r#"
proxy-providers:
  local-proxies:
    type: file
    path: ./providers/proxies.yaml
rule-providers:
  local-rules:
    type: file
    path: ./rules/domestic.yaml
rules:
  - RULE-SET,local-rules,DIRECT
"#,
        );
        let before: Value = serde_yaml::from_str(original.as_str()).unwrap();
        let compiled = compile_runtime_config(&original, &overlay()).unwrap();
        let after: Value = serde_yaml::from_str(&compiled.yaml).unwrap();
        assert_eq!(before.get("proxy-providers"), after.get("proxy-providers"));
        assert_eq!(before.get("rule-providers"), after.get("rule-providers"));

        let home = PathBuf::from("/source/config");
        let config = MihomoSupervisorConfig {
            executable: "mihomo".into(),
            source: MihomoConfigSource::Inline {
                label: "test".to_owned(),
                contents: Arc::from(original.as_str()),
            },
            home_dir: home.clone(),
            runtime_config: "/private/runtime/mihomo.yaml".into(),
            overlay: overlay(),
            restart: MihomoRestartPolicy::default(),
        };
        let arguments = mihomo_runtime_args(&config, true);
        assert_eq!(
            arguments,
            vec![
                OsString::from("-t"),
                OsString::from("-d"),
                home.into_os_string(),
                OsString::from("-f"),
                OsString::from("/private/runtime/mihomo.yaml"),
            ]
        );
    }

    #[test]
    fn overlay_merges_mesh_and_tailscale_routes_without_duplicates() {
        let original = source(
            r#"
tun:
  route-exclude-address:
    - 100.64.0.0/10
    - 10.44.0.0/16
rules:
  - IP-CIDR,10.44.0.0/16,DIRECT,no-resolve
  - PROCESS-NAME,easytier-core,DIRECT
  - MATCH,DIRECT
"#,
        );
        let first = compile_runtime_config(&original, &overlay()).unwrap();
        let second_source = source(&first.yaml);
        let second = compile_runtime_config(&second_source, &overlay()).unwrap();
        let document: Value = serde_yaml::from_str(&second.yaml).unwrap();
        let root = document.as_mapping().unwrap();
        let tun = root.get(yaml_key("tun")).unwrap().as_mapping().unwrap();
        let routes = tun
            .get(yaml_key("route-exclude-address"))
            .unwrap()
            .as_sequence()
            .unwrap();
        let route_strings: BTreeSet<_> = routes.iter().filter_map(Value::as_str).collect();
        assert_eq!(route_strings.len(), routes.len());
        assert!(route_strings.contains("100.64.0.0/10"));
        assert!(route_strings.contains("fd7a:115c:a1e0::/48"));
        assert!(route_strings.contains("10.44.0.0/16"));
        assert!(route_strings.contains("fd00:44::/48"));

        let rules = root.get(yaml_key("rules")).unwrap().as_sequence().unwrap();
        assert_eq!(
            rules
                .iter()
                .filter(|rule| { rule.as_str() == Some("IP-CIDR,10.44.0.0/16,DIRECT,no-resolve") })
                .count(),
            1
        );
        assert_eq!(second.report.inserted_process_rules, 0);
        assert_eq!(second.report.inserted_address_rules, 0);
    }

    #[test]
    fn overlay_uses_real_hev_sidecar_name() {
        let compiled = compile_runtime_config(&source("rules: []\n"), &overlay()).unwrap();
        assert!(compiled.yaml.contains("easytier-hev-socks-egress"));
        assert!(compiled.yaml.contains("PROCESS-NAME,easytier-gost,DIRECT"));
        assert!(
            compiled
                .yaml
                .contains("PROCESS-NAME,easytier-gost.exe,DIRECT")
        );
        assert!(
            !compiled
                .yaml
                .contains("PROCESS-NAME,easytier-socks-egress,")
        );
    }

    #[test]
    fn overlay_replaces_public_controller_and_redacts_secret_debug() {
        let compiled = compile_runtime_config(
            &source("external-controller: 0.0.0.0:9090\nrules: []\n"),
            &overlay(),
        )
        .unwrap();
        let document: Value = serde_yaml::from_str(&compiled.yaml).unwrap();
        assert!(document.get("external-controller").is_none());
        assert_eq!(
            compiled.report.removed_controller_fields,
            vec!["external-controller"]
        );
        assert!(!format!("{:?}", overlay()).contains("test-only-secret"));
    }

    #[test]
    fn runtime_copy_replaces_existing_file_and_cleans_stale_temp() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join("runtime.yaml");
        fs::write(&runtime, "old").unwrap();
        fs::write(directory.path().join(".runtime.yaml.tmp-stale"), "stale").unwrap();
        write_runtime_copy(&runtime, "new").unwrap();
        assert_eq!(fs::read_to_string(runtime).unwrap(), "new");
        assert!(!directory.path().join(".runtime.yaml.tmp-stale").exists());
    }

    #[test]
    fn materialized_user_config_is_instance_local_and_preserves_legacy_inline() {
        let directory = tempfile::tempdir().unwrap();
        let instance_id = uuid::Uuid::new_v4();
        let (path, contents, created) = materialize_user_config(
            directory.path(),
            instance_id,
            Some("secret: original\nrules: []\n"),
        )
        .unwrap();
        assert!(created);
        assert_eq!(path.file_name().unwrap(), "autogen.yaml");
        assert!(
            path.to_string_lossy()
                .contains(&instance_id.simple().to_string())
        );
        assert_eq!(contents, "secret: original\nrules: []\n");
        save_user_config(&path, "secret: changed\nrules: []\n").unwrap();
        assert_eq!(
            load_user_config(&path).unwrap(),
            "secret: changed\nrules: []\n"
        );
    }

    #[test]
    fn controller_secret_uses_yaml_then_runtime_override() {
        let controller = MihomoPrivateController::Loopback("127.0.0.1:19090".parse().unwrap());
        let loaded = source("secret: yaml-secret\nrules: []\n");
        let mut from_yaml = MihomoOverlay::new(
            "et-policy-test".to_owned(),
            Vec::new(),
            controller.clone(),
            None,
        );
        from_yaml.resolve_controller_secret(&loaded).unwrap();
        assert_eq!(from_yaml.controller_secret.expose_to_child(), "yaml-secret");
        assert!(from_yaml.dashboard_url().unwrap().contains("port=19090"));
        assert!(from_yaml.dashboard_url().unwrap().contains("yaml-secret"));

        let mut overridden = MihomoOverlay::new(
            "et-policy-test".to_owned(),
            Vec::new(),
            controller,
            Some("runtime-secret".to_owned()),
        );
        overridden.resolve_controller_secret(&loaded).unwrap();
        assert_eq!(
            overridden.controller_secret.expose_to_child(),
            "runtime-secret"
        );
    }

    #[test]
    fn supervisor_does_not_report_running_before_health_gate() {
        assert_ne!(
            MihomoProcessState::Starting.as_str(),
            MihomoProcessState::Running.as_str()
        );
        let status = MihomoStatus {
            state: MihomoProcessState::Starting,
            pid: Some(123),
            ..Default::default()
        };
        assert_ne!(status.state, MihomoProcessState::Running);
    }

    #[test]
    fn controller_health_requires_http_200_and_matching_active_tun() {
        let response =
            b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"tun\":{\"enable\":true,\"device\":\"et-policy-test\"}}";
        let body = parse_controller_response(response).unwrap();
        validate_active_tun(&body, "et-policy-test").unwrap();
        assert!(validate_active_tun(&body, "wrong-device").is_err());
        assert!(
            parse_controller_response(
                b"HTTP/1.0 401 Unauthorized\r\n\r\n{\"message\":\"unauthorized\"}"
            )
            .is_err()
        );
    }

    #[test]
    fn macos_health_accepts_only_the_platform_assigned_utun_name() {
        assert!(active_tun_device_matches("utun18", "et-policy-test", true));
        assert!(!active_tun_device_matches("utun", "et-policy-test", true));
        assert!(!active_tun_device_matches(
            "utun18-extra",
            "et-policy-test",
            true
        ));
        assert!(!active_tun_device_matches("tun18", "et-policy-test", true));
        assert!(!active_tun_device_matches("utun18", "utun17", true));
        assert!(!active_tun_device_matches(
            "utun18",
            "et-policy-test",
            false
        ));
    }

    #[test]
    fn core_owner_rejects_second_network_instance() {
        let first = uuid::Uuid::new_v4();
        assert!(ensure_owner_available(None, first).is_ok());
        assert!(ensure_owner_available(Some(first), first).is_err());
        assert!(ensure_owner_available(Some(first), uuid::Uuid::new_v4()).is_err());
    }

    #[test]
    fn private_controller_is_loopback_only() {
        let runtime = OwnedRuntimeDirectory::create(uuid::Uuid::new_v4()).unwrap();
        let controller = core_private_controller(&runtime).unwrap();
        let MihomoPrivateController::Loopback(address) = controller else {
            panic!("the managed dashboard requires a loopback TCP controller");
        };
        assert!(address.is_ipv4());
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                fs::symlink_metadata(&runtime.path).unwrap().mode() & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn controller_readiness_is_bounded_when_the_peer_never_responds() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let stalled_server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let home = tempfile::tempdir().unwrap();
        let config = MihomoSupervisorConfig {
            executable: "mihomo".into(),
            source: MihomoConfigSource::Inline {
                label: "test".to_owned(),
                contents: Arc::from("rules: []\n"),
            },
            home_dir: home.path().to_owned(),
            runtime_config: home.path().join("runtime.yaml"),
            overlay: MihomoOverlay::test_only(
                "et-policy-test".to_owned(),
                Vec::new(),
                MihomoPrivateController::Loopback(address),
            ),
            restart: MihomoRestartPolicy::default(),
        };
        let started = tokio::time::Instant::now();
        let error = probe_readiness_until(
            &config,
            &CancellationToken::new(),
            started + Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        stalled_server.abort();
        assert!(error.to_string().contains("readiness timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
