use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::Write as _,
    net::IpAddr,
    os::{
        fd::AsRawFd as _,
        unix::{fs::OpenOptionsExt as _, process::CommandExt as _},
    },
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::Context as _;
use nix::libc;

use super::{PolicyBackendNetwork, PolicyUnderlayTransition};

const ROUTE_PATH: &str = "/sbin/route";
const NETSTAT_PATH: &str = "/usr/sbin/netstat";
const POLICY_DNS_RESOLVER_PATH: &str = "/etc/resolver/easytier-policy";
const POLICY_DNS_TARGET_PREFIX: &str = "/var/run/easytier-policy-dns-";
const POLICY_DNS_LOCK_PATH: &str = "/var/run/easytier-policy-dns.lock";

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

impl RouteFamily {
    fn netstat_name(self) -> &'static str {
        match self {
            Self::Ipv4 => "inet",
            Self::Ipv6 => "inet6",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedDefaultSpec {
    family: RouteFamily,
    gateway: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledScopedDefault {
    spec: ScopedDefaultSpec,
    owned: bool,
}

#[derive(Debug, Default)]
struct DefaultRouteSnapshot {
    physical: Vec<ScopedDefaultSpec>,
    scoped: Vec<ScopedDefaultSpec>,
}

pub(crate) struct PolicyRoutingGuard {
    outbound_interface: String,
    tun_interface: String,
    enable_ipv6: bool,
    installed: Vec<RouteSpec>,
    scoped_defaults: Vec<InstalledScopedDefault>,
    has_v4_underlay: bool,
    dns_hijack: Option<PolicyDnsHijackGuard>,
}

struct PolicyDnsHijackGuard {
    capture_address: IpAddr,
    resolver_path: PathBuf,
    target_path: PathBuf,
    cleanup: std::process::Child,
    _lock: fs::File,
}

impl PolicyDnsHijackGuard {
    fn install(capture_address: IpAddr) -> anyhow::Result<Self> {
        let resolver_path = PathBuf::from(POLICY_DNS_RESOLVER_PATH);
        let target_path =
            PathBuf::from(format!("{POLICY_DNS_TARGET_PREFIX}{}", std::process::id()));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // Opening must not mutate a lock file before ownership is acquired.
            .truncate(false)
            .mode(0o600)
            .open(POLICY_DNS_LOCK_PATH)
            .context("failed to open macOS policy DNS ownership lock")?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("macOS policy DNS is already owned by another process");
        }
        fs::create_dir_all(
            resolver_path
                .parent()
                .expect("policy resolver path has a parent"),
        )
        .context("failed to create /etc/resolver")?;
        remove_stale_owned_resolver(&resolver_path)?;
        let _ = fs::remove_file(&target_path);

        // Start the pipe-owned cleanup before publishing either filesystem
        // entry. SIGKILL closes the pipe and removes any partially installed
        // resolver; a reboot clears the /var/run target and leaves at most an
        // inert dangling symlink that the next guarded start removes.
        let mut cleanup = spawn_policy_dns_cleanup(&resolver_path, &target_path)?;
        let install_result = (|| -> anyhow::Result<()> {
            let mut target = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&target_path)
                .with_context(|| {
                    format!(
                        "failed to create temporary policy resolver {}",
                        target_path.display()
                    )
                })?;
            writeln!(target, "{}", policy_dns_resolver_contents(capture_address))?;
            target.sync_all()?;
            std::os::unix::fs::symlink(&target_path, &resolver_path)
                .context("failed to install temporary macOS policy resolver")
        })();
        if let Err(error) = install_result {
            drop(cleanup.stdin.take());
            let _ = cleanup.wait();
            return Err(error);
        }
        flush_macos_dns_cache();
        Ok(Self {
            capture_address,
            resolver_path,
            target_path,
            cleanup,
            _lock: lock,
        })
    }

    fn update_capture_address(&mut self, capture_address: IpAddr) -> anyhow::Result<()> {
        replace_policy_dns_target(&self.target_path, capture_address)?;
        self.capture_address = capture_address;
        flush_macos_dns_cache();
        Ok(())
    }

    fn remove(&mut self) {
        drop(self.cleanup.stdin.take());
        let _ = self.cleanup.wait();
        if resolver_points_to(&self.resolver_path, &self.target_path) {
            let _ = fs::remove_file(&self.resolver_path);
        }
        let _ = fs::remove_file(&self.target_path);
        flush_macos_dns_cache();
    }
}

fn spawn_policy_dns_cleanup(
    resolver_path: &Path,
    target_path: &Path,
) -> anyhow::Result<std::process::Child> {
    let mut cleanup = Command::new("/bin/sh");
    cleanup
        .arg("-c")
        .arg(
            "IFS= read -r _ || true; if [ \"$(/usr/bin/readlink \"$1\" 2>/dev/null)\" = \"$2\" ]; then rm -f -- \"$1\"; fi; rm -f -- \"$2\"; /usr/bin/killall -HUP mDNSResponder >/dev/null 2>&1 || true",
        )
        .arg("easytier-policy-dns-cleanup")
        .arg(resolver_path)
        .arg(target_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        cleanup.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    cleanup
        .spawn()
        .context("failed to start macOS policy DNS cleanup guard")
}

impl Drop for PolicyDnsHijackGuard {
    fn drop(&mut self) {
        self.remove();
    }
}

fn resolver_points_to(resolver_path: &Path, target_path: &Path) -> bool {
    fs::read_link(resolver_path).is_ok_and(|candidate| candidate == target_path)
}

fn replace_policy_dns_target(target_path: &Path, capture_address: IpAddr) -> anyhow::Result<()> {
    let staging_path = target_path.with_extension("new");
    let _ = fs::remove_file(&staging_path);
    let update_result = (|| -> anyhow::Result<()> {
        let mut staging = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&staging_path)
            .with_context(|| {
                format!(
                    "failed to create staged policy resolver {}",
                    staging_path.display()
                )
            })?;
        writeln!(staging, "{}", policy_dns_resolver_contents(capture_address))?;
        staging.sync_all()?;
        fs::rename(&staging_path, target_path)
            .context("failed to atomically replace macOS policy resolver")
    })();
    if let Err(error) = update_result {
        let _ = fs::remove_file(&staging_path);
        return Err(error);
    }
    Ok(())
}

fn remove_stale_owned_resolver(resolver_path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(resolver_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(resolver_path)?;
            if !target
                .to_string_lossy()
                .starts_with(POLICY_DNS_TARGET_PREFIX)
            {
                anyhow::bail!(
                    "refusing to replace non-EasyTier resolver symlink {} -> {}",
                    resolver_path.display(),
                    target.display()
                );
            }
            fs::remove_file(resolver_path)?;
            let _ = fs::remove_file(target);
            Ok(())
        }
        Ok(_) => anyhow::bail!(
            "refusing to replace existing resolver {}",
            resolver_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn flush_macos_dns_cache() {
    let _ = Command::new("/usr/bin/killall")
        .args(["-HUP", "mDNSResponder"])
        .status();
}

fn policy_dns_resolver_contents(capture_address: IpAddr) -> String {
    // The root-domain resolver has precedence over supplemental VPN DNS such
    // as Tailscale. The selected backend owns the address and its DNS handling;
    // Mihomo hijacks TCP and UDP port 53, while deprecated Leaf retains its
    // existing UDP FakeDNS compatibility path.
    format!("domain .\nnameserver {capture_address}\nsearch_order 1")
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

        // Resolve and scope the physical defaults before the more-specific
        // policy capture routes can affect route lookup.
        let defaults = discover_default_routes(outbound_interface, enable_ipv6)?;
        if !defaults
            .physical
            .iter()
            .any(|route| route.family == RouteFamily::Ipv4)
        {
            anyhow::bail!(
                "policy outbound interface {outbound_interface} has no unscoped IPv4 default route"
            );
        }

        let mut guard = Self {
            outbound_interface: outbound_interface.to_owned(),
            tun_interface: tun_interface.to_owned(),
            enable_ipv6,
            installed: Vec::new(),
            scoped_defaults: Vec::new(),
            has_v4_underlay: false,
            dns_hijack: None,
        };
        if let Err(error) = guard.reconcile_scoped_defaults(&defaults.physical, &defaults.scoped) {
            guard.remove_all();
            return Err(error);
        }
        guard.has_v4_underlay = true;

        for route in desired_routes(enable_ipv6) {
            if let Err(error) = add_capture_route(&guard.tun_interface, &route) {
                guard.remove_all();
                return Err(error);
            }
            guard.installed.push(route);
        }
        Ok(guard)
    }

    pub(crate) fn refresh(&mut self) -> anyhow::Result<PolicyUnderlayTransition> {
        if let Err(error) = validate_interface(&self.outbound_interface) {
            return self.fail_closed_refresh(error);
        }
        if let Err(error) = validate_interface(&self.tun_interface) {
            return self.fail_closed_refresh(error);
        }
        let was_available = self.has_v4_underlay;
        let defaults = match discover_default_routes(&self.outbound_interface, self.enable_ipv6) {
            Ok(routes) => routes,
            Err(error) => return self.fail_closed_refresh(error),
        };
        let is_available = defaults
            .physical
            .iter()
            .any(|route| route.family == RouteFamily::Ipv4);
        if !is_available {
            let routes_changed = !self.scoped_defaults.is_empty();
            self.has_v4_underlay = false;
            self.remove_scoped_defaults()?;
            return Ok(classify_underlay_transition(
                was_available,
                false,
                routes_changed,
            ));
        }

        let routes_changed =
            match self.reconcile_scoped_defaults(&defaults.physical, &defaults.scoped) {
                Ok(changed) => changed,
                Err(error) => return self.fail_closed_refresh(error),
            };
        self.has_v4_underlay = true;
        Ok(classify_underlay_transition(
            was_available,
            true,
            routes_changed,
        ))
    }

    pub(crate) fn has_usable_underlay(&self) -> bool {
        self.has_v4_underlay
    }

    pub(crate) fn activate_backend(
        &mut self,
        network: &PolicyBackendNetwork,
    ) -> anyhow::Result<()> {
        if network.capture_interface.is_some() {
            anyhow::bail!("backend-owned policy TUN is unsupported on macOS");
        }
        if self
            .dns_hijack
            .as_ref()
            .is_some_and(|guard| guard.capture_address == network.dns_capture_address)
        {
            return Ok(());
        }
        if let Some(guard) = self.dns_hijack.as_mut() {
            guard.update_capture_address(network.dns_capture_address)?;
        } else {
            self.dns_hijack = Some(PolicyDnsHijackGuard::install(network.dns_capture_address)?);
        }
        Ok(())
    }

    fn reconcile_scoped_defaults(
        &mut self,
        desired: &[ScopedDefaultSpec],
        observed: &[ScopedDefaultSpec],
    ) -> anyhow::Result<bool> {
        let mut changed = false;
        changed |= self.reconcile_scoped_default(
            RouteFamily::Ipv4,
            desired
                .iter()
                .find(|route| route.family == RouteFamily::Ipv4)
                .cloned(),
            observed
                .iter()
                .find(|route| route.family == RouteFamily::Ipv4)
                .cloned(),
        )?;
        changed |= self.reconcile_scoped_default(
            RouteFamily::Ipv6,
            desired
                .iter()
                .find(|route| route.family == RouteFamily::Ipv6)
                .cloned(),
            observed
                .iter()
                .find(|route| route.family == RouteFamily::Ipv6)
                .cloned(),
        )?;
        Ok(changed)
    }

    fn reconcile_scoped_default(
        &mut self,
        family: RouteFamily,
        desired: Option<ScopedDefaultSpec>,
        mut active: Option<ScopedDefaultSpec>,
    ) -> anyhow::Result<bool> {
        let current = self
            .scoped_defaults
            .iter()
            .position(|route| route.spec.family == family)
            .map(|position| self.scoped_defaults.remove(position));

        let had_current = current.is_some();
        if let Some(current) = current {
            if let Some(desired) = &desired
                && current.spec == *desired
                && active.as_ref().is_some_and(|route| {
                    gateways_equal(&route.gateway, &desired.gateway, &self.outbound_interface)
                })
            {
                self.scoped_defaults.push(current);
                return Ok(false);
            }
            let owns_active = current.owned
                && active.as_ref().is_some_and(|route| {
                    gateways_equal(
                        &route.gateway,
                        &current.spec.gateway,
                        &self.outbound_interface,
                    )
                });
            if owns_active {
                if let Err(error) = delete_scoped_default(&self.outbound_interface, &current.spec) {
                    self.scoped_defaults.push(current);
                    return Err(error);
                }
                active = None;
            }
        }

        if let Some(desired) = desired {
            let installed =
                ensure_scoped_default(&self.outbound_interface, desired, active.as_ref())?;
            self.scoped_defaults.push(installed);
            return Ok(true);
        }

        Ok(had_current)
    }

    fn fail_closed_refresh<T>(&mut self, error: anyhow::Error) -> anyhow::Result<T> {
        self.has_v4_underlay = false;
        self.remove_scoped_defaults()
            .context("failed to remove scoped defaults while marking macOS underlay unavailable")?;
        Err(error)
    }

    fn remove_scoped_defaults(&mut self) -> anyhow::Result<()> {
        let mut first_error = None;
        let mut retained = Vec::new();
        for route in std::mem::take(&mut self.scoped_defaults).into_iter().rev() {
            if !route.owned {
                continue;
            }
            let still_active = query_scoped_default(&self.outbound_interface, route.spec.family)
                .map(|active| {
                    active.is_some_and(|active| {
                        gateways_equal(
                            &active.gateway,
                            &route.spec.gateway,
                            &self.outbound_interface,
                        )
                    })
                })
                .unwrap_or(true);
            if still_active
                && let Err(error) = delete_scoped_default(&self.outbound_interface, &route.spec)
            {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                retained.push(route);
            }
        }
        retained.reverse();
        self.scoped_defaults = retained;
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn remove_all(&mut self) {
        self.dns_hijack.take();
        self.has_v4_underlay = false;
        for route in self.installed.drain(..).rev() {
            if let Err(error) = delete_capture_route(&self.tun_interface, &route) {
                tracing::warn!(
                    ?error,
                    interface = self.tun_interface,
                    destination = route.destination,
                    "failed to remove macOS policy capture route"
                );
            }
        }
        if let Err(error) = self.remove_scoped_defaults() {
            tracing::warn!(
                ?error,
                interface = self.outbound_interface,
                "failed to remove macOS policy scoped default"
            );
        }
    }
}

impl Drop for PolicyRoutingGuard {
    fn drop(&mut self) {
        self.remove_all();
    }
}

fn classify_underlay_transition(
    was_available: bool,
    is_available: bool,
    routes_changed: bool,
) -> PolicyUnderlayTransition {
    if was_available && !is_available {
        PolicyUnderlayTransition::Lost
    } else if !was_available && is_available {
        PolicyUnderlayTransition::Recovered
    } else if routes_changed {
        PolicyUnderlayTransition::RoutesChanged
    } else {
        PolicyUnderlayTransition::Unchanged
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

fn discover_default_routes(
    interface: &str,
    enable_ipv6: bool,
) -> anyhow::Result<DefaultRouteSnapshot> {
    let mut defaults = DefaultRouteSnapshot::default();
    discover_default_routes_for_family(interface, RouteFamily::Ipv4, &mut defaults)?;
    if enable_ipv6 {
        discover_default_routes_for_family(interface, RouteFamily::Ipv6, &mut defaults)?;
    }
    Ok(defaults)
}

fn discover_default_routes_for_family(
    interface: &str,
    family: RouteFamily,
    defaults: &mut DefaultRouteSnapshot,
) -> anyhow::Result<()> {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(NETSTAT_PATH)
        .args(["-rn", "-f", family.netstat_name()])
        .output()?;
    if !status.success() {
        anyhow::bail!(
            "netstat {} failed: {}{}",
            family.netstat_name(),
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
    }
    let routes = String::from_utf8_lossy(&stdout);
    if let Some(route) = parse_default_route(&routes, interface, family, false)? {
        defaults.physical.push(route);
    }
    if let Some(route) = parse_default_route(&routes, interface, family, true)? {
        defaults.scoped.push(route);
    }
    Ok(())
}

fn parse_physical_default(
    routes: &str,
    interface: &str,
    family: RouteFamily,
) -> anyhow::Result<Option<ScopedDefaultSpec>> {
    parse_default_route(routes, interface, family, false)
}

fn parse_default_route(
    routes: &str,
    interface: &str,
    family: RouteFamily,
    scoped: bool,
) -> anyhow::Result<Option<ScopedDefaultSpec>> {
    let mut gateways = routes.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let destination = fields.next()?;
        let gateway = fields.next()?;
        let flags = fields.next()?;
        let route_interface = fields.next()?;
        if destination != "default"
            || route_interface != interface
            || !flags.contains('U')
            || !flags.contains('G')
            || flags.contains('I') != scoped
            || gateway.starts_with("link#")
        {
            return None;
        }
        Some(gateway.to_owned())
    });
    let Some(gateway) = gateways.next() else {
        return Ok(None);
    };
    if gateways.any(|candidate| !gateways_equal(&gateway, &candidate, interface)) {
        anyhow::bail!(
            "policy outbound interface {interface} has multiple {}scoped {} default gateways",
            if scoped { "" } else { "un" },
            family.netstat_name(),
        );
    }
    Ok(Some(ScopedDefaultSpec { family, gateway }))
}

fn ensure_scoped_default(
    interface: &str,
    desired: ScopedDefaultSpec,
    existing: Option<&ScopedDefaultSpec>,
) -> anyhow::Result<InstalledScopedDefault> {
    if !scoped_default_requires_install(interface, existing, &desired)? {
        return Ok(InstalledScopedDefault {
            spec: desired,
            owned: false,
        });
    }

    run_route_command(scoped_default_args("add", interface, &desired))?;
    match scoped_default_matches(interface, &desired) {
        Ok(true) => Ok(InstalledScopedDefault {
            spec: desired,
            owned: true,
        }),
        result => {
            let cleanup = run_route_command(scoped_default_args("delete", interface, &desired));
            match (result, cleanup) {
                (Err(error), Ok(())) => Err(error),
                (Err(error), Err(cleanup)) => Err(error.context(format!(
                    "also failed to remove unverified scoped default: {cleanup:#}"
                ))),
                (Ok(false), Ok(())) => anyhow::bail!(
                    "scoped {} default did not become active on {interface}",
                    desired.family.netstat_name()
                ),
                (Ok(false), Err(cleanup)) => anyhow::bail!(
                    "scoped {} default did not become active on {interface} and cleanup failed: \
                     {cleanup:#}",
                    desired.family.netstat_name()
                ),
                (Ok(true), _) => unreachable!(),
            }
        }
    }
}

fn scoped_default_requires_install(
    interface: &str,
    existing: Option<&ScopedDefaultSpec>,
    desired: &ScopedDefaultSpec,
) -> anyhow::Result<bool> {
    let Some(existing) = existing else {
        return Ok(true);
    };
    if gateways_equal(&existing.gateway, &desired.gateway, interface) {
        return Ok(false);
    }
    anyhow::bail!(
        "policy outbound interface {interface} already has a conflicting scoped {} default via {}",
        desired.family.netstat_name(),
        existing.gateway
    )
}

fn scoped_default_matches(interface: &str, expected: &ScopedDefaultSpec) -> anyhow::Result<bool> {
    Ok(query_scoped_default(interface, expected.family)?
        .is_some_and(|existing| gateways_equal(&existing.gateway, &expected.gateway, interface)))
}

fn query_scoped_default(
    interface: &str,
    family: RouteFamily,
) -> anyhow::Result<Option<ScopedDefaultSpec>> {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(NETSTAT_PATH)
        .args(["-rn", "-f", family.netstat_name()])
        .output()?;
    if !status.success() {
        anyhow::bail!(
            "netstat {} failed: {}{}",
            family.netstat_name(),
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
    }
    parse_default_route(&String::from_utf8_lossy(&stdout), interface, family, true)
}

fn gateways_equal(left: &str, right: &str, interface: &str) -> bool {
    fn without_scope<'a>(gateway: &'a str, interface: &str) -> &'a str {
        gateway
            .strip_suffix(&format!("%{interface}"))
            .unwrap_or(gateway)
    }
    without_scope(left, interface).eq_ignore_ascii_case(without_scope(right, interface))
}

fn add_capture_route(interface: &str, route: &RouteSpec) -> anyhow::Result<()> {
    run_route_command(capture_route_args("add", interface, route))
}

fn delete_capture_route(interface: &str, route: &RouteSpec) -> anyhow::Result<()> {
    run_route_command(capture_route_args("delete", interface, route))
}

fn delete_scoped_default(interface: &str, route: &ScopedDefaultSpec) -> anyhow::Result<()> {
    run_route_command(scoped_default_args("delete", interface, route))
}

fn capture_route_args(action: &'static str, interface: &str, route: &RouteSpec) -> Vec<String> {
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

fn scoped_default_args(
    action: &'static str,
    interface: &str,
    route: &ScopedDefaultSpec,
) -> Vec<String> {
    let mut args = vec!["-n".to_owned(), action.to_owned()];
    if route.family == RouteFamily::Ipv6 {
        args.push("-inet6".to_owned());
    }
    args.extend([
        "-ifscope".to_owned(),
        interface.to_owned(),
        "default".to_owned(),
        route.gateway.clone(),
    ]);
    args
}

fn run_route_command(args: Vec<String>) -> anyhow::Result<()> {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(ROUTE_PATH).args(&args).output()?;
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

    const ROUTES: &str = "\
Routing tables

Internet:
Destination        Gateway            Flags               Netif Expire
default            192.168.234.1      UGScg                 en0
default            192.168.234.1      UGScIg                en0
default            link#23            UCSIg               utun4
0/1                utun5              USc                 utun5
128.0/1            utun5              USc                 utun5
";

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
    fn physical_default_parser_ignores_scoped_tunnels_and_capture_subranges() {
        assert_eq!(
            parse_physical_default(ROUTES, "en0", RouteFamily::Ipv4).unwrap(),
            Some(ScopedDefaultSpec {
                family: RouteFamily::Ipv4,
                gateway: "192.168.234.1".to_owned(),
            })
        );
        assert_eq!(
            parse_physical_default(ROUTES, "en7", RouteFamily::Ipv4).unwrap(),
            None
        );
        let ambiguous =
            format!("{ROUTES}default            192.168.234.254    UGScg                 en0\n");
        assert!(parse_physical_default(&ambiguous, "en0", RouteFamily::Ipv4).is_err());
    }

    #[test]
    fn scoped_default_parser_requires_matching_interface_and_scope_flag() {
        assert_eq!(
            parse_default_route(ROUTES, "en0", RouteFamily::Ipv4, true).unwrap(),
            Some(ScopedDefaultSpec {
                family: RouteFamily::Ipv4,
                gateway: "192.168.234.1".to_owned(),
            })
        );
        assert_eq!(
            parse_default_route(ROUTES, "en7", RouteFamily::Ipv4, true).unwrap(),
            None
        );
    }

    #[test]
    fn gateway_comparison_normalizes_darwin_ipv6_scope_suffix() {
        assert!(gateways_equal("fe80::1%en0", "FE80::1", "en0"));
        assert!(!gateways_equal("fe80::1%en0", "fe80::2%en0", "en0"));
    }

    #[test]
    fn preexisting_matching_scoped_default_is_reused_not_owned() {
        let desired = ScopedDefaultSpec {
            family: RouteFamily::Ipv4,
            gateway: "192.0.2.1".to_owned(),
        };
        assert!(!scoped_default_requires_install("en0", Some(&desired), &desired).unwrap());
        assert!(scoped_default_requires_install("en0", None, &desired).unwrap());

        let conflicting = ScopedDefaultSpec {
            family: RouteFamily::Ipv4,
            gateway: "192.0.2.2".to_owned(),
        };
        assert!(scoped_default_requires_install("en0", Some(&conflicting), &desired).is_err());
    }

    #[test]
    fn route_arguments_do_not_use_a_shell() {
        let capture = &desired_routes(false)[0];
        assert_eq!(
            capture_route_args("add", "utun7", capture),
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
        let scoped = ScopedDefaultSpec {
            family: RouteFamily::Ipv4,
            gateway: "192.0.2.1".to_owned(),
        };
        assert_eq!(
            scoped_default_args("add", "en0", &scoped),
            ["-n", "add", "-ifscope", "en0", "default", "192.0.2.1"]
        );
        let scoped_v6 = ScopedDefaultSpec {
            family: RouteFamily::Ipv6,
            gateway: "fe80::1%en0".to_owned(),
        };
        assert_eq!(
            scoped_default_args("delete", "en0", &scoped_v6),
            [
                "-n",
                "delete",
                "-inet6",
                "-ifscope",
                "en0",
                "default",
                "fe80::1%en0",
            ]
        );
    }

    #[test]
    fn underlay_transition_classification_tracks_loss_and_recovery() {
        assert_eq!(
            classify_underlay_transition(true, true, false),
            PolicyUnderlayTransition::Unchanged
        );
        assert_eq!(
            classify_underlay_transition(true, true, true),
            PolicyUnderlayTransition::RoutesChanged
        );
        assert_eq!(
            classify_underlay_transition(true, false, true),
            PolicyUnderlayTransition::Lost
        );
        assert_eq!(
            classify_underlay_transition(false, true, true),
            PolicyUnderlayTransition::Recovered
        );
        assert_eq!(
            classify_underlay_transition(false, false, false),
            PolicyUnderlayTransition::Unchanged
        );
    }

    #[test]
    fn policy_dns_resolver_uses_the_backend_declared_capture_address() {
        let capture_address = "198.18.0.1".parse().unwrap();
        assert_eq!(
            policy_dns_resolver_contents(capture_address),
            "domain .\nnameserver 198.18.0.1\nsearch_order 1"
        );
        assert!(!policy_dns_resolver_contents(capture_address).contains("system"));
    }

    #[test]
    fn stale_cleanup_only_removes_easytier_owned_resolver_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let resolver = directory.path().join("resolver");
        let owned_target = PathBuf::from(format!("{POLICY_DNS_TARGET_PREFIX}test-nonexistent"));
        std::os::unix::fs::symlink(&owned_target, &resolver).unwrap();
        remove_stale_owned_resolver(&resolver).unwrap();
        assert!(!resolver.exists());

        let foreign = directory.path().join("foreign");
        std::os::unix::fs::symlink("/var/run/foreign-dns", &foreign).unwrap();
        assert!(remove_stale_owned_resolver(&foreign).is_err());
        assert!(fs::symlink_metadata(&foreign).is_ok());
    }

    #[test]
    fn policy_dns_capture_address_replacement_is_atomic_for_the_published_path() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("resolver-target");
        fs::write(
            &target,
            policy_dns_resolver_contents("198.18.0.1".parse().unwrap()),
        )
        .unwrap();

        replace_policy_dns_target(&target, "198.19.0.1".parse().unwrap()).unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "domain .\nnameserver 198.19.0.1\nsearch_order 1\n"
        );
        assert!(!target.with_extension("new").exists());
    }
}
