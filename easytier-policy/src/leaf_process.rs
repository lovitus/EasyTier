use std::{
    future::Future,
    os::fd::RawFd,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(target_os = "linux")]
use std::ffi::CString;

use tokio::process::{Child, Command};

use crate::{
    LeafOwnedTunConfig, LeafPacketBridge, MeshServerResolver, PolicyRevision, PolicyRuntime,
    PolicyRuntimeBuildFuture, PolicyRuntimeFactory,
};

const LEAF_TUN_FD: RawFd = 3;
const LEAF_CONFIG_VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);
static CONFIG_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static OWNED_TUN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Allocate a bounded, process-unique Linux TUN identity for one transactional
/// Leaf candidate. The address pool is RFC 2544 benchmarking space and does
/// not overlap EasyTier's default 198.19.0.0/16 FakeIP pool.
pub fn next_leaf_owned_tun_config() -> LeafOwnedTunConfig {
    leaf_owned_tun_config(
        std::process::id(),
        OWNED_TUN_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    )
}

fn leaf_owned_tun_config(process_id: u32, sequence: u64) -> LeafOwnedTunConfig {
    let slot = (sequence as u16) & 0x3fff;
    let offset = u32::from(slot) * 4;
    let third = (offset >> 8) as u8;
    let fourth = (offset & 0xff) as u8;
    LeafOwnedTunConfig {
        name: format!("etp{:04x}{:04x}", process_id & 0xffff, sequence & 0xffff),
        gateway: format!("198.18.{third}.{}", fourth + 1),
        address: format!("198.18.{third}.{}", fourth + 2),
        netmask: "255.255.255.252".to_owned(),
        mtu: 1_500,
    }
}

pub struct LeafProcessFactory {
    executable: PathBuf,
    base_dir: PathBuf,
    outbound_interface: Option<String>,
    resolver: Arc<dyn MeshServerResolver + Send + Sync>,
}

impl LeafProcessFactory {
    pub fn new(
        executable: PathBuf,
        base_dir: PathBuf,
        outbound_interface: Option<String>,
        resolver: Arc<dyn MeshServerResolver + Send + Sync>,
    ) -> Self {
        Self {
            executable,
            base_dir,
            outbound_interface,
            resolver,
        }
    }
}

pub struct LeafProcessRuntime {
    revision_id: String,
    bridge: Arc<LeafPacketBridge>,
    child: Mutex<Option<Child>>,
    config_path: PathBuf,
    owned_tun: Option<LeafOwnedTunConfig>,
}

impl LeafProcessRuntime {
    pub fn bridge(&self) -> Arc<LeafPacketBridge> {
        self.bridge.clone()
    }

    pub fn owned_tun_interface(&self) -> Option<&str> {
        self.owned_tun.as_ref().map(|tun| tun.name.as_str())
    }

    pub async fn start(
        executable: &Path,
        base_dir: &Path,
        outbound_interface: Option<&str>,
        resolver: &dyn MeshServerResolver,
        revision: Arc<PolicyRevision>,
    ) -> Result<Arc<Self>, String> {
        let dns_servers = system_dns_servers()?;
        Self::start_with_dns_servers(
            executable,
            base_dir,
            outbound_interface,
            resolver,
            &dns_servers,
            revision,
        )
        .await
    }

    pub async fn start_with_dns_servers(
        executable: &Path,
        base_dir: &Path,
        outbound_interface: Option<&str>,
        resolver: &dyn MeshServerResolver,
        dns_servers: &[std::net::IpAddr],
        revision: Arc<PolicyRevision>,
    ) -> Result<Arc<Self>, String> {
        Self::start_with_dns_servers_and_options(
            executable,
            base_dir,
            outbound_interface,
            resolver,
            dns_servers,
            revision,
            crate::LeafConfigOptions::default(),
        )
        .await
    }

    pub async fn start_with_dns_servers_and_options(
        executable: &Path,
        base_dir: &Path,
        outbound_interface: Option<&str>,
        resolver: &dyn MeshServerResolver,
        dns_servers: &[std::net::IpAddr],
        revision: Arc<PolicyRevision>,
        options: crate::LeafConfigOptions,
    ) -> Result<Arc<Self>, String> {
        #[cfg(not(target_os = "linux"))]
        if options.leaf_owned_tun.is_some() {
            return Err("Leaf-owned policy TUN is currently supported only on Linux".to_owned());
        }
        let owned_tun = options.leaf_owned_tun.clone();
        #[cfg(target_os = "linux")]
        if let Some(tun) = owned_tun.as_ref()
            && interface_index(&tun.name)?.is_some()
        {
            return Err(format!("Leaf-owned TUN {} already exists", tun.name));
        }
        let (bridge, endpoint) = LeafPacketBridge::pair().map_err(|error| error.to_string())?;
        let tun_fd = if owned_tun.is_some() { -1 } else { LEAF_TUN_FD };
        let config = crate::compile_leaf_config_with_options(
            &revision,
            tun_fd,
            base_dir,
            resolver,
            dns_servers,
            options,
        )
        .map_err(|error| error.to_string())?;
        let config_path = std::env::temp_dir().join(format!(
            "easytier-leaf-{}-{}-{}.json",
            std::process::id(),
            revision.id,
            CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        write_private_file(&config_path, config.as_bytes()).map_err(|error| error.to_string())?;

        let validation = match run_leaf_config_validation(
            executable,
            &config_path,
            outbound_interface,
            LEAF_CONFIG_VALIDATION_TIMEOUT,
        )
        .await
        {
            Ok(validation) => validation,
            Err(error) => {
                let _ = std::fs::remove_file(&config_path);
                return Err(error);
            }
        };
        if !validation.status.success() {
            let _ = std::fs::remove_file(&config_path);
            return Err(format!(
                "Leaf rejected generated config: {}{}",
                String::from_utf8_lossy(&validation.stdout),
                String::from_utf8_lossy(&validation.stderr)
            ));
        }

        let endpoint_fd = if owned_tun.is_some() {
            drop(endpoint);
            None
        } else {
            Some(endpoint.into_raw_fd())
        };
        let mut command = Command::new(executable);
        command
            .arg("-c")
            .arg(&config_path)
            .stdin(Stdio::null())
            // Never leave child pipes unread: Leaf's console logger writes to stdout, while worker
            // startup errors use stderr. Inherit both so neither can deadlock on a full pipe.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(interface) = outbound_interface {
            command.arg("-b").arg(interface);
        }
        let parent_pid = unsafe { libc::getpid() };
        #[cfg(target_os = "macos")]
        command.arg("--parent-pid").arg(parent_pid.to_string());
        unsafe {
            command.pre_exec(move || {
                configure_parent_death(parent_pid)?;
                if let Some(endpoint_fd) = endpoint_fd {
                    if libc::dup2(endpoint_fd, LEAF_TUN_FD) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if endpoint_fd != LEAF_TUN_FD {
                        libc::close(endpoint_fd);
                    }
                }
                Ok(())
            });
        }
        let child_result = command.spawn();
        if let Some(endpoint_fd) = endpoint_fd {
            unsafe {
                libc::close(endpoint_fd);
            }
        }
        let mut child = match child_result {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&config_path);
                return Err(format!("failed to start Leaf: {error}"));
            }
        };

        match wait_for_leaf_readiness(&mut child, owned_tun.as_ref()).await {
            Ok(()) => {
                if let Err(error) = remove_private_config(&config_path) {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(format!(
                        "failed to remove private Leaf config after readiness: {error}"
                    ));
                }
                Ok(Arc::new(Self {
                    revision_id: revision.id.clone(),
                    bridge: Arc::new(bridge),
                    child: Mutex::new(Some(child)),
                    config_path,
                    owned_tun,
                }))
            }
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = std::fs::remove_file(&config_path);
                Err(error)
            }
        }
    }

    pub async fn stop(&self) {
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        #[cfg(target_os = "linux")]
        if let Some(tun) = self.owned_tun.as_ref() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while interface_index(&tun.name).ok().flatten().is_some()
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if interface_index(&tun.name).ok().flatten().is_some() {
                tracing::warn!(
                    interface = tun.name,
                    "Leaf-owned policy TUN remained after bounded worker shutdown"
                );
            }
        }
        let _ = std::fs::remove_file(&self.config_path);
    }

    pub fn is_running(&self) -> bool {
        self.child
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
}

async fn wait_for_leaf_readiness(
    child: &mut Child,
    owned_tun: Option<&LeafOwnedTunConfig>,
) -> Result<(), String> {
    let Some(tun) = owned_tun else {
        tokio::time::sleep(Duration::from_millis(250)).await;
        return match child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(format!("Leaf exited during readiness ({status})")),
            Err(error) => Err(format!("failed to inspect Leaf readiness: {error}")),
        };
    };
    #[cfg(target_os = "linux")]
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("Leaf exited during readiness ({status})"));
                }
                Err(error) => {
                    return Err(format!("failed to inspect Leaf readiness: {error}"));
                }
                Ok(None) => {}
            }
            if interface_index(&tun.name)?.is_some() && interface_is_up(&tun.name)? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "Leaf owned TUN {} did not become ready within three seconds",
                    tun.name
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = tun;
        unreachable!()
    }
}

#[cfg(target_os = "linux")]
fn interface_index(name: &str) -> Result<Option<u32>, String> {
    let name = CString::new(name).map_err(|_| "TUN name contains a NUL byte".to_owned())?;
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    Ok((index != 0).then_some(index))
}

#[cfg(target_os = "linux")]
fn interface_is_up(name: &str) -> Result<bool, String> {
    let flags_path = Path::new("/sys/class/net").join(name).join("flags");
    let flags = match std::fs::read_to_string(&flags_path) {
        Ok(flags) => flags,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "failed to read flags for Leaf owned TUN {name}: {error}"
            ));
        }
    };
    let flags = parse_linux_interface_flags(&flags)
        .map_err(|error| format!("invalid flags for Leaf owned TUN {name}: {error}"))?;
    Ok(flags & libc::IFF_UP as u32 != 0)
}

#[cfg(target_os = "linux")]
fn parse_linux_interface_flags(flags: &str) -> Result<u32, String> {
    let flags = flags.trim();
    let flags = flags
        .strip_prefix("0x")
        .or_else(|| flags.strip_prefix("0X"))
        .unwrap_or(flags);
    u32::from_str_radix(flags, 16).map_err(|error| error.to_string())
}

async fn run_leaf_config_validation(
    executable: &Path,
    config_path: &Path,
    outbound_interface: Option<&str>,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut command = Command::new(executable);
    command
        .arg("-T")
        .arg("-c")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(interface) = outbound_interface {
        command.arg("-b").arg(interface);
    }

    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("failed to execute Leaf config validation: {error}")),
        Err(_) => Err(format!(
            "Leaf config validation timed out after {timeout:?}"
        )),
    }
}

#[cfg(target_os = "linux")]
fn configure_parent_death(parent_pid: libc::pid_t) -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != parent_pid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "EasyTier parent exited while starting Leaf",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn configure_parent_death(_parent_pid: libc::pid_t) -> std::io::Result<()> {
    Ok(())
}

pub fn system_dns_servers() -> Result<Vec<std::net::IpAddr>, String> {
    const PRIMARY: &str = "/etc/resolv.conf";
    const MANAGED_CANDIDATES: &[&str] = &[
        "/run/systemd/resolve/resolv.conf",
        "/run/NetworkManager/no-stub-resolv.conf",
    ];
    let mut failures = Vec::new();

    match std::fs::read_to_string(PRIMARY) {
        Ok(contents) => match classify_system_dns_servers(&contents) {
            SystemDnsSource::Servers(servers) => return Ok(servers),
            SystemDnsSource::ManagedResolverStub => {
                failures.push(format!("{PRIMARY}: contains only loopback resolver stubs"))
            }
            SystemDnsSource::Unavailable => {
                return Err(format!(
                    "no directly usable system DNS server found ({PRIMARY}: contains no usable nameserver; refusing managed resolver fallback without an explicit loopback stub)"
                ));
            }
        },
        Err(error) => failures.push(format!("{PRIMARY}: {error}")),
    }

    for path in MANAGED_CANDIDATES {
        match std::fs::read_to_string(path) {
            Ok(contents) => match classify_system_dns_servers(&contents) {
                SystemDnsSource::Servers(servers) => return Ok(servers),
                SystemDnsSource::ManagedResolverStub => {
                    failures.push(format!("{path}: contains only loopback resolver stubs"));
                }
                SystemDnsSource::Unavailable => {
                    failures.push(format!("{path}: contains no usable nameserver"));
                }
            },
            Err(error) => failures.push(format!("{path}: {error}")),
        }
    }
    Err(format!(
        "no directly usable system DNS server found ({})",
        failures.join("; ")
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum SystemDnsSource {
    Servers(Vec<std::net::IpAddr>),
    ManagedResolverStub,
    Unavailable,
}

fn classify_system_dns_servers(contents: &str) -> SystemDnsSource {
    let mut servers = Vec::new();
    let mut has_loopback_stub = false;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut fields = line.split_whitespace();
        if fields.next() != Some("nameserver") {
            continue;
        }
        let Some(address) = fields.next() else {
            continue;
        };
        let Ok(address) = address.parse() else {
            continue;
        };
        if usable_dns_server(address) && !servers.contains(&address) {
            servers.push(address);
        } else if address.is_loopback() {
            has_loopback_stub = true;
        }
        if servers.len() == 4 {
            break;
        }
    }
    if !servers.is_empty() {
        SystemDnsSource::Servers(servers)
    } else if has_loopback_stub {
        SystemDnsSource::ManagedResolverStub
    } else {
        SystemDnsSource::Unavailable
    }
}

#[cfg(test)]
fn parse_system_dns_servers(contents: &str) -> Result<Vec<std::net::IpAddr>, String> {
    match classify_system_dns_servers(contents) {
        SystemDnsSource::Servers(servers) => Ok(servers),
        SystemDnsSource::ManagedResolverStub => {
            Err("contains only loopback resolver stubs unusable by Leaf".to_owned())
        }
        SystemDnsSource::Unavailable => {
            Err("contains no non-loopback IP nameserver usable by Leaf".to_owned())
        }
    }
}

fn usable_dns_server(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
        std::net::IpAddr::V6(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
        }
    }
}

impl Drop for LeafProcessRuntime {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.start_kill();
        }
        let _ = std::fs::remove_file(&self.config_path);
    }
}

impl PolicyRuntime for LeafProcessRuntime {
    fn revision_id(&self) -> &str {
        &self.revision_id
    }

    fn is_running(&self) -> bool {
        LeafProcessRuntime::is_running(self)
    }

    fn shutdown(self: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move { self.stop().await })
    }
}

impl PolicyRuntimeFactory for LeafProcessFactory {
    fn build(&self, revision: Arc<PolicyRevision>) -> PolicyRuntimeBuildFuture {
        let executable = self.executable.clone();
        let base_dir = self.base_dir.clone();
        let outbound_interface = self.outbound_interface.clone();
        let resolver = self.resolver.clone();
        Box::pin(async move {
            LeafProcessRuntime::start(
                &executable,
                &base_dir,
                outbound_interface.as_deref(),
                resolver.as_ref(),
                revision,
            )
            .await
            .map(|runtime| runtime as Arc<dyn PolicyRuntime>)
        })
    }
}

fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt as _};

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, contents)?;
    file.sync_all()
}

fn remove_private_config(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, time::Instant};

    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_owned_tun_readiness_requires_interface_up_flag() {
        assert_ne!(
            parse_linux_interface_flags("0x1003\n").unwrap() & libc::IFF_UP as u32,
            0
        );
        assert_eq!(
            parse_linux_interface_flags("0x1002\n").unwrap() & libc::IFF_UP as u32,
            0
        );
        assert!(parse_linux_interface_flags("not-flags").is_err());
    }

    #[test]
    fn owned_tun_identity_is_bounded_unique_and_outside_default_fake_ip() {
        let first = leaf_owned_tun_config(0x12345, 1);
        let second = leaf_owned_tun_config(0x12345, 2);
        assert_ne!(first.name, second.name);
        assert!(first.name.len() < libc::IFNAMSIZ);
        assert!(first.address.starts_with("198.18."));
        assert!(first.gateway.starts_with("198.18."));
        assert!(!first.address.starts_with("198.19."));
        assert_eq!(first.netmask, "255.255.255.252");
    }

    fn unresolved_mesh(
        _proxy_name: &str,
        _instance_id: Option<uuid::Uuid>,
        _virtual_ip: Option<std::net::IpAddr>,
        _port: Option<u16>,
    ) -> Option<crate::ResolvedMeshServer> {
        None
    }

    #[tokio::test]
    async fn starts_worker_without_retaining_private_config_and_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-leaf");
        fs::write(
            &executable,
            b"#!/bin/sh\nif [ \"$1\" = \"-T\" ]; then exit 0; fi\nwhile :; do sleep 1; done\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let revision = Arc::new(
            PolicyRevision::parse("version: 1\nrules: [\"FINAL,DIRECT\"]\n", dir.path()).unwrap(),
        );

        let runtime =
            LeafProcessRuntime::start(&executable, dir.path(), None, &unresolved_mesh, revision)
                .await
                .unwrap();
        let config_path = runtime.config_path.clone();
        assert!(!config_path.exists());
        assert!(runtime.is_running());
        runtime.stop().await;
        assert!(!runtime.is_running());
        assert!(!config_path.exists());
    }

    #[tokio::test]
    async fn config_validation_timeout_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("hanging-leaf");
        let config_path = dir.path().join("leaf.json");
        fs::write(
            &executable,
            b"#!/bin/sh\nif [ \"$1\" = \"-T\" ]; then while :; do :; done; fi\n",
        )
        .unwrap();
        fs::write(&config_path, b"{}").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        let started = Instant::now();
        let error =
            run_leaf_config_validation(&executable, &config_path, None, Duration::from_millis(50))
                .await
                .unwrap_err();

        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn validation_execution_failure_removes_private_config() {
        let dir = tempfile::tempdir().unwrap();
        let revision = Arc::new(
            PolicyRevision::parse(
                format!(
                    "version: 1\nrules: [\"DOMAIN,{}.invalid,DIRECT\", \"FINAL,DIRECT\"]\n",
                    uuid::Uuid::from_u128(0x8ee5_6f6a_5db0_4f71_a8f0_3a53_7cb4_88e2)
                ),
                dir.path(),
            )
            .unwrap(),
        );
        let prefix = format!("easytier-leaf-{}-{}-", std::process::id(), revision.id);
        let existing = matching_temp_configs(&prefix);

        let error = match LeafProcessRuntime::start(
            &dir.path().join("missing-leaf"),
            dir.path(),
            None,
            &unresolved_mesh,
            revision,
        )
        .await
        {
            Ok(_) => panic!("missing Leaf executable unexpectedly started"),
            Err(error) => error,
        };

        assert!(error.contains("failed to execute Leaf config validation"));
        assert_eq!(matching_temp_configs(&prefix), existing);
    }

    fn matching_temp_configs(prefix: &str) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".json"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[test]
    fn parses_bounded_system_dns_servers_without_fallback_defaults() {
        let servers = parse_system_dns_servers(
            "# generated\nnameserver 127.0.0.53\nnameserver invalid\n\
             nameserver 1.1.1.1 # primary\nnameserver 127.0.0.53\n",
        )
        .unwrap();
        assert_eq!(
            servers,
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()]
        );
        assert!(parse_system_dns_servers("nameserver 127.0.0.53\nnameserver ::1\n").is_err());
        assert!(parse_system_dns_servers("nameserver fe80::1\n").is_err());
        assert!(parse_system_dns_servers("search example.test\n").is_err());
    }

    #[test]
    fn only_explicit_loopback_stub_allows_managed_resolver_fallback() {
        assert_eq!(
            classify_system_dns_servers("nameserver 127.0.0.53\nnameserver ::1\n"),
            SystemDnsSource::ManagedResolverStub
        );
        assert_eq!(
            classify_system_dns_servers(""),
            SystemDnsSource::Unavailable
        );
        assert_eq!(
            classify_system_dns_servers("search example.test\n"),
            SystemDnsSource::Unavailable
        );
        assert_eq!(
            classify_system_dns_servers("nameserver 0.0.0.0\nnameserver fe80::1\n"),
            SystemDnsSource::Unavailable
        );
    }
}
