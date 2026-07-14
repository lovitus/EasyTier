use std::{collections::BTreeSet, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{PolicyError, PolicyRevision, ProxyServer, validate_policy_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyPreflightReport {
    pub valid: bool,
    pub revision_id: Option<String>,
    pub sha256: Option<String>,
    pub diagnostics: Vec<PolicyDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct PolicyPreflight {
    pub report: PolicyPreflightReport,
    revision: Option<Arc<PolicyRevision>>,
}

impl PolicyPreflight {
    pub fn revision(&self) -> Option<&Arc<PolicyRevision>> {
        self.revision.as_ref()
    }

    pub fn into_revision(self) -> Option<Arc<PolicyRevision>> {
        self.revision
    }
}

pub fn preflight_policy_source(source: impl Into<Arc<str>>, base_dir: &Path) -> PolicyPreflight {
    preflight_result(PolicyRevision::parse(source, base_dir))
}

pub fn preflight_policy_file(path: &Path) -> PolicyPreflight {
    preflight_result(validate_policy_file(path))
}

pub fn report_for_policy_revision(revision: &PolicyRevision) -> PolicyPreflightReport {
    PolicyPreflightReport {
        valid: true,
        revision_id: Some(revision.id.clone()),
        sha256: Some(
            revision
                .digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        ),
        diagnostics: warnings(revision),
    }
}

fn preflight_result(result: Result<PolicyRevision, PolicyError>) -> PolicyPreflight {
    match result {
        Ok(revision) => {
            let report = report_for_policy_revision(&revision);
            PolicyPreflight {
                report,
                revision: Some(Arc::new(revision)),
            }
        }
        Err(error) => PolicyPreflight {
            report: PolicyPreflightReport {
                valid: false,
                revision_id: None,
                sha256: None,
                diagnostics: vec![diagnostic_for_error(&error)],
            },
            revision: None,
        },
    }
}

fn diagnostic_for_error(error: &PolicyError) -> PolicyDiagnostic {
    let (code, path) = match error {
        PolicyError::Read { .. } => ("policy.read", "policy".to_owned()),
        PolicyError::TooLarge => ("policy.too_large", "policy".to_owned()),
        PolicyError::Yaml(_) => ("policy.yaml", "policy".to_owned()),
        PolicyError::UnsupportedVersion(_) => ("policy.version", "version".to_owned()),
        PolicyError::NoRules => ("rules.empty", "rules".to_owned()),
        PolicyError::Limit { .. } => ("policy.limit", "policy".to_owned()),
        PolicyError::ReservedName(name) => ("actor.name_reserved", actor_path(name)),
        PolicyError::InvalidActorName(name) => ("actor.name_invalid", actor_path(name)),
        PolicyError::UnknownReference { owner, .. } => ("actor.reference_unknown", owner.clone()),
        PolicyError::Cycle(name) => ("group.cycle", format!("groups.{name}")),
        PolicyError::ChainTooDeep(name) => ("group.expansion_limit", format!("groups.{name}")),
        PolicyError::InvalidServer { name, .. } => ("proxy.invalid", format!("proxies.{name}")),
        PolicyError::InvalidRule { index, .. } => ("rule.invalid", format!("rules[{index}]")),
        PolicyError::InvalidRuleSet { name, .. } => {
            ("rule_set.invalid", format!("rule-sets.{name}"))
        }
    };
    PolicyDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.to_owned(),
        path,
        message: error.to_string(),
    }
}

fn actor_path(name: &str) -> String {
    format!("actors.{name}")
}

fn warnings(revision: &PolicyRevision) -> Vec<PolicyDiagnostic> {
    let mut diagnostics = Vec::new();
    for (name, rule_set) in &revision.document.rule_sets {
        if rule_set.sha256.is_none() {
            diagnostics.push(PolicyDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "rule_set.digest_missing".to_owned(),
                path: format!("rule-sets.{name}.sha256"),
                message: "rule data has no pinned SHA-256 digest".to_owned(),
            });
        }
    }
    for (name, proxy) in &revision.document.proxies {
        if matches!(
            &proxy.server,
            ProxyServer::Mesh {
                instance_id: None,
                virtual_ip: Some(_)
            }
        ) {
            diagnostics.push(PolicyDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "proxy.identity_unpinned".to_owned(),
                path: format!("proxies.{name}.server"),
                message: "virtual-ip-only mesh actor follows the current owner of that address"
                    .to_owned(),
            });
        }
    }
    for (index, source) in revision.document.rules.iter().enumerate() {
        let parts: Vec<&str> = source.split(',').map(str::trim).collect();
        let Some(target) = rule_target(&parts) else {
            continue;
        };
        if !matches!(
            revision
                .document
                .actor_supports_udp(target, &mut BTreeSet::new()),
            Ok(false)
        ) {
            continue;
        }
        let network =
            (parts[0].eq_ignore_ascii_case("NETWORK")).then(|| parts[1].to_ascii_lowercase());
        if network.as_deref() == Some("tcp") {
            continue;
        }
        let (code, message) = if network.as_deref() == Some("udp") {
            (
                "rule.udp_actor_unreachable",
                format!(
                    "UDP matches this rule but actor {target} is TCP-only; runtime skips it and continues"
                ),
            )
        } else {
            (
                "rule.udp_fallthrough",
                format!(
                    "actor {target} is TCP-only; matching UDP continues with the next configured rule"
                ),
            )
        };
        diagnostics.push(PolicyDiagnostic {
            severity: DiagnosticSeverity::Warning,
            code: code.to_owned(),
            path: format!("rules[{index}]"),
            message,
        });
    }
    diagnostics
}

fn rule_target<'a>(parts: &'a [&str]) -> Option<&'a str> {
    let modifier_count = usize::from(
        parts
            .last()
            .is_some_and(|part| part.eq_ignore_ascii_case("no-resolve")),
    );
    parts
        .get(parts.len().checked_sub(1 + modifier_count)?)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_stable_field_diagnostics() {
        let report = preflight_policy_source(
            "version: 1\nrules: [\"NETWORK,quic,DIRECT\"]\n",
            Path::new("."),
        )
        .report;
        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "rule.invalid");
        assert_eq!(report.diagnostics[0].path, "rules[0]");
    }

    #[test]
    fn valid_revision_is_reusable_and_reports_safe_warnings() {
        let preflight = preflight_policy_source(
            r#"
version: 1
proxies:
  exit:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    port: 1080
    via: mesh
    udp: true
rules: ["MATCH,exit"]
"#,
            Path::new("."),
        );
        assert!(preflight.report.valid);
        assert!(preflight.report.revision_id.is_some());
        assert_eq!(preflight.report.sha256.as_deref().map(str::len), Some(64));
        assert_eq!(
            preflight.report.diagnostics[0].code,
            "proxy.identity_unpinned"
        );
        assert!(preflight.revision().is_some());
    }

    #[test]
    fn reports_udp_fallthrough_without_rejecting_ordered_rules() {
        let preflight = preflight_policy_source(
            r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules:
  - DOMAIN-SUFFIX,example.com,tcp-only
  - NETWORK,udp,DIRECT
  - MATCH,tcp-only
"#,
            Path::new("."),
        );
        assert!(preflight.report.valid);
        assert_eq!(preflight.report.diagnostics.len(), 2);
        assert_eq!(preflight.report.diagnostics[0].code, "rule.udp_fallthrough");
        assert_eq!(preflight.report.diagnostics[0].path, "rules[0]");
        assert_eq!(preflight.report.diagnostics[1].code, "rule.udp_fallthrough");
        assert_eq!(preflight.report.diagnostics[1].path, "rules[2]");
    }

    #[test]
    fn reports_explicit_udp_rule_with_tcp_only_actor_as_unreachable() {
        let preflight = preflight_policy_source(
            r#"
version: 1
proxies:
  tcp-only:
    type: socks5
    server: 127.0.0.1
    port: 1080
    udp: false
rules:
  - NETWORK,udp,tcp-only
  - MATCH,DIRECT
"#,
            Path::new("."),
        );
        assert!(preflight.report.valid);
        assert_eq!(
            preflight.report.diagnostics[0].code,
            "rule.udp_actor_unreachable"
        );
        assert_eq!(preflight.report.diagnostics[0].path, "rules[0]");
    }

    #[test]
    fn accepts_no_resolve_without_treating_modifier_as_actor() {
        let preflight = preflight_policy_source(
            r#"
version: 1
rules:
  - GEOIP,lan,DIRECT,no-resolve
  - MATCH,DIRECT
"#,
            Path::new("."),
        );
        assert!(preflight.report.valid);
        assert!(preflight.report.diagnostics.is_empty());
    }
}
