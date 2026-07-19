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

use super::PolicyUnderlayTransition;

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

    pub(crate) fn refresh(&mut self) -> anyhow::Result<PolicyUnderlayTransition> {
        let was_available = self.has_v4_bypass;
        let previous_index = self.outbound_index;
        let previous_addresses = self.outbound_addresses.clone();
        let outbound_index = match NetlinkIfConfiger::get_interface_index(&self.outbound_interface)
        {
            Ok(index) => index,
            Err(error) => return self.fail_closed_refresh(error.into()),
        };
        let addresses = match NetlinkIfConfiger::list_addresses(&self.outbound_interface) {
            Ok(addresses) => addresses,
            Err(error) => return self.fail_closed_refresh(error.into()),
        }
        .into_iter()
        .map(|address| address.address())
        .filter(|address| usable_source(*address))
        .filter(|address| self.enable_ipv6 || address.is_ipv4())
        .collect::<Vec<_>>();
        let v4_routes = match NetlinkIfConfiger::list_route_messages(AddressFamily::Inet) {
            Ok(routes) => routes,
            Err(error) => return self.fail_closed_refresh(error.into()),
        };
        let v6_routes = match NetlinkIfConfiger::list_route_messages(AddressFamily::Inet6) {
            Ok(routes) => routes,
            Err(error) => return self.fail_closed_refresh(error.into()),
        };
        self.outbound_index = outbound_index;
        let routes_changed = self.refresh_with(v4_routes, v6_routes, addresses)?;
        Ok(classify_underlay_transition(
            was_available,
            self.has_v4_bypass,
            previous_index,
            self.outbound_index,
            &previous_addresses,
            &self.outbound_addresses,
            routes_changed,
        ))
    }

    pub(crate) fn has_usable_underlay(&self) -> bool {
        self.has_v4_bypass
    }

    fn fail_closed_refresh<T>(&mut self, error: anyhow::Error) -> anyhow::Result<T> {
        self.reconcile_unavailable()?;
        Err(error)
    }

    fn reconcile_unavailable(&mut self) -> anyhow::Result<bool> {
        let desired_routes = policy_boundary_routes(self.tun_index, self.enable_ipv6);
        let desired_rules = policy_mark_rules(self.socket_mark, self.enable_ipv6);
        let changed = !same_members(&desired_routes, &self.routes)
            || !same_members(&desired_rules, &self.rules)
            || !self.outbound_addresses.is_empty()
            || self.has_v4_bypass
            || self.has_v6_bypass;
        reconcile_routes(&mut self.routes, desired_routes)?;
        reconcile_rules(&mut self.rules, desired_rules)?;
        self.outbound_addresses.clear();
        self.has_v4_bypass = false;
        self.has_v6_bypass = false;
        Ok(changed)
    }

    fn refresh_with(
        &mut self,
        v4_routes: Vec<RouteMessage>,
        v6_routes: Vec<RouteMessage>,
        addresses: Vec<IpAddr>,
    ) -> anyhow::Result<bool> {
        let has_v4_address = addresses.iter().any(IpAddr::is_ipv4);
        let v4_physical = if has_v4_address {
            physical_routes(&v4_routes, self.outbound_index)
        } else {
            Vec::new()
        };
        let has_v4_bypass = has_v4_address
            && v4_physical
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
        let has_v6_address = addresses.iter().any(IpAddr::is_ipv6);
        let v6_physical = if self.enable_ipv6 && has_v6_address {
            physical_routes(&v6_routes, self.outbound_index)
        } else {
            Vec::new()
        };
        let has_v6_bypass = self.enable_ipv6
            && has_v6_address
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
        // Keep capture and a terminal private-table route installed for every
        // enabled family. If the physical default disappears, marked Leaf and
        // EasyTier sockets must fail closed instead of falling through to the
        // main table or bypassing policy during the transition.
        desired_routes.extend(policy_boundary_routes(self.tun_index, self.enable_ipv6));

        let mut desired_rules = addresses
            .iter()
            .copied()
            .filter(|address| address.is_ipv4() || has_v6_bypass)
            .map(source_rule)
            .collect::<Vec<_>>();
        desired_rules.extend(policy_mark_rules(self.socket_mark, self.enable_ipv6));
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

fn classify_underlay_transition(
    was_available: bool,
    is_available: bool,
    previous_index: u32,
    current_index: u32,
    previous_addresses: &[IpAddr],
    current_addresses: &[IpAddr],
    routes_changed: bool,
) -> PolicyUnderlayTransition {
    if was_available && !is_available {
        return PolicyUnderlayTransition::Lost;
    }
    if !was_available && is_available {
        return PolicyUnderlayTransition::Recovered;
    }
    if is_available
        && (previous_index != current_index
            || previous_addresses
                .iter()
                .any(|address| !current_addresses.contains(address)))
    {
        return PolicyUnderlayTransition::IdentityChanged;
    }
    if routes_changed {
        PolicyUnderlayTransition::RoutesChanged
    } else {
        PolicyUnderlayTransition::Unchanged
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
    // Linux reserves the maximum IPv6 metric for its implicit unreachable
    // sentinel. An explicit IPv6 route with that metric fails with EEXIST,
    // including on the CentOS 7 / Linux 3.10 validation hosts. MAX - 1 keeps
    // EasyTier's terminal route last without colliding with the kernel route.
    let priority = if family == AddressFamily::Inet6 {
        u32::MAX - 1
    } else {
        u32::MAX
    };
    route.attributes.push(RouteAttribute::Priority(priority));
    route
}

fn policy_boundary_routes(tun_index: u32, enable_ipv6: bool) -> Vec<RouteMessage> {
    let mut routes = vec![
        fail_closed_route(AddressFamily::Inet),
        capture_route(IpAddr::V4(Ipv4Addr::UNSPECIFIED), tun_index),
        capture_route(IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)), tun_index),
    ];
    if enable_ipv6 {
        routes.extend([
            fail_closed_route(AddressFamily::Inet6),
            capture_route(IpAddr::V6(Ipv6Addr::UNSPECIFIED), tun_index),
            capture_route(
                IpAddr::V6(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0)),
                tun_index,
            ),
        ]);
    }
    routes
}

fn policy_mark_rules(mark: u32, enable_ipv6: bool) -> Vec<RuleMessage> {
    let mut rules = vec![mark_rule(mark, AddressFamily::Inet)];
    if enable_ipv6 {
        rules.push(mark_rule(mark, AddressFamily::Inet6));
    }
    rules
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
        assert_eq!(route_priority(&route), Some(u32::MAX));

        let ipv6 = fail_closed_route(AddressFamily::Inet6);
        assert_eq!(ipv6.header.kind, RouteType::Unreachable);
        assert_eq!(route_table(&ipv6), POLICY_TABLE);
        assert_eq!(route_priority(&ipv6), Some(u32::MAX - 1));
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

    #[test]
    fn underlay_transition_separates_routes_identity_and_availability() {
        let first = [IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))];
        let second = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11));
        assert_eq!(
            classify_underlay_transition(true, true, 3, 3, &first, &first, false),
            PolicyUnderlayTransition::Unchanged
        );
        assert_eq!(
            classify_underlay_transition(true, true, 3, 3, &first, &first, true),
            PolicyUnderlayTransition::RoutesChanged
        );
        let added = [first[0], second];
        assert_eq!(
            classify_underlay_transition(true, true, 3, 3, &first, &added, true),
            PolicyUnderlayTransition::RoutesChanged
        );
        assert_eq!(
            classify_underlay_transition(true, true, 3, 3, &added, &first, true),
            PolicyUnderlayTransition::IdentityChanged
        );
        assert_eq!(
            classify_underlay_transition(true, true, 3, 4, &first, &first, true),
            PolicyUnderlayTransition::IdentityChanged
        );
        assert_eq!(
            classify_underlay_transition(true, false, 3, 3, &first, &[], true),
            PolicyUnderlayTransition::Lost
        );
        assert_eq!(
            classify_underlay_transition(false, true, 3, 3, &[], &first, true),
            PolicyUnderlayTransition::Recovered
        );
    }

    #[test]
    fn policy_boundary_remains_terminal_for_every_enabled_family() {
        let ipv4 = policy_boundary_routes(7, false);
        assert_eq!(ipv4.len(), 3);
        assert_eq!(
            ipv4.iter()
                .filter(|route| route.header.kind == RouteType::Unreachable)
                .count(),
            1
        );
        let dual_stack = policy_boundary_routes(7, true);
        assert_eq!(dual_stack.len(), 6);
        assert_eq!(
            dual_stack
                .iter()
                .filter(|route| route.header.kind == RouteType::Unreachable)
                .count(),
            2
        );
        assert_eq!(policy_mark_rules(9, false).len(), 1);
        assert_eq!(policy_mark_rules(9, true).len(), 2);
    }
}
