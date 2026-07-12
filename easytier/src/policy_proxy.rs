use std::{
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Context as _;

mod mesh_socks_bridge;
mod policy_routing;

pub(crate) use mesh_socks_bridge::{MeshProxyBridgeSet, MeshProxyTarget};
pub(crate) use policy_routing::PolicyRoutingGuard;

pub(crate) const POLICY_SOCKET_MARK: u32 = 0x4554_5001;

#[derive(Debug, Clone)]
pub struct PolicyProcessConfig {
    pub policy_file: PathBuf,
    pub leaf_executable: PathBuf,
    pub outbound_interface: String,
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
    if outbound_interface.trim().is_empty() {
        anyhow::bail!("policy mode requires a non-empty outbound interface");
    }
    require_regular_file(&policy_file, "policy config")?;
    let leaf_executable = resolve_executable(&leaf_executable)?;
    easytier_policy::validate_policy_file(&policy_file)
        .with_context(|| format!("invalid policy config {}", policy_file.display()))?;
    POLICY_CONFIG
        .set(PolicyProcessConfig {
            policy_file,
            leaf_executable,
            outbound_interface,
        })
        .map_err(|_| anyhow::anyhow!("policy process config was initialized more than once"))
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

pub fn configured() -> Option<&'static PolicyProcessConfig> {
    POLICY_CONFIG.get()
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
