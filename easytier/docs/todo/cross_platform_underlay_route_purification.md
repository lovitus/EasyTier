# Cross-platform underlay route purification assessment

> Status: FEASIBILITY_PROVEN / EXISTING_MECHANISM_FOUND / RECOMMEND_NO_NEW_FEATURE / USER_DECISION_PENDING
>
> [!CAUTION]
> **DEPRECATED PERMANENTLY - DO NOT IMPLEMENT THIS TODO.**
>
> The proposed broad `enable_underlay_socket_purify` feature is abandoned. EasyTier
> already applies resolved interface binding on the ordinary TCP, UDP, WebSocket, and
> WireGuard connector paths, so a second cross-platform purification subsystem would
> duplicate working ownership and expand lifecycle, fallback, and platform state.
>
> Exact-artifact capture did confirm a separate, path-specific problem: a shared
> wildcard QUIC socket can be captured by a policy TUN and handled by Mihomo before it
> reaches the physical interface. That evidence does not justify this design. Fixing a
> shared socket generically would require per-interface socket replication or per-packet
> interface control and could change EasyTier's intentional multi-interface probing,
> NAT mappings, hole punching, QUIC endpoint reuse, and path migration behavior.
>
> Any future work must use a new, narrowly scoped investigation for UDP/QUIC duplicate
> handling. It must not revive this flag, add global routes/firewall ownership, or alter
> healthy connector paths. The historical analysis below is retained only as rejected
> design evidence.
>
> Proposed flag `enable_underlay_socket_purify`: `DEPRECATED_AND_REJECTED`.
> Do not add this flag, CLI option, environment variable or GUI control unless later
> evidence identifies behavior that cannot be represented by existing `bind_device`.

## 1. Final recommendation

Do not implement a second underlay-purification subsystem.

Standalone Linux, macOS and Windows probes proved that constraining an outbound socket
to an interface already selected by EasyTier can bypass a policy TUN without reducing
multi-interface fan-out. A subsequent current-source audit found that EasyTier already
implements this exact mechanism for the socket paths that can accept it safely:

- `bind_device` defaults to `true`;
- peer connectors receive an `UnderlayInterfaceSnapshot`;
- existing source addresses are converted to `ResolvedBindAddr` values;
- TCP, UDP, WebSocket and WireGuard connectors consume those resolved addresses;
- `bind_resolved()` applies the platform interface constraint;
- policy mode refuses to start with `bind_device=false` on supported desktop paths.

The remaining sockets are primarily wildcard/shared/raw paths that do not already
carry a stable per-interface identity. Safely binding them would require changing the
socket or endpoint topology, which violates the proposed feature's minimality and
multi-interface invariants.

Therefore a new default-off flag would duplicate current behavior on safe paths while
leaving the difficult paths unchanged. It would add configuration, tests, support and
status semantics without delivering the measured probe benefit to additional traffic.

## 2. Original objective

The investigation asked whether EasyTier underlay traffic could bypass an active
Mihomo/Leaf/system policy TUN to avoid:

- a second user-space packet traversal;
- avoidable CPU, latency and throughput cost;
- underlay recursion or route instability;
- dependence on proxy process-rule matching.

The optimization must preserve EasyTier's deliberate multi-address and multi-interface
fan-out, which is part of aggressive NAT traversal and hole punching.

## 3. Non-negotiable invariants

Any future targeted fix must preserve:

- local IPv4/IPv6 candidate sets;
- candidate ordering and attempt count;
- one-interface-per-attempt fan-out;
- hole-punch timing, concurrency and retry behavior;
- peer endpoint and relay selection;
- TCP/UDP/QUIC/WebSocket/WireGuard/FakeTCP preference;
- KCP/QUIC/native fallback semantics;
- listener reachability;
- mesh routing behavior.

It must not introduce:

- a global/default physical-interface selector;
- a second interface cache or snapshot;
- automatic TUN polling or route monitoring;
- an EasyTier-owned general firewall/route manager;
- per-peer host routes;
- proxy-backend YAML rewriting;
- a helper process or background retry task;
- work in the packet hot path.

## 4. Standalone feasibility evidence

The probes used temporary C, Python and PowerShell tools. They did not modify EasyTier
source or configuration.

### 4.1 Linux IPv4 and IPv6

An isolated namespace topology contained two physical-like paths and a third
higher-priority simulated policy-TUN path.

Final `SO_BINDTODEVICE` result:

- four unbound IPv4/IPv6 A/B packets entered the simulated TUN;
- explicit-source A/B sockets constrained to their selected interfaces used A/B;
- wildcard-source sockets carrying an explicit A/B interface identity also used A/B;
- all eight constrained packets appeared on the physical receiver;
- zero constrained packets appeared on the simulated TUN;
- namespace, veth, capture and process state returned to baseline.

An earlier `SO_MARK` prototype also worked when paired with a policy rule, but that
design is rejected because it unnecessarily adds host-global ownership. Linux does not
need a new mark/rule manager for per-interface sockets.

### 4.2 macOS IPv4

The host had a real active policy TUN plus Ethernet and Wi-Fi with independent
gateways. The public destination used the policy TUN by default.

- wildcard selected the policy-TUN address and all five packets entered the TUN;
- `IP_BOUND_IF` Ethernet selected Ethernet and all five packets used only Ethernet;
- `IP_BOUND_IF` Wi-Fi selected Wi-Fi and all five packets used only Wi-Fi;
- zero constrained packets entered the TUN;
- zero Ethernet/Wi-Fi cross-interface packets appeared.

### 4.3 macOS IPv6

The public IPv6 destination used the policy TUN. Wi-Fi had a global IPv6 address and a
physical scoped default route; Ethernet had no IPv6 default and was not treated as a
usable IPv6 candidate.

- wildcard selected the policy-TUN IPv6 address and all five packets entered the TUN;
- `IPV6_BOUND_IF` Wi-Fi selected its global physical source;
- all five constrained packets used Wi-Fi;
- zero constrained packets entered the TUN.

### 4.4 Windows IPv4 and IPv6 fan-out

On a host with three active Wi-Fi adapters:

- two usable IPv4 interfaces each emitted about 209 KB when constrained;
- source-only and source-plus-interface TCP both connected on those interfaces;
- a third interface failed both controls, proving its failure was existing
  reachability rather than the socket option;
- two independent IPv6 interfaces emitted about 214 KB and 212 KB respectively.

### 4.5 Windows real Mihomo TUN bypass

On a separate Windows host with an active Mihomo TUN, the same 200-packet,
1000-byte UDP workload produced:

| Mode | Mihomo adapter | Physical adapter |
|---|---:|---:|
| wildcard | 209,474 bytes | 206,577 bytes |
| physical `IP_UNICAST_IF` | 7,287 bytes | 221,329 bytes |

The Mihomo-adapter delta fell by approximately 96.5%. The counters include small
background traffic, so this proves path ownership rather than end-to-end performance.

## 5. Current-source audit

### 5.1 Existing configuration already enables the mechanism

`easytier/src/common/config.rs` sets:

```rust,ignore
bind_device: true
```

This is the default, not an experimental opt-in.

Supported desktop policy startup also rejects `bind_device=false` in
`easytier/src/instance/virtual_nic.rs`, explicitly treating interface binding as part
of underlay recursion prevention.

### 5.2 Existing connector preparation resolves every safe attempt

`easytier/src/connector/mod.rs` already:

- consumes the current `UnderlayInterfaceSnapshot`;
- builds one `ResolvedBindAddr` for each usable source address;
- preserves the interface name and index;
- passes the complete vector to the connector when `bind_device=true`.

This is the exact post-selection model proven by the standalone tools. Adding
`enable_underlay_socket_purify` would not produce a new candidate or new socket policy
for these connectors.

### 5.3 Existing platform helper already applies the options

`easytier/src/tunnel/common.rs::bind_resolved()` reuses the resolved identity and
applies the existing platform behavior:

| Platform | Existing behavior |
|---|---|
| Linux | `SO_BINDTODEVICE` using the resolved interface name |
| macOS/iOS | `IP_BOUND_IF` / `IPV6_BOUND_IF` using the resolved index |
| Windows | existing Windows socket setup receives the resolved device identity |

TCP, WebSocket and WireGuard visibly consume `ResolvedBindAddr`; the common connector
contract also carries resolved bind addresses for supported tunnel connectors.

### 5.4 Linux already owns mark-based coverage where needed

Linux Leaf policy mode already requires a non-zero underlay `socket_mark` and installs
an owned policy-routing table/rule set through `PolicyRoutingGuard`.

Existing mark propagation covers paths that do not necessarily use resolved connector
addresses, including:

- listeners;
- QUIC endpoints;
- FakeTCP;
- STUN/NAT discovery;
- UDP hole-punch sockets;
- control-plane DNS.

`PolicyRoutingGuard` has an ownership lock, stale cleanup, refresh and reverse-order
route/rule removal. A second mark/rule owner is unsafe and unnecessary.

Mihomo and non-Leaf combinations must be assessed from exact runtime evidence rather
than assuming Leaf's owned mark table applies to them.

### 5.5 The uncovered paths are not compatible with the proposed minimal helper

Examples in `easytier/src/connector/udp_hole_punch/common.rs` create wildcard UDP
sockets such as:

```rust,ignore
UdpSocket::bind("0.0.0.0:0")
```

These sockets intentionally have no stable per-interface identity. The same design
constraint applies to shared QUIC endpoints and some raw/FakeTCP paths.

To constrain them on macOS or Windows would require one socket/endpoint per interface,
selection and lifecycle changes, or a platform-specific global bypass. Those are
transport-architecture changes, not a small socket-policy addition.

The safe proposal explicitly excluded these paths. Consequently its entire remaining
scope is already implemented by `bind_device`.

## 6. Why the proposed new flag should not be implemented

The proposed contract was:

```text
enable_underlay_socket_purify = false by default
```

That contract conflicts with current reality:

- `bind_device` is already true by default;
- policy mode already depends on it;
- safe per-interface connector paths are already constrained;
- Linux additionally propagates socket marks;
- unsafe wildcard/shared paths would remain unchanged under the narrow proposal.

Possible outcomes of adding the flag are all undesirable:

- it aliases `bind_device`, creating two controls for one behavior;
- it appears enabled but changes nothing on already covered paths;
- it disables existing protection when false, causing a regression;
- it expands into shared endpoint redesign, violating the minimality requirement.

## 7. Recommended next action

Do not add production code yet.

Run a focused exact-artifact audit with current `bind_device=true` and an active policy
TUN. Classify real EasyTier traffic by socket path:

- ordinary TCP/UDP peer connectors;
- WebSocket and WireGuard;
- QUIC;
- FakeTCP;
- STUN;
- UDP hole punch;
- listeners/control-plane traffic.

For each path record whether packets:

- bypass the policy TUN;
- enter it and are immediately DIRECT;
- loop or retry;
- show measurable CPU/latency/throughput cost.

Only a path with a reproducible defect or meaningful cost should receive a new,
path-specific TODO and implementation. Do not create a generic feature in anticipation
of an unmeasured gap.

## 8. Targeted implementation gate, if a real gap is found

A later fix is acceptable only when:

- it names one concrete socket path;
- flag-off/current behavior remains available if compatibility risk exists;
- it reuses existing snapshot and socket helpers;
- candidate count/order remain identical;
- no global interface selector is introduced;
- no second route/mark owner is introduced;
- no per-packet work is added;
- exact-artifact capture proves the path changed as intended;
- functional and performance evidence justify the maintenance cost.

Stop if fixing the path requires a general endpoint graph rewrite unless that rewrite
has an independently justified transport-level goal.

## 9. Decision options

### Option A: no new feature, exact-artifact gap audit

Recommended. Preserve existing code and use current artifacts to identify whether any
important path is still captured unnecessarily.

### Option B: add an alias for `bind_device`

Rejected. It creates duplicate configuration without new behavior.

### Option C: redesign wildcard/shared endpoints per interface

Not recommended without strong measured evidence. This has materially higher
correctness, lifecycle and cross-platform risk than the observed optimization value.
## 2026-08-10 exact-artifact gap audit

Status: `AUDIT_COMPLETE / BROAD_FEATURE_REJECTED / NARROW_QUIC_FOLLOWUP_UNDECIDED`

This audit used an installed macOS ARM64 EasyTier 3.0.13 artifact with EasyTier Core,
the managed GOST entry, Mihomo, two active physical interfaces, the EasyTier mesh TUN,
and the Mihomo policy TUN all running. No service, route, or configuration was changed.

### What the installed artifact already gets right

- The default resolved-bind path is effective in the real product. Established IPv4 and
  IPv6 underlay flows used their selected physical-interface addresses even while the
  ordinary route lookup for the same destination selected the policy TUN.
- A 30-packet, 1,200-byte remote-mesh stimulus produced about 133 KiB on the selected
  physical IPv6 interface, while the policy TUN carried only about 3.2 KiB of unrelated
  low-rate maintenance traffic. The data path did not scale through Mihomo.
- The idle residual observed on the policy TUN was about 202 packets and 15,012 payload
  bytes over 92.4 seconds, or 2.19 packets/s and 162 B/s. Most of it belonged to one
  wildcard UDP/QUIC flow; short UDP discovery attempts made up the remainder.

### Confirmed remaining gap

- A second 30-packet, 1,200-byte direct-peer stimulus selected the wildcard IPv4 QUIC
  path. The same flow appeared first on the policy TUN with a Mihomo FakeIP source and
  then on the physical interface with a native source.
- The capture contained about 88 KiB on each side of that transition. This is real
  double handling by EasyTier -> policy TUN -> Mihomo -> physical interface, not merely
  an idle keepalive or a route-table theory.
- The result is path-dependent: explicit/resolved TCP and IPv6 paths bypassed the policy
  TUN, while this shared wildcard QUIC path did not. Therefore neither "all underlay is
  already purified" nor "all underlay is recaptured" is accurate.

### Decision after audit

Do not implement the broad `enable_underlay_socket_purify` feature described by the
earlier proposal. It would duplicate the existing resolved-bind mechanism on healthy
paths, while the actual gap is concentrated in shared wildcard UDP/QUIC and hole-punch
sockets. Binding those sockets generically risks changing EasyTier's intentional
multi-interface probing, NAT mapping, socket sharing, and path migration semantics.

If this work is revisited, it must start as a narrow QUIC outbound-endpoint prototype,
not as a global routing or socket-purification subsystem. The prototype must preserve
the wildcard listener and hole-punch behavior, prove that a selected outbound QUIC path
can use an existing resolved interface identity, and stop before production integration
if it requires cross-platform socket replication or a second route-ownership layer.

Required evidence for reconsideration:

- the same direct-peer stimulus produces no EasyTier-owned payload on the policy TUN;
- exactly one physical copy remains and throughput/CPU improve measurably;
- IPv4, IPv6, dual-stack, interface failover, QUIC fallback, and concurrent hole punching
  retain their current behavior;
- unsupported platforms keep the current path without degradation;
- the implementation reuses existing snapshot, invalidation, and bind primitives and
  introduces no new global route, firewall, mark, daemon, or policy-backend coupling.

Until that narrow prototype meets all conditions, the residual recapture is an accepted,
documented performance cost and not a correctness defect that justifies new production
complexity.

## 2026-08-10 maintainer decision

Final decision: make no production change.

- Keep the existing TCP, UDP, WebSocket, WireGuard, QUIC, FakeTCP, hole-punch, routing,
  and policy-backend behavior unchanged.
- Do not add `enable_underlay_socket_purify`, a replacement switch, dynamic endpoint
  route exclusions, firewall ownership, or another cross-platform socket abstraction.
- Treat the confirmed wildcard UDP/QUIC recapture as a known, path-dependent local
  processing cost. It is not duplicate physical transmission and is not currently a
  release blocker.
- Preserve the existing multi-interface probing and NAT/hole-punch behavior instead of
  trading those core EasyTier properties for an unproven optimization.

Evidence supporting this decision:

- Idle policy-TUN capture: 202 EasyTier-port packets and 15,012 payload bytes over
  92.4 seconds, approximately 2.19 packets/s and 162 B/s. Most belonged to one wildcard
  QUIC flow; the remainder were short discovery probes.
- Remote-mesh data-path capture: about 132,855 payload bytes used the selected physical
  IPv6 interface while the policy TUN carried only about 3,157 bytes of background
  traffic. The main data path was already correctly isolated.
- Direct-peer QUIC capture: the same approximately 88 KiB appeared first on the policy
  TUN and then once on the physical interface. This proves local TUN/Mihomo reprocessing,
  but not duplicate physical transmission. The bounded 30-packet test completed without
  loss and averaged approximately 10.3 ms RTT.
- Source audit: ordinary resolved-bind connector paths already reuse interface identity;
  the remaining affected paths are shared wildcard QUIC endpoints and wildcard
  hole-punch/discovery sockets whose semantics depend on multi-interface reachability.

This TODO remains permanently deprecated. A future investigation may be opened only
after independent profiling demonstrates a material real-workload throughput or CPU
regression and a standalone prototype proves that it can preserve endpoint reuse,
multi-interface probing, NAT mappings, hole punching, fallback, and unsupported-platform
behavior. Such work must use a new narrowly scoped document rather than reopening this
proposal.
