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

use tokio::process::{Child, Command};

use crate::{
    LeafPacketBridge, MeshServerResolver, PolicyRevision, PolicyRuntime, PolicyRuntimeFactory,
    compile_leaf_config,
};

const LEAF_TUN_FD: RawFd = 3;
static CONFIG_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
}

impl LeafProcessRuntime {
    pub fn bridge(&self) -> Arc<LeafPacketBridge> {
        self.bridge.clone()
    }

    pub async fn start(
        executable: &Path,
        base_dir: &Path,
        outbound_interface: Option<&str>,
        resolver: &dyn MeshServerResolver,
        revision: Arc<PolicyRevision>,
    ) -> Result<Arc<Self>, String> {
        let (bridge, endpoint) = LeafPacketBridge::pair().map_err(|error| error.to_string())?;
        let dns_servers = system_dns_servers()?;
        let config = compile_leaf_config(&revision, LEAF_TUN_FD, base_dir, resolver, &dns_servers)
            .map_err(|error| error.to_string())?;
        let config_path = std::env::temp_dir().join(format!(
            "easytier-leaf-{}-{}-{}.json",
            std::process::id(),
            revision.id,
            CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        write_private_file(&config_path, config.as_bytes()).map_err(|error| error.to_string())?;

        let mut validation_command = Command::new(executable);
        validation_command.arg("-T").arg("-c").arg(&config_path);
        if let Some(interface) = outbound_interface {
            validation_command.arg("-b").arg(interface);
        }
        let validation = validation_command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|error| format!("failed to execute Leaf config validation: {error}"))?;
        if !validation.status.success() {
            let _ = std::fs::remove_file(&config_path);
            return Err(format!(
                "Leaf rejected generated config: {}{}",
                String::from_utf8_lossy(&validation.stdout),
                String::from_utf8_lossy(&validation.stderr)
            ));
        }

        let endpoint_fd = endpoint.into_raw_fd();
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
        unsafe {
            command.pre_exec(move || {
                configure_parent_death(parent_pid)?;
                if libc::dup2(endpoint_fd, LEAF_TUN_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if endpoint_fd != LEAF_TUN_FD {
                    libc::close(endpoint_fd);
                }
                Ok(())
            });
        }
        let child_result = command.spawn();
        unsafe {
            libc::close(endpoint_fd);
        }
        let mut child = match child_result {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&config_path);
                return Err(format!("failed to start Leaf: {error}"));
            }
        };

        tokio::time::sleep(Duration::from_millis(250)).await;
        match child.try_wait() {
            Ok(None) => Ok(Arc::new(Self {
                revision_id: revision.id.clone(),
                bridge: Arc::new(bridge),
                child: Mutex::new(Some(child)),
                config_path,
            })),
            Ok(Some(status)) => {
                let _ = std::fs::remove_file(&config_path);
                Err(format!("Leaf exited during readiness ({status})"))
            }
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = std::fs::remove_file(&config_path);
                Err(format!("failed to inspect Leaf readiness: {error}"))
            }
        }
    }

    pub async fn stop(&self) {
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill().await;
            let _ = child.wait().await;
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

fn system_dns_servers() -> Result<Vec<std::net::IpAddr>, String> {
    const CANDIDATES: &[&str] = &[
        "/etc/resolv.conf",
        "/run/systemd/resolve/resolv.conf",
        "/run/NetworkManager/no-stub-resolv.conf",
    ];
    let mut failures = Vec::new();
    for path in CANDIDATES {
        match std::fs::read_to_string(path) {
            Ok(contents) => match parse_system_dns_servers(&contents) {
                Ok(servers) => return Ok(servers),
                Err(error) => failures.push(format!("{path}: {error}")),
            },
            Err(error) => failures.push(format!("{path}: {error}")),
        }
    }
    Err(format!(
        "no directly usable system DNS server found ({})",
        failures.join("; ")
    ))
}

fn parse_system_dns_servers(contents: &str) -> Result<Vec<std::net::IpAddr>, String> {
    let mut servers = Vec::new();
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
        }
        if servers.len() == 4 {
            break;
        }
    }
    if servers.is_empty() {
        return Err("contains no non-loopback IP nameserver usable by Leaf".to_owned());
    }
    Ok(servers)
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
    fn build(
        &self,
        revision: Arc<PolicyRevision>,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<dyn PolicyRuntime>, String>> + Send>> {
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use super::*;

    fn unresolved_mesh(
        _proxy_name: &str,
        _instance_id: Option<uuid::Uuid>,
        _virtual_ip: Option<std::net::IpAddr>,
        _port: u16,
    ) -> Option<crate::ResolvedMeshServer> {
        None
    }

    #[tokio::test]
    async fn starts_and_stops_isolated_worker_and_removes_config() {
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
        assert!(config_path.exists());
        assert!(runtime.is_running());
        runtime.stop().await;
        assert!(!runtime.is_running());
        assert!(!config_path.exists());
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
}
