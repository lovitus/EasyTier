use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use netlink_packet_route::{
    AddressFamily, RouteNetlinkMessage,
    route::{
        RouteAddress, RouteAttribute, RouteHeader, RouteMessage, RouteProtocol, RouteScope,
        RouteType,
    },
    rule::{RuleAction, RuleAttribute, RuleMessage},
};

use crate::common::ifcfg::netlink::{NetlinkIfConfiger, send_netlink_req_and_wait_one_resp};

const POLICY_TABLE: u32 = 52_000;
const POLICY_RULE_PRIORITY: u32 = 10_900;
const POLICY_ROUTE_PROTOCOL: RouteProtocol = RouteProtocol::Other(99);

pub(crate) struct PolicyRoutingGuard {
    routes: Vec<RouteMessage>,
    rules: Vec<RuleMessage>,
    outbound_addresses: Vec<IpAddr>,
}

impl PolicyRoutingGuard {
    pub(crate) fn install(
        outbound_interface: &str,
        tun_interface: &str,
        enable_ipv6: bool,
    ) -> anyhow::Result<Self> {
        let outbound_index = NetlinkIfConfiger::get_interface_index(outbound_interface)?;
        let tun_index = NetlinkIfConfiger::get_interface_index(tun_interface)?;
        let addresses = NetlinkIfConfiger::list_addresses(outbound_interface)?
            .into_iter()
            .map(|address| address.address())
            .filter(|address| usable_source(*address))
            .filter(|address| enable_ipv6 || address.is_ipv4())
            .collect::<Vec<_>>();
        if !addresses.iter().any(IpAddr::is_ipv4) {
            anyhow::bail!(
                "policy outbound interface {outbound_interface} has no usable IPv4 address"
            );
        }

        let v4_routes = NetlinkIfConfiger::list_route_messages(AddressFamily::Inet)?;
        // Always inspect IPv6 so a restart with IPv6 disabled still removes
        // an owned table left by a previous dual-stack process crash.
        let v6_routes = NetlinkIfConfiger::list_route_messages(AddressFamily::Inet6)?;
        cleanup_stale(&v4_routes, &v6_routes)?;

        let mut guard = Self {
            routes: Vec::new(),
            rules: Vec::new(),
            outbound_addresses: addresses,
        };
        let result = (|| -> anyhow::Result<()> {
            let v4_physical = physical_routes(&v4_routes, outbound_index);
            if !v4_physical
                .iter()
                .any(|route| route.header.destination_prefix_length == 0)
            {
                anyhow::bail!(
                    "policy outbound interface {outbound_interface} has no IPv4 default route"
                );
            }
            for route in v4_physical {
                guard.add_route(bypass_route(route, outbound_index))?;
            }

            let has_v6_bypass = if enable_ipv6 {
                let v6_physical = physical_routes(&v6_routes, outbound_index);
                if v6_physical
                    .iter()
                    .any(|route| route.header.destination_prefix_length == 0)
                {
                    for route in v6_physical {
                        guard.add_route(bypass_route(route, outbound_index))?;
                    }
                    true
                } else {
                    tracing::warn!(
                        interface = outbound_interface,
                        "policy outbound interface has no IPv6 default route; IPv6 policy traffic remains unavailable"
                    );
                    false
                }
            } else {
                false
            };

            for address in guard.outbound_addresses.clone() {
                if address.is_ipv6() && !has_v6_bypass {
                    continue;
                }
                guard.add_rule(source_rule(address))?;
            }
            guard.add_route(capture_route(IpAddr::V4(Ipv4Addr::UNSPECIFIED), tun_index))?;
            guard.add_route(capture_route(
                IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)),
                tun_index,
            ))?;
            if has_v6_bypass {
                guard.add_route(capture_route(IpAddr::V6(Ipv6Addr::UNSPECIFIED), tun_index))?;
                guard.add_route(capture_route(
                    IpAddr::V6(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0)),
                    tun_index,
                ))?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            guard.remove_all();
            return Err(error);
        }
        Ok(guard)
    }

    fn add_route(&mut self, route: RouteMessage) -> anyhow::Result<()> {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::NewRoute(route.clone()), false)?;
        self.routes.push(route);
        Ok(())
    }

    fn add_rule(&mut self, rule: RuleMessage) -> anyhow::Result<()> {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::NewRule(rule.clone()), false)?;
        self.rules.push(rule);
        Ok(())
    }

    fn remove_all(&mut self) {
        for route in self.routes.drain(..).rev() {
            if let Err(error) =
                send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRoute(route), true)
            {
                tracing::warn!(?error, "failed to remove policy route");
            }
        }
        for rule in self.rules.drain(..).rev() {
            if let Err(error) =
                send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRule(rule), true)
            {
                tracing::warn!(?error, "failed to remove policy rule");
            }
        }
    }
}

impl Drop for PolicyRoutingGuard {
    fn drop(&mut self) {
        self.remove_all();
    }
}

fn usable_source(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_link_local()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
        }
    }
}

fn physical_routes(routes: &[RouteMessage], ifindex: u32) -> Vec<&RouteMessage> {
    routes
        .iter()
        .filter(|route| {
            route.header.kind == RouteType::Unicast
                && route_table(route) == RouteHeader::RT_TABLE_MAIN as u32
                && route.attributes.iter().any(
                    |attribute| matches!(attribute, RouteAttribute::Oif(index) if *index == ifindex),
                )
        })
        .collect()
}

fn route_table(route: &RouteMessage) -> u32 {
    route
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            RouteAttribute::Table(table) => Some(*table),
            _ => None,
        })
        .unwrap_or(route.header.table as u32)
}

fn bypass_route(source: &RouteMessage, ifindex: u32) -> RouteMessage {
    let mut route = base_route(
        source.header.address_family,
        source.header.destination_prefix_length,
        POLICY_TABLE,
    );
    route.header.source_prefix_length = source.header.source_prefix_length;
    route.header.tos = source.header.tos;
    route.header.scope = source.header.scope;
    route.attributes.push(RouteAttribute::Oif(ifindex));
    for attribute in &source.attributes {
        match attribute {
            RouteAttribute::Destination(_)
            | RouteAttribute::Source(_)
            | RouteAttribute::Gateway(_)
            | RouteAttribute::Via(_)
            | RouteAttribute::Priority(_)
            | RouteAttribute::PrefSource(_) => route.attributes.push(attribute.clone()),
            _ => {}
        }
    }
    route
}

fn capture_route(destination: IpAddr, ifindex: u32) -> RouteMessage {
    let (family, destination) = match destination {
        IpAddr::V4(address) => (AddressFamily::Inet, RouteAddress::Inet(address)),
        IpAddr::V6(address) => (AddressFamily::Inet6, RouteAddress::Inet6(address)),
    };
    let mut route = base_route(family, 1, RouteHeader::RT_TABLE_MAIN as u32);
    route
        .attributes
        .push(RouteAttribute::Destination(destination));
    route.attributes.push(RouteAttribute::Oif(ifindex));
    route.attributes.push(RouteAttribute::Priority(65_535));
    route
}

fn base_route(family: AddressFamily, prefix: u8, table: u32) -> RouteMessage {
    let mut route = RouteMessage::default();
    route.header.address_family = family;
    route.header.destination_prefix_length = prefix;
    route.header.protocol = POLICY_ROUTE_PROTOCOL;
    route.header.scope = RouteScope::Universe;
    route.header.kind = RouteType::Unicast;
    if let Ok(table) = u8::try_from(table) {
        route.header.table = table;
    } else {
        route.header.table = RouteHeader::RT_TABLE_UNSPEC;
        route.attributes.push(RouteAttribute::Table(table));
    }
    route
}

fn source_rule(source: IpAddr) -> RuleMessage {
    let mut rule = RuleMessage::default();
    rule.header.family = if source.is_ipv4() {
        AddressFamily::Inet
    } else {
        AddressFamily::Inet6
    };
    rule.header.src_len = if source.is_ipv4() { 32 } else { 128 };
    rule.header.action = RuleAction::ToTable;
    rule.attributes.push(RuleAttribute::Source(source));
    rule.attributes
        .push(RuleAttribute::Priority(POLICY_RULE_PRIORITY));
    rule.attributes.push(RuleAttribute::Table(POLICY_TABLE));
    rule
}

fn cleanup_stale(v4_routes: &[RouteMessage], v6_routes: &[RouteMessage]) -> anyhow::Result<()> {
    let stale = v4_routes
        .iter()
        .chain(v6_routes)
        .filter(|route| route_table(route) == POLICY_TABLE)
        .cloned()
        .collect::<Vec<_>>();
    if stale
        .iter()
        .any(|route| route.header.protocol != POLICY_ROUTE_PROTOCOL)
    {
        anyhow::bail!("routing table {POLICY_TABLE} is already used by another application");
    }
    if stale.is_empty() {
        return Ok(());
    }
    for family in [AddressFamily::Inet, AddressFamily::Inet6] {
        for rule in NetlinkIfConfiger::list_rule_messages(family)? {
            if rule.attributes.iter().any(
                |attribute| matches!(attribute, RuleAttribute::Table(table) if *table == POLICY_TABLE),
            ) && rule.attributes.iter().any(
                |attribute| matches!(attribute, RuleAttribute::Priority(priority) if *priority == POLICY_RULE_PRIORITY),
            ) {
                send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRule(rule), true)?;
            }
        }
    }
    for route in stale {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRoute(route), true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_control_and_link_local_sources() {
        assert!(!usable_source(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!usable_source(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!usable_source(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!usable_source(IpAddr::V6("fe80::1".parse().unwrap())));
        assert!(usable_source(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(usable_source(IpAddr::V6("2001:db8::1".parse().unwrap())));
    }

    #[test]
    fn capture_routes_split_each_address_family() {
        let low = capture_route(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7);
        let high = capture_route(IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)), 7);
        assert_eq!(low.header.destination_prefix_length, 1);
        assert_eq!(high.header.destination_prefix_length, 1);
        assert_ne!(low.attributes, high.attributes);
    }
}
