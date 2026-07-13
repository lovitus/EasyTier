use std::{
    fs::{File, OpenOptions},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use netlink_packet_route::{
    AddressFamily, RouteNetlinkMessage,
    route::{
        RouteAddress, RouteAttribute, RouteHeader, RouteMessage, RouteProtocol, RouteScope,
        RouteType,
    },
    rule::{RuleAction, RuleAttribute, RuleMessage},
};

use crate::common::ifcfg::netlink::{
    NetlinkIfConfiger, replace_netlink_route, send_netlink_req_and_wait_one_resp,
};

const POLICY_TABLE: u32 = 52_000;
// Stay ahead of the kernel main/default rules while avoiding the common low
// priorities used by administrators for hand-written policy routing.
const POLICY_RULE_PRIORITY: u32 = 10_900;
const POLICY_ROUTE_PROTOCOL: RouteProtocol = RouteProtocol::Other(99);
const POLICY_LOCK_PATH: &str = "/run/easytier-policy-routing.lock";

pub(crate) struct PolicyRoutingGuard {
    outbound_interface: String,
    outbound_index: u32,
    tun_index: u32,
    enable_ipv6: bool,
    has_v4_bypass: bool,
    has_v6_bypass: bool,
    socket_mark: u32,
    routes: Vec<RouteMessage>,
    rules: Vec<RuleMessage>,
    outbound_addresses: Vec<IpAddr>,
    _lock: nix::fcntl::Flock<File>,
}

impl PolicyRoutingGuard {
    pub(crate) fn install(
        outbound_interface: &str,
        tun_interface: &str,
        enable_ipv6: bool,
        socket_mark: Option<u32>,
    ) -> anyhow::Result<Self> {
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(POLICY_LOCK_PATH)?;
        let lock = nix::fcntl::Flock::lock(lock_file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_, error)| {
                anyhow::anyhow!("policy routing is owned by another process: {error}")
            })?;
        let socket_mark = socket_mark.filter(|mark| *mark != 0).ok_or_else(|| {
            anyhow::anyhow!("policy mode requires a non-zero underlay socket mark")
        })?;
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
        if !physical_routes(&v4_routes, outbound_index)
            .iter()
            .any(|route| route.header.destination_prefix_length == 0)
        {
            anyhow::bail!(
                "policy outbound interface {outbound_interface} has no IPv4 default route"
            );
        }

        let mut guard = Self {
            outbound_interface: outbound_interface.to_owned(),
            outbound_index,
            tun_index,
            enable_ipv6,
            has_v4_bypass: false,
            has_v6_bypass: false,
            socket_mark,
            routes: Vec::new(),
            rules: Vec::new(),
            outbound_addresses: addresses,
            _lock: lock,
        };
        let result = guard.refresh_with(v4_routes, v6_routes, guard.outbound_addresses.clone());
        if let Err(error) = result {
            guard.remove_all();
            return Err(error);
        }
        Ok(guard)
    }

    pub(crate) fn refresh(&mut self) -> anyhow::Result<bool> {
        self.outbound_index = NetlinkIfConfiger::get_interface_index(&self.outbound_interface)?;
        let addresses = NetlinkIfConfiger::list_addresses(&self.outbound_interface)?
            .into_iter()
            .map(|address| address.address())
            .filter(|address| usable_source(*address))
            .filter(|address| self.enable_ipv6 || address.is_ipv4())
            .collect::<Vec<_>>();
        if !addresses.iter().any(IpAddr::is_ipv4) {
            anyhow::bail!(
                "policy outbound interface {} has no usable IPv4 address",
                self.outbound_interface
            );
        }
        let v4_routes = NetlinkIfConfiger::list_route_messages(AddressFamily::Inet)?;
        let v6_routes = NetlinkIfConfiger::list_route_messages(AddressFamily::Inet6)?;
        self.refresh_with(v4_routes, v6_routes, addresses)
    }

    fn refresh_with(
        &mut self,
        v4_routes: Vec<RouteMessage>,
        v6_routes: Vec<RouteMessage>,
        addresses: Vec<IpAddr>,
    ) -> anyhow::Result<bool> {
        let v4_physical = physical_routes(&v4_routes, self.outbound_index);
        let has_v4_bypass = v4_physical
            .iter()
            .any(|route| route.header.destination_prefix_length == 0);
        if !has_v4_bypass && (self.routes.is_empty() || self.has_v4_bypass) {
            tracing::warn!(
                interface = self.outbound_interface,
                "policy outbound interface has no IPv4 default route; non-mesh traffic is fail-closed"
            );
        }
        let mut desired_routes = v4_physical
            .into_iter()
            .map(|route| bypass_route(route, self.outbound_index))
            .collect::<Vec<_>>();
        // A terminal route prevents marked underlay sockets from falling
        // through to the main table's TUN capture when the physical route
        // disappears during a network transition.
        desired_routes.push(fail_closed_route(AddressFamily::Inet));
        let v6_physical = physical_routes(&v6_routes, self.outbound_index);
        let has_v6_bypass = self.enable_ipv6
            && v6_physical
                .iter()
                .any(|route| route.header.destination_prefix_length == 0);
        if has_v6_bypass {
            desired_routes.extend(
                v6_physical
                    .into_iter()
                    .map(|route| bypass_route(route, self.outbound_index)),
            );
        } else if self.enable_ipv6 && (self.routes.is_empty() || self.has_v6_bypass) {
            tracing::warn!(
                interface = self.outbound_interface,
                "policy outbound interface has no IPv6 default route; IPv6 policy traffic remains unavailable"
            );
        }
        desired_routes.push(capture_route(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            self.tun_index,
        ));
        desired_routes.push(capture_route(
            IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)),
            self.tun_index,
        ));
        if has_v6_bypass {
            desired_routes.push(capture_route(
                IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                self.tun_index,
            ));
            desired_routes.push(capture_route(
                IpAddr::V6(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0)),
                self.tun_index,
            ));
        }

        let mut desired_rules = addresses
            .iter()
            .copied()
            .filter(|address| address.is_ipv4() || has_v6_bypass)
            .map(source_rule)
            .collect::<Vec<_>>();
        desired_rules.push(mark_rule(self.socket_mark, AddressFamily::Inet));
        if has_v6_bypass {
            desired_rules.push(mark_rule(self.socket_mark, AddressFamily::Inet6));
        }
        if same_members(&desired_routes, &self.routes)
            && same_members(&desired_rules, &self.rules)
            && same_members(&addresses, &self.outbound_addresses)
        {
            return Ok(false);
        }

        reconcile_routes(&mut self.routes, desired_routes)?;
        reconcile_rules(&mut self.rules, desired_rules)?;
        self.outbound_addresses = addresses;
        self.has_v4_bypass = has_v4_bypass;
        self.has_v6_bypass = has_v6_bypass;
        Ok(true)
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

fn same_members<T: PartialEq>(left: &[T], right: &[T]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut matched = vec![false; right.len()];
    left.iter().all(|item| {
        let Some(position) = right.iter().enumerate().find_map(|(position, candidate)| {
            (!matched[position] && candidate == item).then_some(position)
        }) else {
            return false;
        };
        matched[position] = true;
        true
    })
}

fn reconcile_routes(
    installed: &mut Vec<RouteMessage>,
    desired: Vec<RouteMessage>,
) -> anyhow::Result<()> {
    for route in &desired {
        if installed.contains(route) {
            continue;
        }
        if let Some(position) = installed.iter().position(|old| same_route_key(old, route)) {
            replace_netlink_route(route.clone())?;
            installed[position] = route.clone();
        } else {
            send_netlink_req_and_wait_one_resp(
                RouteNetlinkMessage::NewRoute(route.clone()),
                false,
            )?;
            installed.push(route.clone());
        }
    }
    let stale = installed
        .iter()
        .filter(|route| !desired.contains(route))
        .cloned()
        .collect::<Vec<_>>();
    for route in stale {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRoute(route.clone()), true)?;
        installed.retain(|current| current != &route);
    }
    Ok(())
}

fn reconcile_rules(
    installed: &mut Vec<RuleMessage>,
    desired: Vec<RuleMessage>,
) -> anyhow::Result<()> {
    for rule in &desired {
        if installed.contains(rule) {
            continue;
        }
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::NewRule(rule.clone()), false)?;
        installed.push(rule.clone());
    }
    let stale = installed
        .iter()
        .filter(|rule| !desired.contains(rule))
        .cloned()
        .collect::<Vec<_>>();
    for rule in stale {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRule(rule.clone()), true)?;
        installed.retain(|current| current != &rule);
    }
    Ok(())
}

fn same_route_key(left: &RouteMessage, right: &RouteMessage) -> bool {
    left.header.address_family == right.header.address_family
        && left.header.kind == right.header.kind
        && left.header.destination_prefix_length == right.header.destination_prefix_length
        && left.header.source_prefix_length == right.header.source_prefix_length
        && left.header.tos == right.header.tos
        && route_table(left) == route_table(right)
        && route_address(left, true) == route_address(right, true)
        && route_address(left, false) == route_address(right, false)
        && route_priority(left) == route_priority(right)
}

fn route_address(route: &RouteMessage, destination: bool) -> Option<&RouteAddress> {
    route
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            RouteAttribute::Destination(address) if destination => Some(address),
            RouteAttribute::Source(address) if !destination => Some(address),
            _ => None,
        })
}

fn route_priority(route: &RouteMessage) -> Option<u32> {
    route
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            RouteAttribute::Priority(priority) => Some(*priority),
            _ => None,
        })
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
    // The /1 prefix wins over a physical /0. The high metric intentionally
    // loses to any pre-existing route with the same prefix length.
    route.attributes.push(RouteAttribute::Priority(65_535));
    route
}

fn fail_closed_route(family: AddressFamily) -> RouteMessage {
    let mut route = base_route(family, 0, POLICY_TABLE);
    route.header.kind = RouteType::Unreachable;
    route.attributes.push(RouteAttribute::Priority(u32::MAX));
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

fn mark_rule(mark: u32, family: AddressFamily) -> RuleMessage {
    let mut rule = RuleMessage::default();
    rule.header.family = family;
    rule.header.action = RuleAction::ToTable;
    rule.attributes.push(RuleAttribute::FwMark(mark));
    rule.attributes.push(RuleAttribute::FwMask(u32::MAX));
    rule.attributes
        .push(RuleAttribute::Priority(POLICY_RULE_PRIORITY - 1));
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
    let mut stale_rules = Vec::new();
    for family in [AddressFamily::Inet, AddressFamily::Inet6] {
        for rule in NetlinkIfConfiger::list_rule_messages(family)? {
            let table = rule
                .attributes
                .iter()
                .find_map(|attribute| match attribute {
                    RuleAttribute::Table(table) => Some(*table),
                    _ => None,
                });
            let priority = rule
                .attributes
                .iter()
                .find_map(|attribute| match attribute {
                    RuleAttribute::Priority(priority) => Some(*priority),
                    _ => None,
                });
            let reserved_priority = priority.is_some_and(|priority| {
                priority == POLICY_RULE_PRIORITY || priority == POLICY_RULE_PRIORITY - 1
            });
            if reserved_priority && table != Some(POLICY_TABLE) {
                anyhow::bail!("ip-rule priority {} is already in use", priority.unwrap());
            }
            if table == Some(POLICY_TABLE) {
                if !reserved_priority || stale.is_empty() {
                    anyhow::bail!(
                        "routing table {POLICY_TABLE} is already used by another application"
                    );
                }
                stale_rules.push(rule);
            }
        }
    }
    for rule in stale_rules {
        send_netlink_req_and_wait_one_resp(RouteNetlinkMessage::DelRule(rule), true)?;
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

    #[test]
    fn fail_closed_route_terminates_policy_lookup() {
        let route = fail_closed_route(AddressFamily::Inet);
        assert_eq!(route.header.kind, RouteType::Unreachable);
        assert_eq!(route.header.destination_prefix_length, 0);
        assert_eq!(route_table(&route), POLICY_TABLE);
    }

    #[test]
    fn route_identity_distinguishes_metrics() {
        let mut first = capture_route(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7);
        let second = first.clone();
        assert!(same_route_key(&first, &second));
        first
            .attributes
            .retain(|attribute| !matches!(attribute, RouteAttribute::Priority(_)));
        first.attributes.push(RouteAttribute::Priority(10));
        assert!(!same_route_key(&first, &second));
    }

    #[test]
    fn member_comparison_ignores_netlink_dump_order() {
        assert!(same_members(&[1, 2, 3], &[3, 1, 2]));
        assert!(!same_members(&[1, 2], &[1, 3]));
    }
}
