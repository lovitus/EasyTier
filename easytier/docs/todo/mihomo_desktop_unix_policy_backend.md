# Mihomo Desktop and Unix Policy Backend, Reduced Design

**Status:** USER-APPROVED DIRECTION, NOT IMPLEMENTED

**Date:** 2026-07-24

**Required platforms:** Linux, macOS, Windows, and every Unix-family target in
the EasyTier release matrix.

**Out of scope:** Android and OHOS Mihomo integration.

## 1. Final user requirement

The integration is deliberately reduced to two independent services:

```text
EasyTier
  -> always starts a local HEV SOCKS5 service
  -> owns mesh, HEV, underlay protection, and HEV status

Mihomo
  -> runs as an independently supervised sidecar
  -> owns its TUN, DNS, rules, proxies, providers, groups, and subscriptions
```

EasyTier must not understand, translate, inject, validate, or manage Mihomo
proxy semantics.

The user composes the two systems with standard Mihomo configuration:

```yaml
proxies:
  - name: p1
    type: socks5
    server: 127.0.0.1
    port: 11080
    udp: true

  - name: peer-socks
    type: socks5
    server: 10.44.0.8
    port: 24443
    udp: true
    dialer-proxy: p1
```

The intended path is:

```text
Mihomo
  -> local HEV at 127.0.0.1:11080
  -> HEV connects to the EasyTier virtual peer address
  -> EasyTier mesh
  -> peer SOCKS5 at 10.44.0.8:24443
  -> destination
```

`p1`, `peer-socks`, `dialer-proxy`, chain selection, fallback, subscriptions,
and all protocol fields are entirely Mihomo-owned.

EasyTier is allowed to manage only:

- The local HEV process/runtime and loopback listener.
- Mihomo process launch and runtime health.
- Mihomo TUN ownership fields required for safe coexistence.
- Exact EasyTier route exclusions.
- Reserved rules that prevent EasyTier and HEV traffic from returning to the
  Mihomo TUN.
- Private Mihomo controller settings required for supervision.
- Cleanup of exact EasyTier-owned and supervised-Mihomo-owned state.

## 2. Hard architecture boundary

### 2.1 EasyTier must never inspect or mutate proxy semantics

The implementation must not read, interpret, rewrite, merge, normalize, or
validate these sections beyond preserving their raw YAML values:

```text
proxies
proxy-providers
proxy-groups
rule-providers
sub-rules
listeners unrelated to the private EasyTier controller
protocol-specific node fields
dialer-proxy
provider overrides
health-check definitions
subscription payloads
```

In particular, EasyTier must never:

- Infer whether a proxy address belongs to the mesh.
- Replace a proxy host with `127.0.0.1`.
- Add HEV as a `dialer-proxy` automatically.
- Resolve proxy server names.
- Convert Mihomo nodes into Leaf nodes.
- Build chain or fallback groups.
- Change provider contents.
- Change rule targets selected by the user.
- Fall back an unavailable proxy to `DIRECT`.

The user owns all Mihomo proxy composition.

### 2.2 Permitted Mihomo runtime overlay

EasyTier may generate a private runtime copy of the user YAML, but may touch
only these paths:

```text
tun.enable
tun.device
tun.auto-route
tun.route-exclude-address
rules reserved prefix
private controller address/secret
```

Any future managed field requires a design update and explicit maintainer
approval.

The source YAML is immutable. EasyTier must never use a generated copy as the
source of a later generation.

### 2.3 No Mihomo source modification

The selected Mihomo release must run unmodified as a separate executable.
EasyTier must not:

- Link Mihomo into Rust.
- Patch Mihomo proxy or rule code.
- Add an EasyTier-specific Mihomo protocol.
- Require an EasyTier-specific subscription format.
- Require Mihomo to call an EasyTier API.

The only data-plane contract is standard SOCKS5 TCP and UDP.

## 3. Backend selection

The desktop and Unix policy selector remains:

```text
off
mihomo
leaf
```

Required compatibility:

| Configuration | Effective backend |
| --- | --- |
| No policy configuration | `off` |
| Legacy `enable_policy_proxy = false` | `off` |
| Legacy `enable_policy_proxy = true` with no backend | `leaf` |
| Explicit `backend = "leaf"` | `leaf` |
| Explicit `backend = "mihomo"` | `mihomo` |
| Conflicting legacy boolean and enum | Reject |

HEV is not selected by this enum. HEV belongs to EasyTier mesh and starts for
every supported EasyTier instance regardless of `off`, `mihomo`, or `leaf`.

## 4. HEV becomes an independent EasyTier service

### 4.1 Required feature ownership

The current artificial dependency must be removed:

```text
leaf-policy-proxy -> easytier-socks-egress
```

The intended feature ownership is:

```text
mesh-socks-egress
  -> easytier-socks-egress
  -> required mesh data-plane primitives

leaf-policy-proxy
  -> easytier-policy

mihomo-policy-proxy
  -> Mihomo supervisor and minimal runtime overlay
```

Release builds for the required platforms include `mesh-socks-egress`.
Neither Leaf nor Mihomo owns HEV.

### 4.2 Startup behavior

Every EasyTier instance on a required platform starts one local HEV SOCKS5
service.

Required listener policy:

- Bind only `127.0.0.1` by default.
- IPv6 loopback may be added only after equivalent validation.
- Never bind `0.0.0.0` or `[::]` by default.
- Try the configured/default candidates `11080`, `11081`, and `11082` in order.
- Publish the selected port through Core status, management API, CLI, and GUI.
- Never silently claim a port owned by another process.
- Never treat a successful TCP bind as proof that UDP is available.
- Verify SOCKS5 readiness before publishing `running`.

The default is an actually running HEV service, not only a registered lazy
capability.

### 4.3 Stable configuration contract

A user may configure Mihomo against the published loopback endpoint:

```yaml
- name: p1
  type: socks5
  server: 127.0.0.1
  port: 11080
  udp: true
```

If the selected HEV port is not the configured Mihomo port, Core must report the
mismatch clearly. EasyTier must not modify the Mihomo proxy entry to hide the
mismatch.

A later enhancement may allow the user to reserve one explicit HEV port and
fail startup if unavailable. Dynamic port rewriting is not part of this design.

### 4.4 HEV route behavior

HEV is a system-routing SOCKS5 service. For a request targeting an EasyTier
virtual address, its outbound socket must follow the EasyTier mesh route.

HEV must not force all outbound sockets onto a physical interface. That would
make this required path fail on macOS, Windows, and some Unix systems:

```text
127.0.0.1:11080 -> HEV -> 10.44.0.8:24443 -> EasyTier TUN
```

The route contract is:

- EasyTier virtual destinations use the EasyTier route.
- Non-mesh destinations use the normal protected system route.
- HEV traffic must not re-enter the Mihomo TUN.
- HEV traffic targeting its own loopback listener is rejected to prevent local
  recursion.
- HEV TCP and UDP use the same route classification.

## 5. Mihomo configuration preservation

### 5.1 Immutable source

EasyTier receives a user Mihomo YAML file or a downloaded full-config source.
The source is read-only.

Required artifacts:

| Artifact | Owner |
| --- | --- |
| User Mihomo YAML | User, immutable |
| Runtime Mihomo YAML | EasyTier, private generated copy |
| Runtime overlay report | EasyTier |
| Runtime manifest | EasyTier |

The private runtime directory must be owner-only and reject symlinks, junction
escapes, ownership mismatch, and world-writable parents.

### 5.2 Raw preservation

The runtime compiler must use a generic YAML representation and retain unknown
fields. It must not deserialize proxy sections into EasyTier proxy structs.

A semantic preservation test must prove:

```text
generated tree - permitted managed paths == source tree
```

The allowed difference set is exactly the list in section 2.2.

### 5.3 Deterministic generation

The runtime manifest includes:

```text
source SHA-256
managed overlay SHA-256
Mihomo executable SHA-256
platform
compiler schema version
generated SHA-256
last validation result
```

A cached runtime copy is reusable only if every hash and permission check
matches. Otherwise it is regenerated from the immutable source.

EasyTier must never repeatedly patch a previous runtime copy.

## 6. TUN coexistence

### 6.1 Managed fields

For `backend = "mihomo"`, the initial managed values are:

```yaml
tun:
  enable: true
  auto-route: true
  device: <EasyTier-owned unique policy device>
  route-exclude-address:
    - <existing user exclusions>
    - <exact current EasyTier-owned routes>
```

`route-exclude-address` is a stable union. User exclusions are never removed.

### 6.2 Exact route discovery

EasyTier must add only routes it can prove it owns:

- The current instance virtual network.
- Current EasyTier peer and subnet routes installed by the instance.
- Exact host routes required to protect EasyTier control and underlay sockets.

Do not hard-code broad private or CGNAT ranges. Do not exclude unrelated LAN,
VPN, enterprise, or Tailscale routes unless a separately validated coexistence
module proves ownership.

### 6.3 TUN conflicts

Startup must reject, before publishing `running`:

- Policy device name owned by another process.
- Another active EasyTier Mihomo policy owner.
- A pre-existing unrelated Mihomo TUN that would be overwritten.
- Interface include/exclude settings that capture the EasyTier TUN as underlay.
- Route settings that make the EasyTier virtual network unreachable.
- Unsupported `auto-redirect` firewall ownership in the first version.
- A private controller collision.

No conflict is resolved by deleting another process's interface, route, rule, or
firewall state.

## 7. Mandatory anti-loop protection

Preventing return flow is a release gate on every required platform.

### 7.1 Defense layers

All of the following are required:

1. Exact EasyTier routes in Mihomo `tun.route-exclude-address`.
2. A reserved first-match Mihomo rule prefix for EasyTier Core and separate HEV
   process identities when process matching is supported reliably.
3. EasyTier socket-level underlay protection independent of Mihomo rules.
4. HEV socket routing that permits EasyTier virtual destinations while
   bypassing Mihomo capture.
5. Loopback-only HEV and controller listeners.
6. Local recursion rejection for HEV's own endpoint.
7. Runtime counters and bounded detection for repeated identical flow cycles.

Process rules are defense in depth. They are not the only protection because
platform process attribution may be unavailable, delayed, or incomplete.

### 7.2 Reserved rule prefix

EasyTier may prepend only the minimum rules needed to prevent return flow.
Every original user rule remains in its original order after the prefix.

Conceptual prefix:

```yaml
rules:
  - PROCESS-NAME,easytier-core,DIRECT
  - PROCESS-NAME,<owned-hev-process>,DIRECT
  - IP-CIDR,<exact-easytier-prefix>,DIRECT,no-resolve
  - <all original user rules, unchanged>
```

The actual process and path rules are platform-specific and must be checked
against the pinned Mihomo implementation.

If equivalent protection cannot be expressed safely on a platform, socket and
route protection must provide the guarantee. The platform remains unsupported
until that path is validated.

### 7.3 Socket-level protection

Required platform behavior:

| Platform | Required primitive |
| --- | --- |
| Linux | Non-zero socket mark plus an owned policy rule/table that still resolves EasyTier virtual routes correctly |
| macOS | Correct IPv4/IPv6 interface or route-scoped socket behavior without forcing mesh destinations onto the physical interface |
| Windows | Correct IPv4 `InterfaceIndex` and IPv6 interface index, with EasyTier virtual routes still selectable |
| Unix | Native bind/route primitive with the same behavior, proved per target |

The underlay protection implementation must distinguish:

- EasyTier transport sockets that must leave through the physical underlay.
- HEV SOCKS outbound sockets targeting EasyTier virtual addresses that must use
  the EasyTier TUN.
- HEV non-mesh destinations that must use the normal direct route without
  returning to Mihomo.

One unconditional physical-interface bind for all HEV traffic is prohibited.

### 7.4 Loop detection

The runtime must expose bounded counters for:

```text
rejected HEV self-targets
repeated identical SYN or UDP flow attempts
Mihomo restart count
route generation count
underlay protection failures
HEV connect failures by route class
```

Detection must not create verbose default logs or an unbounded flow table.
Repeated loop signatures trigger a visible degraded/failed status and bounded
shutdown of the affected policy path, not a restart storm.

## 8. Preventing accidental host blocking

### 8.1 Startup transaction

Required order:

1. Start EasyTier mesh.
2. Start HEV and prove TCP/UDP readiness.
3. Verify the pinned Mihomo executable.
4. Parse the source generically and build only the permitted overlay.
5. Run Mihomo config-test mode against the generated copy.
6. Acquire the host-global Mihomo policy-owner lock.
7. Snapshot relevant route, TUN, DNS, and firewall state.
8. Start Mihomo under OS-native child ownership.
9. Wait for process, private controller, TUN, route, and DNS readiness.
10. Verify EasyTier mesh and HEV remain reachable.
11. Publish `running` only after every check passes.

Any failure before step 10 must stop the owned Mihomo child, remove only owned
policy state, verify baseline restoration, and leave EasyTier mesh and HEV
running.

### 8.2 Runtime failure policy

Accidental stale-route blocking is prohibited.

The backend exposes an explicit failure policy:

```text
restore-network
fail-closed
```

Default desktop behavior is `restore-network`:

- Stop the exact owned Mihomo child.
- Remove exact owned Mihomo TUN/routes.
- Verify the pre-start host route baseline is restored.
- Keep EasyTier mesh and HEV running.
- Report that policy enforcement is unavailable.

`fail-closed` is an explicit advanced setting:

- Policy traffic may remain blocked intentionally.
- Mesh and HEV control traffic must remain available.
- Status must clearly say the block is intentional.
- No hidden direct fallback is allowed.

The selected policy is persisted and shown in Core/CLI/GUI status. It must never
change automatically.

### 8.3 Cleanup ownership

Cleanup may remove only objects carrying the exact recorded ownership identity:

```text
child process launch nonce
process creation identity
Mihomo executable hash
runtime config hash
TUN identity
route table/rule identity
controller identity
```

PID alone is insufficient because of PID reuse.

## 9. Full-platform Mihomo supervision

### 9.1 Required states

Core exposes one authoritative state machine:

```text
off
validating
starting
waiting_for_controller
waiting_for_tun
verifying_routes
running
degraded
restart_backoff
restoring_network
stopping
failed
conflict
unsupported
```

Required status fields:

```text
backend
compiled_in
configured
mihomo_version
mihomo_binary_hash
source_hash
runtime_hash
process_identity
process_uptime
controller_transport
controller_health
policy_tun
route_generation
hev_endpoint
hev_tcp_ready
hev_udp_ready
restart_count
next_restart
last_exit
last_error
failure_policy
cleanup_status
```

### 9.2 Process ownership

| Platform | Required mechanism |
| --- | --- |
| Linux | Supervised process group plus parent-death/stale-state recovery primitive |
| macOS | Supervised process group and exact child identity with sleep/wake handling |
| Windows | Job Object with kill-on-close and Service-compatible ownership |
| Unix | Native process group/wait mechanism plus stale-state audit |

A Mihomo process not started by the current EasyTier owner must never be killed,
reconfigured, or adopted.

### 9.3 Private controller

Preferred transports:

```text
Unix-family: owner-only Unix-domain socket
Windows: owner-only named pipe
Fallback: random loopback port plus random per-launch secret
```

The controller must never be exposed on a non-loopback address by the generated
runtime overlay.

The supervisor uses the controller only for:

- Readiness.
- Version and runtime health.
- TUN/runtime status.
- Graceful shutdown or bounded reload when supported.

It does not inspect or alter proxy selection, providers, groups, rules, or
subscriptions.

### 9.4 Bounded restart

Restart behavior:

- Singleflight: only one start/reload/restart operation at a time.
- Exponential backoff.
- Maximum attempts per time window.
- Stable-runtime period before resetting the budget.
- No retry for deterministic config, permission, ownership, or binary errors.
- Network changes are deduplicated by normalized generation/hash.
- Repeated identical events do not restart Mihomo.
- New generations cancel unpublished old candidates.

A depleted restart budget transitions to `failed` and applies the configured
failure policy.

## 10. Platform requirements

### 10.1 Linux

Must support and validate:

- glibc and musl packages.
- Current kernels and the oldest supported kernel/userspace.
- x86_64 and aarch64 where released.
- systemd and direct Core startup.
- Socket mark and route-rule ownership.
- IPv4, IPv6, and dual-stack EasyTier virtual routes.
- Mihomo crash, Core crash, stale TUN, stale route, and cleanup.
- No nftables/iptables ownership in the first release unless separately
  designed and approved.

### 10.2 macOS

Must support and validate:

- Apple Silicon and Intel where released.
- IPv4 and IPv6 route scoping.
- Correct physical-underlay versus EasyTier-virtual route selection.
- Wi-Fi/Ethernet switching.
- Sleep/wake.
- App bundle sidecar location and permissions.
- Code signatures for EasyTier and Mihomo.
- Process-group cleanup and stale-state recovery.

### 10.3 Windows

Must support and validate:

- x86_64 and ARM64 where released.
- Wintun/TUN identity and cleanup.
- Correct IPv4 interface index.
- Correct IPv6 interface index, including a physical adapter with IPv6
  disabled.
- Route metrics and adapter replacement.
- Job Object ownership.
- Windows Service and interactive Core startup.
- Named pipe or protected loopback controller.
- Sleep/wake and network switching.
- Signed EasyTier and Mihomo binaries.

### 10.4 Unix-family targets

Phase 0 must enumerate every Unix target in the actual EasyTier release matrix.
Each listed target must pass real runtime validation for:

- HEV startup and TCP/UDP readiness.
- Mihomo startup and supervision.
- TUN and route ownership.
- IPv4 and IPv6 where supported by the OS.
- Underlay bypass and mesh route selection.
- Crash recovery and cleanup.
- Private controller permissions.

Linux evidence cannot substitute for another Unix target.

Android and OHOS are the only predefined `N/A` targets for this project.

## 11. Development plan

### Phase 0: exact platform and source inventory

Record:

- Exact EasyTier release targets.
- Exact pinned Mihomo source revision and binary strategy.
- Mihomo TUN/controller/process support per required platform.
- Current HEV platform support and startup model.
- Current Leaf feature coupling points.
- Existing platform socket-bind, route, mark, and interface-index behavior.
- License and source-distribution obligations.

Any required platform without a viable path blocks implementation.

### Phase 1: detach and always start HEV

Implement:

- Independent `mesh-socks-egress` feature.
- Move HEV ownership out of Leaf policy modules.
- Start HEV for every EasyTier instance on required platforms.
- Loopback-only listener and port candidates.
- TCP/UDP readiness.
- Core status and cleanup.
- Route classification that permits virtual peer destinations.

Do not add Mihomo code in this phase.

### Phase 2: backend enum and Mihomo supervisor

Implement:

- `off | mihomo | leaf` enum and legacy migration.
- Mihomo executable verification.
- OS-native process ownership.
- Private controller.
- State machine and status API.
- Bounded restart.
- Failure policy.

Use a minimal fixture config before real user configurations.

### Phase 3: minimal runtime overlay

Implement only:

- Immutable source loading.
- Generic YAML preservation.
- Managed TUN fields.
- Exact route-exclude merge.
- Reserved anti-loop rule prefix.
- Private controller settings.
- Deterministic manifest and audit report.

Add a test that fails if any proxy/provider/group/dialer-proxy value changes.

### Phase 4: platform anti-loop and cleanup

Implement Linux, macOS, Windows, and Unix platform lanes in parallel:

- Underlay socket protection.
- HEV mesh-route selection.
- TUN ownership.
- Route baseline snapshot and cleanup.
- Crash recovery.
- Network-change generation handling.

No platform may fall through to a generic unvalidated implementation.

### Phase 5: Core, CLI, API, and GUI

Implement:

- Core-only startup path.
- TOML and CLI backend selection.
- Management protobuf/REST/Tauri status.
- HEV selected-port display.
- Mihomo runtime state and failure-policy display.
- Read-only overlay report.
- No proxy editor or Mihomo proxy translation in EasyTier.

### Phase 6: immutable candidate validation

Use one complete candidate per repository workflow rules. Validate all required
platforms from the exact same source SHA and platform-specific artifact hashes.

## 12. Required tests

### 12.1 HEV independence

- HEV builds without Leaf.
- HEV starts with backend `off`.
- HEV starts with backend `mihomo`.
- HEV starts with backend `leaf`.
- Removing/stopping Leaf does not stop HEV.
- Stopping Mihomo does not stop HEV.
- HEV cleanup occurs only when the EasyTier instance stops.
- TCP and UDP work on every required platform.

### 12.2 Zero proxy mutation

Fixtures must include:

- SOCKS5, Shadowsocks, Trojan, VMess, VLESS, and every protocol in the pinned
  Mihomo build.
- WebSocket, TLS, Reality, QUIC, and supported transports.
- `dialer-proxy` chains.
- Proxy providers with overrides.
- Select, fallback, URL-test, and load-balance groups.
- Unknown future fields.

After overlay generation, every value under proxy/provider/group paths must be
identical to the source semantic tree.

### 12.3 Required chain scenario

Every platform must prove this exact pattern for TCP and UDP:

```yaml
- name: p1
  type: socks5
  server: 127.0.0.1
  port: <published-hev-port>
  udp: true

- name: peer-socks
  type: socks5
  server: <easytier-peer-ip>
  port: <peer-socks-port>
  udp: true
  dialer-proxy: p1
```

Required evidence:

- Mihomo connects to the published local HEV endpoint.
- HEV connects to the virtual peer through EasyTier mesh.
- The remote peer SOCKS receives the request.
- TCP application data completes.
- UDP application data completes.
- Removing `dialer-proxy` changes the path according to Mihomo configuration
  without an EasyTier restart.
- EasyTier never reads the proxy names or chain fields.

### 12.4 Anti-loop failure injection

- Remove the process guard while retaining socket protection.
- Remove route exclusion in a test-only generated fixture.
- Replace the default gateway.
- Restart Mihomo during active mesh traffic.
- Restart HEV during active chained traffic.
- Target the HEV listener through itself.
- Repeatedly connect to an unavailable peer SOCKS.
- Change IPv4/IPv6 route preference.
- Disable IPv6 on the physical Windows adapter.
- Switch macOS physical interfaces.

Any detected busy loop, repeated route generation, unbounded reconnect, CPU
storm, or traffic amplification immediately fails the candidate.

### 12.5 Blocking and recovery

- Invalid source YAML before startup.
- Valid YAML with a TUN conflict.
- Mihomo exits before readiness.
- Mihomo exits after readiness.
- Controller disappears.
- Core exits normally.
- Core is terminated forcibly.
- Host resumes from sleep.
- Interface and DNS generations change repeatedly.
- Runtime directory becomes read-only.
- Disk fills during generation.

For `restore-network`, ordinary host networking and EasyTier mesh must recover.
For `fail-closed`, policy traffic may remain blocked but EasyTier mesh and Core
control must remain available.

## 13. Performance and resource gates

Measure the same host and path for:

```text
EasyTier without HEV feature
EasyTier with always-running idle HEV
Mihomo standalone
Mihomo supervised by EasyTier
Mihomo -> local HEV -> mesh peer SOCKS
Leaf legacy comparator
```

Collect:

- Idle CPU.
- Active CPU.
- RSS.
- Threads.
- FDs or handles.
- TCP upload/download.
- UDP throughput/loss/jitter.
- Connection latency.
- Startup and restart latency.
- Route generation and restart counts.

Acceptance:

- Idle HEV must not busy-loop.
- Always-running HEV must have a documented bounded resource cost.
- Mihomo supervision without traffic must not cause periodic restart/reload.
- No counter, queue, cache, retry list, or flow table may grow without a bound.
- Resource counts return to baseline after stop.
- No-policy mesh performance remains within measurement noise except the
  documented idle HEV cost.

## 14. Release gates

The project is not releasable until:

- HEV is independent of Leaf.
- Every required EasyTier instance starts HEV successfully.
- Core publishes the actual HEV port and TCP/UDP readiness.
- Mihomo runs and is supervised on Linux, macOS, Windows, and every required
  Unix target.
- Proxy/provider/group/dialer-proxy semantic preservation tests pass.
- The exact `p1 -> HEV -> mesh peer SOCKS` chain passes TCP and UDP everywhere.
- Underlay and HEV traffic cannot return to Mihomo TUN.
- Mihomo crash cannot leave accidental stale-route host blocking.
- Network changes cannot create restart or route-generation storms.
- An unrelated Mihomo process, TUN, route, or controller is never modified.
- Clean stop and crash recovery return owned resources to baseline.
- Required platform packages contain the correct Mihomo binary, architecture,
  permissions, signature, version, and license/source notices.
- Policy `off` and Leaf behavior have no functional regression.
- Security review has no open P0 or P1 issue.

## 15. Explicit non-goals

- Android Mihomo backend.
- OHOS Mihomo backend.
- EasyTier proxy editor for Mihomo.
- Proxy/provider/group translation.
- Automatic `dialer-proxy` insertion.
- Proxy-host parsing or rewriting.
- EasyTier-managed Mihomo subscription semantics.
- In-process Mihomo linking.
- Automatic Mihomo executable update.
- Multiple host-global Mihomo policy owners in the first version.
- Mihomo `auto-redirect` firewall ownership in the first version.

## 16. Review checklist

Every implementation review must answer:

- Did this change touch a Mihomo proxy/provider/group/dialer-proxy value?
- Can source YAML be modified?
- Can a generated file become a later source?
- Can EasyTier or HEV traffic return to Mihomo TUN?
- Can HEV still route an EasyTier virtual destination through the mesh?
- Can a Mihomo crash leave stale host-blocking routes?
- Can a network event trigger duplicate starts or a restart storm?
- Can cleanup affect an unrelated process or route?
- Does the same behavior have real evidence on every required platform?
- Does Core work without GUI involvement?

Any uncertain answer blocks merge.
