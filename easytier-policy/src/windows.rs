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

    let mut destinations = vec![
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(200, 1, 1, 1)),
    ];
    if include_ipv6 {
        destinations.extend([
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888)),
            IpAddr::V6(Ipv6Addr::new(0xc001, 0, 0, 0, 0, 0, 0, 1)),
        ]);
    }

    for destination in destinations {
        if best_interface_for(destination)? != expected_index {
            return Ok(false);
        }
    }
    Ok(true)
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

fn best_interface_for(destination: IpAddr) -> Result<u32, String> {
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::GetBestInterfaceEx,
        Networking::WinSock::{
            AF_INET, AF_INET6, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, SOCKADDR, SOCKADDR_IN,
            SOCKADDR_IN6,
        },
    };

    let mut index = 0;
    let result = match destination {
        IpAddr::V4(address) => {
            let socket_address = SOCKADDR_IN {
                sin_family: AF_INET,
                sin_port: 0,
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from_ne_bytes(address.octets()),
                    },
                },
                sin_zero: [0; 8],
            };
            unsafe {
                GetBestInterfaceEx(
                    std::ptr::from_ref(&socket_address).cast::<SOCKADDR>(),
                    &mut index,
                )
            }
        }
        IpAddr::V6(address) => {
            let socket_address = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_port: 0,
                sin6_flowinfo: 0,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 {
                        Byte: address.octets(),
                    },
                },
                Anonymous: unsafe { std::mem::zeroed() },
            };
            unsafe {
                GetBestInterfaceEx(
                    std::ptr::from_ref(&socket_address).cast::<SOCKADDR>(),
                    &mut index,
                )
            }
        }
    };
    if result == 0 {
        Ok(index)
    } else {
        Err(format!(
            "failed to resolve the Windows route for {destination}: error {result}"
        ))
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
