//! Narrow lifecycle wrapper for the HEV SOCKS5 egress service.
//!
//! The platform backend owns SOCKS5 TCP/UDP protocol state. Linux and macOS use
//! HEV, Android retains its in-process HEV path, and Windows uses the locked
//! minimal Leaf runtime. FreeBSD's neutral mesh entry is provided separately by
//! GOST; OHOS has no managed sidecar in this crate. This crate owns process
//! lifetime, private configuration, and deterministic TCP listener fallback
//! without depending on EasyTier routing, DNS, policy, or mesh types.

use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Write as _,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context as _, bail};
use tokio::{net::TcpStream, process::Child};

pub const MESH_ENTRY_PORT_CANDIDATES: [u16; 3] = [11080, 11081, 11082];
pub const LEAF_DIRECT_EGRESS_PORT_CANDIDATES: [u16; 3] = [11180, 11181, 11182];
pub const DEFAULT_PORT_CANDIDATES: [u16; 3] = MESH_ENTRY_PORT_CANDIDATES;

const START_TIMEOUT: Duration = Duration::from_secs(2);
const READY_INTERVAL: Duration = Duration::from_millis(25);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(all(
    feature = "managed-sidecar-bin",
    any(target_os = "linux", target_os = "macos"),
    not(target_env = "ohos")
))]
unsafe extern "C" {
    fn hev_socks5_server_main_from_file(config_path: *const std::ffi::c_char) -> std::ffi::c_int;
}

/// Runs the target's managed SOCKS server in the sidecar process.
///
/// Linux and macOS use pinned HEV. Windows uses the already
/// pinned Leaf core with only SOCKS inbound, direct outbound, and JSON config
/// enabled. Keeping the portable backend in a separate process avoids sharing
/// Leaf's global runtime IDs or lifecycle with policy routing.
#[cfg(all(
    feature = "managed-sidecar-bin",
    any(target_os = "linux", target_os = "macos"),
    not(target_env = "ohos")
))]
pub fn run_managed_server_from_file(config_path: &Path, _workers: usize) -> Result<(), String> {
    let config_path = std::ffi::CString::new(config_path.as_os_str().as_encoded_bytes())
        .map_err(|_| "HEV configuration path contains a NUL byte".to_owned())?;
    let status = unsafe { hev_socks5_server_main_from_file(config_path.as_ptr()) };
    if status != 0 {
        return Err(format!("HEV exited with status {status}"));
    }
    Ok(())
}

#[cfg(all(feature = "managed-sidecar-bin", windows))]
pub fn run_managed_server_from_file(config_path: &Path, workers: usize) -> Result<(), String> {
    if !(1..=32).contains(&workers) {
        return Err("portable SOCKS worker count must be in 1..=32".to_owned());
    }
    let runtime_opt = if workers == 1 {
        leaf::RuntimeOption::SingleThread
    } else {
        leaf::RuntimeOption::MultiThread(workers, 2 * 1024 * 1024)
    };
    leaf::start(
        0,
        leaf::StartOptions {
            config: leaf::Config::File(config_path.to_string_lossy().into_owned()),
            runtime_opt,
        },
    )
    .map_err(|error| format!("portable Leaf SOCKS server failed: {error}"))
}

pub fn managed_backend_name() -> &'static str {
    option_env!("EASYTIER_SOCKS_BACKEND").unwrap_or("unavailable")
}

pub fn managed_backend_revision() -> &'static str {
    option_env!("EASYTIER_SOCKS_BACKEND_REV").unwrap_or("unknown")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksEgressConfig {
    pub listen_address: IpAddr,
    pub port_candidates: Vec<u16>,
    pub workers: usize,
    pub bind_interface: Option<String>,
    pub socket_mark: Option<u32>,
}

impl Default for SocksEgressConfig {
    fn default() -> Self {
        Self {
            // Mihomo 0a87b948 component/tsnet/tsnet.go::{retryStartSocks5TCP,
            // retryStartSocks5UDP} exposes its SOCKS ingress only inside the
            // userspace tailnet. EasyTier owns the equivalent userspace mesh
            // ingress separately, so the private hop into HEV stays loopback-only.
            listen_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port_candidates: DEFAULT_PORT_CANDIDATES.to_vec(),
            workers: 1,
            bind_interface: None,
            socket_mark: None,
        }
    }
}

impl SocksEgressConfig {
    /// Public loopback SOCKS entry whose outbound sockets follow the host routing
    /// table. Do not bind this role to a physical interface or apply the Leaf
    /// policy mark: EasyTier virtual destinations must remain able to enter the
    /// EasyTier TUN.
    pub fn mesh_entry() -> Self {
        Self::default()
    }

    /// Private Leaf egress hop. This role deliberately uses a disjoint listener
    /// range and retains the platform loop-prevention binding selected by Leaf.
    pub fn leaf_direct_egress(bind_interface: Option<String>, socket_mark: Option<u32>) -> Self {
        Self {
            port_candidates: LEAF_DIRECT_EGRESS_PORT_CANDIDATES.to_vec(),
            bind_interface,
            socket_mark,
            ..Self::default()
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.port_candidates.is_empty() || self.port_candidates.len() > 8 {
            bail!("HEV requires 1..=8 TCP port candidates");
        }
        let mut unique = BTreeSet::new();
        if self
            .port_candidates
            .iter()
            .any(|port| *port == 0 || !unique.insert(*port))
        {
            bail!("HEV TCP port candidates must be unique and non-zero");
        }
        if !(1..=32).contains(&self.workers) {
            bail!("HEV worker count must be in 1..=32");
        }
        if self.bind_interface.as_deref().is_some_and(|name| {
            name.is_empty()
                || name
                    .chars()
                    .any(|character| matches!(character, '\n' | '\r' | '\0'))
        }) {
            bail!("HEV bind interface contains unsupported characters");
        }
        #[cfg(windows)]
        if self.bind_interface.is_some() || self.socket_mark.is_some() {
            bail!(
                "the portable SOCKS backend is a neutral mesh entry and cannot bind a physical interface or apply a policy socket mark"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessConfig {
    pub executable: PathBuf,
    pub server: SocksEgressConfig,
}

impl ProcessConfig {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            server: SocksEgressConfig::default(),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.executable.as_os_str().is_empty() {
            bail!("HEV executable path is empty");
        }
        self.server.validate()
    }
}

pub struct ProcessRuntime {
    child: Child,
    endpoint: SocketAddr,
}

impl ProcessRuntime {
    pub async fn start(config: ProcessConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let private_dir = tempfile::Builder::new()
            .prefix("easytier-hev-")
            .tempdir()
            .context("failed to create private HEV configuration directory")?;
        let mut failures = Vec::new();

        for port in &config.server.port_candidates {
            if let Err(error) =
                ensure_candidate_listener_available(config.server.listen_address, *port)
            {
                failures.push(format!("{port}: {error:#}"));
                continue;
            }
            let config_path = private_dir
                .path()
                .join(format!("managed-{port}.{}", managed_config_extension()));
            write_private_file(
                &config_path,
                render_managed_config(&config.server, *port).as_bytes(),
            )
            .with_context(|| {
                format!("failed to write managed SOCKS configuration for port {port}")
            })?;
            match start_candidate(
                &config.executable,
                &config_path,
                config.server.listen_address,
                *port,
                config.server.workers,
            )
            .await
            {
                Ok((mut child, endpoint)) => {
                    // HEV parses the complete YAML in
                    // hev-config.c::hev_config_init_from_file before opening the listener. Do
                    // not retain configuration on disk after readiness: it may eventually
                    // contain credentials, and a SIGKILL cannot run TempDir::drop.
                    if let Err(error) = private_dir.close() {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return Err(error)
                            .context("failed to remove private HEV configuration directory");
                    }
                    tracing::info!(%endpoint, "HEV SOCKS egress started");
                    return Ok(Self { child, endpoint });
                }
                Err(error) => failures.push(format!("{port}: {error:#}")),
            }
        }

        bail!(
            "HEV failed all TCP listener candidates: {}",
            failures.join("; ")
        )
    }

    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    pub async fn shutdown(mut self) {
        self.terminate().await;
    }

    pub async fn run_until_cancel(
        mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.terminate().await;
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            if let Some(status) = self
                .child
                .try_wait()
                .context("failed to inspect HEV process")?
            {
                if status.success() {
                    return Ok(());
                }
                bail!("HEV process exited unexpectedly with {status}");
            }
        }
    }

    async fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        if let Err(error) = self.child.start_kill() {
            tracing::debug!(?error, "failed to request HEV process termination");
            return;
        }
        if tokio::time::timeout(STOP_TIMEOUT, self.child.wait())
            .await
            .is_err()
        {
            tracing::warn!("timed out waiting for HEV process termination");
        }
    }
}

impl Drop for ProcessRuntime {
    fn drop(&mut self) {
        // The owning instance normally awaits run_until_cancel. This is the
        // last-resort path for runtime abort/panic and must never detach HEV.
        let _ = self.child.start_kill();
    }
}

async fn start_candidate(
    executable: &Path,
    config_path: &Path,
    listen_address: IpAddr,
    port: u16,
    workers: usize,
) -> anyhow::Result<(Child, SocketAddr)> {
    #[cfg(target_os = "linux")]
    let parent_pid = unsafe { linux_getpid() };
    let mut command = tokio::process::Command::new(executable);
    command
        .arg(config_path)
        .arg("--workers")
        .arg(workers.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(target_os = "macos")]
    command
        .arg("--parent-pid")
        .arg(unsafe { libc::getpid() }.to_string());
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(move || configure_parent_death(parent_pid));
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to execute {}", executable.display()))?;
    let readiness_ip = match listen_address {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let endpoint = SocketAddr::new(readiness_ip, port);
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;

    loop {
        if let Some(status) = child.try_wait().context("failed to inspect HEV process")? {
            bail!("process exited before readiness with {status}");
        }
        if TcpStream::connect(endpoint).await.is_ok() {
            tokio::time::sleep(READY_INTERVAL).await;
            if child
                .try_wait()
                .context("failed to inspect HEV process")?
                .is_none()
            {
                return Ok((child, endpoint));
            }
            bail!("process exited during readiness confirmation");
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            let _ = child.wait().await;
            bail!("listener did not become ready within {START_TIMEOUT:?}");
        }
        tokio::time::sleep(READY_INTERVAL).await;
    }
}

#[cfg(not(windows))]
fn managed_config_extension() -> &'static str {
    "yml"
}

#[cfg(windows)]
fn managed_config_extension() -> &'static str {
    "json"
}

#[cfg(not(windows))]
fn render_managed_config(config: &SocksEgressConfig, port: u16) -> String {
    render_hev_config(config, port)
}

#[cfg(windows)]
fn render_managed_config(config: &SocksEgressConfig, port: u16) -> String {
    format!(
        concat!(
            "{{\n",
            "  \"log\": {{\"level\": \"off\"}},\n",
            "  \"inbounds\": [{{\n",
            "    \"tag\": \"easytier-mesh-entry\",\n",
            "    \"protocol\": \"socks\",\n",
            "    \"address\": \"{}\",\n",
            "    \"port\": {}\n",
            "  }}],\n",
            "  \"outbounds\": [{{\n",
            "    \"tag\": \"direct\",\n",
            "    \"protocol\": \"direct\"\n",
            "  }}]\n",
            "}}\n"
        ),
        config.listen_address, port
    )
}

fn ensure_candidate_listener_available(listen_address: IpAddr, port: u16) -> anyhow::Result<()> {
    let primary = SocketAddr::new(listen_address, port);
    let listener = std::net::TcpListener::bind(primary)
        .with_context(|| format!("HEV listener candidate {primary} is unavailable"))?;
    drop(listener);

    // HEV maps an unspecified IPv4 configuration to a dual-stack listener on
    // platforms that support it. Probe the IPv6 wildcard separately, after
    // dropping our IPv4 reservation, so a v6-only owner is also respected.
    if listen_address == IpAddr::V4(Ipv4Addr::UNSPECIFIED) {
        let secondary = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
        match std::net::TcpListener::bind(secondary) {
            Ok(listener) => drop(listener),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
                ) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("HEV listener candidate {secondary} is unavailable"));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    #[link_name = "getpid"]
    fn linux_getpid() -> std::ffi::c_int;
    #[link_name = "getppid"]
    fn linux_getppid() -> std::ffi::c_int;
    #[link_name = "prctl"]
    fn linux_prctl(
        option: std::ffi::c_int,
        arg2: std::ffi::c_ulong,
        arg3: std::ffi::c_ulong,
        arg4: std::ffi::c_ulong,
        arg5: std::ffi::c_ulong,
    ) -> std::ffi::c_int;
}

#[cfg(target_os = "linux")]
fn configure_parent_death(parent_pid: std::ffi::c_int) -> std::io::Result<()> {
    // sing-box cmd/sing-box/cmd_run_userns_linux.go::runInUserNamespaceIfNeeded
    // (b789a2e6) uses Pdeathsig=SIGKILL for the same externally owned child
    // invariant. HEV has no state that must outlive EasyTier, so guarantee that a
    // killed parent cannot leave a listener behind.
    const PR_SET_PDEATHSIG: std::ffi::c_int = 1;
    const SIGKILL: std::ffi::c_ulong = 9;
    if unsafe { linux_prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { linux_getppid() } != parent_pid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "EasyTier parent exited while starting HEV",
        ));
    }
    Ok(())
}

fn render_hev_config(config: &SocksEgressConfig, port: u16) -> String {
    let bind_interface = config
        .bind_interface
        .as_deref()
        .unwrap_or_default()
        .replace('\'', "''");
    format!(
        "main:\n  workers: {}\n  port: {}\n  listen-address: '{}'\n  udp-port: 0\n  udp-listen-address: '{}'\n  listen-ipv6-only: false\n  bind-interface: '{}'\n  mark: {}\nmisc:\n  connect-timeout: 10000\n  tcp-read-write-timeout: 300000\n  udp-read-write-timeout: 120000\n  log-file: stderr\n  log-level: warn\n",
        config.workers,
        port,
        config.listen_address,
        config.listen_address,
        bind_interface,
        config.socket_mark.unwrap_or_default(),
    )
}

fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_port_candidates() {
        let mut config = SocksEgressConfig {
            port_candidates: vec![11080, 11080],
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.port_candidates = vec![0];
        assert!(config.validate().is_err());
    }

    #[test]
    fn renders_bounded_direct_egress_config() {
        let config = SocksEgressConfig::leaf_direct_egress(Some("eth0".to_owned()), Some(0x2333));
        assert_eq!(
            config.listen_address,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "the managed HEV hop must not expose an unauthenticated host listener"
        );
        let rendered = render_hev_config(&config, 11080);
        assert!(rendered.contains("port: 11080"));
        assert!(rendered.contains("udp-port: 0"));
        assert!(rendered.contains("listen-address: '127.0.0.1'"));
        assert!(rendered.contains("udp-listen-address: '127.0.0.1'"));
        assert!(rendered.contains("bind-interface: 'eth0'"));
        assert!(rendered.contains("mark: 9011"));
    }

    #[test]
    fn mesh_entry_never_inherits_leaf_route_binding() {
        let config = SocksEgressConfig::mesh_entry();
        assert_eq!(config.port_candidates, MESH_ENTRY_PORT_CANDIDATES.to_vec());
        assert_eq!(config.bind_interface, None);
        assert_eq!(config.socket_mark, None);
    }

    #[test]
    fn mesh_entry_and_leaf_direct_egress_ports_are_disjoint() {
        let mesh = MESH_ENTRY_PORT_CANDIDATES
            .into_iter()
            .collect::<BTreeSet<_>>();
        let direct = LEAF_DIRECT_EGRESS_PORT_CANDIDATES
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert!(mesh.is_disjoint(&direct));
    }

    #[test]
    fn occupied_candidate_is_rejected_without_connecting_to_owner() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();

        let error =
            ensure_candidate_listener_available(IpAddr::V4(Ipv4Addr::LOCALHOST), port).unwrap_err();

        assert!(error.to_string().contains("unavailable"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[cfg(all(target_os = "android", feature = "hev-inprocess"))]
mod android {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread::JoinHandle,
    };

    use anyhow::{Context as _, bail};
    use tokio_util::sync::CancellationToken;

    use super::{
        READY_INTERVAL, START_TIMEOUT, SocksEgressConfig, ensure_candidate_listener_available,
        render_hev_config,
    };

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        fn hev_socks5_server_main_from_str(config: *const u8, length: u32) -> i32;
        fn hev_socks5_server_quit();
    }

    pub struct InProcessRuntime {
        thread: Option<JoinHandle<i32>>,
        endpoint: std::net::SocketAddr,
    }

    impl InProcessRuntime {
        pub async fn start(config: SocksEgressConfig) -> anyhow::Result<Self> {
            config.validate()?;
            let mut failures = Vec::new();
            for port in &config.port_candidates {
                if let Err(error) =
                    ensure_candidate_listener_available(config.listen_address, *port)
                {
                    failures.push(format!("{port}: {error:#}"));
                    continue;
                }
                match start_candidate(&config, *port).await {
                    Ok(runtime) => return Ok(runtime),
                    Err(error) => failures.push(format!("{port}: {error:#}")),
                }
            }
            bail!(
                "in-process HEV failed all TCP listener candidates: {}",
                failures.join("; ")
            )
        }

        pub fn endpoint(&self) -> std::net::SocketAddr {
            self.endpoint
        }

        pub async fn run_until_cancel(mut self, cancel: CancellationToken) -> anyhow::Result<()> {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        self.stop().await?;
                        return Ok(());
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                }
                if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                    let status = self.join().await?;
                    if status == 0 {
                        return Ok(());
                    }
                    bail!("in-process HEV exited unexpectedly with status {status}");
                }
            }
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            unsafe { hev_socks5_server_quit() };
            let status = self.join().await?;
            if status != 0 {
                bail!("in-process HEV shutdown returned status {status}");
            }
            Ok(())
        }

        async fn join(&mut self) -> anyhow::Result<i32> {
            let Some(thread) = self.thread.take() else {
                return Ok(0);
            };
            let joined = tokio::task::spawn_blocking(move || thread.join()).await;
            ACTIVE.store(false, Ordering::Release);
            joined
                .context("failed to join in-process HEV task")?
                .map_err(|_| anyhow::anyhow!("in-process HEV task panicked"))
        }
    }

    impl Drop for InProcessRuntime {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                unsafe { hev_socks5_server_quit() };
                let _ = std::thread::Builder::new()
                    .name("easytier-hev-cleanup".to_owned())
                    .spawn(move || {
                        let _ = thread.join();
                        ACTIVE.store(false, Ordering::Release);
                    });
            }
        }
    }

    async fn start_candidate(
        config: &SocksEgressConfig,
        port: u16,
    ) -> anyhow::Result<InProcessRuntime> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow::anyhow!("an in-process HEV runtime is already active"))?;
        let bytes = render_hev_config(config, port).into_bytes();
        let thread = match std::thread::Builder::new()
            .name("easytier-hev".to_owned())
            .spawn(move || {
                let status =
                    unsafe { hev_socks5_server_main_from_str(bytes.as_ptr(), bytes.len() as u32) };
                status
            }) {
            Ok(thread) => thread,
            Err(error) => {
                ACTIVE.store(false, Ordering::Release);
                return Err(error).context("failed to start in-process HEV thread");
            }
        };
        let readiness_ip = match config.listen_address {
            std::net::IpAddr::V4(_) => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            std::net::IpAddr::V6(_) => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        };
        let endpoint = std::net::SocketAddr::new(readiness_ip, port);
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if thread.is_finished() {
                let status = thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("in-process HEV startup task panicked"))?;
                ACTIVE.store(false, Ordering::Release);
                bail!("in-process HEV exited before readiness with status {status}");
            }
            if tokio::net::TcpStream::connect(endpoint).await.is_ok() {
                tokio::time::sleep(READY_INTERVAL).await;
                if !thread.is_finished() {
                    return Ok(InProcessRuntime {
                        thread: Some(thread),
                        endpoint,
                    });
                }
            }
            if tokio::time::Instant::now() >= deadline {
                unsafe { hev_socks5_server_quit() };
                let _ = tokio::task::spawn_blocking(move || thread.join()).await;
                ACTIVE.store(false, Ordering::Release);
                bail!("in-process HEV listener did not become ready within {START_TIMEOUT:?}");
            }
            tokio::time::sleep(READY_INTERVAL).await;
        }
    }
}

#[cfg(all(target_os = "android", feature = "hev-inprocess"))]
pub use android::InProcessRuntime;
