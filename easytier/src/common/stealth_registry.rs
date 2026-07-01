use std::collections::HashSet;

use anyhow::bail;

use crate::proto::common::{StealthTransportProtocol, TransportStealthCapability};

pub const STEALTH_LEVEL_SILENT: u32 = 1 << 0;
pub const STEALTH_LEVEL_AUTHENTICATED: u32 = 1 << 1;
pub const STEALTH_LEVEL_CAMOUFLAGED: u32 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StealthProtocol {
    Udp,
    Tcp,
    FakeTcp,
    Quic,
    Wg,
    Ws,
    Wss,
}

impl StealthProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "udp",
            Self::Tcp => "tcp",
            Self::FakeTcp => "faketcp",
            Self::Quic => "quic",
            Self::Wg => "wg",
            Self::Ws => "ws",
            Self::Wss => "wss",
        }
    }

    pub fn is_implemented(self) -> bool {
        match self {
            Self::Udp | Self::Tcp => true,
            Self::FakeTcp => cfg!(feature = "faketcp"),
            Self::Quic => cfg!(feature = "quic"),
            Self::Wg => cfg!(feature = "wireguard"),
            Self::Ws | Self::Wss => cfg!(feature = "websocket"),
        }
    }

    fn capability(self) -> TransportStealthCapability {
        let (protocol, level_mask) = match self {
            Self::Udp => (StealthTransportProtocol::Udp, STEALTH_LEVEL_SILENT),
            Self::Tcp => (StealthTransportProtocol::Tcp, STEALTH_LEVEL_AUTHENTICATED),
            Self::FakeTcp => (
                StealthTransportProtocol::FakeTcp,
                STEALTH_LEVEL_AUTHENTICATED,
            ),
            Self::Quic => (StealthTransportProtocol::Quic, STEALTH_LEVEL_SILENT),
            Self::Wg => (StealthTransportProtocol::Wg, STEALTH_LEVEL_SILENT),
            Self::Ws => (StealthTransportProtocol::Ws, STEALTH_LEVEL_AUTHENTICATED),
            Self::Wss => (StealthTransportProtocol::Wss, STEALTH_LEVEL_CAMOUFLAGED),
        };
        TransportStealthCapability {
            protocol: protocol.into(),
            wire_version: 1,
            level_mask,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StealthProtocolSet {
    protocols: Vec<StealthProtocol>,
}

impl StealthProtocolSet {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(Self::default());
        }
        let mut seen = HashSet::new();
        let mut protocols = Vec::new();
        for raw in value.split(',').map(str::trim) {
            let protocol = match raw.to_ascii_lowercase().as_str() {
                "udp" => StealthProtocol::Udp,
                "tcp" => StealthProtocol::Tcp,
                "faketcp" => StealthProtocol::FakeTcp,
                "quic" => StealthProtocol::Quic,
                "wg" => StealthProtocol::Wg,
                "ws" => StealthProtocol::Ws,
                "wss" => StealthProtocol::Wss,
                "" => bail!("stealth_protocols contains an empty protocol"),
                _ => bail!("unknown stealth protocol: {raw}"),
            };
            if !seen.insert(protocol) {
                bail!("duplicate stealth protocol: {}", protocol.as_str());
            }
            protocols.push(protocol);
        }
        Ok(Self { protocols })
    }

    pub fn effective_protocols(&self, stealth_mode: bool) -> Vec<StealthProtocol> {
        if !stealth_mode {
            Vec::new()
        } else if self.protocols.is_empty() {
            vec![StealthProtocol::Udp]
        } else {
            self.protocols
                .iter()
                .copied()
                .filter(|protocol| protocol.is_implemented())
                .collect()
        }
    }

    pub fn configured_protocols(&self) -> impl Iterator<Item = StealthProtocol> + '_ {
        self.protocols.iter().copied()
    }

    pub fn capabilities(&self, stealth_mode: bool) -> Vec<TransportStealthCapability> {
        self.effective_protocols(stealth_mode)
            .into_iter()
            .map(StealthProtocol::capability)
            .collect()
    }

    pub fn contains(&self, stealth_mode: bool, protocol: StealthProtocol) -> bool {
        self.effective_protocols(stealth_mode).contains(&protocol)
    }
}

pub fn protocol_enabled(flags: &crate::common::config::Flags, protocol: StealthProtocol) -> bool {
    StealthProtocolSet::parse(&flags.stealth_protocols)
        .expect("stealth_protocols is validated while loading configuration")
        .contains(flags.stealth_mode, protocol)
}

pub fn peer_supports_protocol(
    feature: &crate::proto::common::PeerFeatureFlag,
    protocol: StealthProtocol,
) -> bool {
    if protocol == StealthProtocol::Udp && feature.stealth_supported {
        return true;
    }
    let required = protocol.capability();
    feature.stealth_capabilities.iter().any(|capability| {
        capability.protocol == required.protocol
            && capability.wire_version == required.wire_version
            && capability.level_mask & required.level_mask != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_configuration_is_udp_only_when_enabled() {
        let protocols = StealthProtocolSet::parse("").unwrap();
        assert!(protocols.effective_protocols(false).is_empty());
        assert_eq!(
            protocols.effective_protocols(true),
            vec![StealthProtocol::Udp]
        );
    }

    #[test]
    fn explicit_protocols_are_strictly_validated() {
        let protocols = StealthProtocolSet::parse("udp,tcp,wss").unwrap();
        let mut expected = vec![StealthProtocol::Udp, StealthProtocol::Tcp];
        if StealthProtocol::Wss.is_implemented() {
            expected.push(StealthProtocol::Wss);
        }
        assert_eq!(protocols.effective_protocols(true), expected);
        assert_eq!(
            protocols.configured_protocols().collect::<Vec<_>>(),
            [
                StealthProtocol::Udp,
                StealthProtocol::Tcp,
                StealthProtocol::Wss
            ]
        );
        assert!(StealthProtocolSet::parse("udp,udp").is_err());
        assert!(StealthProtocolSet::parse("udp,unknown").is_err());
        assert!(StealthProtocolSet::parse("udp,").is_err());
    }

    #[test]
    fn unimplemented_protocols_are_not_enabled_or_advertised() {
        let protocols = StealthProtocolSet::parse("ws,wss").unwrap();
        if cfg!(feature = "websocket") {
            assert_eq!(protocols.effective_protocols(true).len(), 2);
            assert_eq!(protocols.capabilities(true).len(), 2);
        } else {
            assert!(protocols.effective_protocols(true).is_empty());
            assert!(protocols.capabilities(true).is_empty());
        }
    }

    #[test]
    fn peer_capability_requires_matching_wire_version_and_level() {
        let protocol = StealthProtocol::Tcp;
        let mut capability = protocol.capability();
        let feature = crate::proto::common::PeerFeatureFlag {
            stealth_capabilities: vec![capability.clone()],
            ..Default::default()
        };
        assert!(peer_supports_protocol(&feature, protocol));

        capability.wire_version = 2;
        let feature = crate::proto::common::PeerFeatureFlag {
            stealth_capabilities: vec![capability],
            ..Default::default()
        };
        assert!(!peer_supports_protocol(&feature, protocol));

        capability.wire_version = 1;
        capability.level_mask = 0;
        let feature = crate::proto::common::PeerFeatureFlag {
            stealth_capabilities: vec![capability],
            ..Default::default()
        };
        assert!(!peer_supports_protocol(&feature, protocol));
        assert!(peer_supports_protocol(
            &crate::proto::common::PeerFeatureFlag {
                stealth_supported: true,
                ..Default::default()
            },
            StealthProtocol::Udp
        ));
    }
}
