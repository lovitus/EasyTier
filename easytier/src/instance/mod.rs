pub mod dns_server;
#[allow(clippy::module_inception)]
pub mod instance;

pub mod listeners;

mod public_ipv6_provider;

pub mod proxy_cidrs_monitor;

#[cfg(feature = "tun")]
pub mod virtual_nic;

#[cfg(all(target_os = "linux", feature = "tun"))]
mod linux_veth;

#[cfg(any(windows, test))]
pub(crate) mod windows_udp_broadcast;
