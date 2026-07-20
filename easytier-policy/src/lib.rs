//! Optional policy-proxy control plane.
//!
//! This crate deliberately contains no EasyTier or Leaf types. It owns the
//! validated policy document and bounded recovery state; packet and proxy
//! runtimes attach through narrow traits in later integration layers.

mod config;
mod geodata;
#[cfg(all(unix, feature = "leaf-inprocess"))]
mod inprocess;
mod leaf_config;
#[cfg(unix)]
mod leaf_process;
mod packet;
mod preflight;
mod shadowsocks;
mod stream_transport;
mod supervisor;
mod trojan;
mod vless;
mod vmess;

pub use config::{
    ChainKind, DEFAULT_FAKE_DNS_IPV4_RANGE, DEFAULT_FAKE_DNS_IPV6_RANGE, PolicyDns, PolicyDocument,
    PolicyError, PolicyMode, PolicyRevision, Proxy, ProxyKind, ProxyServer, ProxyTls,
    ProxyTransport, ProxyUdp, ProxyVia, RuleSet, RuleSetKind, reload_policy_file_if_changed,
    reload_policy_file_if_changed_with_rule_set_provider, validate_policy_file,
    validate_policy_file_with_rule_set_provider,
};
pub use geodata::{
    ManagedRuleDataKind, list_managed_rule_data_categories, validate_managed_rule_data,
};
#[cfg(all(unix, feature = "leaf-inprocess"))]
pub use inprocess::{InProcessLeafFactory, InProcessLeafRuntime};
pub use leaf_config::{
    LeafConfigError, LeafConfigOptions, LeafOwnedTunConfig, MeshServerResolver, ResolvedMeshServer,
    compile_leaf_config, compile_leaf_config_with_options,
};
#[cfg(unix)]
pub use leaf_process::{
    LeafProcessFactory, LeafProcessRuntime, next_leaf_owned_tun_config, system_dns_servers,
};
#[cfg(unix)]
pub use packet::{LeafPacketBridge, LeafPacketEndpoint};
pub use packet::{MeshRouteSnapshot, PacketClass, PacketClassifier, PacketError};
pub use preflight::{
    DiagnosticSeverity, PolicyDiagnostic, PolicyPreflight, PolicyPreflightReport,
    preflight_policy_file, preflight_policy_source, report_for_policy_revision,
};
pub use supervisor::{
    ApplyResult, HealthEvent, PolicyRuntime, PolicyRuntimeBuildFuture, PolicyRuntimeFactory,
    PolicyStatus, PolicySupervisor, RetryDecision, RetryPolicy, RuntimeRestartBudget,
    RuntimeRestartDecision,
};
