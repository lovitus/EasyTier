#[cfg(feature = "ffi-dataplane")]
use crate::launcher::{DataPlaneTcpListener, DataPlaneTcpStream, DataPlaneUdpSocket};
use anyhow::Context as _;
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
#[cfg(feature = "mesh-socks-egress")]
use std::{sync::mpsc as std_mpsc, thread, time::Duration};
#[cfg(feature = "mesh-socks-egress")]
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

#[cfg(all(feature = "mesh-socks-egress", not(mobile)))]
#[path = "gost.rs"]
mod gost;

use crate::{
    common::{
        config::{
            ConfigFileControl, ConfigLoader, ConfigSource, NetworkIdentity, NicBackend,
            PolicyProxyConfig, TomlConfigLoader,
        },
        global_ctx::{EventBusSubscriber, GlobalCtxEvent},
        log,
    },
    launcher::{NetworkInstance, NetworkInstanceRunningInfo},
    mihomo::{MihomoConfigSource, MihomoCoreOwner, MihomoCoreStartRequest, MihomoCoreStatus},
    proto::{self},
    rpc_service::InstanceRpcService,
};

const BOOTSTRAP_PEER_DIR: &str = "peer-bootstrap";

fn bootstrap_peer_url_supported(url: &url::Url) -> bool {
    url.scheme() != "ring" && crate::tunnel::TunnelScheme::try_from(url).is_ok()
}

fn bootstrap_peer_cache_key(identity: &NetworkIdentity) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"easytier-bootstrap-peer-v1\0");
    hasher.update((identity.network_name.len() as u64).to_be_bytes());
    hasher.update(identity.network_name.as_bytes());

    let secret_digest = identity.network_secret_digest.or_else(|| {
        identity.network_secret.as_ref().and_then(|secret| {
            NetworkIdentity::new(identity.network_name.clone(), secret.clone())
                .network_secret_digest
        })
    });
    if let Some(secret_digest) = secret_digest {
        hasher.update([1]);
        hasher.update(secret_digest);
    } else {
        hasher.update([0]);
    }

    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bootstrap_peer_cache_path(config_dir: &Path, identity: &NetworkIdentity) -> PathBuf {
    config_dir
        .join(BOOTSTRAP_PEER_DIR)
        .join(format!("{}.txt", bootstrap_peer_cache_key(identity)))
}

fn bootstrap_peer_backup_path(config_dir: &Path, identity: &NetworkIdentity) -> PathBuf {
    bootstrap_peer_cache_path(config_dir, identity).with_extension("txt.bak")
}

fn read_bootstrap_peer_urls(path: &Path) -> std::io::Result<Vec<url::Url>> {
    let contents = std::fs::read_to_string(path)?;
    let mut urls = contents
        .lines()
        .filter_map(|line| line.trim().parse::<url::Url>().ok())
        .filter(bootstrap_peer_url_supported)
        .collect::<Vec<_>>();
    urls.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    urls.dedup();
    Ok(urls)
}

fn load_bootstrap_peer_urls(
    config_dir: Option<&Path>,
    identity: &NetworkIdentity,
) -> std::io::Result<Vec<url::Url>> {
    let Some(config_dir) = config_dir else {
        return Ok(Vec::new());
    };
    let primary_path = bootstrap_peer_cache_path(config_dir, identity);
    let primary = read_bootstrap_peer_urls(&primary_path);
    if let Ok(urls) = &primary
        && !urls.is_empty()
    {
        return Ok(urls.clone());
    }

    let backup = read_bootstrap_peer_urls(&bootstrap_peer_backup_path(config_dir, identity));
    match backup {
        Ok(urls) if !urls.is_empty() => Ok(urls),
        _ => match primary {
            Ok(urls) => Ok(urls),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        },
    }
}

fn persist_bootstrap_peer_urls(
    config_dir: Option<&Path>,
    identity: &NetworkIdentity,
    urls: &[url::Url],
) -> std::io::Result<()> {
    let Some(config_dir) = config_dir else {
        return Ok(());
    };
    let mut urls = urls
        .iter()
        .filter(|url| bootstrap_peer_url_supported(url))
        .map(url::Url::as_str)
        .collect::<Vec<_>>();
    urls.sort_unstable();
    urls.dedup();
    if urls.is_empty() {
        return Ok(());
    }

    let path = bootstrap_peer_cache_path(config_dir, identity);
    let previous_urls = match read_bootstrap_peer_urls(&path) {
        Ok(urls) => urls,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let previous = previous_urls
        .iter()
        .map(url::Url::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let next = urls
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if next == previous || (next.len() < previous.len() && next.is_subset(&previous)) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !previous.is_empty() {
        std::fs::copy(&path, bootstrap_peer_backup_path(config_dir, identity))?;
    }
    let mut contents = urls.join("\n");
    contents.push('\n');
    std::fs::write(path, contents)
}

#[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
fn ensure_policy_socket_mark(config: &TomlConfigLoader) -> anyhow::Result<Option<u32>> {
    if !config
        .get_policy_proxy_config()
        .is_some_and(|policy| policy.is_leaf_enabled())
    {
        return Ok(None);
    }
    let mut flags = config.get_flags();
    let mark = *flags
        .socket_mark
        .get_or_insert(crate::policy_proxy::POLICY_SOCKET_MARK);
    if mark == 0 {
        anyhow::bail!("policy_proxy requires a non-zero socket_mark");
    }
    config.set_flags(flags);
    Ok(Some(mark))
}

fn build_mihomo_start_request(
    config: &TomlConfigLoader,
    policy: PolicyProxyConfig,
    config_dir: Option<&std::path::Path>,
) -> anyhow::Result<MihomoCoreStartRequest> {
    let resolved_file = policy.resolved_active_config_file();
    let source = if let Some(path) = resolved_file {
        MihomoConfigSource::File(path)
    } else if let Some(contents) = policy.active_config_inline().cloned() {
        MihomoConfigSource::Inline {
            label: format!("network {} inline Mihomo config", config.get_id()),
            contents: contents.into(),
        }
    } else {
        anyhow::bail!("Mihomo policy backend requires config_file or config_inline");
    };

    let executable = resolve_mihomo_executable(policy.mihomo_executable.as_deref())?;
    let mut route_exclude_addresses = BTreeMap::<String, ()>::new();
    if let Some(ipv4) = config.get_ipv4() {
        route_exclude_addresses.insert(ipv4.network().to_string(), ());
    }
    if let Some(ipv6) = config.get_ipv6() {
        route_exclude_addresses.insert(ipv6.network().to_string(), ());
    }
    let instance_id = config.get_id();
    let compact_id = instance_id.simple().to_string();
    // Match Clash Verge Rev's ownership model: an imported file is only a
    // source. Mihomo always receives an EasyTier-owned, platform-neutral home
    // and generated runtime copy. The temp fallback keeps standalone Core
    // usable when no persistent --config-dir was supplied.
    let managed_base_dir = config_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("easytier"));
    Ok(MihomoCoreStartRequest {
        instance_id,
        ownership_token: uuid::Uuid::new_v4(),
        executable,
        source,
        managed_base_dir,
        tun_device: format!("etm{}", &compact_id[..8]),
        route_exclude_addresses: route_exclude_addresses.into_keys().collect(),
        controller_secret_override: policy.mihomo_controller_secret.clone(),
    })
}

fn resolve_mihomo_executable(configured: Option<&std::path::Path>) -> anyhow::Result<PathBuf> {
    let current_executable =
        std::env::current_exe().context("failed to locate easytier-core executable")?;
    let directory = current_executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("easytier-core executable has no parent directory"))?;
    let default_name = if cfg!(windows) {
        "easytier-mihomo.exe"
    } else {
        "easytier-mihomo"
    };
    Ok(match configured {
        Some(path) if path.is_absolute() => path.to_owned(),
        Some(path) => directory.join(path),
        None => directory.join(default_name),
    })
}

fn mihomo_status_proto(
    status: &MihomoCoreStatus,
) -> crate::proto::api::manage::MihomoProcessStatus {
    crate::proto::api::manage::MihomoProcessStatus {
        state: status.process.state.as_str().to_owned(),
        pid: status.process.pid,
        restart_count: status.process.restart_count,
        last_exit: status.process.last_exit.clone(),
        last_error: status.process.last_error.clone(),
        owner_instance_id: status
            .owner_instance_id
            .map(|instance_id| instance_id.to_string()),
        version: status.process.version.clone(),
        mixed_port: status.process.mixed_port.map(u32::from),
        http_port: status.process.http_port.map(u32::from),
        socks_port: status.process.socks_port.map(u32::from),
        tun_device: status.process.tun_device.clone(),
        bind_address: status.process.bind_address.clone(),
    }
}

#[cfg(feature = "mesh-socks-egress")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeutralMeshEntryState {
    Unsupported,
    Starting,
    Running,
    Backoff,
    Stopping,
    Stopped,
    Failed,
}

#[cfg(feature = "mesh-socks-egress")]
impl NeutralMeshEntryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Backoff => "backoff",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

#[cfg(feature = "mesh-socks-egress")]
#[derive(Debug, Clone)]
pub struct NeutralMeshEntryStatus {
    pub state: NeutralMeshEntryState,
    pub endpoint: Option<std::net::SocketAddr>,
    pub selected_port: Option<u16>,
    pub backend: String,
    pub core_leases: u32,
    pub restart_count: u32,
    pub last_error: Option<String>,
}

#[cfg(feature = "mesh-socks-egress")]
impl Default for NeutralMeshEntryStatus {
    fn default() -> Self {
        let supported = neutral_mesh_entry_supported_on_current_target();
        Self {
            state: if supported {
                NeutralMeshEntryState::Stopped
            } else {
                NeutralMeshEntryState::Unsupported
            },
            endpoint: None,
            selected_port: None,
            backend: if !supported {
                "unsupported".to_owned()
            } else if cfg!(mobile) {
                easytier_socks_egress::managed_backend_name().to_owned()
            } else {
                "gost".to_owned()
            },
            core_leases: 0,
            restart_count: 0,
            last_error: None,
        }
    }
}

#[cfg(feature = "mesh-socks-egress")]
const fn neutral_mesh_entry_supported_on_current_target() -> bool {
    cfg!(target_os = "android")
        || cfg!(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
        || cfg!(all(
            target_os = "macos",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
        || cfg!(all(
            target_os = "windows",
            any(
                target_arch = "x86_64",
                target_arch = "x86",
                target_arch = "aarch64"
            )
        ))
        || cfg!(all(target_os = "freebsd", target_arch = "x86_64"))
}

#[cfg(feature = "mesh-socks-egress")]
impl NeutralMeshEntryStatus {
    fn into_proto(self) -> crate::proto::api::manage::NeutralMeshEntryStatus {
        crate::proto::api::manage::NeutralMeshEntryStatus {
            state: self.state.as_str().to_owned(),
            endpoint: self.endpoint.map(|endpoint| endpoint.to_string()),
            selected_port: self.selected_port.map(u32::from),
            backend: self.backend,
            core_leases: self.core_leases,
            restart_count: self.restart_count,
            last_error: self.last_error,
        }
    }
}

#[cfg(feature = "mesh-socks-egress")]
fn neutral_mesh_entry_acquire_starts_owner(current_leases: u32) -> bool {
    current_leases == 0
}

#[cfg(feature = "mesh-socks-egress")]
fn neutral_mesh_entry_release_stops_owner(remaining_leases: u32) -> bool {
    remaining_leases == 0
}

#[cfg(feature = "mesh-socks-egress")]
fn with_mesh_entry_status(
    status: &Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
    update: impl FnOnce(&mut NeutralMeshEntryStatus),
) {
    let mut status = status
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    update(&mut status);
}

#[cfg(feature = "mesh-socks-egress")]
struct CoreMeshEntryGuard {
    endpoint: std::net::SocketAddr,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "mesh-socks-egress")]
impl CoreMeshEntryGuard {
    async fn shutdown(self) {
        self.cancel.cancel();
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[cfg(all(feature = "mesh-socks-egress", not(mobile)))]
async fn start_core_mesh_entry_guard(
    status: Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
) -> anyhow::Result<CoreMeshEntryGuard> {
    let executable_name = format!("easytier-gost{}", std::env::consts::EXE_SUFFIX);
    let executable = std::env::current_exe()
        .context("failed to resolve EasyTier executable for Core mesh entry")?
        .parent()
        .context("EasyTier executable has no parent directory")?
        .join(executable_name);
    if !executable.is_file() {
        anyhow::bail!(
            "managed GOST mesh-entry is missing: {}",
            executable.display()
        );
    }

    let mut process_config = gost::GostProcessConfig::new(executable);
    let runtime = gost::GostRuntime::start(process_config.clone()).await?;
    let endpoint = runtime.endpoint();
    process_config.port_candidates = vec![endpoint.port()];
    let cancel = CancellationToken::new();
    let runtime_cancel = cancel.clone();
    let task_status = status.clone();
    let task = tokio::spawn(async move {
        let mut runtime = runtime;
        let mut delay = Duration::from_secs(1);
        let mut running_since = tokio::time::Instant::now();
        loop {
            let result = runtime.run_until_cancel(runtime_cancel.clone()).await;
            if runtime_cancel.is_cancelled() {
                return;
            }
            with_mesh_entry_status(&task_status, |status| {
                status.state = NeutralMeshEntryState::Backoff;
                status.last_error = Some(match &result {
                    Ok(()) => "neutral mesh-entry exited unexpectedly".to_owned(),
                    Err(error) => format!("{error:#}"),
                });
            });

            if running_since.elapsed() >= Duration::from_secs(60) {
                delay = Duration::from_secs(1);
            }
            loop {
                tokio::select! {
                    _ = runtime_cancel.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                match gost::GostRuntime::start(process_config.clone()).await {
                    Ok(restarted) => {
                        runtime = restarted;
                        running_since = tokio::time::Instant::now();
                        with_mesh_entry_status(&task_status, |status| {
                            status.state = NeutralMeshEntryState::Running;
                            status.restart_count = status.restart_count.saturating_add(1);
                            status.last_error = None;
                        });
                        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                        break;
                    }
                    Err(error) => {
                        with_mesh_entry_status(&task_status, |status| {
                            status.state = NeutralMeshEntryState::Backoff;
                            status.last_error = Some(format!("{error:#}"));
                        });
                        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                    }
                }
            }
        }
    });
    Ok(CoreMeshEntryGuard {
        endpoint,
        cancel,
        task,
    })
}

#[cfg(all(feature = "mesh-socks-egress", mobile, target_os = "android"))]
async fn start_core_mesh_entry_guard(
    status: Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
) -> anyhow::Result<CoreMeshEntryGuard> {
    let mut server = easytier_socks_egress::SocksEgressConfig::mesh_entry();
    let runtime = easytier_socks_egress::InProcessRuntime::start(server.clone()).await?;
    let endpoint = runtime.endpoint();
    server.port_candidates = vec![endpoint.port()];
    let cancel = CancellationToken::new();
    let runtime_cancel = cancel.clone();
    let task_status = status.clone();
    let task = tokio::spawn(async move {
        let mut runtime = runtime;
        loop {
            let result = runtime.run_until_cancel(runtime_cancel.clone()).await;
            if runtime_cancel.is_cancelled() {
                return;
            }
            let exit_reason = match result {
                Ok(()) => "Android mesh entry exited unexpectedly".to_owned(),
                Err(error) => format!("{error:#}"),
            };
            with_mesh_entry_status(&task_status, |status| {
                status.state = NeutralMeshEntryState::Backoff;
                status.last_error = Some(exit_reason);
            });

            let mut delay = Duration::from_secs(1);
            loop {
                tokio::select! {
                    _ = runtime_cancel.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                match easytier_socks_egress::InProcessRuntime::start(server.clone()).await {
                    Ok(restarted) => {
                        runtime = restarted;
                        with_mesh_entry_status(&task_status, |status| {
                            status.state = NeutralMeshEntryState::Running;
                            status.restart_count = status.restart_count.saturating_add(1);
                            status.last_error = None;
                        });
                        break;
                    }
                    Err(error) => {
                        with_mesh_entry_status(&task_status, |status| {
                            status.state = NeutralMeshEntryState::Backoff;
                            status.last_error = Some(format!("{error:#}"));
                        });
                        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                    }
                }
            }
        }
    });
    Ok(CoreMeshEntryGuard {
        endpoint,
        cancel,
        task,
    })
}

#[cfg(all(feature = "mesh-socks-egress", mobile, not(target_os = "android")))]
async fn start_core_mesh_entry_guard(
    _status: Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
) -> anyhow::Result<CoreMeshEntryGuard> {
    anyhow::bail!("neutral mesh-entry is unsupported on this mobile host")
}

#[cfg(feature = "mesh-socks-egress")]
enum CoreMeshEntryCommand {
    Acquire {
        response: std_mpsc::SyncSender<anyhow::Result<()>>,
    },
    Release {
        response: std_mpsc::SyncSender<()>,
    },
}

#[cfg(feature = "mesh-socks-egress")]
pub struct CoreMeshEntryHandle {
    status: Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
}

#[cfg(feature = "mesh-socks-egress")]
impl CoreMeshEntryHandle {
    pub fn global() -> Arc<Self> {
        core_mesh_entry_owner().handle.clone()
    }

    pub fn status(&self) -> NeutralMeshEntryStatus {
        self.status
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn endpoint(&self) -> anyhow::Result<std::net::SocketAddr> {
        let status = self.status();
        if status.state != NeutralMeshEntryState::Running {
            anyhow::bail!(
                "neutral mesh-entry is not running: state={} error={}",
                status.state.as_str(),
                status.last_error.as_deref().unwrap_or("none")
            );
        }
        status
            .endpoint
            .ok_or_else(|| anyhow::anyhow!("neutral mesh-entry has no published endpoint"))
    }
}

#[cfg(all(feature = "mesh-socks-egress", feature = "leaf-policy-proxy"))]
#[async_trait::async_trait]
impl crate::policy_proxy::LocalSocksEndpointProvider for CoreMeshEntryHandle {
    async fn endpoint(&self) -> anyhow::Result<std::net::SocketAddr> {
        CoreMeshEntryHandle::endpoint(self)
    }
}

#[cfg(feature = "mesh-socks-egress")]
struct CoreMeshEntryOwner {
    commands: tokio::sync::mpsc::UnboundedSender<CoreMeshEntryCommand>,
    handle: Arc<CoreMeshEntryHandle>,
}

#[cfg(feature = "mesh-socks-egress")]
fn core_mesh_entry_owner() -> &'static CoreMeshEntryOwner {
    static OWNER: OnceLock<CoreMeshEntryOwner> = OnceLock::new();
    OWNER.get_or_init(|| {
        let status = Arc::new(std::sync::RwLock::new(NeutralMeshEntryStatus::default()));
        let handle = Arc::new(CoreMeshEntryHandle {
            status: status.clone(),
        });
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let _ = thread::Builder::new()
            .name("easytier-mesh-entry-owner".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        with_mesh_entry_status(&status, |status| {
                            status.state = NeutralMeshEntryState::Failed;
                            status.last_error = Some(format!(
                                "failed to create neutral mesh-entry runtime: {error}"
                            ));
                        });
                        return;
                    }
                };
                runtime.block_on(core_mesh_entry_owner_loop(receiver, status));
            });
        CoreMeshEntryOwner { commands, handle }
    })
}

#[cfg(feature = "mesh-socks-egress")]
async fn core_mesh_entry_owner_loop(
    mut commands: tokio::sync::mpsc::UnboundedReceiver<CoreMeshEntryCommand>,
    status: Arc<std::sync::RwLock<NeutralMeshEntryStatus>>,
) {
    let mut leases = 0_u32;
    let mut guard: Option<CoreMeshEntryGuard> = None;
    while let Some(command) = commands.recv().await {
        match command {
            CoreMeshEntryCommand::Acquire { response } => {
                let started_owner = neutral_mesh_entry_acquire_starts_owner(leases);
                if started_owner {
                    with_mesh_entry_status(&status, |status| {
                        status.state = NeutralMeshEntryState::Starting;
                        status.last_error = None;
                    });
                    match start_core_mesh_entry_guard(status.clone()).await {
                        Ok(started) => {
                            with_mesh_entry_status(&status, |status| {
                                status.state = NeutralMeshEntryState::Running;
                                status.endpoint = Some(started.endpoint);
                                status.selected_port = Some(started.endpoint.port());
                            });
                            guard = Some(started);
                        }
                        Err(error) => {
                            with_mesh_entry_status(&status, |status| {
                                status.state = NeutralMeshEntryState::Failed;
                                status.endpoint = None;
                                status.selected_port = None;
                                status.last_error = Some(format!("{error:#}"));
                            });
                            let _ = response.send(Err(error));
                            continue;
                        }
                    }
                }
                if response.send(Ok(())).is_err() {
                    if started_owner && let Some(owned) = guard.take() {
                        with_mesh_entry_status(&status, |status| {
                            status.state = NeutralMeshEntryState::Stopping
                        });
                        owned.shutdown().await;
                        with_mesh_entry_status(&status, |status| {
                            status.state = NeutralMeshEntryState::Stopped;
                            status.endpoint = None;
                            status.selected_port = None;
                        });
                    }
                    continue;
                }
                leases = leases.saturating_add(1);
                with_mesh_entry_status(&status, |status| status.core_leases = leases);
            }
            CoreMeshEntryCommand::Release { response } => {
                leases = leases.saturating_sub(1);
                with_mesh_entry_status(&status, |status| status.core_leases = leases);
                if neutral_mesh_entry_release_stops_owner(leases)
                    && let Some(owned) = guard.take()
                {
                    with_mesh_entry_status(&status, |status| {
                        status.state = NeutralMeshEntryState::Stopping
                    });
                    owned.shutdown().await;
                    with_mesh_entry_status(&status, |status| {
                        status.state = NeutralMeshEntryState::Stopped;
                        status.endpoint = None;
                        status.selected_port = None;
                    });
                }
                let _ = response.send(());
            }
        }
    }
    if let Some(owned) = guard {
        owned.shutdown().await;
    }
}

#[cfg(feature = "mesh-socks-egress")]
struct CoreMeshEntryLease {
    commands: tokio::sync::mpsc::UnboundedSender<CoreMeshEntryCommand>,
}

#[cfg(feature = "mesh-socks-egress")]
impl CoreMeshEntryLease {
    fn acquire() -> anyhow::Result<Self> {
        let owner = core_mesh_entry_owner();
        // A rendezvous channel prevents late startup from publishing a lease
        // after recv_timeout has dropped the only receiver.
        let (response, result) = std_mpsc::sync_channel(0);
        owner
            .commands
            .send(CoreMeshEntryCommand::Acquire { response })
            .map_err(|_| anyhow::anyhow!("neutral mesh-entry owner is unavailable"))?;
        result
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| anyhow::anyhow!("neutral mesh-entry startup timed out"))??;
        Ok(Self {
            commands: owner.commands.clone(),
        })
    }
}

#[cfg(feature = "mesh-socks-egress")]
impl Drop for CoreMeshEntryLease {
    fn drop(&mut self) {
        let (response, stopped) = std_mpsc::sync_channel(1);
        if self
            .commands
            .send(CoreMeshEntryCommand::Release { response })
            .is_ok()
        {
            let _ = stopped.recv_timeout(Duration::from_secs(6));
        }
    }
}

#[cfg(test)]
mod mihomo_request_tests {
    use super::*;
    use crate::common::config::{PolicyProxyBackend, PolicyProxyConfig};

    #[test]
    fn core_request_uses_mesh_cidrs_managed_home_and_unique_tun_name() {
        let config = TomlConfigLoader::default();
        let instance_id = uuid::Uuid::new_v4();
        config.set_id(instance_id);
        config.set_ipv4(Some("10.44.0.80/24".parse().unwrap()));
        config.set_ipv6(Some("fd00:44::80/64".parse().unwrap()));
        let source_home = tempfile::tempdir().unwrap();
        let policy = PolicyProxyConfig {
            backend: Some(PolicyProxyBackend::Mihomo),
            mihomo_config_inline: Some("rules: []\n".to_owned()),
            mihomo_executable: Some("easytier-mihomo".into()),
            source_dir: Some(source_home.path().to_owned()),
            ..Default::default()
        };

        let managed_base = tempfile::tempdir().unwrap();
        let request =
            build_mihomo_start_request(&config, policy, Some(managed_base.path())).unwrap();
        assert_eq!(request.instance_id, instance_id);
        assert_eq!(request.managed_base_dir, managed_base.path());
        assert_eq!(request.tun_device.len(), 11);
        assert!(request.tun_device.starts_with("etm"));
        assert!(
            request
                .route_exclude_addresses
                .contains(&"10.44.0.0/24".to_owned())
        );
        assert!(
            request
                .route_exclude_addresses
                .contains(&"fd00:44::/64".to_owned())
        );
    }
}

pub(crate) struct DaemonGuard {
    guard: Option<Arc<()>>,
    stop_check_notifier: Arc<tokio::sync::Notify>,
}
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.stop_check_notifier.notify_one();
    }
}

pub struct NetworkInstanceManager {
    instance_map: Arc<DashMap<uuid::Uuid, NetworkInstance>>,
    instance_stop_tasks: Arc<DashMap<uuid::Uuid, InstanceStopTask>>,
    stop_check_notifier: Arc<tokio::sync::Notify>,
    instance_error_messages: Arc<DashMap<uuid::Uuid, String>>,
    config_dir: OnceLock<PathBuf>,
    bootstrap_state_dir: OnceLock<PathBuf>,
    guard_counter: Arc<()>,
    remote_mutation_lock: Arc<tokio::sync::Mutex<()>>,
    mihomo_owner: Arc<MihomoCoreOwner>,
    #[cfg(feature = "mesh-socks-egress")]
    _mesh_entry_lease: Option<CoreMeshEntryLease>,
    nic_backend: NicBackend,
}

struct InstanceStopTask {
    generation: uuid::Uuid,
    mihomo_ownership_token: Arc<std::sync::RwLock<Option<uuid::Uuid>>>,
    _handle: AbortOnDropHandle<()>,
}

impl Default for NetworkInstanceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkInstanceManager {
    pub fn new() -> Self {
        #[cfg(feature = "mesh-socks-egress")]
        let mesh_entry_lease = if neutral_mesh_entry_supported_on_current_target() {
            match CoreMeshEntryLease::acquire() {
                Ok(lease) => Some(lease),
                Err(error) => {
                    log::error!("failed to start Core-owned neutral mesh entry: {error:#}");
                    None
                }
            }
        } else {
            None
        };

        NetworkInstanceManager {
            instance_map: Arc::new(DashMap::new()),
            instance_stop_tasks: Arc::new(DashMap::new()),
            stop_check_notifier: Arc::new(tokio::sync::Notify::new()),
            instance_error_messages: Arc::new(DashMap::new()),
            config_dir: OnceLock::new(),
            bootstrap_state_dir: OnceLock::new(),
            guard_counter: Arc::new(()),
            remote_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            mihomo_owner: MihomoCoreOwner::global(),
            #[cfg(feature = "mesh-socks-egress")]
            _mesh_entry_lease: mesh_entry_lease,
            nic_backend: NicBackend::Tun,
        }
    }

    pub fn with_config_path(self, config_dir: Option<PathBuf>) -> Self {
        if let Some(config_dir) = config_dir {
            let _ = self.config_dir.set(config_dir);
        }
        self
    }

    pub fn with_bootstrap_state_path(self, state_dir: Option<PathBuf>) -> Self {
        if let Some(state_dir) = state_dir {
            let _ = self.bootstrap_state_dir.set(state_dir);
        }
        self
    }

    pub fn set_bootstrap_state_path(&self, state_dir: PathBuf) -> anyhow::Result<()> {
        anyhow::ensure!(
            !state_dir.as_os_str().is_empty(),
            "bootstrap state path is empty"
        );
        if let Some(existing) = self.bootstrap_state_dir.get() {
            anyhow::ensure!(
                existing == &state_dir,
                "bootstrap state path is already initialized as {}",
                existing.display()
            );
            return Ok(());
        }
        self.bootstrap_state_dir
            .set(state_dir)
            .map_err(|_| anyhow::anyhow!("bootstrap state path was initialized concurrently"))
    }

    pub fn set_config_path(&self, config_dir: PathBuf) -> anyhow::Result<()> {
        anyhow::ensure!(
            !config_dir.as_os_str().is_empty(),
            "persistent config directory cannot be empty"
        );
        if let Some(existing) = self.config_dir.get() {
            anyhow::ensure!(
                existing == &config_dir,
                "persistent config directory is already initialized as {}",
                existing.display()
            );
            return Ok(());
        }
        anyhow::ensure!(
            self.instance_map.is_empty(),
            "persistent config directory must be initialized before starting an instance"
        );
        self.config_dir.set(config_dir).map_err(|_| {
            anyhow::anyhow!("persistent config directory was initialized concurrently")
        })
    }

    pub fn with_nic_backend(mut self, nic_backend: NicBackend) -> Self {
        self.nic_backend = nic_backend;
        self
    }

    pub fn remote_mutation_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.remote_mutation_lock.clone()
    }

    fn start_instance_task(
        &self,
        instance_id: uuid::Uuid,
        initial_mihomo_ownership_token: Option<uuid::Uuid>,
    ) -> Result<(), anyhow::Error> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(anyhow::anyhow!(
                "tokio runtime not found, cannot start instance task"
            ));
        }

        let instance = self
            .instance_map
            .get(&instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance {} not found", instance_id))?;
        let instance_stop_notifier = instance.get_stop_notifier();
        let instance_event_receiver = instance.subscribe_event();

        let instance_map = self.instance_map.clone();
        let instance_stop_tasks = self.instance_stop_tasks.clone();
        let instance_error_messages = self.instance_error_messages.clone();
        let mihomo_owner = self.mihomo_owner.clone();
        let task_generation = uuid::Uuid::new_v4();
        let mihomo_ownership_token =
            Arc::new(std::sync::RwLock::new(initial_mihomo_ownership_token));
        let task_mihomo_ownership_token = mihomo_ownership_token.clone();
        let (registered, wait_until_registered) = tokio::sync::oneshot::channel();

        let stop_check_notifier = self.stop_check_notifier.clone();
        let handle = AbortOnDropHandle::new(tokio::spawn(async move {
            if wait_until_registered.await.is_err() {
                return;
            }
            let Some(instance_stop_notifier) = instance_stop_notifier else {
                return;
            };
            let _t = instance_event_receiver
                .map(|event| AbortOnDropHandle::new(handle_event(instance_id, event)));
            instance_stop_notifier.notified().await;
            if let Some(instance) = instance_map.get(&instance_id)
                && let Some(error) = instance.get_latest_error_msg()
            {
                log::error!(%error, "instance {} stopped", instance_id);
                instance_error_messages.insert(instance_id, error);
            }
            // An overwritten instance keeps the same UUID. Its delayed stop
            // notification must not tear down the replacement's Mihomo.
            let ownership_token = task_mihomo_ownership_token
                .read()
                .map(|token| *token)
                .unwrap_or_default();
            if let Some(ownership_token) = ownership_token
                && let Err(error) = mihomo_owner.stop_if_owned(instance_id, ownership_token)
            {
                log::error!(%error, "failed to stop Mihomo owner for instance {}", instance_id);
            }
            stop_check_notifier.notify_one();
            if let dashmap::mapref::entry::Entry::Occupied(entry) =
                instance_stop_tasks.entry(instance_id)
                && entry.get().generation == task_generation
            {
                entry.remove();
            }
            instance_stop_tasks.shrink_to_fit();
        }));
        self.instance_stop_tasks.insert(
            instance_id,
            InstanceStopTask {
                generation: task_generation,
                mihomo_ownership_token,
                _handle: handle,
            },
        );
        let _ = registered.send(());
        Ok(())
    }

    pub fn run_network_instance(
        &self,
        cfg: TomlConfigLoader,
        watch_event: bool,
        config_file_control: ConfigFileControl,
    ) -> Result<uuid::Uuid, anyhow::Error> {
        cfg.set_nic_backend(self.nic_backend);
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        if let Some(mark) = ensure_policy_socket_mark(&cfg)? {
            crate::common::dns::set_control_plane_socket_mark(Some(mark));
        }
        let runtime_initial_peers = match load_bootstrap_peer_urls(
            self.get_bootstrap_state_dir().map(PathBuf::as_path),
            &cfg.get_network_identity(),
        ) {
            Ok(peers) => peers,
            Err(error) => {
                log::warn!(%error, "failed to load auxiliary bootstrap peer snapshot");
                Vec::new()
            }
        };
        let mihomo_request = if let Some(policy) = cfg.get_policy_proxy_config() {
            policy.validate_runtime_support()?;
            if policy.is_mihomo_enabled() {
                Some(build_mihomo_start_request(
                    &cfg,
                    policy,
                    self.get_config_dir().map(PathBuf::as_path),
                )?)
            } else {
                None
            }
        } else {
            None
        };
        if cfg.get_flags().no_tun && self.nic_backend != NicBackend::Tun {
            anyhow::bail!("--no-tun conflicts with --nic-backend veth/auto");
        }
        let instance_id = cfg.get_id();
        let mihomo_ownership_token = mihomo_request
            .as_ref()
            .map(|request| request.ownership_token);
        if self.instance_map.contains_key(&instance_id) {
            anyhow::bail!("instance {} already exists", instance_id);
        }
        if mihomo_request.is_some() && !watch_event {
            anyhow::bail!(
                "Mihomo policy instances require lifecycle watching so Core can guarantee cleanup"
            );
        }

        let mut instance = NetworkInstance::new(cfg, config_file_control);
        instance.set_runtime_initial_peers(runtime_initial_peers);
        instance.start()?;
        if let Some(request) = mihomo_request {
            self.mihomo_owner.start(request)?;
        }

        self.instance_map.insert(instance_id, instance);
        if watch_event
            && let Err(error) = self.start_instance_task(instance_id, mihomo_ownership_token)
        {
            self.instance_map.remove(&instance_id);
            let _ = self.mihomo_owner.stop(instance_id);
            return Err(error);
        }
        Ok(instance_id)
    }

    pub async fn validate_mihomo_candidate(
        &self,
        cfg: &TomlConfigLoader,
        contents: String,
    ) -> anyhow::Result<()> {
        let policy = cfg
            .get_policy_proxy_config()
            .ok_or_else(|| anyhow::anyhow!("Mihomo policy config is unavailable"))?;
        anyhow::ensure!(
            policy.is_mihomo_enabled(),
            "selected policy backend is not Mihomo"
        );
        let mut request =
            build_mihomo_start_request(cfg, policy, self.get_config_dir().map(PathBuf::as_path))?;
        request.source = MihomoConfigSource::Inline {
            label: format!("edited Mihomo config for network {}", cfg.get_id()),
            contents: contents.into(),
        };
        self.mihomo_owner.validate(request).await
    }

    pub async fn prepare_mihomo_geox_resources(
        &self,
        cfg: &TomlConfigLoader,
        contents: String,
        proxy: crate::mihomo::MihomoGeoxProxy,
    ) -> anyhow::Result<Vec<crate::mihomo::MihomoPreparedGeoxResource>> {
        let policy = cfg
            .get_policy_proxy_config()
            .ok_or_else(|| anyhow::anyhow!("Mihomo policy config is unavailable"))?;
        anyhow::ensure!(
            policy.is_mihomo_enabled(),
            "selected policy backend is not Mihomo"
        );
        let mut request =
            build_mihomo_start_request(cfg, policy, self.get_config_dir().map(PathBuf::as_path))?;
        request.source = MihomoConfigSource::Inline {
            label: format!("edited Mihomo config for network {}", cfg.get_id()),
            contents: contents.clone().into(),
        };
        let install =
            crate::mihomo::prepare_mihomo_geox_resources(&request, &contents, proxy).await?;
        self.mihomo_owner.validate(request).await?;
        Ok(install.commit())
    }

    pub fn control_mihomo_runtime(
        &self,
        instance_id: uuid::Uuid,
        action: &str,
    ) -> anyhow::Result<()> {
        let ownership_token = self
            .instance_stop_tasks
            .get(&instance_id)
            .map(|task| task.mihomo_ownership_token.clone())
            .ok_or_else(|| anyhow::anyhow!("network instance {} is not running", instance_id))?;

        if action == "stop" || action == "restart" {
            self.mihomo_owner.stop(instance_id)?;
            *ownership_token
                .write()
                .map_err(|_| anyhow::anyhow!("Mihomo ownership state is unavailable"))? = None;
            if action == "stop" {
                return Ok(());
            }
        } else if action != "start" {
            anyhow::bail!("unsupported Mihomo runtime action: {action}");
        }

        let instance = self
            .instance_map
            .get(&instance_id)
            .ok_or_else(|| anyhow::anyhow!("network instance {} is not running", instance_id))?;
        let config = instance.get_config();
        drop(instance);
        let policy = config
            .get_policy_proxy_config()
            .ok_or_else(|| anyhow::anyhow!("Mihomo policy config is unavailable"))?;
        anyhow::ensure!(
            policy.is_mihomo_enabled(),
            "network instance does not use the Mihomo policy backend"
        );
        let request = build_mihomo_start_request(
            &config,
            policy,
            self.get_config_dir().map(PathBuf::as_path),
        )?;
        let new_token = request.ownership_token;
        *ownership_token
            .write()
            .map_err(|_| anyhow::anyhow!("Mihomo ownership state is unavailable"))? =
            Some(new_token);

        if let Err(error) = self.mihomo_owner.start(request) {
            *ownership_token
                .write()
                .map_err(|_| anyhow::anyhow!("Mihomo ownership state is unavailable"))? = None;
            return Err(error);
        }

        let task_still_owns_instance = self
            .instance_stop_tasks
            .get(&instance_id)
            .is_some_and(|task| Arc::ptr_eq(&task.mihomo_ownership_token, &ownership_token));
        if !self.instance_map.contains_key(&instance_id) || !task_still_owns_instance {
            self.mihomo_owner.stop_if_owned(instance_id, new_token)?;
            anyhow::bail!("network instance stopped while Mihomo was starting");
        }
        Ok(())
    }

    pub fn retain_network_instance(
        &self,
        instance_ids: Vec<uuid::Uuid>,
    ) -> Result<Vec<uuid::Uuid>, anyhow::Error> {
        let removed_ids = self
            .list_network_instance_ids()
            .into_iter()
            .filter(|instance_id| !instance_ids.contains(instance_id))
            .collect::<Vec<_>>();
        for instance_id in &removed_ids {
            self.mihomo_owner.stop(*instance_id)?;
        }
        for instance_id in removed_ids {
            self.remove_instance_and_persist_bootstrap(instance_id);
        }
        self.instance_map.shrink_to_fit();
        self.instance_error_messages
            .retain(|k, _| instance_ids.contains(k));
        self.instance_error_messages.shrink_to_fit();
        Ok(self.list_network_instance_ids())
    }

    pub fn delete_network_instance(
        &self,
        instance_ids: Vec<uuid::Uuid>,
    ) -> Result<Vec<uuid::Uuid>, anyhow::Error> {
        for instance_id in &instance_ids {
            self.mihomo_owner.stop(*instance_id)?;
        }
        for instance_id in &instance_ids {
            self.remove_instance_and_persist_bootstrap(*instance_id);
        }
        self.instance_map.shrink_to_fit();
        self.instance_error_messages
            .retain(|k, _| !instance_ids.contains(k));
        self.instance_error_messages.shrink_to_fit();
        Ok(self.list_network_instance_ids())
    }

    pub async fn collect_network_infos(
        &self,
    ) -> Result<BTreeMap<uuid::Uuid, NetworkInstanceRunningInfo>, anyhow::Error> {
        let mut ret = BTreeMap::new();
        for instance in self.instance_map.iter() {
            if let Ok(info) = instance.get_running_info().await {
                ret.insert(*instance.key(), info);
            }
        }
        for v in self.instance_error_messages.iter() {
            ret.insert(
                *v.key(),
                NetworkInstanceRunningInfo {
                    error_msg: Some(v.value().clone()),
                    ..Default::default()
                },
            );
        }
        let mihomo = self.mihomo_owner.status().await;
        if let Some(owner) = mihomo.owner_instance_id
            && let Some(info) = ret.get_mut(&owner)
        {
            info.mihomo_status = Some(mihomo_status_proto(&mihomo));
        }
        let neutral_mesh_entry_status = self.get_neutral_mesh_entry_status();
        for info in ret.values_mut() {
            info.neutral_mesh_entry_status = Some(neutral_mesh_entry_status.clone());
        }
        Ok(ret)
    }

    pub fn collect_network_infos_sync(
        &self,
    ) -> Result<BTreeMap<uuid::Uuid, NetworkInstanceRunningInfo>, anyhow::Error> {
        tokio::runtime::Runtime::new()?.block_on(self.collect_network_infos())
    }

    #[cfg(feature = "ffi-dataplane")]
    pub async fn data_plane_tcp_connect(
        &self,
        instance_id: &uuid::Uuid,
        dst_addr: std::net::SocketAddr,
        timeout: std::time::Duration,
    ) -> Result<DataPlaneTcpStream, anyhow::Error> {
        let instance = self
            .instance_map
            .get(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance {} not found", instance_id))?;
        instance.data_plane_tcp_connect(dst_addr, timeout).await
    }

    #[cfg(feature = "ffi-dataplane")]
    pub async fn data_plane_tcp_bind(
        &self,
        instance_id: &uuid::Uuid,
        local_port: u16,
        timeout: std::time::Duration,
    ) -> Result<DataPlaneTcpListener, anyhow::Error> {
        let instance = self
            .instance_map
            .get(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance {} not found", instance_id))?;
        instance.data_plane_tcp_bind(local_port, timeout).await
    }

    #[cfg(feature = "ffi-dataplane")]
    pub async fn data_plane_udp_bind(
        &self,
        instance_id: &uuid::Uuid,
        local_port: u16,
        timeout: std::time::Duration,
    ) -> Result<DataPlaneUdpSocket, anyhow::Error> {
        let instance = self
            .instance_map
            .get(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance {} not found", instance_id))?;
        instance.data_plane_udp_bind(local_port, timeout).await
    }

    #[cfg(feature = "ffi-dataplane")]
    pub fn data_plane_wait_runtime_handle(
        &self,
        instance_id: &uuid::Uuid,
        timeout: std::time::Duration,
    ) -> Option<tokio::runtime::Handle> {
        self.instance_map
            .get(instance_id)
            .and_then(|inst| inst.wait_runtime_handle(timeout))
    }

    pub async fn get_network_info(
        &self,
        instance_id: &uuid::Uuid,
    ) -> Option<NetworkInstanceRunningInfo> {
        if let Some(err_msg) = self.instance_error_messages.get(instance_id) {
            return Some(NetworkInstanceRunningInfo {
                error_msg: Some(err_msg.value().clone()),
                neutral_mesh_entry_status: Some(self.get_neutral_mesh_entry_status()),
                ..Default::default()
            });
        }
        let mut info = self
            .instance_map
            .get(instance_id)?
            .get_running_info()
            .await
            .ok()?;
        let mihomo = self.mihomo_owner.status().await;
        if mihomo.owner_instance_id.as_ref() == Some(instance_id) {
            info.mihomo_status = Some(mihomo_status_proto(&mihomo));
        }
        info.neutral_mesh_entry_status = Some(self.get_neutral_mesh_entry_status());
        Some(info)
    }

    pub async fn get_mihomo_status(&self) -> MihomoCoreStatus {
        self.mihomo_owner.status().await
    }

    pub async fn get_mihomo_dashboard_url(
        &self,
        instance_id: uuid::Uuid,
    ) -> anyhow::Result<String> {
        self.mihomo_owner.dashboard_url(instance_id).await
    }

    pub fn shutdown_mihomo_owner(&self) -> anyhow::Result<()> {
        self.mihomo_owner.shutdown()
    }

    pub fn get_neutral_mesh_entry_status(
        &self,
    ) -> crate::proto::api::manage::NeutralMeshEntryStatus {
        #[cfg(feature = "mesh-socks-egress")]
        {
            CoreMeshEntryHandle::global().status().into_proto()
        }

        #[cfg(not(feature = "mesh-socks-egress"))]
        crate::proto::api::manage::NeutralMeshEntryStatus {
            state: "unsupported".to_owned(),
            endpoint: None,
            selected_port: None,
            backend: "disabled".to_owned(),
            core_leases: 0,
            restart_count: 0,
            last_error: None,
        }
    }

    pub fn list_network_instance_ids(&self) -> Vec<uuid::Uuid> {
        self.instance_map.iter().map(|item| *item.key()).collect()
    }

    pub fn get_instance_name(&self, instance_id: &uuid::Uuid) -> Option<String> {
        self.instance_map
            .get(instance_id)
            .map(|instance| instance.value().get_inst_name())
    }

    pub fn get_network_name(&self, instance_id: &uuid::Uuid) -> Option<String> {
        self.instance_map
            .get(instance_id)
            .map(|instance| instance.value().get_network_name())
    }

    pub fn iter(&self) -> dashmap::iter::Iter<'_, uuid::Uuid, NetworkInstance> {
        self.instance_map.iter()
    }

    pub fn get_instance_config_control(
        &self,
        instance_id: &uuid::Uuid,
    ) -> Option<ConfigFileControl> {
        self.instance_map
            .get(instance_id)
            .map(|instance| instance.value().get_config_file_control().clone())
    }

    pub fn get_instance_config(&self, instance_id: &uuid::Uuid) -> Option<TomlConfigLoader> {
        self.instance_map
            .get(instance_id)
            .map(|instance| instance.value().get_config())
    }

    pub fn get_instance_network_config_source(
        &self,
        instance_id: &uuid::Uuid,
    ) -> Option<ConfigSource> {
        self.instance_map
            .get(instance_id)
            .map(|instance| instance.value().get_network_config_source())
    }

    pub fn get_instance_service(
        &self,
        instance_id: &uuid::Uuid,
    ) -> Option<Arc<dyn InstanceRpcService>> {
        self.instance_map
            .get(instance_id)
            .and_then(|instance| instance.value().get_api_service())
    }

    pub fn set_tun_fd(&self, instance_id: &uuid::Uuid, fd: i32) -> Result<(), anyhow::Error> {
        self.set_mobile_tun(instance_id, fd, Vec::new())
    }

    pub fn set_mobile_tun(
        &self,
        instance_id: &uuid::Uuid,
        fd: i32,
        dns_servers: Vec<std::net::IpAddr>,
    ) -> Result<(), anyhow::Error> {
        let sender = self
            .instance_map
            .get(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance not found"))?
            .get_tun_fd_sender()
            .ok_or_else(|| anyhow::anyhow!("tun fd sender not found"))?;

        sender
            .try_send(Some(crate::launcher::MobileTunConfig {
                fd,
                dns_servers,
                network_key: String::new(),
                completion: None,
            }))
            .map_err(|e| anyhow::anyhow!("failed to send tun fd: {}", e))?;

        Ok(())
    }

    pub async fn set_mobile_tun_and_wait(
        &self,
        instance_id: &uuid::Uuid,
        fd: i32,
        dns_servers: Vec<std::net::IpAddr>,
        network_key: String,
    ) -> Result<(), anyhow::Error> {
        let sender = self
            .instance_map
            .get(instance_id)
            .ok_or_else(|| anyhow::anyhow!("instance not found"))?
            .get_tun_fd_sender()
            .ok_or_else(|| anyhow::anyhow!("tun fd sender not found"))?;
        let (completion, result) = tokio::sync::oneshot::channel();
        sender
            .try_send(Some(crate::launcher::MobileTunConfig {
                fd,
                dns_servers,
                network_key,
                completion: Some(completion),
            }))
            .map_err(|error| anyhow::anyhow!("failed to send tun fd: {error}"))?;
        tokio::time::timeout(std::time::Duration::from_secs(20), result)
            .await
            .map_err(|_| anyhow::anyhow!("mobile virtual NIC readiness timed out"))?
            .map_err(|_| anyhow::anyhow!("mobile virtual NIC setup task stopped"))?
            .map_err(anyhow::Error::msg)
    }

    pub fn update_mobile_network(
        &self,
        instance_id: &uuid::Uuid,
        state: crate::launcher::MobileNetworkState,
    ) -> Result<(), anyhow::Error> {
        #[cfg(mobile)]
        {
            self.instance_map
                .get(instance_id)
                .ok_or_else(|| anyhow::anyhow!("instance not found"))?
                .update_mobile_network(state)
        }
        #[cfg(not(mobile))]
        {
            let _ = (instance_id, state);
            anyhow::bail!("mobile network updates are unavailable on this platform")
        }
    }

    pub fn get_config_dir(&self) -> Option<&PathBuf> {
        self.config_dir.get()
    }

    pub fn get_bootstrap_state_dir(&self) -> Option<&PathBuf> {
        self.bootstrap_state_dir
            .get()
            .or_else(|| self.config_dir.get())
    }

    fn remove_instance_and_persist_bootstrap(&self, instance_id: uuid::Uuid) {
        let Some((_, mut instance)) = self.instance_map.remove(&instance_id) else {
            return;
        };
        let identity = instance.get_config().get_network_identity();
        let urls = instance.stop_and_take_bootstrap_peer_urls();
        if let Err(error) = persist_bootstrap_peer_urls(
            self.get_bootstrap_state_dir().map(PathBuf::as_path),
            &identity,
            &urls,
        ) {
            log::warn!(%error, %instance_id, "failed to persist auxiliary bootstrap peer snapshot");
        }
    }

    pub fn shutdown_and_persist_instances(&self) {
        for instance_id in self.list_network_instance_ids() {
            if let Err(error) = self.mihomo_owner.stop(instance_id) {
                log::warn!(%error, %instance_id, "failed to stop Mihomo during manager shutdown");
            }
            self.remove_instance_and_persist_bootstrap(instance_id);
        }
        self.instance_map.shrink_to_fit();
    }

    pub(crate) fn register_daemon(&self) -> DaemonGuard {
        DaemonGuard {
            guard: Some(self.guard_counter.clone()),
            stop_check_notifier: self.stop_check_notifier.clone(),
        }
    }

    pub(crate) fn notify_stop_check(&self) {
        self.stop_check_notifier.notify_one();
    }

    pub async fn wait(&self) {
        loop {
            let local_instance_running = self
                .instance_map
                .iter()
                .any(|item| item.value().is_easytier_running());
            let daemon_running = Arc::strong_count(&self.guard_counter) > 1;

            if !local_instance_running && !daemon_running {
                break;
            }

            self.stop_check_notifier.notified().await;
        }
    }
}

impl Drop for NetworkInstanceManager {
    fn drop(&mut self) {
        self.shutdown_and_persist_instances();
    }
}

macro_rules! event {
    ($lvl:ident, category: $cat:expr, $($args:tt)+) => {
        event!(@impl $lvl, concat!("INSTANCE::", $cat), $($args)+)
    };

    ($lvl:ident, $($args:tt)+) => {
        event!(@impl $lvl, "INSTANCE", $($args)+)
    };

    (@impl $lvl:ident, $cat:expr, $($args:tt)+) => {
        log::$lvl!(
            category: $cat,
            $($args)+
        );
    };
}

#[tracing::instrument]
fn handle_event(
    instance_id: uuid::Uuid,
    mut events: EventBusSubscriber,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Ok(e) = events.recv().await {
                match e {
                    GlobalCtxEvent::PeerAdded(peer_id) => {
                        event!(info, peer_id, "[{}] new peer added", instance_id);
                    }

                    GlobalCtxEvent::PeerRemoved(peer_id) => {
                        event!(info, peer_id, "[{}] peer removed", instance_id);
                    }

                    GlobalCtxEvent::PeerConnAdded(conn_info) => {
                        event!(
                            info,
                            category: "CONNECTION",
                            %conn_info,
                            "[{}] new peer connection added",
                            instance_id,
                        );
                    }

                    GlobalCtxEvent::PeerConnRemoved(conn_info) => {
                        event!(
                            info,
                            category: "CONNECTION",
                            %conn_info,
                            "[{}] peer connection removed",
                            instance_id,
                        );
                    }

                    GlobalCtxEvent::ListenerAddFailed(listener, msg) => {
                        event!(warn, %listener, msg, "[{}] listener add failed", instance_id);
                    }

                    GlobalCtxEvent::ListenerAcceptFailed(listener, msg) => {
                        event!(warn,  %listener, msg, "[{}] listener accept failed", instance_id);
                    }

                    GlobalCtxEvent::ListenerAdded(listener) => {
                        if listener.scheme() == "ring" {
                            continue;
                        }
                        event!(
                            info,
                            %listener,
                            "[{}] new listener added",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::ConnectionAccepted(local, remote) => {
                        event!(info, category: "CONNECTION", local, remote, "[{}] new connection accepted", instance_id);
                    }

                    GlobalCtxEvent::ConnectionError(local, remote, err) => {
                        event!(info, category: "CONNECTION", local, remote, err, "[{}] connection error", instance_id);
                    }

                    GlobalCtxEvent::ListenerPortMappingEstablished {
                        local_listener,
                        mapped_listener,
                        backend,
                    } => {
                        event!(
                            info,
                            %local_listener,
                            %mapped_listener,
                            backend,
                            "[{}] listener port mapping established",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::TunDeviceReady(dev) => {
                        event!(info, dev, "[{}] tun device ready", instance_id);
                    }

                    GlobalCtxEvent::TunDeviceError(err) => {
                        event!(error, %err, "[{}] tun device error", instance_id);
                    }

                    GlobalCtxEvent::Connecting(dst) => {
                        event!(info, category: "CONNECTION", %dst, "[{}] connecting to peer", instance_id);
                    }

                    GlobalCtxEvent::ConnectError(dst, ip_version, error) => {
                        event!(
                            info,
                            category: "CONNECTION",
                            dst,
                            ip_version,
                            %error,
                            "[{}] connect to peer error",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::VpnPortalStarted(portal) => {
                        event!(info, portal, "[{}] vpn portal started", instance_id);
                    }

                    GlobalCtxEvent::VpnPortalClientConnected(portal, client_addr) => {
                        event!(
                            info,
                            portal,
                            client_addr,
                            "[{}] vpn portal client connected",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::VpnPortalClientDisconnected(portal, client_addr) => {
                        event!(
                            info,
                            portal,
                            client_addr,
                            "[{}] vpn portal client disconnected",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::DhcpIpv4Changed(old, new) => {
                        event!(info, ?old, ?new, "[{}] dhcp ip changed", instance_id);
                    }

                    GlobalCtxEvent::DhcpIpv4Conflicted(ip) => {
                        event!(info, ?ip, "[{}] dhcp ip conflict", instance_id);
                    }

                    GlobalCtxEvent::PublicIpv6Changed(old, new) => {
                        event!(info, ?old, ?new, "[{}] public ipv6 changed", instance_id);
                    }

                    GlobalCtxEvent::PublicIpv6RoutesUpdated(added, removed) => {
                        event!(
                            info,
                            ?added,
                            ?removed,
                            "[{}] public ipv6 routes updated",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::PortForwardAdded(cfg) => {
                        event!(
                            info,
                            local = %cfg.bind_addr.unwrap(),
                            remote = %cfg.dst_addr.unwrap(),
                            proto = %cfg.socket_type().as_str_name(),
                            "[{}] port forward added",
                            instance_id,
                        );
                    }

                    GlobalCtxEvent::ConfigPatched(patch) => {
                        event!(info, ?patch, "[{}] config patched", instance_id);
                    }

                    GlobalCtxEvent::ProxyCidrsUpdated(added, removed) => {
                        event!(
                            info,
                            ?added,
                            ?removed,
                            "[{}] proxy CIDRs updated",
                            instance_id
                        );
                    }

                    GlobalCtxEvent::UdpBroadcastRelayStartResult {
                        capture_backend,
                        error,
                    } => {
                        if let Some(error) = error {
                            event!(
                                warn,
                                ?capture_backend,
                                %error,
                                "[{}] UDP broadcast relay start failed",
                                instance_id
                            );
                        } else {
                            event!(
                                info,
                                ?capture_backend,
                                "[{}] UDP broadcast relay started",
                                instance_id
                            );
                        }
                    }

                    GlobalCtxEvent::CredentialChanged => {
                        event!(info, "[{}] credential changed", instance_id);
                    }
                }
            } else {
                events = events.resubscribe();
            }
        }
    })
}

impl Display for proto::api::instance::PeerConnInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerConnInfo")
            .field("my_peer_id", &self.my_peer_id)
            .field("dst_peer_id", &self.peer_id)
            .field("tunnel_info", &self.tunnel)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config::*;

    #[cfg(feature = "mesh-socks-egress")]
    #[test]
    fn neutral_mesh_entry_first_core_owner_starts_and_other_networks_reuse_it() {
        assert!(neutral_mesh_entry_acquire_starts_owner(0));
        assert!(!neutral_mesh_entry_acquire_starts_owner(1));
        assert!(!neutral_mesh_entry_acquire_starts_owner(8));
    }

    #[cfg(feature = "mesh-socks-egress")]
    #[test]
    fn neutral_mesh_entry_only_stops_after_last_core_owner_releases() {
        assert!(!neutral_mesh_entry_release_stops_owner(2));
        assert!(!neutral_mesh_entry_release_stops_owner(1));
        assert!(neutral_mesh_entry_release_stops_owner(0));
    }

    #[cfg(feature = "mesh-socks-egress")]
    #[test]
    fn neutral_mesh_entry_status_publishes_selected_port_and_endpoint() {
        let endpoint = "127.0.0.1:11081".parse().unwrap();
        let status = NeutralMeshEntryStatus {
            state: NeutralMeshEntryState::Running,
            endpoint: Some(endpoint),
            selected_port: Some(11081),
            backend: "test".to_owned(),
            core_leases: 3,
            restart_count: 2,
            last_error: None,
        }
        .into_proto();

        assert_eq!(status.state, "running");
        assert_eq!(status.endpoint.as_deref(), Some("127.0.0.1:11081"));
        assert_eq!(status.selected_port, Some(11081));
        assert_eq!(status.core_leases, 3);
        assert_eq!(status.restart_count, 2);
    }

    #[cfg(feature = "mesh-socks-egress")]
    #[test]
    fn neutral_mesh_entry_handle_is_process_global() {
        let first = CoreMeshEntryHandle::global();
        let second = CoreMeshEntryHandle::global();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
    #[test]
    fn policy_toml_gets_a_default_socket_mark_without_overwriting_an_explicit_mark() {
        let config = TomlConfigLoader::new_from_str(
            r#"
[policy_proxy]
enabled = true
config_inline = 'version: 1\nrules: ["FINAL,DIRECT"]'
outbound_interface = "eth0"
"#,
        )
        .unwrap();
        assert_eq!(
            ensure_policy_socket_mark(&config).unwrap(),
            Some(crate::policy_proxy::POLICY_SOCKET_MARK)
        );
        assert_eq!(
            config.get_flags().socket_mark,
            Some(crate::policy_proxy::POLICY_SOCKET_MARK)
        );

        let mut flags = config.get_flags();
        flags.socket_mark = Some(77);
        config.set_flags(flags);
        assert_eq!(ensure_policy_socket_mark(&config).unwrap(), Some(77));
        assert_eq!(config.get_flags().socket_mark, Some(77));

        let mut flags = config.get_flags();
        flags.socket_mark = Some(0);
        config.set_flags(flags);
        assert!(
            ensure_policy_socket_mark(&config)
                .unwrap_err()
                .to_string()
                .contains("non-zero")
        );
    }

    #[test]
    fn bootstrap_cache_is_stable_isolated_and_tolerates_malformed_lines() {
        let shared = NetworkIdentity::new("mesh-a".to_owned(), "secret-a".to_owned());
        let shared_again = NetworkIdentity::new("mesh-a".to_owned(), "secret-a".to_owned());
        let other_secret = NetworkIdentity::new("mesh-a".to_owned(), "secret-b".to_owned());
        let credential = NetworkIdentity::new_credential("mesh-a".to_owned());
        assert_eq!(
            bootstrap_peer_cache_key(&shared),
            bootstrap_peer_cache_key(&shared_again)
        );
        assert_ne!(
            bootstrap_peer_cache_key(&shared),
            bootstrap_peer_cache_key(&other_secret)
        );
        assert_ne!(
            bootstrap_peer_cache_key(&shared),
            bootstrap_peer_cache_key(&credential)
        );

        let temp = tempfile::tempdir().unwrap();
        let path = bootstrap_peer_cache_path(temp.path(), &shared);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "not-a-url\nring://local-only\nunsupported://192.0.2.9:1\ntcp://192.0.2.2:11010\ntcp://192.0.2.2:11010\n",
        )
        .unwrap();
        let loaded = load_bootstrap_peer_urls(Some(temp.path()), &shared).unwrap();
        assert_eq!(loaded, vec!["tcp://192.0.2.2:11010".parse().unwrap()]);
        assert!(
            load_bootstrap_peer_urls(Some(temp.path()), &other_secret)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn nonempty_bootstrap_snapshot_replaces_file_and_empty_preserves_it() {
        let temp = tempfile::tempdir().unwrap();
        let identity = NetworkIdentity::new("mesh".to_owned(), "secret".to_owned());
        let first = vec![
            "udp://192.0.2.3:11010".parse().unwrap(),
            "tcp://192.0.2.2:11010".parse().unwrap(),
            "udp://192.0.2.3:11010".parse().unwrap(),
            "ring://ignored".parse().unwrap(),
        ];
        persist_bootstrap_peer_urls(Some(temp.path()), &identity, &first).unwrap();
        let path = bootstrap_peer_cache_path(temp.path(), &identity);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "tcp://192.0.2.2:11010\nudp://192.0.2.3:11010\n");

        persist_bootstrap_peer_urls(Some(temp.path()), &identity, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);

        persist_bootstrap_peer_urls(Some(temp.path()), &identity, &first).unwrap();
        assert!(!bootstrap_peer_backup_path(temp.path(), &identity).exists());

        let subset = vec!["tcp://192.0.2.2:11010".parse().unwrap()];
        persist_bootstrap_peer_urls(Some(temp.path()), &identity, &subset).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        assert!(!bootstrap_peer_backup_path(temp.path(), &identity).exists());

        let changed = vec![
            "tcp://192.0.2.2:11010".parse().unwrap(),
            "udp://192.0.2.4:11010".parse().unwrap(),
        ];
        persist_bootstrap_peer_urls(Some(temp.path()), &identity, &changed).unwrap();
        assert_eq!(
            std::fs::read_to_string(bootstrap_peer_backup_path(temp.path(), &identity)).unwrap(),
            contents
        );
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "tcp://192.0.2.2:11010\nudp://192.0.2.4:11010\n"
        );
    }

    #[test]
    fn bootstrap_primary_is_preferred_and_invalid_primary_falls_back_once() {
        let temp = tempfile::tempdir().unwrap();
        let identity = NetworkIdentity::new("mesh".to_owned(), "secret".to_owned());
        let primary = bootstrap_peer_cache_path(temp.path(), &identity);
        let backup = bootstrap_peer_backup_path(temp.path(), &identity);
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::write(&primary, "tcp://192.0.2.1:11010\n").unwrap();
        std::fs::write(&backup, "tcp://192.0.2.2:11010\n").unwrap();
        assert_eq!(
            load_bootstrap_peer_urls(Some(temp.path()), &identity).unwrap(),
            vec!["tcp://192.0.2.1:11010".parse().unwrap()]
        );

        std::fs::write(&primary, "ring://local\nunsupported://host\n").unwrap();
        assert_eq!(
            load_bootstrap_peer_urls(Some(temp.path()), &identity).unwrap(),
            vec!["tcp://192.0.2.2:11010".parse().unwrap()]
        );
    }

    #[test]
    fn manager_persistent_path_is_optional_and_initialized_once() {
        let manager = NetworkInstanceManager::new();
        assert!(manager.get_config_dir().is_none());
        let first = std::env::temp_dir().join("easytier-bootstrap-first");
        manager.set_config_path(first.clone()).unwrap();
        manager.set_config_path(first.clone()).unwrap();
        assert_eq!(manager.get_config_dir(), Some(&first));
        assert!(
            manager
                .set_config_path(std::env::temp_dir().join("easytier-bootstrap-second"))
                .is_err()
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn it_works() {
        let manager = NetworkInstanceManager::new();
        let cfg_str = r#"
            listeners = []
            "#;

        let port = crate::utils::find_free_tcp_port(10012..65534).expect("no free tcp port found");

        let instance_id1 = manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str)
                    .inspect(|c| {
                        c.set_listeners(vec![format!("tcp://0.0.0.0:{}", port).parse().unwrap()]);
                    })
                    .unwrap(),
                true,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();
        let instance_id2 = manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                true,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();
        let instance_id3 = manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                false,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();
        let instance_id4 = manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                true,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();
        let instance_id5 = manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                false,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await; // to make instance actually started

        assert!(!crate::utils::check_tcp_available(port));

        assert!(manager.instance_map.contains_key(&instance_id1));
        assert!(manager.instance_map.contains_key(&instance_id2));
        assert!(manager.instance_map.contains_key(&instance_id3));
        assert!(manager.instance_map.contains_key(&instance_id4));
        assert!(manager.instance_map.contains_key(&instance_id5));
        assert_eq!(manager.list_network_instance_ids().len(), 5);
        assert_eq!(manager.instance_stop_tasks.len(), 3); // FFI and GUI instance does not have a stop task

        manager
            .delete_network_instance(vec![instance_id3, instance_id4, instance_id5])
            .unwrap();
        assert!(!manager.instance_map.contains_key(&instance_id3));
        assert!(!manager.instance_map.contains_key(&instance_id4));
        assert!(!manager.instance_map.contains_key(&instance_id5));
        assert_eq!(manager.list_network_instance_ids().len(), 2);
    }

    #[test]
    #[serial_test::serial]
    fn watcher_registration_failure_rolls_back_the_new_instance() {
        let manager = NetworkInstanceManager::new();
        let cfg_str = r#"
            listeners = []
            "#;

        let port = crate::utils::find_free_tcp_port(10012..65534).expect("no free tcp port found");

        assert!(
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                    true,
                    ConfigFileControl::STATIC_CONFIG
                )
                .is_err()
        );
        assert!(
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                    true,
                    ConfigFileControl::STATIC_CONFIG
                )
                .is_err()
        );
        assert!(
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str)
                        .inspect(|c| {
                            c.set_listeners(vec![
                                format!("tcp://0.0.0.0:{}", port).parse().unwrap(),
                            ]);
                        })
                        .unwrap(),
                    false,
                    ConfigFileControl::STATIC_CONFIG
                )
                .is_ok()
        );
        assert!(
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                    true,
                    ConfigFileControl::STATIC_CONFIG
                )
                .is_err()
        );
        assert!(
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str).unwrap(),
                    false,
                    ConfigFileControl::STATIC_CONFIG
                )
                .is_ok()
        );

        std::thread::sleep(std::time::Duration::from_secs(1)); // wait instance actually started

        assert!(!crate::utils::check_tcp_available(port));

        assert_eq!(manager.list_network_instance_ids().len(), 2);
        assert_eq!(
            manager
                .instance_map
                .iter()
                .map(|item| item.is_easytier_running())
                .filter(|x| *x)
                .count(),
            2
        ); // Failed watcher registration must not leave unmanaged instances running.
        assert_eq!(manager.instance_stop_tasks.len(), 0);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_single_instance_failed() {
        let free_tcp_port =
            crate::utils::find_free_tcp_port(10012..65534).expect("no free tcp port found");

        // Test with event watching enabled (for CLI/File/RPC usage) - instance should auto-stop on error
        for watch_event in [true] {
            let _port_holder =
                std::net::TcpListener::bind(format!("0.0.0.0:{}", free_tcp_port)).unwrap();

            let cfg_str = format!(
                r#"
            listeners = ["tcp://0.0.0.0:{}"]
            "#,
                free_tcp_port
            );

            let manager = NetworkInstanceManager::new();
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str.as_str()).unwrap(),
                    watch_event,
                    ConfigFileControl::STATIC_CONFIG,
                )
                .unwrap();

            tokio::select! {
                _ = manager.wait() => {
                    assert_eq!(manager.list_network_instance_ids().len(), 1);
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    panic!("instance manager with single failed instance({:?}) should not running", watch_event);
                }
            }
        }

        // Test without event watching (for FFI usage) - instance should remain even if failed
        {
            let watch_event = false;
            let _port_holder =
                std::net::TcpListener::bind(format!("0.0.0.0:{}", free_tcp_port)).unwrap();

            let cfg_str = format!(
                r#"
            listeners = ["tcp://0.0.0.0:{}"]
            "#,
                free_tcp_port
            );

            let manager = NetworkInstanceManager::new();
            manager
                .run_network_instance(
                    TomlConfigLoader::new_from_str(cfg_str.as_str()).unwrap(),
                    watch_event,
                    ConfigFileControl::STATIC_CONFIG,
                )
                .unwrap();

            assert_eq!(manager.list_network_instance_ids().len(), 1);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_multiple_instances_one_failed() {
        let free_tcp_port =
            crate::utils::find_free_tcp_port(10012..65534).expect("no free tcp port found");

        let manager = NetworkInstanceManager::new();
        let cfg_str = format!(
            r#"
            listeners = ["tcp://0.0.0.0:{}"]
            [flags]
            enable_ipv6 = false
            "#,
            free_tcp_port
        );

        manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str.as_str()).unwrap(),
                true,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        manager
            .run_network_instance(
                TomlConfigLoader::new_from_str(cfg_str.as_str()).unwrap(),
                true,
                ConfigFileControl::STATIC_CONFIG,
            )
            .unwrap();

        tokio::select! {
            _ = manager.wait() => {
                panic!("instance manager with multiple instances one failed should still running");
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                assert_eq!(manager.list_network_instance_ids().len(), 2);
            }
        }
    }
}
