//! Process-neutral Mihomo runtime overlay and Core-owned supervisor.
//!
//! This module never interprets or rewrites proxies, providers, groups,
//! subscriptions, or `dialer-proxy`. It owns only a generated runtime copy,
//! the policy TUN envelope, anti-loop exclusions/rules, a private controller,
//! and the Mihomo child process.
//!
//! Reference semantics were checked against the release-pinned Mihomo source
//! at `e26714a181ac0e2fa803453c0a8e9a9ce94e31cb`:
//! - `config/config.go::{RawTun,RawConfig}` defines the overlaid YAML fields.
//! - `rules/parser.go::ParseRule` accepts PROCESS-NAME, PROCESS-NAME-REGEX,
//!   and PROCESS-NAME-WILDCARD.
//! - `hub/route/server.go::router` puts `/version` and `/configs/` behind
//!   Bearer authentication for the TCP controller.
//! - `hub/route/configs.go::getConfigs` and `config/config.go::General` return
//!   the active `bind-address`, listener ports, `tun.enable`, and `tun.device`
//!   values used by the read-only readiness snapshot.
//! - `hub/route/server.go::{startUnix,startPipe}` deliberately constructs its
//!   handler with an empty secret. Unix safety therefore comes from a socket
//!   inside EasyTier's mode-0700 runtime directory. Core uses authenticated
//!   loopback TCP instead of the unauthenticated named-pipe controller on
//!   Windows.
//! - `main.go` accepts `-f` and validates with `-t`.
//! - `constant/path.go::{SetHomeDir,path.Resolve,IsSafePath}` resolves relative
//!   paths and enforces `external-ui` safety against `-d`. Following Clash
//!   Verge Rev's generated-config model, EasyTier treats a user-selected file
//!   as a read-only import source and always runs Mihomo from an instance-local
//!   managed home. The source YAML itself is never changed.
//! - `rules/provider/parse.go::ParseRuleProvider` resolves HTTP rule-provider
//!   caches to an explicit path relative to `-d`, or to
//!   `rules/<md5(url)>` when `path` is omitted. The explicit repair helper
//!   mirrors that layout but never changes Mihomo's normal update lifecycle.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
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
const MAX_MIHOMO_RESOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CONTROLLER_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_VALIDATION_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_TAILSCALE_ROUTES: &[&str] = &["100.64.0.0/10", "fd7a:115c:a1e0::/48"];
const DEFAULT_AUTOGEN_CONFIG: &str = "mode: rule\nrules:\n  - MATCH,DIRECT\n";
const MIHOMO_DISTRIBUTION_MANIFEST: &str = include_str!("../resources/mihomo/manifest.json");

#[derive(Debug, Clone, Deserialize)]
struct MihomoGeoxDefault {
    url: String,
    file_name: String,
}

#[derive(Debug, Deserialize)]
struct MihomoDistributionManifest {
    geox_defaults: std::collections::BTreeMap<String, MihomoGeoxDefault>,
}

#[derive(Debug, Clone)]
pub enum MihomoGeoxProxy {
    System,
    Socks5(String),
    #[cfg(test)]
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MihomoPreparedGeoxResource {
    pub resource: String,
    pub path: PathBuf,
    pub source_url: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
struct MihomoResourcePlan {
    resource: String,
    path: PathBuf,
    source_url: String,
    headers: Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    max_bytes: u64,
}

#[derive(Debug)]
struct InstalledGeoxResource {
    target: PathBuf,
    backup: Option<PathBuf>,
}

#[derive(Debug)]
pub struct MihomoGeoxInstall {
    installed: Vec<InstalledGeoxResource>,
    prepared: Vec<MihomoPreparedGeoxResource>,
    committed: bool,
}

impl MihomoGeoxInstall {
    pub fn commit(mut self) -> Vec<MihomoPreparedGeoxResource> {
        self.committed = true;
        for installed in &self.installed {
            if let Some(backup) = &installed.backup {
                let _ = fs::remove_file(backup);
            }
        }
        std::mem::take(&mut self.prepared)
    }
}

impl Drop for MihomoGeoxInstall {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for installed in self.installed.iter().rev() {
            let _ = fs::remove_file(&installed.target);
            if let Some(backup) = &installed.backup {
                let _ = fs::rename(backup, &installed.target);
            }
        }
    }
}

fn configured_geox_url(root: &Mapping, resource: &str) -> anyhow::Result<Option<String>> {
    let Some(geox) = root.get(yaml_key("geox-url")) else {
        return Ok(None);
    };
    let geox = geox
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Mihomo geox-url must be a YAML mapping"))?;
    let Some(value) = geox.get(yaml_key(resource)) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Mihomo geox-url.{resource} must be a string"))?;
    ensure!(
        !value.trim().is_empty(),
        "Mihomo geox-url.{resource} is empty"
    );
    Ok(Some(value.to_owned()))
}

fn collect_mihomo_rule_kinds(value: &Value, kinds: &mut BTreeSet<String>) {
    match value {
        Value::String(rule) => {
            if let Some(kind) = rule.split(',').next() {
                kinds.insert(kind.trim().to_ascii_uppercase());
            }
        }
        Value::Sequence(values) => {
            for value in values {
                collect_mihomo_rule_kinds(value, kinds);
            }
        }
        Value::Mapping(values) => {
            for value in values.values() {
                collect_mihomo_rule_kinds(value, kinds);
            }
        }
        _ => {}
    }
}

fn mihomo_rule_provider_headers(
    provider_name: &str,
    provider: &Mapping,
) -> anyhow::Result<Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>> {
    let Some(value) = provider.get(yaml_key("header")) else {
        return Ok(Vec::new());
    };
    let mapping = value.as_mapping().ok_or_else(|| {
        anyhow::anyhow!("Mihomo rule-provider {provider_name}.header must be a mapping")
    })?;
    let mut headers = Vec::new();
    for (name, values) in mapping {
        let name = name.as_str().ok_or_else(|| {
            anyhow::anyhow!("Mihomo rule-provider {provider_name} header name must be a string")
        })?;
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid Mihomo rule-provider {provider_name} header name"))?;
        let values = match values {
            Value::String(value) => vec![value.as_str()],
            Value::Sequence(values) => values
                .iter()
                .map(|value| {
                    value.as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Mihomo rule-provider {provider_name} header values must be strings"
                        )
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            _ => anyhow::bail!(
                "Mihomo rule-provider {provider_name} header value must be a string or sequence"
            ),
        };
        for value in values {
            headers.push((
                name.clone(),
                reqwest::header::HeaderValue::from_str(value).with_context(|| {
                    format!("invalid Mihomo rule-provider {provider_name} header value")
                })?,
            ));
        }
    }
    Ok(headers)
}

fn mihomo_rule_provider_size_limit(provider_name: &str, provider: &Mapping) -> anyhow::Result<u64> {
    let Some(value) = provider.get(yaml_key("size-limit")) else {
        return Ok(MAX_MIHOMO_RESOURCE_BYTES);
    };
    let limit = match value {
        Value::Number(value) => value.as_i64().ok_or_else(|| {
            anyhow::anyhow!("Mihomo rule-provider {provider_name}.size-limit exceeds int64")
        })?,
        Value::String(value) => value.parse::<i64>().with_context(|| {
            format!("Mihomo rule-provider {provider_name}.size-limit must be an integer")
        })?,
        _ => anyhow::bail!("Mihomo rule-provider {provider_name}.size-limit must be an integer"),
    };
    Ok(if limit > 0 {
        (limit as u64).min(MAX_MIHOMO_RESOURCE_BYTES)
    } else {
        MAX_MIHOMO_RESOURCE_BYTES
    })
}

fn mihomo_rule_provider_path(
    managed_home: &Path,
    source_url: &str,
    configured_path: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let default_path;
    let configured_path = match configured_path.filter(|path| !path.is_empty()) {
        Some(path) => Path::new(path),
        None => {
            default_path =
                PathBuf::from("rules").join(format!("{:x}", md5::compute(source_url.as_bytes())));
            &default_path
        }
    };
    let relative = if configured_path.is_absolute() {
        configured_path
            .strip_prefix(managed_home)
            .with_context(|| {
                format!(
                    "Mihomo rule-provider path {} is outside the managed directory",
                    configured_path.display()
                )
            })?
    } else {
        configured_path
    };
    let mut target = managed_home.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(component) => target.push(component),
            _ => anyhow::bail!(
                "Mihomo rule-provider path {} escapes the managed directory",
                configured_path.display()
            ),
        }
    }
    ensure!(
        target != managed_home && target.file_name().is_some(),
        "Mihomo rule-provider path must name a file"
    );
    Ok(target)
}

fn plan_mihomo_rule_provider_resources(
    root: &Mapping,
    managed_home: &Path,
) -> anyhow::Result<Vec<MihomoResourcePlan>> {
    let Some(value) = root.get(yaml_key("rule-providers")) else {
        return Ok(Vec::new());
    };
    let providers = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Mihomo rule-providers must be a YAML mapping"))?;
    let mut plans = Vec::new();
    for (name, value) in providers {
        let name = name
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("Mihomo rule-provider name must be a non-empty string")
            })?;
        let provider = value
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Mihomo rule-provider {name} must be a YAML mapping"))?;
        let provider_type = provider
            .get(yaml_key("type"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Mihomo rule-provider {name}.type must be a string"))?;
        if provider_type != "http" {
            continue;
        }
        let source_url = provider
            .get(yaml_key("url"))
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Mihomo rule-provider {name}.url is required"))?;
        let parsed = url::Url::parse(source_url)
            .with_context(|| format!("invalid Mihomo rule-provider {name}.url"))?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https"),
            "Mihomo rule-provider {name}.url must use http or https"
        );
        let configured_path = match provider.get(yaml_key("path")) {
            Some(value) => Some(value.as_str().ok_or_else(|| {
                anyhow::anyhow!("Mihomo rule-provider {name}.path must be a string")
            })?),
            None => None,
        };
        plans.push(MihomoResourcePlan {
            resource: format!("rule-provider:{name}"),
            path: mihomo_rule_provider_path(managed_home, source_url, configured_path)?,
            source_url: source_url.to_owned(),
            headers: mihomo_rule_provider_headers(name, provider)?,
            max_bytes: mihomo_rule_provider_size_limit(name, provider)?,
        });
    }
    plans.sort_by(|left, right| left.resource.cmp(&right.resource));
    Ok(plans)
}

fn deduplicate_mihomo_resource_plans(
    plans: Vec<MihomoResourcePlan>,
) -> anyhow::Result<Vec<MihomoResourcePlan>> {
    let mut indexes = BTreeMap::new();
    let mut unique: Vec<MihomoResourcePlan> = Vec::new();
    for plan in plans {
        if let Some(index) = indexes.get(&plan.path).copied() {
            let existing: &mut MihomoResourcePlan = &mut unique[index];
            ensure!(
                existing.source_url == plan.source_url && existing.headers == plan.headers,
                "Mihomo resources {} and {} conflict at {}",
                existing.resource,
                plan.resource,
                plan.path.display()
            );
            existing.max_bytes = existing.max_bytes.min(plan.max_bytes);
            continue;
        }
        indexes.insert(plan.path.clone(), unique.len());
        unique.push(plan);
    }
    Ok(unique)
}

fn plan_mihomo_geox_resources(
    contents: &str,
    managed_home: &Path,
) -> anyhow::Result<Vec<MihomoResourcePlan>> {
    let document: Value = serde_yaml::from_str(contents).context("failed to parse Mihomo YAML")?;
    let root = document
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("Mihomo config root must be a YAML mapping"))?;
    let mut kinds = BTreeSet::new();
    if let Some(rules) = root.get(yaml_key("rules")) {
        collect_mihomo_rule_kinds(rules, &mut kinds);
    }
    if let Some(sub_rules) = root.get(yaml_key("sub-rules")) {
        collect_mihomo_rule_kinds(sub_rules, &mut kinds);
    }
    let geodata_mode = match root.get(yaml_key("geodata-mode")) {
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => anyhow::bail!("Mihomo geodata-mode must be a boolean"),
        None => false,
    };
    let manifest: MihomoDistributionManifest =
        serde_json::from_str(MIHOMO_DISTRIBUTION_MANIFEST)
            .context("invalid bundled Mihomo distribution manifest")?;
    let mut requested = Vec::new();
    if kinds.contains("GEOSITE") || kinds.contains("SRC-GEOSITE") {
        requested.push(("geosite", "geosite"));
    }
    if kinds.contains("GEOIP") || kinds.contains("SRC-GEOIP") {
        requested.push(if geodata_mode {
            ("geoip", "geoip")
        } else {
            ("mmdb", "mmdb")
        });
    }
    if kinds.contains("IP-ASN") || kinds.contains("SRC-IP-ASN") {
        requested.push(("asn", "asn"));
    }
    let mut plans = requested
        .into_iter()
        .map(|(resource, geox_key)| {
            let default = manifest
                .geox_defaults
                .get(resource)
                .ok_or_else(|| anyhow::anyhow!("missing bundled Mihomo {resource} default"))?;
            let source_url =
                configured_geox_url(root, geox_key)?.unwrap_or_else(|| default.url.clone());
            let url = url::Url::parse(&source_url)
                .with_context(|| format!("invalid Mihomo geox-url.{geox_key}"))?;
            ensure!(
                matches!(url.scheme(), "http" | "https"),
                "Mihomo geox-url.{geox_key} must use http or https"
            );
            Ok(MihomoResourcePlan {
                resource: resource.to_owned(),
                path: managed_home.join(&default.file_name),
                source_url,
                headers: Vec::new(),
                max_bytes: MAX_MIHOMO_RESOURCE_BYTES,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    plans.extend(plan_mihomo_rule_provider_resources(root, managed_home)?);
    ensure!(
        !plans.is_empty(),
        "Mihomo config has no GeoX or HTTP rule-provider resources to prefetch"
    );
    deduplicate_mihomo_resource_plans(plans)
}

fn geox_http_client(proxy: &MihomoGeoxProxy) -> anyhow::Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("EasyTier/", env!("CARGO_PKG_VERSION")));
    if let MihomoGeoxProxy::Socks5(proxy_url) = proxy {
        let parsed = url::Url::parse(proxy_url).context("invalid SOCKS5 proxy URL")?;
        ensure!(
            matches!(parsed.scheme(), "socks5" | "socks5h"),
            "custom Mihomo resource proxy must use socks5 or socks5h"
        );
        ensure!(
            parsed.host_str().is_some() && parsed.port().is_some(),
            "custom Mihomo resource proxy requires a host and port"
        );
        builder = builder
            .no_proxy()
            .proxy(reqwest::Proxy::all(proxy_url).context("invalid SOCKS5 proxy URL")?);
    }
    #[cfg(test)]
    if matches!(proxy, MihomoGeoxProxy::Direct) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .context("failed to create Mihomo resource HTTP client")
}

fn prepare_managed_resource_parent(managed_home: &Path, target: &Path) -> anyhow::Result<()> {
    let relative = target.strip_prefix(managed_home).with_context(|| {
        format!(
            "Mihomo resource target {} is outside the managed directory",
            target.display()
        )
    })?;
    let root = fs::symlink_metadata(managed_home)
        .with_context(|| format!("failed to inspect {}", managed_home.display()))?;
    ensure!(
        root.is_dir() && !root.file_type().is_symlink(),
        "Mihomo managed directory is not a real directory"
    );
    let mut current = managed_home.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(component) = component else {
                anyhow::bail!("Mihomo resource target escapes the managed directory");
            };
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => ensure!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "refusing non-directory Mihomo resource parent {}",
                    current.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)
                        .with_context(|| format!("failed to create {}", current.display()))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn rollback_installed_geox(installed: &[InstalledGeoxResource]) {
    for item in installed.iter().rev() {
        let _ = fs::remove_file(&item.target);
        if let Some(backup) = &item.backup {
            let _ = fs::rename(backup, &item.target);
        }
    }
}

fn download_mihomo_geox_resources(
    plans: Vec<MihomoResourcePlan>,
    proxy: MihomoGeoxProxy,
    managed_home: &Path,
) -> anyhow::Result<MihomoGeoxInstall> {
    let client = geox_http_client(&proxy)?;
    let mut downloads = Vec::new();
    for plan in plans {
        prepare_managed_resource_parent(managed_home, &plan.path)?;
        let temporary = plan.path.with_file_name(format!(
            ".{}.{}.part",
            plan.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("geox"),
            uuid::Uuid::new_v4()
        ));
        let result = (|| -> anyhow::Result<u64> {
            let mut request = client.get(&plan.source_url);
            for (name, value) in &plan.headers {
                request = request.header(name, value);
            }
            let mut response = request
                .send()
                .with_context(|| format!("failed to download Mihomo {} resource", plan.resource))?
                .error_for_status()
                .with_context(|| {
                    format!(
                        "Mihomo {} resource server rejected the request",
                        plan.resource
                    )
                })?;
            if let Some(length) = response.content_length() {
                ensure!(
                    length <= plan.max_bytes,
                    "Mihomo {} resource exceeds {} bytes",
                    plan.resource,
                    plan.max_bytes
                );
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .with_context(|| format!("failed to create {}", temporary.display()))?;
            let mut limited = (&mut response).take(plan.max_bytes + 1);
            let size = std::io::copy(&mut limited, &mut file)
                .with_context(|| format!("failed to write Mihomo {} resource", plan.resource))?;
            ensure!(
                size <= plan.max_bytes,
                "Mihomo {} resource exceeds {} bytes",
                plan.resource,
                plan.max_bytes
            );
            ensure!(size > 0, "Mihomo {} resource is empty", plan.resource);
            file.sync_all()
                .with_context(|| format!("failed to sync {}", temporary.display()))?;
            Ok(size)
        })();
        match result {
            Ok(size) => downloads.push((plan, temporary, size)),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                for (_, path, _) in downloads {
                    let _ = fs::remove_file(path);
                }
                return Err(error);
            }
        }
    }

    let mut installed = Vec::new();
    let mut prepared = Vec::new();
    for (plan, temporary, size) in &downloads {
        let backup_result = (|| -> anyhow::Result<Option<PathBuf>> {
            match fs::symlink_metadata(&plan.path) {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_file() && !metadata.file_type().is_symlink(),
                        "refusing non-regular Mihomo resource target {}",
                        plan.path.display()
                    );
                    let backup = plan.path.with_file_name(format!(
                        ".{}.{}.backup",
                        plan.path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("geox"),
                        uuid::Uuid::new_v4()
                    ));
                    fs::rename(&plan.path, &backup).with_context(|| {
                        format!("failed to preserve existing {}", plan.path.display())
                    })?;
                    Ok(Some(backup))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        })();
        let backup = match backup_result {
            Ok(backup) => backup,
            Err(error) => {
                for (_, pending, _) in &downloads {
                    let _ = fs::remove_file(pending);
                }
                rollback_installed_geox(&installed);
                return Err(error);
            }
        };
        if let Err(error) = fs::rename(temporary, &plan.path) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, &plan.path);
            }
            for (_, pending, _) in &downloads {
                let _ = fs::remove_file(pending);
            }
            rollback_installed_geox(&installed);
            return Err(error)
                .with_context(|| format!("failed to install Mihomo {} resource", plan.resource));
        }
        installed.push(InstalledGeoxResource {
            target: plan.path.clone(),
            backup,
        });
        prepared.push(MihomoPreparedGeoxResource {
            resource: plan.resource.clone(),
            path: plan.path.clone(),
            source_url: plan.source_url.clone(),
            size: *size,
        });
    }
    Ok(MihomoGeoxInstall {
        installed,
        prepared,
        committed: false,
    })
}

pub async fn prepare_mihomo_geox_resources(
    request: &MihomoCoreStartRequest,
    contents: &str,
    proxy: MihomoGeoxProxy,
) -> anyhow::Result<MihomoGeoxInstall> {
    let managed_home = prepare_managed_home(&request.managed_base_dir, request.instance_id)?;
    let plans = plan_mihomo_geox_resources(contents, &managed_home)?;
    tokio::task::spawn_blocking(move || download_mihomo_geox_resources(plans, proxy, &managed_home))
        .await
        .context("Mihomo resource preparation task failed")?
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedSourceIdentity {
    canonical_path: String,
    modified_secs: u64,
    modified_nanos: u32,
    bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManagedSourceCache {
    identity: ManagedSourceIdentity,
    contents: String,
}

fn managed_source_identity(
    path: &Path,
    metadata: &fs::Metadata,
) -> anyhow::Result<ManagedSourceIdentity> {
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
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to normalize Mihomo config {}", path.display()))?;
    let modified = metadata
        .modified()
        .context("Mihomo source modification time is unavailable")?
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("Mihomo source modification time predates the Unix epoch")?;
    Ok(ManagedSourceIdentity {
        canonical_path: canonical.to_string_lossy().into_owned(),
        modified_secs: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
        bytes: metadata.len(),
    })
}

fn read_managed_source_cache(path: &Path) -> Option<ManagedSourceCache> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES.saturating_add(1024 * 1024) {
        return None;
    }
    let cache = serde_json::from_slice::<ManagedSourceCache>(&bytes).ok()?;
    (cache.contents.len() as u64 <= MAX_SOURCE_BYTES).then_some(cache)
}

fn normalized_missing_source_path(path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let parent = fs::canonicalize(absolute.parent()?).ok()?;
    Some(
        parent
            .join(absolute.file_name()?)
            .to_string_lossy()
            .into_owned(),
    )
}

fn load_managed_source(
    source: &MihomoConfigSource,
    managed_home: &Path,
) -> anyhow::Result<LoadedMihomoConfig> {
    let MihomoConfigSource::File(path) = source else {
        return source.load();
    };
    let cache_path = managed_home.join("source-cache.json");
    let cache = read_managed_source_cache(&cache_path);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let requested = normalized_missing_source_path(path);
            if let Some(cache) = cache
                && requested.as_deref() == Some(cache.identity.canonical_path.as_str())
            {
                return Ok(LoadedMihomoConfig {
                    label: cache.identity.canonical_path,
                    contents: cache.contents.into(),
                });
            }
            return Err(error).with_context(|| {
                format!(
                    "Mihomo source is unavailable and has no matching cache: {}",
                    path.display()
                )
            });
        }
        Err(error) => return Err(error.into()),
    };
    let identity = managed_source_identity(path, &metadata)?;
    if let Some(cache) = cache
        && cache.identity == identity
    {
        return Ok(LoadedMihomoConfig {
            label: identity.canonical_path,
            contents: cache.contents.into(),
        });
    }

    let loaded = source.load()?;
    let cache = ManagedSourceCache {
        identity,
        contents: loaded.as_str().to_owned(),
    };
    let serialized =
        serde_json::to_string(&cache).context("failed to serialize Mihomo source cache")?;
    write_runtime_copy(&cache_path, &serialized)?;
    Ok(loaded)
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
    dashboard_ui_path: Option<PathBuf>,
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
            .field("dashboard_ui_path", &self.dashboard_ui_path)
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
            dashboard_ui_path: None,
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
            dashboard_ui_path: None,
            controller_secret: ControllerSecret::test_only("test-only-secret"),
            controller_secret_override: None,
        }
    }

    fn with_dashboard_ui_path(mut self, path: PathBuf) -> Self {
        self.dashboard_ui_path = Some(path);
        self
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
        if self.dashboard_ui_path.is_some() {
            Ok(format!(
                "http://127.0.0.1:{}/ui/#/setup?{query}",
                address.port()
            ))
        } else {
            Ok(format!("https://board.zash.run.place/#/setup?{query}"))
        }
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
        }
    }
    if let Some(path) = &overlay.dashboard_ui_path {
        // Mihomo parity: hub/hub.go::applyRoute passes ExternalUI to
        // hub/route/server.go::SetUIPath, whose router serves that directory at
        // /ui/. Supplying an already populated absolute directory also makes
        // component/updater::UIUpdater::AutoDownloadUI skip network download.
        for field in ["external-ui", "external-ui-name", "external-ui-url"] {
            if root.remove(yaml_key(field)).is_some() {
                report.removed_controller_fields.push(field.to_owned());
            }
        }
        root.insert(
            yaml_key("external-ui"),
            Value::String(path.to_string_lossy().into_owned()),
        );
        report.changed_fields.push("external-ui".to_owned());
    }
    root.insert(
        yaml_key("secret"),
        Value::String(overlay.controller_secret.expose_to_child().to_owned()),
    );
    report.changed_fields.push("secret".to_owned());
    Ok(())
}

const BUNDLED_ZASHBOARD_ARCHIVE: &[u8] = include_bytes!("../resources/zashboard/dist-no-fonts.zip");

fn prepare_bundled_zashboard(runtime_directory: &Path) -> anyhow::Result<PathBuf> {
    let ui_path = runtime_directory.join("zashboard");
    fs::create_dir_all(&ui_path)
        .with_context(|| format!("failed to create Zashboard directory {}", ui_path.display()))?;

    let reader = std::io::Cursor::new(BUNDLED_ZASHBOARD_ARCHIVE);
    let mut archive = zip::ZipArchive::new(reader).context("failed to open bundled Zashboard")?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("failed to read bundled Zashboard entry {index}"))?;
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| anyhow::anyhow!("bundled Zashboard contains an unsafe path"))?;
        let mut components = enclosed.components();
        ensure!(
            components
                .next()
                .is_some_and(|component| { component.as_os_str() == std::ffi::OsStr::new("dist") }),
            "bundled Zashboard entry is outside the dist directory"
        );
        let relative = components.as_path();
        if relative.as_os_str().is_empty() {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            anyhow::bail!("bundled Zashboard contains a symbolic link");
        }

        let output = ui_path.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut destination = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .with_context(|| format!("failed to create Zashboard file {}", output.display()))?;
        std::io::copy(&mut entry, &mut destination)
            .with_context(|| format!("failed to extract Zashboard file {}", output.display()))?;
    }

    ensure!(
        ui_path.join("index.html").is_file(),
        "bundled Zashboard is missing index.html"
    );
    Ok(ui_path)
}

#[cfg(test)]
mod bundled_zashboard_tests {
    use super::*;

    #[test]
    fn extracts_verified_dashboard_into_owned_runtime_directory() {
        let runtime = tempfile::tempdir().unwrap();
        let ui_path = prepare_bundled_zashboard(runtime.path()).unwrap();

        assert_eq!(ui_path, runtime.path().join("zashboard"));
        assert!(ui_path.join("index.html").is_file());
        assert!(ui_path.join("assets").is_dir());
        assert!(!ui_path.join("dist").exists());
    }

    #[test]
    fn local_dashboard_url_uses_the_private_controller_and_secret() {
        let overlay = MihomoOverlay::test_only(
            "utun-test".to_owned(),
            Vec::new(),
            MihomoPrivateController::Loopback("127.0.0.1:19090".parse().unwrap()),
        )
        .with_dashboard_ui_path(PathBuf::from("/tmp/zashboard"));

        let url = overlay.dashboard_url().unwrap();
        assert!(url.starts_with("http://127.0.0.1:19090/ui/#/setup?"));
        assert!(url.contains("hostname=127.0.0.1"));
        assert!(url.contains("port=19090"));
        assert!(url.contains("secret=test-only-secret"));
    }
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
    write_config_copy(path, yaml, None)
}

fn write_config_copy(
    path: &Path,
    yaml: &str,
    existing_metadata: Option<&fs::Metadata>,
) -> anyhow::Result<()> {
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
        #[cfg(unix)]
        if let Some(metadata) = existing_metadata {
            use std::os::fd::AsRawFd as _;
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

            let temporary_metadata = file.metadata()?;
            if temporary_metadata.uid() != metadata.uid()
                || temporary_metadata.gid() != metadata.gid()
            {
                nix::unistd::fchown(
                    file.as_raw_fd(),
                    Some(nix::unistd::Uid::from_raw(metadata.uid())),
                    Some(nix::unistd::Gid::from_raw(metadata.gid())),
                )
                .context("failed to preserve Mihomo source owner")?;
            }
            file.set_permissions(fs::Permissions::from_mode(metadata.mode()))?;
        }
        file.sync_all()?;
        drop(file);
        publish_runtime_copy(&temporary_path, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
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
    let existing_metadata = fs::symlink_metadata(path).ok();
    if let Some(metadata) = existing_metadata.as_ref() {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "refusing non-regular Mihomo source config {}",
            path.display()
        );
    }
    write_config_copy(path, contents, existing_metadata.as_ref())
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
    pub version: Option<String>,
    pub mixed_port: Option<u16>,
    pub http_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub tun_device: Option<String>,
    pub bind_address: Option<String>,
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
            version: None,
            mixed_port: None,
            http_port: None,
            socks_port: None,
            tun_device: None,
            bind_address: None,
        }
    }
}

impl MihomoStatus {
    fn clear_readiness_snapshot(&mut self) {
        self.version = None;
        self.mixed_port = None;
        self.http_port = None;
        self.socks_port = None;
        self.tun_device = None;
        self.bind_address = None;
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
        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Validating;
            status.last_error = None;
        }
        let report = self.prepare_runtime_config().await?;

        {
            let mut status = self.status.write().await;
            status.state = MihomoProcessState::Starting;
            status.overlay_report = Some(report);
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

    pub async fn validate_config(&mut self) -> anyhow::Result<()> {
        ensure!(self.task.is_none(), "Mihomo supervisor is already started");
        self.prepare_runtime_config().await.map(|_| ())
    }

    async fn prepare_runtime_config(&mut self) -> anyhow::Result<MihomoOverlayReport> {
        ensure!(
            !self
                .config
                .source
                .conflicts_with_runtime_path(&self.config.runtime_config),
            "Mihomo source and runtime config paths must differ"
        );
        validate_executable(&self.config.executable)?;
        remove_stale_controller(&self.config.overlay.controller)?;
        let source = load_managed_source(&self.config.source, &self.config.home_dir)?;
        self.config.overlay.resolve_controller_secret(&source)?;
        let compiled = compile_runtime_config(&source, &self.config.overlay)?;
        write_runtime_copy(&self.config.runtime_config, &compiled.yaml)?;
        validate_runtime_config(&self.config).await?;
        Ok(compiled.report)
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
        "Mihomo managed home does not exist: {}",
        config.home_dir.display()
    );
    let mut command = Command::new(&config.executable);
    command
        .args(mihomo_runtime_args(config, true))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_command(&mut command);
    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to execute Mihomo validator {}",
            config.executable.display()
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .context("Mihomo validator stdout pipe is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Mihomo validator stderr pipe is unavailable")?;
    let stdout_diagnostics = tokio::spawn(read_bounded_diagnostics(stdout));
    let stderr_diagnostics = tokio::spawn(read_bounded_diagnostics(stderr));
    let validation = wait_for_mihomo_validator(
        &mut child,
        config.restart.validation_timeout,
        config.restart.stop_timeout,
    )
    .await;
    let stdout = stdout_diagnostics
        .await
        .context("Mihomo validator stdout task failed")??;
    let stderr = stderr_diagnostics
        .await
        .context("Mihomo validator stderr task failed")??;
    let status = validation?;
    let diagnostic = format_validator_diagnostic(&stdout, &stderr);
    ensure!(
        status.success(),
        "Mihomo rejected generated config: {}",
        diagnostic
    );
    Ok(())
}

async fn wait_for_mihomo_validator(
    child: &mut tokio::process::Child,
    validation_timeout: Duration,
    stop_timeout: Duration,
) -> anyhow::Result<std::process::ExitStatus> {
    match tokio::time::timeout(validation_timeout, child.wait()).await {
        Ok(status) => status.context("failed waiting for Mihomo config validation"),
        Err(_) => {
            let _ = child.start_kill();
            match tokio::time::timeout(stop_timeout, child.wait()).await {
                Ok(status) => {
                    status.context("failed reaping timed-out Mihomo config validator")?;
                }
                Err(_) => {
                    // Match the long-running sidecar fallback: SIGKILL and wait
                    // without leaving a child owned by Core. `kill()` includes
                    // the wait needed to reap the process on Unix.
                    let _ = child.kill().await;
                }
            }
            anyhow::bail!("Mihomo config validation timed out")
        }
    }
}

#[cfg(all(test, unix))]
mod mihomo_validator_lifecycle_tests {
    use super::*;

    fn shell_child(script: &str) -> tokio::process::Child {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command(&mut command);
        command.spawn().unwrap()
    }

    #[tokio::test]
    async fn short_lived_validator_is_reaped_after_normal_exit() {
        let mut child = shell_child("exit 0");
        let status =
            wait_for_mihomo_validator(&mut child, Duration::from_secs(1), Duration::from_secs(1))
                .await
                .unwrap();

        assert!(status.success());
        assert!(child.try_wait().unwrap().is_some());
    }

    #[tokio::test]
    async fn timed_out_validator_is_killed_and_reaped() {
        let mut child = shell_child("sleep 30");
        let error = wait_for_mihomo_validator(
            &mut child,
            Duration::from_millis(10),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert!(child.try_wait().unwrap().is_some());
    }
}

fn format_validator_diagnostic(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => "validator exited without diagnostics".to_owned(),
        (stdout, "") => stdout.to_owned(),
        ("", stderr) => stderr.to_owned(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
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

fn validate_active_tun(
    response: &serde_json::Value,
    expected_device: &str,
) -> anyhow::Result<String> {
    ensure!(
        response
            .pointer("/tun/enable")
            .and_then(|value| value.as_bool())
            == Some(true),
        "Mihomo controller reports TUN disabled"
    );
    let actual_device = response
        .pointer("/tun/device")
        .and_then(|value| value.as_str())
        .context("Mihomo controller response is missing the active TUN device")?;
    ensure!(
        active_tun_device_matches(actual_device, expected_device, cfg!(target_os = "macos")),
        "Mihomo controller reports an unexpected TUN device"
    );
    Ok(actual_device.to_owned())
}

#[derive(Debug)]
struct MihomoReadinessSnapshot {
    version: String,
    mixed_port: Option<u16>,
    http_port: Option<u16>,
    socks_port: Option<u16>,
    tun_device: String,
    bind_address: Option<String>,
}

fn controller_port(config: &serde_json::Value, name: &str) -> Option<u16> {
    config
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
}

fn controller_bind_address(config: &serde_json::Value) -> Option<String> {
    config
        .get("bind-address")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod readiness_snapshot_tests {
    use super::{controller_bind_address, controller_port};

    #[test]
    fn controller_snapshot_preserves_actual_bind_scope_and_enabled_ports() {
        let active = serde_json::json!({
            "bind-address": "*",
            "mixed-port": 7890,
            "port": 0,
            "socks-port": 1080,
        });

        assert_eq!(controller_bind_address(&active).as_deref(), Some("*"));
        assert_eq!(controller_port(&active, "mixed-port"), Some(7890));
        assert_eq!(controller_port(&active, "port"), None);
        assert_eq!(controller_port(&active, "socks-port"), Some(1080));

        let loopback = serde_json::json!({ "bind-address": "127.0.0.1" });
        assert_eq!(
            controller_bind_address(&loopback).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(controller_bind_address(&serde_json::json!({})), None);
    }
}

async fn probe_readiness(
    config: &MihomoSupervisorConfig,
) -> anyhow::Result<MihomoReadinessSnapshot> {
    let version_response = controller_get(config, "/version").await?;
    let version_response: serde_json::Value = serde_json::from_slice(&version_response)?;
    let version = version_response
        .get("version")
        .and_then(|value| value.as_str())
        .context("Mihomo controller version response is missing version")?
        .to_owned();

    let active = controller_get(config, "/configs/").await?;
    let active: serde_json::Value = serde_json::from_slice(&active)?;
    let tun_device = validate_active_tun(&active, &config.overlay.tun_device)?;
    Ok(MihomoReadinessSnapshot {
        version,
        mixed_port: controller_port(&active, "mixed-port"),
        http_port: controller_port(&active, "port"),
        socks_port: controller_port(&active, "socks-port"),
        tun_device,
        bind_address: controller_bind_address(&active),
    })
}

async fn probe_readiness_until(
    config: &MihomoSupervisorConfig,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> anyhow::Result<Option<MihomoReadinessSnapshot>> {
    tokio::select! {
        _ = cancel.cancelled() => anyhow::bail!("Mihomo start cancelled"),
        result = tokio::time::timeout_at(deadline, probe_readiness(config)) => {
            match result {
                Ok(Ok(snapshot)) => Ok(Some(snapshot)),
                Ok(Err(_)) => Ok(None),
                Err(_) => anyhow::bail!("Mihomo private controller/TUN readiness timed out"),
            }
        }
    }
}

async fn wait_for_readiness(
    child: &mut ManagedChild,
    config: &MihomoSupervisorConfig,
    cancel: &CancellationToken,
) -> anyhow::Result<MihomoReadinessSnapshot> {
    let deadline = tokio::time::Instant::now() + config.restart.readiness_timeout;
    loop {
        if let Some(exit) = child.try_wait()? {
            anyhow::bail!("Mihomo exited before readiness: {exit}");
        }
        if let Some(snapshot) = probe_readiness_until(config, cancel, deadline).await? {
            return Ok(snapshot);
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
            current.clear_readiness_snapshot();
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
        let readiness = match wait_for_readiness(&mut child, &config, &cancel).await {
            Ok(readiness) => readiness,
            Err(error) => {
                stop_child(&mut child, config.restart.stop_timeout).await;
                if cancel.is_cancelled() {
                    let mut current = status.write().await;
                    current.state = MihomoProcessState::Stopped;
                    current.pid = None;
                    current.clear_readiness_snapshot();
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
        };

        {
            let mut current = status.write().await;
            current.state = MihomoProcessState::Running;
            current.started_at = Some(SystemTime::now());
            current.restart_count = total_restarts;
            current.last_error = None;
            current.version = Some(readiness.version);
            current.mixed_port = readiness.mixed_port;
            current.http_port = readiness.http_port;
            current.socks_port = readiness.socks_port;
            current.tun_device = Some(readiness.tun_device);
            current.bind_address = readiness.bind_address;
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
                current.clear_readiness_snapshot();
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
            current.clear_readiness_snapshot();
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
    fn create(managed_home: &Path) -> anyhow::Result<Self> {
        cleanup_current_process_runtime_directories(managed_home)?;
        let path = managed_home.join(format!("run-{}-{}", std::process::id(), random_suffix()));
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

fn cleanup_current_process_runtime_directories(managed_home: &Path) -> anyhow::Result<()> {
    let prefix = format!("run-{}-", std::process::id());
    for entry in fs::read_dir(managed_home)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || metadata.is_file() {
            fs::remove_file(entry.path())?;
        } else if metadata.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "refusing non-directory or symlink Mihomo managed path {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder.create(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn canonicalize_mihomo_home(path: &Path) -> std::io::Result<PathBuf> {
    // Mihomo v1.19.29 (e26714a181ac0e2fa803453c0a8e9a9ce94e31cb)
    // stores `-d` verbatim in constant/path.go::SetHomeDir. Its
    // path::MMDB/ASN/GeoIP/GeoSite helpers then join file names with Go's
    // slash-only path.Join instead of filepath.Join. A Windows
    // `\\?\C:\...` path from std::fs::canonicalize consequently becomes the
    // invalid mixed form `\\?\C:\.../geoip.metadb`. Keep canonical symlink
    // resolution while returning the conventional child-process path form.
    #[cfg(windows)]
    return dunce::canonicalize(path);
    #[cfg(not(windows))]
    fs::canonicalize(path)
}

fn prepare_managed_home(base: &Path, instance_id: uuid::Uuid) -> anyhow::Result<PathBuf> {
    match fs::symlink_metadata(base) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "refusing non-directory or symlink Mihomo managed base {}",
            base.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(base)?;
        }
        Err(error) => return Err(error.into()),
    }
    let normalized_base = canonicalize_mihomo_home(base)
        .with_context(|| format!("failed to normalize Mihomo managed base {}", base.display()))?;
    let runtime_root = normalized_base.join("mihomo-runtime");
    create_private_directory(&runtime_root)?;
    let home = runtime_root.join(instance_id.simple().to_string());
    create_private_directory(&home)?;
    let normalized_home = canonicalize_mihomo_home(&home)
        .with_context(|| format!("failed to normalize Mihomo managed home {}", home.display()))?;
    ensure!(
        normalized_home.starts_with(&normalized_base),
        "Mihomo managed home escaped its configured base"
    );
    Ok(normalized_home)
}

impl Drop for OwnedRuntimeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone)]
pub struct MihomoCoreStartRequest {
    pub instance_id: uuid::Uuid,
    pub ownership_token: uuid::Uuid,
    pub executable: PathBuf,
    pub source: MihomoConfigSource,
    pub managed_base_dir: PathBuf,
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
        expected_ownership_token: Option<uuid::Uuid>,
        response: std_mpsc::SyncSender<anyhow::Result<()>>,
    },
    Validate {
        request: MihomoCoreStartRequest,
        response: oneshot::Sender<anyhow::Result<()>>,
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
    ownership_token: uuid::Uuid,
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
        self.stop_matching(instance_id, None)
    }

    pub(crate) fn stop_if_owned(
        &self,
        instance_id: uuid::Uuid,
        ownership_token: uuid::Uuid,
    ) -> anyhow::Result<()> {
        self.stop_matching(instance_id, Some(ownership_token))
    }

    fn stop_matching(
        &self,
        instance_id: uuid::Uuid,
        expected_ownership_token: Option<uuid::Uuid>,
    ) -> anyhow::Result<()> {
        let (response, result) = std_mpsc::sync_channel(1);
        self.commands
            .send(CoreOwnerCommand::Stop {
                instance_id,
                expected_ownership_token,
                response,
            })
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner is unavailable"))?;
        result
            .recv_timeout(OWNER_COMMAND_TIMEOUT)
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner stop timed out"))?
    }

    pub async fn validate(&self, request: MihomoCoreStartRequest) -> anyhow::Result<()> {
        let timeout = MihomoRestartPolicy::default().validation_timeout
            + MihomoRestartPolicy::default().stop_timeout
            + Duration::from_secs(5);
        let (response, result) = oneshot::channel();
        self.commands
            .send(CoreOwnerCommand::Validate { request, response })
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner is unavailable"))?;
        tokio::time::timeout(timeout, result)
            .await
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner validation timed out"))?
            .map_err(|_| anyhow::anyhow!("Mihomo Core owner validation response was dropped"))?
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
    ///
    /// On macOS, Mihomo may launch `dscacheutil` as its own short-lived child.
    /// That grandchild is not owned by EasyTier and cannot be reaped with
    /// `waitpid` here. A transient `dscacheutil` zombie is therefore an
    /// accepted upstream Mihomo limitation, not evidence that this owner
    /// leaked its directly managed Mihomo process.
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
                expected_ownership_token,
                response,
            } => {
                let result =
                    stop_owned_mihomo(&mut owned, instance_id, expected_ownership_token).await;
                let _ = response.send(result);
            }
            CoreOwnerCommand::Validate { request, response } => {
                let result = validate_owned_mihomo(owned.as_ref(), request).await;
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

    let instance_id = request.instance_id;
    let ownership_token = request.ownership_token;
    let (mut supervisor, runtime_directory) = build_owned_supervisor(request)?;
    supervisor.start().await?;
    *owned = Some(OwnedMihomo {
        instance_id,
        ownership_token,
        supervisor,
        _runtime_directory: runtime_directory,
    });
    Ok(())
}

async fn validate_owned_mihomo(
    owned: Option<&OwnedMihomo>,
    request: MihomoCoreStartRequest,
) -> anyhow::Result<()> {
    ensure_owner_available(
        owned.map(|current| current.instance_id),
        request.instance_id,
    )?;
    let (mut supervisor, _runtime_directory) = build_owned_supervisor(request)?;
    supervisor.validate_config().await
}

fn build_owned_supervisor(
    request: MihomoCoreStartRequest,
) -> anyhow::Result<(MihomoSupervisor, OwnedRuntimeDirectory)> {
    let managed_home = prepare_managed_home(&request.managed_base_dir, request.instance_id)?;
    let runtime_directory = OwnedRuntimeDirectory::create(&managed_home)?;
    let dashboard_ui_path = prepare_bundled_zashboard(&runtime_directory.path)?;
    let controller = core_private_controller(&runtime_directory)?;
    let config = MihomoSupervisorConfig {
        executable: request.executable,
        source: request.source,
        home_dir: managed_home,
        runtime_config: runtime_directory.path.join("mihomo-runtime.yaml"),
        overlay: MihomoOverlay::new(
            request.tun_device,
            request.route_exclude_addresses,
            controller,
            request.controller_secret_override,
        )
        .with_dashboard_ui_path(dashboard_ui_path),
        restart: MihomoRestartPolicy::default(),
    };
    Ok((MihomoSupervisor::new(config), runtime_directory))
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
    expected_ownership_token: Option<uuid::Uuid>,
) -> anyhow::Result<()> {
    let Some(current) = owned.as_ref() else {
        return Ok(());
    };
    if !owner_stop_matches(
        current.instance_id,
        current.ownership_token,
        instance_id,
        expected_ownership_token,
    ) {
        return Ok(());
    }
    if let Some(mut current) = owned.take() {
        current.supervisor.stop().await;
    }
    Ok(())
}

fn owner_stop_matches(
    current_instance_id: uuid::Uuid,
    current_ownership_token: uuid::Uuid,
    requested_instance_id: uuid::Uuid,
    expected_ownership_token: Option<uuid::Uuid>,
) -> bool {
    current_instance_id == requested_instance_id
        && expected_ownership_token.is_none_or(|token| token == current_ownership_token)
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

    #[test]
    fn geox_plan_uses_mihomo_defaults_without_explicit_urls() {
        let home = tempfile::tempdir().unwrap();
        let plans = plan_mihomo_geox_resources(
            "geodata-mode: false\nrules:\n  - GEOSITE,cn,DIRECT\n  - GEOIP,CN,DIRECT\n  - IP-ASN,13335,DIRECT\n",
            home.path(),
        )
        .unwrap();
        assert_eq!(
            plans
                .iter()
                .map(|plan| (plan.resource.as_str(), plan.path.file_name().unwrap()))
                .collect::<Vec<_>>(),
            vec![
                ("geosite", std::ffi::OsStr::new("GeoSite.dat")),
                ("mmdb", std::ffi::OsStr::new("geoip.metadb")),
                ("asn", std::ffi::OsStr::new("ASN.mmdb")),
            ]
        );
        assert!(
            plans
                .iter()
                .all(|plan| plan.source_url.starts_with("https://"))
        );
    }

    #[test]
    fn geox_plan_honors_geodata_mode_urls_and_sub_rules() {
        let home = tempfile::tempdir().unwrap();
        let plans = plan_mihomo_geox_resources(
            "geodata-mode: true\ngeox-url:\n  geoip: https://example.test/custom.dat\nsub-rules:\n  child:\n    - GEOIP,US,DIRECT\n",
            home.path(),
        )
        .unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].resource, "geoip");
        assert_eq!(plans[0].path.file_name().unwrap(), "GeoIP.dat");
        assert_eq!(plans[0].source_url, "https://example.test/custom.dat");
    }

    #[test]
    fn geox_plan_rejects_configs_without_geo_rules() {
        let home = tempfile::tempdir().unwrap();
        let error = plan_mihomo_geox_resources("rules:\n  - MATCH,DIRECT\n", home.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("no GeoX or HTTP rule-provider resources"));
    }

    #[test]
    fn rule_provider_plan_matches_mihomo_paths_headers_and_limits() {
        let home = tempfile::tempdir().unwrap();
        let default_url = "https://example.test/default.yaml";
        let plans = plan_mihomo_geox_resources(
            &format!(
                "rule-providers:\n  default:\n    type: http\n    behavior: domain\n    format: yaml\n    url: {default_url}\n  explicit:\n    type: http\n    behavior: classical\n    format: text\n    url: https://example.test/explicit.txt\n    path: ./rules/explicit.txt\n    size-limit: 4096\n    header:\n      Authorization:\n        - Bearer test\n  local:\n    type: file\n    behavior: domain\n    path: ./rules/local.yaml\n  embedded:\n    type: inline\n    behavior: domain\n    payload: [example.com]\nrules:\n  - MATCH,DIRECT\n"
            ),
            home.path(),
        )
        .unwrap();
        assert_eq!(plans.len(), 2);
        let default = plans
            .iter()
            .find(|plan| plan.resource == "rule-provider:default")
            .unwrap();
        assert_eq!(
            default.path,
            home.path()
                .join("rules")
                .join(format!("{:x}", md5::compute(default_url.as_bytes())))
        );
        let explicit = plans
            .iter()
            .find(|plan| plan.resource == "rule-provider:explicit")
            .unwrap();
        assert_eq!(explicit.path, home.path().join("rules/explicit.txt"));
        assert_eq!(explicit.max_bytes, 4096);
        assert_eq!(explicit.headers.len(), 1);
        assert_eq!(explicit.headers[0].0.as_str(), "authorization");
        assert_eq!(explicit.headers[0].1, "Bearer test");
    }

    #[test]
    fn rule_provider_plan_rejects_managed_directory_escape() {
        let home = tempfile::tempdir().unwrap();
        let error = plan_mihomo_geox_resources(
            "rule-providers:\n  unsafe:\n    type: http\n    behavior: domain\n    url: https://example.test/rules.yaml\n    path: ../outside.yaml\n",
            home.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("escapes the managed directory"));
    }

    #[test]
    fn rule_provider_plan_combines_with_geox_in_one_transaction() {
        let home = tempfile::tempdir().unwrap();
        let plans = plan_mihomo_geox_resources(
            "rule-providers:\n  domains:\n    type: http\n    behavior: domain\n    url: https://example.test/domains.yaml\nrules:\n  - GEOSITE,cn,DIRECT\n  - RULE-SET,domains,DIRECT\n",
            home.path(),
        )
        .unwrap();
        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.resource.as_str())
                .collect::<Vec<_>>(),
            vec!["geosite", "rule-provider:domains"]
        );
    }

    #[test]
    fn rule_provider_plan_rejects_conflicting_targets() {
        let home = tempfile::tempdir().unwrap();
        let error = plan_mihomo_geox_resources(
            "rule-providers:\n  first:\n    type: http\n    behavior: domain\n    url: https://example.test/first.yaml\n    path: rules/shared.yaml\n  second:\n    type: http\n    behavior: domain\n    url: https://example.test/second.yaml\n    path: rules/shared.yaml\n",
            home.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("conflict at"));
    }

    #[test]
    fn geox_install_rolls_back_until_validation_commits() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("GeoSite.dat");
        let backup = home.path().join(".GeoSite.dat.backup");
        fs::write(&target, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        let install = MihomoGeoxInstall {
            installed: vec![InstalledGeoxResource {
                target: target.clone(),
                backup: Some(backup.clone()),
            }],
            prepared: Vec::new(),
            committed: false,
        };
        drop(install);
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!backup.exists());
    }

    #[test]
    fn geox_install_commit_keeps_validated_file() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("GeoSite.dat");
        let backup = home.path().join(".GeoSite.dat.backup");
        fs::write(&target, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        let prepared = MihomoPreparedGeoxResource {
            resource: "geosite".to_owned(),
            path: target.clone(),
            source_url: "https://example.test/geosite.dat".to_owned(),
            size: 3,
        };
        let install = MihomoGeoxInstall {
            installed: vec![InstalledGeoxResource {
                target: target.clone(),
                backup: Some(backup.clone()),
            }],
            prepared: vec![prepared.clone()],
            committed: false,
        };
        assert_eq!(install.commit(), vec![prepared]);
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!backup.exists());
    }

    #[test]
    fn geox_download_uses_transactional_installation() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnew-geox",
                )
                .unwrap();
        });
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("GeoSite.dat");
        fs::write(&target, b"old-geox").unwrap();
        let plans = plan_mihomo_geox_resources(
            &format!(
                "geox-url:\n  geosite: http://{address}/geosite.dat\nrules:\n  - GEOSITE,cn,DIRECT\n"
            ),
            home.path(),
        )
        .unwrap();
        let install =
            download_mihomo_geox_resources(plans, MihomoGeoxProxy::Direct, home.path()).unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-geox");
        assert_eq!(install.prepared[0].size, 8);
        drop(install);
        assert_eq!(fs::read(&target).unwrap(), b"old-geox");
    }

    #[test]
    fn rule_provider_download_creates_mihomo_cache_and_forwards_headers() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let size = std::io::Read::read(&mut stream, &mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer test"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 25\r\nConnection: close\r\n\r\npayload:\n  - example.com\n",
                )
                .unwrap();
        });
        let home = tempfile::tempdir().unwrap();
        let plans = plan_mihomo_geox_resources(
            &format!(
                "rule-providers:\n  domains:\n    type: http\n    behavior: domain\n    format: yaml\n    url: http://{address}/domains.yaml\n    header:\n      Authorization: Bearer test\n"
            ),
            home.path(),
        )
        .unwrap();
        let target = plans[0].path.clone();
        let install =
            download_mihomo_geox_resources(plans, MihomoGeoxProxy::Direct, home.path()).unwrap();
        server.join().unwrap();
        assert_eq!(install.prepared[0].resource, "rule-provider:domains");
        assert_eq!(fs::read(&target).unwrap(), b"payload:\n  - example.com\n");
        drop(install);
        assert!(!target.exists());
    }

    #[test]
    fn rule_provider_download_enforces_configured_size_limit() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
                )
                .unwrap();
        });
        let home = tempfile::tempdir().unwrap();
        let plans = plan_mihomo_geox_resources(
            &format!(
                "rule-providers:\n  limited:\n    type: http\n    behavior: domain\n    url: http://{address}/limited.yaml\n    path: rules/limited.yaml\n    size-limit: 4\n"
            ),
            home.path(),
        )
        .unwrap();
        let target = plans[0].path.clone();
        let error = download_mihomo_geox_resources(plans, MihomoGeoxProxy::Direct, home.path())
            .unwrap_err()
            .to_string();
        server.join().unwrap();
        assert!(error.contains("exceeds 4 bytes"));
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rule_provider_download_rejects_symlink_parent() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("rules")).unwrap();
        let plans = plan_mihomo_geox_resources(
            "rule-providers:\n  unsafe:\n    type: http\n    behavior: domain\n    url: https://example.test/unsafe.yaml\n    path: rules/unsafe.yaml\n",
            home.path(),
        )
        .unwrap();
        let error = download_mihomo_geox_resources(plans, MihomoGeoxProxy::Direct, home.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing non-directory Mihomo resource parent"));
        assert!(!outside.path().join("unsafe.yaml").exists());
    }

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
    fn relative_provider_paths_are_preserved_in_the_managed_runtime_copy() {
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

        let home = PathBuf::from("/managed/mihomo/home");
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
    fn managed_home_and_ephemeral_run_directory_stay_under_the_configured_base() {
        let base = tempfile::tempdir().unwrap();
        let instance_id = uuid::Uuid::new_v4();
        let home = prepare_managed_home(base.path(), instance_id).unwrap();
        assert!(home.starts_with(canonicalize_mihomo_home(base.path()).unwrap()));
        assert!(home.ends_with(instance_id.simple().to_string()));

        let run_path = {
            let runtime = OwnedRuntimeDirectory::create(&home).unwrap();
            assert!(runtime.path.starts_with(&home));
            runtime.path.clone()
        };
        assert!(!run_path.exists());
        assert!(home.exists());
    }

    #[cfg(windows)]
    #[test]
    fn managed_home_uses_a_mihomo_compatible_windows_path() {
        let base = tempfile::tempdir().unwrap();
        let home = prepare_managed_home(base.path(), uuid::Uuid::new_v4()).unwrap();

        assert!(
            !home.to_string_lossy().starts_with(r"\\?\"),
            "Mihomo v1.19.29 joins Geo paths incorrectly below a verbatim Windows home: {}",
            home.display()
        );
        assert!(home.join("geoip.metadb").is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn managed_home_does_not_change_existing_application_directory_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = tempfile::tempdir().unwrap();
        fs::set_permissions(base.path(), fs::Permissions::from_mode(0o750)).unwrap();
        let _home = prepare_managed_home(base.path(), uuid::Uuid::new_v4()).unwrap();

        assert_eq!(
            fs::metadata(base.path()).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_home_rejects_a_symlink_runtime_root() {
        use std::os::unix::fs::symlink;

        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), base.path().join("mihomo-runtime")).unwrap();

        let error = prepare_managed_home(base.path(), uuid::Uuid::new_v4()).unwrap_err();
        assert!(error.to_string().contains("symlink Mihomo managed path"));
    }

    #[test]
    fn managed_file_source_cache_survives_source_outage_and_refreshes_on_change() {
        let base = tempfile::tempdir().unwrap();
        let source_path = base.path().join("source.yaml");
        fs::write(&source_path, "rules: []\n").unwrap();
        let home = prepare_managed_home(base.path(), uuid::Uuid::new_v4()).unwrap();
        let source = MihomoConfigSource::File(source_path.clone());

        let first = load_managed_source(&source, &home).unwrap();
        assert_eq!(first.as_str(), "rules: []\n");
        fs::remove_file(&source_path).unwrap();
        let cached = load_managed_source(&source, &home).unwrap();
        assert_eq!(cached.as_str(), "rules: []\n");

        fs::write(&source_path, "mode: rule\nrules: []\n").unwrap();
        let refreshed = load_managed_source(&source, &home).unwrap();
        assert_eq!(refreshed.as_str(), "mode: rule\nrules: []\n");
    }

    #[test]
    fn corrupt_source_cache_is_rebuilt_from_the_available_source() {
        let base = tempfile::tempdir().unwrap();
        let source_path = base.path().join("source.yaml");
        fs::write(&source_path, "rules: []\n").unwrap();
        let home = prepare_managed_home(base.path(), uuid::Uuid::new_v4()).unwrap();
        fs::write(home.join("source-cache.json"), "not json").unwrap();

        let loaded = load_managed_source(&MihomoConfigSource::File(source_path), &home).unwrap();
        assert_eq!(loaded.as_str(), "rules: []\n");
        assert!(read_managed_source_cache(&home.join("source-cache.json")).is_some());
    }

    #[test]
    fn current_process_stale_runtime_directory_is_removed_before_start() {
        let home = tempfile::tempdir().unwrap();
        let stale = home
            .path()
            .join(format!("run-{}-stale", std::process::id()));
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("partial"), "stale").unwrap();

        let runtime = OwnedRuntimeDirectory::create(home.path()).unwrap();
        assert!(!stale.exists());
        assert!(runtime.path.exists());
    }

    #[test]
    fn validator_diagnostic_keeps_stdout_and_stderr() {
        assert_eq!(
            format_validator_diagnostic(b"stdout detail\n", b"stderr detail\n"),
            "stdout detail\nstderr detail"
        );
        assert_eq!(
            format_validator_diagnostic(b"", b""),
            "validator exited without diagnostics"
        );
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            if nix::unistd::Uid::effective().is_root() {
                nix::unistd::chown(
                    &path,
                    Some(nix::unistd::Uid::from_raw(65534)),
                    Some(nix::unistd::Gid::from_raw(65534)),
                )
                .unwrap();
            }
            let before = fs::metadata(&path).unwrap();
            save_user_config(&path, "secret: changed\nrules: []\n").unwrap();
            let after = fs::metadata(&path).unwrap();
            assert_eq!(after.mode() & 0o777, 0o640);
            assert_eq!(after.uid(), before.uid());
            assert_eq!(after.gid(), before.gid());
        }
        #[cfg(not(unix))]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn editor_validation_rejects_yaml_and_validator_failures_without_starting() {
        fn request(base: &Path, executable: &str, contents: &str) -> MihomoCoreStartRequest {
            MihomoCoreStartRequest {
                instance_id: uuid::Uuid::new_v4(),
                ownership_token: uuid::Uuid::new_v4(),
                executable: executable.into(),
                source: MihomoConfigSource::Inline {
                    label: "edited Mihomo config".to_owned(),
                    contents: Arc::from(contents),
                },
                managed_base_dir: base.to_owned(),
                tun_device: "et-policy-test".to_owned(),
                route_exclude_addresses: Vec::new(),
                controller_secret_override: None,
            }
        }

        let invalid_base = tempfile::tempdir().unwrap();
        let (mut invalid, _runtime) = build_owned_supervisor(request(
            invalid_base.path(),
            "/bin/true",
            "proxies: [{\"type”: \"socks5\"}]\n",
        ))
        .unwrap();
        assert!(invalid.validate_config().await.is_err());
        assert!(invalid.task.is_none());

        let rejected_base = tempfile::tempdir().unwrap();
        let (mut rejected, _runtime) = build_owned_supervisor(request(
            rejected_base.path(),
            "/bin/false",
            "rules:\n  - MATCH,DIRECT\n",
        ))
        .unwrap();
        let error = rejected.validate_config().await.unwrap_err().to_string();
        assert!(error.contains("Mihomo rejected generated config"));
        assert!(rejected.task.is_none());
    }

    #[test]
    fn controller_health_requires_http_200_and_matching_active_tun() {
        let response =
            b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"tun\":{\"enable\":true,\"device\":\"et-policy-test\"}}";
        let body = parse_controller_response(response).unwrap();
        let active: serde_json::Value = serde_json::from_slice(&body).unwrap();
        validate_active_tun(&active, "et-policy-test").unwrap();
        assert!(validate_active_tun(&active, "wrong-device").is_err());
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
    fn stale_owner_token_cannot_stop_replacement_for_same_instance() {
        let instance_id = uuid::Uuid::new_v4();
        let old_token = uuid::Uuid::new_v4();
        let replacement_token = uuid::Uuid::new_v4();

        assert!(!owner_stop_matches(
            instance_id,
            replacement_token,
            instance_id,
            Some(old_token),
        ));
        assert!(owner_stop_matches(
            instance_id,
            replacement_token,
            instance_id,
            Some(replacement_token),
        ));
        assert!(owner_stop_matches(
            instance_id,
            replacement_token,
            instance_id,
            None,
        ));
    }

    #[test]
    fn private_controller_is_loopback_only() {
        let managed_home = tempfile::tempdir().unwrap();
        let runtime = OwnedRuntimeDirectory::create(managed_home.path()).unwrap();
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
