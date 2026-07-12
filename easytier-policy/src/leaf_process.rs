use std::{
    future::Future,
    os::fd::RawFd,
    os::unix::process::CommandExt as _,
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
        let config = compile_leaf_config(&revision, LEAF_TUN_FD, base_dir, resolver)
            .map_err(|error| error.to_string())?;
        let config_path = std::env::temp_dir().join(format!(
            "easytier-leaf-{}-{}-{}.conf",
            std::process::id(),
            revision.id,
            CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        write_private_file(&config_path, config.as_bytes()).map_err(|error| error.to_string())?;

        let validation = Command::new(executable)
            .arg("-T")
            .arg("-c")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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
            .arg("--single-thread")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(interface) = outbound_interface {
            command.arg("-b").arg(interface);
        }
        unsafe {
            command.pre_exec(move || {
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
            LeafProcessRuntime::start(&executable, dir.path(), None, &|_, _| None, revision)
                .await
                .unwrap();
        let config_path = runtime.config_path.clone();
        assert!(config_path.exists());
        runtime.stop().await;
        assert!(!config_path.exists());
    }
}
