//! Optional policy-proxy control plane.
//!
//! This crate deliberately contains no EasyTier or Leaf types. It owns the
//! validated policy document and bounded recovery state; packet and proxy
//! runtimes attach through narrow traits in later integration layers.

mod config;
mod leaf_config;
#[cfg(unix)]
mod leaf_process;
mod packet;
mod supervisor;

pub use config::{
    ChainKind, PolicyDocument, PolicyError, PolicyMode, PolicyRevision, ProxyKind, ProxyServer,
    ProxyVia, RuleSetKind, validate_policy_file,
};
pub use leaf_config::{LeafConfigError, MeshServerResolver, compile_leaf_config};
#[cfg(unix)]
pub use leaf_process::{LeafProcessFactory, LeafProcessRuntime};
#[cfg(unix)]
pub use packet::{LeafPacketBridge, LeafPacketEndpoint};
pub use packet::{MeshRouteSnapshot, PacketClass, PacketClassifier, PacketError};
pub use supervisor::{
    ApplyResult, HealthEvent, PolicyRuntime, PolicyRuntimeFactory, PolicyStatus, PolicySupervisor,
    RetryDecision, RetryPolicy,
};
