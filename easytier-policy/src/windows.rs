use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUnderlay {
    pub dns_servers: Vec<IpAddr>,
    pub signature: String,
}

/// Capture the explicitly selected physical adapter before Leaf creates its
/// Wintun and changes Windows route priority.
///
/// This follows sing-box `dns/transport/local/resolv_windows.go::dnsReadConfig`:
/// only an operational, gateway-bearing, non-tunnel adapter is eligible.
pub fn windows_underlay(interface: &str) -> Result<WindowsUnderlay, String> {
    let adapters = ipconfig::get_adapters()
        .map_err(|error| format!("failed to enumerate adapters: {error}"))?;
    let adapter = adapters
        .iter()
        .find(|adapter| {
            adapter.friendly_name().eq_ignore_ascii_case(interface)
                || adapter.adapter_name().eq_ignore_ascii_case(interface)
        })
        .ok_or_else(|| format!("Windows outbound interface {interface:?} was not found"))?;
    if adapter.oper_status() != ipconfig::OperStatus::IfOperStatusUp {
        return Err(format!(
            "Windows outbound interface {interface:?} is not operational"
        ));
    }
    if adapter.if_type() == ipconfig::IfType::Tunnel {
        return Err(format!(
            "Windows outbound interface {interface:?} cannot be a tunnel"
        ));
    }
    if adapter.gateways().is_empty() {
        return Err(format!(
            "Windows outbound interface {interface:?} has no gateway"
        ));
    }
    let probe = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("1.1.1.1:53")?;
            socket.local_addr()
        })
        .map_err(|error| format!("failed to resolve the Windows default underlay: {error}"))?;
    if !adapter.ip_addresses().contains(&probe.ip()) {
        return Err(format!(
            "Windows outbound interface {interface:?} is not the active IPv4 default interface"
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
        signature: format!(
            "{}|{}|{}|{}",
            adapter.adapter_name(),
            addresses.join(","),
            gateways.join(","),
            servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        dns_servers: servers,
    })
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
}
