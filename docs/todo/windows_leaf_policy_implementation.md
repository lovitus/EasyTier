# Windows Leaf policy implementation

Status: implementation candidate

## Reference contract

- Failure-baseline Leaf source: `https://github.com/lovitus/leaf.git` at
  `013a1497dd29355a00cd776628ff2de72e02e861` (the exact revision used by the
  prior artifact). The combined Windows/macOS candidate locks
  `43515219f84df0bf5a9ed9e49bb60fdb4018ac06`.
  - `leaf/src/proxy/tun/inbound.rs::new` captures the physical IPv4 address before
    creating Wintun, binds direct outbound traffic to that address, configures
    Wintun with metric `0`, and applies the configured DNS servers.
  - `leaf/src/lib.rs::start`, `shutdown`, and `is_running` provide independently
    keyed, bounded in-process runtime ownership.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev` at
  `0a87b94845ef908c15f8495871e4cd8e33116328`.
  - `listener/sing_tun/server_windows.go::tunNew` owns Wintun creation and returns
    startup failures instead of silently starting without policy routing.
  - `listener/sing_tun/server.go::New` closes partial TUN, stack, route, and monitor
    state on startup failure; `Server.Close` closes all owned state together.
  - `component/dialer/bind_windows.go::{bind4,bind6,bindIfaceToDialer}` applies
    `IP_UNICAST_IF`/`IPV6_UNICAST_IF` before connection so another TUN cannot
    recapture the proxy's own underlay sockets.
- sing-box reference: `/Users/fanli/Documents/singbox-withfallback` at
  `a9cd6f89d919a55353ec2170bf88add0d87882f1`.
  - `route/network.go::{NewNetworkManager,AutoDetectInterfaceFunc,
    notifyInterfaceUpdate}` makes interface monitoring part of runtime lifetime,
    binds each dial to the resolved interface, and resets network state after an
    interface generation change.
  - `protocol/tun/inbound.go::NewInbound`, `Inbound.Start`, and `Inbound.Close`
    create the TUN before publishing readiness and close the stack, TUN, and
    redirect state as one owner.
  - `dns/transport/local/resolv_windows.go::dnsReadConfig` takes DNS only from
    operational, gateway-bearing, non-tunnel adapters and excludes the policy
    interface.

Externally observable semantics followed here:

1. Policy startup is fail-closed. A Wintun or Leaf failure never degrades to an
   ordinary, unfiltered Internet path.
2. The physical underlay is selected before the policy Wintun changes the default
   route, and every Leaf TCP/UDP socket is bound to that Windows interface index.
3. One EasyTier instance owns the Windows policy worker, Wintun, mesh proxy
   bridges, and cleanup lifetime.
4. The ordinary EasyTier Windows TUN continues to own mesh routes. Leaf owns only
   the more general IPv4 policy route, so longest-prefix routing keeps mesh
   traffic in EasyTier.
5. Automatic underlay follows address/gateway/DNS changes on the same adapter and
   WLAN/Ethernet/USB handoff by replacing the worker. When another VPN owns the
   default route and multiple physical adapters are simultaneously eligible,
   automatic mode refuses to guess; an explicit physical interface remains
   supported and is hard-bound below that VPN/TUN.
6. Leaf must own both halves of every enabled default-route family before policy
   readiness is published and throughout runtime. If Mihomo, sing-box, or another
   VPN takes capture precedence, EasyTier stops fail-closed instead of reporting
   policy as running while traffic bypasses it. EasyTier does not rewrite or
   delete the competing VPN's routes.

## Intentional EasyTier boundary

Windows uses a worker-owned Wintun and does not port the Unix datagram/FD packet
bridge. This is required by the Windows TUN model and keeps the existing
Linux/macOS/Android packet paths unchanged. EasyTier adds the Windows IPv6
default route to the same Wintun because the locked Leaf raw-IP stack accepts
both families even though the `tun` crate configures only its IPv4 interface
tuple.

## Candidate implementation

- Extend the existing bounded Leaf worker lifecycle to Windows while retaining
  the packet bridge and FD inheritance only on Unix.
- Compile a Windows Leaf-owned TUN with an explicit, single-owner interface
  identity and the pre-TUN physical DNS servers.
- Start and retain the Windows runtime from `NicCtx`, reuse the existing mesh
  SOCKS bridge, runtime ID, restart budget, and shutdown primitives, and leave the
  EasyTier NIC stream/sink unchanged.
- Package the architecture-matched Leaf worker with Windows Core/GUI builds.
  The worker watches the EasyTier parent and exits if it disappears.
- Detect physical adapter address/gateway/DNS changes conservatively and rebuild
  the instance and worker so Leaf's process-level outbound binding is refreshed.
- Preserve an explicitly supplied Leaf Windows interface and bind sockets with
  `IP_UNICAST_IF`/`IPV6_UNICAST_IF`; never reuse the locked Leaf behavior that
  picked the first enumerated gateway adapter or overwrote the supplied value.
- Offer Windows `auto` as the GUI default. Accept only adapters carrying
  Windows' `HardwareInterface` flag; explicit mode is required instead of an
  ambiguous fallback when another VPN hides multiple physical candidates.
- Resolve the adapter alias through `ConvertInterfaceAliasToLuid` and
  `GetIfEntry2`, then compare `GetBestInterfaceEx` with the resulting
  `MIB_IF_ROW2.InterfaceIndex`. Do not substitute `IP_ADAPTER_ADDRESSES`
  `Ipv6IfIndex`, which may be zero when IPv6 is disabled and is not the API
  contract returned by the route query.
- Verify IPv4 lower/upper-half and, when enabled, IPv6 lower/upper-half route
  ownership after Wintun startup and every five seconds. This uses bounded local
  route-table queries only; it sends no probe traffic and does not poll on the
  packet hot path.
- Reject unsafe Windows combinations before Wintun creation rather than silently
  bypassing policy.

## Tests

- Config compiler: owned TUN uses `fd=-1`, explicit addressing, Wintun DNS
  servers, and leaves legacy FD mode byte-for-byte unchanged apart from the new
  owned-TUN-only field.
- Runtime/platform gates: Windows feature is recognized; unsupported builds still
  reject enabled policy.
- Pure adapter selection: only up, hardware, non-tunnel, gateway-bearing
  adapters are eligible; the EasyTier, Leaf, Mihomo, sing-box, and other virtual
  interfaces are excluded by platform identity rather than display-name guesses.
- Windows target tests/build: explicit Leaf interface survives both pre-TUN
  initialization sites; IPv4 index uses network byte order; IPv6 index uses host
  order; TCP and UDP DIRECT/proxy dials remain on the selected physical adapter
  while a competing Wintun owns the system default.
- Competing-TUN tests: startup is rejected when another interface owns either
  half of the IPv4/IPv6 capture routes; runtime stops within one monitor interval
  if route ownership changes; removing the competitor permits a clean restart;
  no foreign routes or interfaces are modified during failure or cleanup.
- Network handoff tests: WLAN to Ethernet, Ethernet to WLAN, and WLAN to USB
  tether rebuild the worker onto the newly resolved hardware adapter; ambiguous
  multi-adapter automatic selection fails closed and explicit selection remains
  usable.
- Lifecycle: startup failure releases the instance lease and mesh bridges;
  shutdown stops the worker once; ordinary Windows NIC operation has
  no policy objects when disabled.
- Windows CI: `--locked` no-run build and focused tests for Core and GUI targets,
  followed by packaged-artifact inspection and a bounded executable smoke.

## Pre-build candidate manifest

- Intended build snapshot: Windows worker-owned Wintun integration, macOS scoped
  underlay/raw-FD framing fixes, platform gates, dependency/feature/workflow
  wiring, focused tests, README guidance, GUI capability/read-only behavior, and
  their compatibility notes.
- Remote `.160` gate: `scripts/leaf-remote-preflight.sh` with the Windows-neutral
  compiler/config/lifecycle tests added to its focused filters. Windows target
  compilation is a recorded GitHub-only exception if the MSVC target/toolchain is
  unavailable on `.160`.
- Required workflows after maintainer approval: profiling beta for the immutable
  Linux regression artifact, then the formal Windows Core and GUI matrices. No
  release workflow is part of this candidate.
- Linux evidence: ordinary mesh, policy disabled, policy enabled, worker failure,
  route recovery, and shutdown cleanup remain unchanged.
- Android evidence: preserved-data upgrade, policy enabled/disabled, captured-UID
  probe, network loss/recovery, and shutdown cleanup remain unchanged.
- Windows evidence: feature-present manifest, Wintun/DLL packaging, policy
  disabled smoke, policy startup/fail-closed behavior, mesh route precedence,
  direct/proxy/DNS rule behavior, runtime stop/restart, and installed GUI smoke.
- Work during waits: inspect the complete diff, `Cargo.lock`, target `cfg`s,
  workflow feature pins, generated files, and prepare bounded Linux/Android/
  Windows evidence commands without mutating the in-flight snapshot.

## Pre-build gate evidence

- The complete combined snapshot passed the dedicated `.160` `--locked`
  no-run build and every configured EasyTier, policy, and netstack focused
  test.
- Three related frontend files passed 32 tests. Dependency-ordered
  `frontend-lib`, Web frontend, VPN plugin, and GUI production builds passed.
- The builder has Linux and Android targets but no Windows target or MSVC
  toolchain. Windows compilation, package inspection, and real Wintun behavior
  remain workflow/device gates and must not be inferred from the Linux result.
