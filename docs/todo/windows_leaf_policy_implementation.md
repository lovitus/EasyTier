# Windows Leaf policy implementation

Status: implementation candidate

## Reference contract

- Failure-baseline Leaf source: `https://github.com/lovitus/leaf.git` at
  `013a1497dd29355a00cd776628ff2de72e02e861` (the exact revision used by the
  prior artifact). The combined Windows/macOS candidate locks
  `e73ec228883965850f6bfbb339e64fd8fe86ef1f`.
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
  - `component/dialer/bind_windows.go::bindIfaceToConnection` binds IPv4 and IPv6
    outbound sockets to the selected physical interface.
- sing-box reference: `/Users/fanli/Documents/singbox-withfallback` at
  `a9cd6f89d919a55353ec2170bf88add0d87882f1`.
  - `route/network.go::NewNetworkManager`, `NetworkManager.Start`, and
    `NetworkManager.Close` make the interface monitor and its cleanup part of the
    same runtime lifetime.
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
   route.
3. One EasyTier instance owns the Windows policy worker, Wintun, mesh proxy
   bridges, and cleanup lifetime.
4. The ordinary EasyTier Windows TUN continues to own mesh routes. Leaf owns only
   the more general IPv4 policy route, so longest-prefix routing keeps mesh
   traffic in EasyTier.

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
- Reject unsafe Windows combinations before Wintun creation rather than silently
  bypassing policy.

## Tests

- Config compiler: owned TUN uses `fd=-1`, explicit addressing, Wintun DNS
  servers, and leaves legacy FD mode byte-for-byte unchanged apart from the new
  owned-TUN-only field.
- Runtime/platform gates: Windows feature is recognized; unsupported builds still
  reject enabled policy.
- Pure adapter selection: only up, non-tunnel, gateway-bearing adapters are
  eligible; the EasyTier and Leaf interfaces are excluded.
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
