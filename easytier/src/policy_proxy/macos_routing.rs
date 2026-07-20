use std::{
    ffi::CString,
    process::{Command, Output},
};

use nix::libc;

use super::PolicyUnderlayTransition;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteSpec {
    family: RouteFamily,
    destination: &'static str,
    prefix_or_mask: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteFamily {
    Ipv4,
    Ipv6,
}

pub(crate) struct PolicyRoutingGuard {
    outbound_interface: String,
    tun_interface: String,
    installed: Vec<RouteSpec>,
}

impl PolicyRoutingGuard {
    pub(crate) fn install(
        outbound_interface: &str,
        tun_interface: &str,
        enable_ipv6: bool,
        _socket_mark: Option<u32>,
    ) -> anyhow::Result<Self> {
        validate_interface(outbound_interface)?;
        validate_interface(tun_interface)?;
        if outbound_interface == tun_interface {
            anyhow::bail!("policy outbound interface cannot be the EasyTier virtual NIC");
        }

        let mut guard = Self {
            outbound_interface: outbound_interface.to_owned(),
            tun_interface: tun_interface.to_owned(),
            installed: Vec::new(),
        };
        for route in desired_routes(enable_ipv6) {
            if let Err(error) = add_route(&guard.tun_interface, &route) {
                guard.remove_all();
                return Err(error);
            }
            guard.installed.push(route);
        }
        Ok(guard)
    }

    pub(crate) fn refresh(&mut self) -> anyhow::Result<PolicyUnderlayTransition> {
        validate_interface(&self.outbound_interface)?;
        validate_interface(&self.tun_interface)?;
        Ok(PolicyUnderlayTransition::Unchanged)
    }

    pub(crate) fn has_usable_underlay(&self) -> bool {
        true
    }

    fn remove_all(&mut self) {
        for route in self.installed.drain(..).rev() {
            if let Err(error) = delete_route(&self.tun_interface, &route) {
                tracing::warn!(
                    ?error,
                    interface = self.tun_interface,
                    destination = route.destination,
                    "failed to remove macOS policy capture route"
                );
            }
        }
    }
}

impl Drop for PolicyRoutingGuard {
    fn drop(&mut self) {
        self.remove_all();
    }
}

fn desired_routes(enable_ipv6: bool) -> Vec<RouteSpec> {
    let mut routes = vec![
        RouteSpec {
            family: RouteFamily::Ipv4,
            destination: "0.0.0.0",
            prefix_or_mask: "128.0.0.0",
        },
        RouteSpec {
            family: RouteFamily::Ipv4,
            destination: "128.0.0.0",
            prefix_or_mask: "128.0.0.0",
        },
    ];
    if enable_ipv6 {
        routes.extend([
            RouteSpec {
                family: RouteFamily::Ipv6,
                destination: "::",
                prefix_or_mask: "1",
            },
            RouteSpec {
                family: RouteFamily::Ipv6,
                destination: "8000::",
                prefix_or_mask: "1",
            },
        ]);
    }
    routes
}

fn validate_interface(interface: &str) -> anyhow::Result<()> {
    if interface.is_empty()
        || interface.len() > libc::IFNAMSIZ.saturating_sub(1)
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        anyhow::bail!("invalid policy interface name: {interface:?}");
    }
    let interface = CString::new(interface)?;
    if unsafe { libc::if_nametoindex(interface.as_ptr()) } == 0 {
        anyhow::bail!(
            "policy interface does not exist: {}",
            interface.to_string_lossy()
        );
    }
    Ok(())
}

fn add_route(interface: &str, route: &RouteSpec) -> anyhow::Result<()> {
    run_route_command(route_args("add", interface, route))
}

fn delete_route(interface: &str, route: &RouteSpec) -> anyhow::Result<()> {
    run_route_command(route_args("delete", interface, route))
}

fn route_args(action: &'static str, interface: &str, route: &RouteSpec) -> Vec<String> {
    let mut args = match route.family {
        RouteFamily::Ipv4 => vec![
            "-n".to_owned(),
            action.to_owned(),
            route.destination.to_owned(),
            "-netmask".to_owned(),
            route.prefix_or_mask.to_owned(),
            "-interface".to_owned(),
            interface.to_owned(),
        ],
        RouteFamily::Ipv6 => vec![
            "-n".to_owned(),
            action.to_owned(),
            "-inet6".to_owned(),
            format!("{}/{}", route.destination, route.prefix_or_mask),
            "-interface".to_owned(),
            interface.to_owned(),
        ],
    };
    if action == "add" {
        args.extend(["-hopcount".to_owned(), "65535".to_owned()]);
    }
    args
}

fn run_route_command(args: Vec<String>) -> anyhow::Result<()> {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new("/sbin/route").args(&args).output()?;
    if status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "route {} failed: {}{}",
        args.join(" "),
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_default_routes_are_deterministic_and_dual_stack_is_optional() {
        let ipv4 = desired_routes(false);
        assert_eq!(ipv4.len(), 2);
        assert!(ipv4.iter().all(|route| route.family == RouteFamily::Ipv4));

        let dual_stack = desired_routes(true);
        assert_eq!(dual_stack.len(), 4);
        assert_eq!(dual_stack[2].destination, "::");
        assert_eq!(dual_stack[3].destination, "8000::");
    }

    #[test]
    fn route_arguments_do_not_use_a_shell() {
        let route = &desired_routes(false)[0];
        assert_eq!(
            route_args("add", "utun7", route),
            [
                "-n",
                "add",
                "0.0.0.0",
                "-netmask",
                "128.0.0.0",
                "-interface",
                "utun7",
                "-hopcount",
                "65535",
            ]
        );
        assert_eq!(
            route_args("delete", "utun7", route),
            [
                "-n",
                "delete",
                "0.0.0.0",
                "-netmask",
                "128.0.0.0",
                "-interface",
                "utun7",
            ]
        );
    }
}
