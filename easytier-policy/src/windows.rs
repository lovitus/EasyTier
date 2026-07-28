use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const AUTOMATIC_WINDOWS_UNDERLAY: &str = "auto";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUnderlay {
    pub interface_name: String,
    pub dns_servers: Vec<IpAddr>,
    pub signature: String,
    pub automatic_environment_signature: Option<String>,
}

/// Capture the explicitly selected physical adapter before Leaf creates its
/// Wintun and changes Windows route priority.
///
/// This follows sing-box `dns/transport/local/resolv_windows.go::dnsReadConfig`:
/// only an operational, gateway-bearing, non-tunnel adapter is eligible.
pub fn windows_underlay(interface: &str) -> Result<WindowsUnderlay, String> {
    let adapters = ipconfig::get_adapters()
        .map_err(|error| format!("failed to enumerate adapters: {error}"))?;
    if interface.eq_ignore_ascii_case(AUTOMATIC_WINDOWS_UNDERLAY) {
        return select_automatic_underlay(&adapters);
    }
    let adapter = find_adapter(&adapters, interface)
        .ok_or_else(|| format!("Windows outbound interface {interface:?} was not found"))?;
    build_underlay(adapter)
}

/// Verify that Windows still routes both halves of each enabled address family
/// into Leaf's Wintun. This detects another VPN taking capture precedence
/// without changing or deleting that VPN's routes.
pub fn windows_interface_owns_default_routes(
    interface: &str,
    include_ipv6: bool,
) -> Result<bool, String> {
    let adapters = ipconfig::get_adapters()
        .map_err(|error| format!("failed to enumerate adapters: {error}"))?;
    let adapter = find_adapter(&adapters, interface)
        .ok_or_else(|| format!("Windows capture interface {interface:?} was not found"))?;
    let expected_index = adapter_interface_row(adapter)?.InterfaceIndex;
    let routes = windows_capture_routes()?;
    Ok(capture_routes_owned_by(
        &routes,
        expected_index,
        include_ipv6,
    ))
}

/// Snapshot every currently eligible physical underlay. Automatic mode uses
/// this to notice a newly connected Ethernet/USB adapter even while the prior
/// WLAN remains operational.
pub fn windows_underlay_environment_signature() -> Result<String, String> {
    let adapters = ipconfig::get_adapters()
        .map_err(|error| format!("failed to enumerate adapters: {error}"))?;
    let candidates = adapters
        .iter()
        .filter_map(|adapter| build_underlay(adapter).ok())
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err("Windows automatic underlay found no active physical interface".to_owned());
    }
    Ok(candidate_environment_signature(&candidates))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsCaptureRoute {
    prefix: IpAddr,
    prefix_len: u8,
    interface_index: u32,
}

fn required_capture_prefixes(include_ipv6: bool) -> Vec<(IpAddr, u8)> {
    let mut prefixes = vec![
        (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1),
        (IpAddr::V4(Ipv4Addr::new(128, 0, 0, 0)), 1),
    ];
    if include_ipv6 {
        prefixes.extend([
            (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 1),
            (IpAddr::V6(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0)), 1),
        ]);
    }
    prefixes
}

fn capture_routes_owned_by(
    routes: &[WindowsCaptureRoute],
    expected_index: u32,
    include_ipv6: bool,
) -> bool {
    required_capture_prefixes(include_ipv6)
        .into_iter()
        .all(|(prefix, prefix_len)| {
            let mut matching = routes
                .iter()
                .filter(|route| route.prefix == prefix && route.prefix_len == prefix_len);
            matching
                .next()
                .is_some_and(|route| route.interface_index == expected_index)
                && matching.all(|route| route.interface_index == expected_index)
        })
}

fn windows_capture_routes() -> Result<Vec<WindowsCaptureRoute>, String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_TABLE2},
        Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_INET},
    };

    struct RouteTable(*mut MIB_IPFORWARD_TABLE2);

    impl Drop for RouteTable {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { FreeMibTable(self.0.cast()) };
            }
        }
    }

    fn prefix_ip(address: &SOCKADDR_INET) -> Option<IpAddr> {
        match unsafe { address.si_family } {
            AF_INET => {
                let bytes = unsafe { address.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes();
                Some(IpAddr::V4(Ipv4Addr::from(bytes)))
            }
            AF_INET6 => {
                let bytes = unsafe { address.Ipv6.sin6_addr.u.Byte };
                Some(IpAddr::V6(Ipv6Addr::from(bytes)))
            }
            _ => None,
        }
    }

    let mut table = std::ptr::null_mut();
    let result = unsafe { GetIpForwardTable2(AF_UNSPEC, &mut table) };
    if result != 0 {
        return Err(format!(
            "failed to enumerate the Windows route table: error {result}"
        ));
    }
    let table = RouteTable(table);
    if table.0.is_null() {
        return Err("Windows returned an empty route-table pointer".to_owned());
    }
    let rows = unsafe {
        std::slice::from_raw_parts((*table.0).Table.as_ptr(), (*table.0).NumEntries as usize)
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(WindowsCaptureRoute {
                prefix: prefix_ip(&row.DestinationPrefix.Prefix)?,
                prefix_len: row.DestinationPrefix.PrefixLength,
                interface_index: row.InterfaceIndex,
            })
        })
        .collect())
}

#[cfg(test)]
mod capture_route_tests {
    use super::*;

    fn route(prefix: &str, prefix_len: u8, interface_index: u32) -> WindowsCaptureRoute {
        WindowsCaptureRoute {
            prefix: prefix.parse().unwrap(),
            prefix_len,
            interface_index,
        }
    }

    #[test]
    fn exact_capture_routes_must_all_belong_to_expected_interface() {
        let routes = [
            route("0.0.0.0", 1, 7),
            route("128.0.0.0", 1, 7),
            route("::", 1, 7),
            route("8000::", 1, 7),
            route("1.1.1.1", 32, 3),
        ];
        assert!(capture_routes_owned_by(&routes, 7, true));
        assert!(capture_routes_owned_by(&routes, 7, false));
    }

    #[test]
    fn missing_or_competing_capture_route_fails_closed() {
        let missing = [route("0.0.0.0", 1, 7)];
        assert!(!capture_routes_owned_by(&missing, 7, false));

        let competing = [
            route("0.0.0.0", 1, 7),
            route("0.0.0.0", 1, 9),
            route("128.0.0.0", 1, 7),
        ];
        assert!(!capture_routes_owned_by(&competing, 7, false));
    }
}

fn find_adapter<'a>(
    adapters: &'a [ipconfig::Adapter],
    interface: &str,
) -> Option<&'a ipconfig::Adapter> {
    adapters.iter().find(|adapter| {
        adapter.friendly_name().eq_ignore_ascii_case(interface)
            || adapter.adapter_name().eq_ignore_ascii_case(interface)
    })
}

fn select_automatic_underlay(adapters: &[ipconfig::Adapter]) -> Result<WindowsUnderlay, String> {
    let mut candidates = adapters
        .iter()
        .filter_map(|adapter| build_underlay(adapter).ok())
        .collect::<Vec<_>>();
    let environment_signature = candidate_environment_signature(&candidates);
    let preferred_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("1.1.1.1:53")?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip());
    if let Some(preferred_ip) = preferred_ip
        && let Some(index) = candidates.iter().position(|candidate| {
            find_adapter(adapters, &candidate.interface_name)
                .is_some_and(|adapter| adapter.ip_addresses().contains(&preferred_ip))
        })
    {
        let mut selected = candidates.swap_remove(index);
        selected.automatic_environment_signature = Some(environment_signature);
        return Ok(selected);
    }
    match candidates.len() {
        0 => Err("Windows automatic underlay found no active physical interface".to_owned()),
        1 => {
            let mut selected = candidates.remove(0);
            selected.automatic_environment_signature = Some(environment_signature);
            Ok(selected)
        }
        _ => Err(
            "Windows automatic underlay is ambiguous while another VPN owns the default route; select a physical interface explicitly"
                .to_owned(),
        ),
    }
}

fn candidate_environment_signature(candidates: &[WindowsUnderlay]) -> String {
    let mut signatures = candidates
        .iter()
        .map(|candidate| candidate.signature.as_str())
        .collect::<Vec<_>>();
    signatures.sort_unstable();
    signatures.join(";")
}

fn build_underlay(adapter: &ipconfig::Adapter) -> Result<WindowsUnderlay, String> {
    let interface = adapter.friendly_name();
    if adapter.oper_status() != ipconfig::OperStatus::IfOperStatusUp {
        return Err(format!(
            "Windows outbound interface {interface:?} is not operational"
        ));
    }
    let interface_row = adapter_interface_row(adapter)?;
    if adapter.if_type() == ipconfig::IfType::Tunnel
        || interface_row.InterfaceAndOperStatusFlags._bitfield & 0x01 == 0
    {
        return Err(format!(
            "Windows outbound interface {interface:?} is not a physical adapter"
        ));
    }
    if adapter.gateways().is_empty() {
        return Err(format!(
            "Windows outbound interface {interface:?} has no gateway"
        ));
    }

    let mut servers = Vec::new();
    for address in adapter.dns_servers().iter().copied() {
        if usable_dns_server(address) && !servers.contains(&address) {
            servers.push(address);
        }
        if servers.len() == 4 {
            break;
        }
    }
    if servers.is_empty() {
        return Err(format!(
            "Windows outbound interface {interface:?} has no directly usable DNS server"
        ));
    }
    let mut addresses = adapter
        .ip_addresses()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    addresses.sort();
    let mut gateways = adapter
        .gateways()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    gateways.sort();
    Ok(WindowsUnderlay {
        interface_name: adapter.friendly_name().to_owned(),
        signature: format!(
            "{}|{}|{}|{}|{}",
            adapter.adapter_name(),
            interface_row.InterfaceIndex,
            addresses.join(","),
            gateways.join(","),
            servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        dns_servers: servers,
        automatic_environment_signature: None,
    })
}

fn adapter_interface_row(
    adapter: &ipconfig::Adapter,
) -> Result<windows_sys::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceAliasToLuid, GetIfEntry2, MIB_IF_ROW2,
    };

    let alias = adapter
        .friendly_name()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut row = unsafe { std::mem::zeroed::<MIB_IF_ROW2>() };
    let result = unsafe { ConvertInterfaceAliasToLuid(alias.as_ptr(), &mut row.InterfaceLuid) };
    if result != 0 {
        return Err(format!(
            "failed to resolve Windows interface {:?}: error {result}",
            adapter.friendly_name()
        ));
    }
    let result = unsafe { GetIfEntry2(&mut row) };
    if result != 0 {
        return Err(format!(
            "failed to inspect Windows interface {:?}: error {result}",
            adapter.friendly_name()
        ));
    }
    Ok(row)
}

fn usable_dns_server(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_unspecified() && !address.is_loopback() && !address.is_multicast()
        }
        IpAddr::V6(address) => {
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dns_addresses_that_would_reenter_the_local_stack() {
        assert!(!usable_dns_server("127.0.0.1".parse().unwrap()));
        assert!(!usable_dns_server("::1".parse().unwrap()));
        assert!(!usable_dns_server("fe80::1".parse().unwrap()));
        assert!(usable_dns_server("1.1.1.1".parse().unwrap()));
        assert!(usable_dns_server("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn automatic_environment_signature_is_order_independent() {
        let candidate = |signature: &str| WindowsUnderlay {
            interface_name: signature.to_owned(),
            dns_servers: Vec::new(),
            signature: signature.to_owned(),
            automatic_environment_signature: None,
        };
        assert_eq!(
            candidate_environment_signature(&[candidate("wlan"), candidate("ethernet")]),
            candidate_environment_signature(&[candidate("ethernet"), candidate("wlan")]),
        );
    }
}
