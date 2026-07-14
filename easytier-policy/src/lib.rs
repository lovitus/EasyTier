//! Optional policy-proxy control plane.
//!
//! This crate deliberately contains no EasyTier or Leaf types. It owns the
//! validated policy document and bounded recovery state; packet and proxy
//! runtimes attach through narrow traits in later integration layers.

mod config;
#[cfg(all(unix, feature = "leaf-inprocess"))]
mod inprocess;
mod leaf_config;
#[cfg(unix)]
mod leaf_process;
mod packet;
mod preflight;
mod supervisor;

pub use config::{
    ChainKind, PolicyDocument, PolicyError, PolicyMode, PolicyRevision, Proxy, ProxyKind,
    ProxyServer, ProxyVia, RuleSetKind, validate_policy_file,
};
#[cfg(all(unix, feature = "leaf-inprocess"))]
pub use inprocess::{InProcessLeafFactory, InProcessLeafRuntime};
pub use leaf_config::{
    LeafConfigError, MeshServerResolver, ResolvedMeshServer, compile_leaf_config,
};
#[cfg(unix)]
pub use leaf_process::{LeafProcessFactory, LeafProcessRuntime};
#[cfg(unix)]
pub use packet::{LeafPacketBridge, LeafPacketEndpoint};
pub use packet::{MeshRouteSnapshot, PacketClass, PacketClassifier, PacketError};
pub use preflight::{
    DiagnosticSeverity, PolicyDiagnostic, PolicyPreflight, PolicyPreflightReport,
    preflight_policy_file, preflight_policy_source,
};
pub use supervisor::{
    ApplyResult, HealthEvent, PolicyRuntime, PolicyRuntimeFactory, PolicyStatus, PolicySupervisor,
    RetryDecision, RetryPolicy, RuntimeRestartBudget, RuntimeRestartDecision,
};
