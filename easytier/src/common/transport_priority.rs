use std::{collections::HashMap, net::IpAddr, str::FromStr};

use anyhow::{Context, bail};

pub const BUILTIN_TRANSPORT_ORDER: [&str; 7] = ["tcp", "udp", "wg", "quic", "ws", "wss", "faketcp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportPriorityScope {
    Global,
    Wan,
    Lan,
    VirtualIp(IpAddr),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransportPriority {
    rules: HashMap<TransportPriorityScope, Vec<String>>,
}

impl TransportPriority {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(Self::default());
        }

        let mut rules = HashMap::new();
        for raw_rule in value.split(';') {
            let raw_rule = raw_rule.trim();
            if raw_rule.is_empty() {
                bail!("transport priority contains an empty rule");
            }
            let (raw_scope, raw_protocols) = if raw_rule.starts_with('[') {
                let separator = raw_rule.find("]:").with_context(|| {
                    format!("bracketed transport priority rule has no ']:' separator: {raw_rule}")
                })?;
                (&raw_rule[..=separator], &raw_rule[separator + 2..])
            } else {
                raw_rule.split_once(':').with_context(|| {
                    format!("transport priority rule has no ':' separator: {raw_rule}")
                })?
            };
            let scope = parse_scope(raw_scope.trim())?;
            if rules.contains_key(&scope) {
                bail!("duplicate transport priority scope: {raw_scope}");
            }

            let raw_protocols = raw_protocols.trim();
            if raw_protocols.is_empty() {
                bail!("transport priority scope {raw_scope} has an empty protocol list");
            }
            let mut protocols = Vec::new();
            for protocol in raw_protocols.split(',').map(str::trim) {
                if protocol.is_empty() {
                    bail!("transport priority scope {raw_scope} contains an empty protocol");
                }
                let protocol = protocol.to_ascii_lowercase();
                if !BUILTIN_TRANSPORT_ORDER.contains(&protocol.as_str()) {
                    bail!("unknown transport protocol: {protocol}");
                }
                if protocols.contains(&protocol) {
                    bail!("duplicate transport protocol {protocol} in scope {raw_scope}");
                }
                protocols.push(protocol);
            }
            rules.insert(scope, protocols);
        }
        Ok(Self { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn has_virtual_ip_rule(&self, ip: IpAddr) -> bool {
        self.rules
            .contains_key(&TransportPriorityScope::VirtualIp(ip))
    }

    pub fn configured_protocols(&self) -> impl Iterator<Item = &str> {
        self.rules
            .values()
            .flat_map(|protocols| protocols.iter().map(String::as_str))
    }

    pub fn order_for(&self, is_lan: bool, virtual_ip: Option<IpAddr>) -> Vec<String> {
        let mut order = BUILTIN_TRANSPORT_ORDER.map(str::to_owned).to_vec();
        self.promote(TransportPriorityScope::Global, &mut order);
        self.promote(
            if is_lan {
                TransportPriorityScope::Lan
            } else {
                TransportPriorityScope::Wan
            },
            &mut order,
        );
        if let Some(ip) = virtual_ip {
            self.promote(TransportPriorityScope::VirtualIp(ip), &mut order);
        }
        order
    }

    fn promote(&self, scope: TransportPriorityScope, inherited: &mut Vec<String>) {
        let Some(promoted) = self.rules.get(&scope) else {
            return;
        };
        inherited.retain(|protocol| !promoted.contains(protocol));
        inherited.splice(0..0, promoted.iter().cloned());
    }
}

fn parse_scope(value: &str) -> anyhow::Result<TransportPriorityScope> {
    match value.to_ascii_lowercase().as_str() {
        "global" => Ok(TransportPriorityScope::Global),
        "wan" => Ok(TransportPriorityScope::Wan),
        "lan" => Ok(TransportPriorityScope::Lan),
        _ => {
            let ip = if value.starts_with('[') || value.ends_with(']') {
                if !(value.starts_with('[') && value.ends_with(']')) {
                    bail!("invalid bracketed virtual IP scope: {value}");
                }
                let ip = IpAddr::from_str(&value[1..value.len() - 1])
                    .with_context(|| format!("invalid virtual IP scope: {value}"))?;
                if ip.is_ipv4() {
                    bail!("IPv4 virtual IP scopes must not use brackets: {value}");
                }
                ip
            } else {
                let ip = IpAddr::from_str(value)
                    .with_context(|| format!("unknown transport priority scope: {value}"))?;
                if ip.is_ipv6() {
                    bail!("IPv6 virtual IP scopes must use brackets: [{value}]");
                }
                ip
            };
            Ok(TransportPriorityScope::VirtualIp(ip))
        }
    }
}

pub fn protocol_is_compiled(protocol: &str) -> bool {
    matches!(protocol, "tcp" | "udp")
        || (protocol == "wg" && cfg!(feature = "wireguard"))
        || (protocol == "quic" && cfg!(feature = "quic"))
        || (matches!(protocol, "ws" | "wss") && cfg!(feature = "websocket"))
        || (protocol == "faketcp" && cfg!(feature = "faketcp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_layers_priority_rules() {
        let parsed = TransportPriority::parse(
            "global:udp,tcp;wan:quic,wss;lan:faketcp;10.44.0.3:tcp,quic;[fd00::3]:quic,tcp",
        )
        .unwrap();

        assert_eq!(
            parsed.order_for(false, Some("10.44.0.3".parse().unwrap())),
            ["tcp", "quic", "wss", "udp", "wg", "ws", "faketcp"]
        );
        assert_eq!(
            parsed.order_for(true, None),
            ["faketcp", "udp", "tcp", "wg", "quic", "ws", "wss"]
        );
        assert_eq!(
            parsed.order_for(false, Some("fd00::3".parse().unwrap())),
            ["quic", "tcp", "wss", "udp", "wg", "ws", "faketcp"]
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_rules() {
        for invalid in [
            "global:",
            "global:tcp,tcp",
            "global:madeup",
            "global:tcp;global:udp",
            "fd00::3:quic",
            "[10.44.0.3]:tcp",
            "unknown:tcp",
            "global:tcp;",
        ] {
            assert!(
                TransportPriority::parse(invalid).is_err(),
                "accepted invalid rule {invalid}"
            );
        }
    }
}
