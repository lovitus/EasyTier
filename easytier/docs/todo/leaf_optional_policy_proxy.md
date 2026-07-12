# Leaf Optional Policy Proxy Integration TODO

**Status**: full-L3 transparent policy spike; minimal TUN boundary integration
**Updated**: 2026-07-12

This TODO is the design source of truth. Update it after each material design
discussion so implementation does not depend on chat history.

## Linux v1 validation snapshot

The current implementation is an opt-in Linux spike behind the
`leaf-policy-proxy` Cargo feature. It is deliberately narrower than the final
cross-platform design below:

- absence of `--policy-config` creates no Leaf process, policy task, default
  route, packet mux, bridge, timer, or session table;
- enabled mode requires `bind_device=true`, an explicit physical
  `--policy-outbound-interface`, no configured instance netns, and one active
  policy instance per process. If a process hosts additional networks, they
  keep the ordinary NIC path rather than failing or inheriting policy routes;
- `easytier-leaf-worker` is a pinned, separately supervised Leaf process. It
  validates `SO_BINDTODEVICE` without sending traffic, uses at most four worker
  threads, and is restarted at most three times per unchanged endpoint
  generation;
- the TUN owner classifies mesh destinations through an immutable IPv4/IPv6
  prefix trie. Mesh packets retain the existing VirtualNic path; other packets
  enter Leaf through a bounded Unix datagram bridge;
- if Leaf is absent or its bounded input/output queue is full, non-mesh packets
  are dropped while mesh packets continue. Drop logs use power-of-two rate
  limiting;
- every `via: mesh` actor is rewritten to an authenticated private loopback
  SOCKS5 bridge. The bridge uses the existing EasyTier TCP/UDP data-plane API,
  has global connection and per-second admission limits, pins UDP sources, and
  ties UDP lifetime to both SOCKS control streams;
- v1 actors are SOCKS5 only. Mesh actors must expose a reachable EasyTier
  virtual IPv4 endpoint. The final proxied destination may still be a domain,
  IPv4, or IPv6 address. The pinned Leaf runtime has no HTTP CONNECT outbound,
  so accepting an HTTP actor would be a false capability rather than a useful
  fallback;
- the remote mesh SOCKS server must accept no-authentication mode. UDP rules
  additionally require a real SOCKS5 UDP ASSOCIATE implementation on that
  server; the existing EasyTier `--socks` remains TCP-only;
- dynamic route changes never migrate actor sessions. A peer identity/endpoint
  generation change closes the old TCP streams and UDP associations; new
  sessions use the replacement. Cost/next-hop-only changes leave sessions
  running. A policy owner refreshes route identity every five seconds because
  the current event bus has no generic remote route-info change event;
- generated Leaf config enables FakeDNS for domain/geosite routing, derives up
  to four DNS server IPs from `/etc/resolv.conf`, and forces those resolver
  sockets through Leaf DIRECT rather than libc's unbound system resolver;
- generated fallback groups disable Leaf's hard-coded active Internet probes.
  Fallback is passive and ordered for each new TCP stream or UDP association,
  so an outage does not start periodic Google/1.1.1.1 probes;
- policy YAML and local rule data are size-bounded, generated-conf delimiters
  are rejected, optional SHA-256 verification is streamed, and rule files are
  never downloaded automatically.
- the sidecar receives a Linux parent-death signal, so an abrupt EasyTier kill
  does not leave a worker, bridge FD, or policy session running;

Not yet implemented in this spike: TOML/RPC/GUI/mobile envelopes, policy file
hot reload, proxy credentials for the remote/native actor, HTTP CONNECT actor
adaptation, a bundled exit-node SOCKS5 UDP service, instance-netns worker
ownership, and non-Linux TUN adapters.
These are release blockers for claiming the full plan, but they do not affect
ordinary EasyTier builds because the feature is off by default.

## Final Operational Plan

### Runtime components

Keep the implementation in a separate optional `easytier-policy` crate with
four narrow components:

1. `PolicyTunMux`: generic L3 ownership split between unchanged VirtualNic and
   Leaf; no domain/rule/group logic.
2. `LeafRuntime`: Leaf TUN backend, DNS, rule engine, standard proxy actors, and
   groups.
3. `EasyTierDataPlaneAdapter`: wraps the existing TCP/UDP data-plane API for
   proxy actors explicitly marked `via: mesh`.
4. `PolicySupervisor`: transactional configuration, platform network events,
   bounded health/retry state, and observability.

EasyTier core exposes no Leaf types. OSPF, PeerManager, wire, Stealth, existing
SOCKS, Proxy Failover, and ordinary VirtualNic packet processing remain
unchanged.

### Public configuration envelope

All launch surfaces map to one configuration:

```toml
[policy_proxy]
enabled = true
mode = "rule"                  # rule | global | direct
profile = "default"
config_file = "policy/default.yaml"
fail_closed = true
```

- absence of the section or `enabled=false` is exactly current EasyTier;
- `config_file` is resolved relative to the EasyTier config directory;
- GUI/mobile/RPC may store `config_inline` instead of a path; the two are
  mutually exclusive;
- `fail_closed=true` is mandatory in v1. A policy failure preserves mesh L3 but
  blocks non-mesh traffic; it never silently changes to DIRECT;
- resource and breaker limits are internal validated defaults in v1 rather
  than a large user-facing tuning surface.

Implemented Linux spike launch surfaces:

```text
CLI:
  easytier-core --config config.toml \
    --policy-config policy/default.yaml \
    --policy-outbound-interface eth0

Environment:
  ET_POLICY_PROXY_CONFIG=/path/to/policy.yaml
  ET_POLICY_OUTBOUND_INTERFACE=eth0

Advanced worker override:
  --policy-leaf-executable /path/to/easytier-leaf-worker
  ET_POLICY_LEAF_EXECUTABLE=/path/to/easytier-leaf-worker
```

For the spike, supplying `--policy-config` or `ET_POLICY_PROXY_CONFIG` enables
policy mode. Omitting both disables it completely. Linux also requires the
physical outbound interface. The worker override is for packaging and testing;
normal packages place `easytier-leaf-worker` beside/on the executable path.
The current spike does not yet read `[policy_proxy]` from the ordinary network
TOML and does not expose policy mode through RPC, GUI, or mobile launchers.

Final launch surfaces after the public envelope is implemented:

```text
TOML:
  [policy_proxy]
  enabled = true
  config_file = "policy/default.yaml"

RPC/GUI/Android:
  enabled plus exactly one of config_file or config_inline
```

CLI/environment overrides TOML. `enabled=false` is authoritative even if a
stored path or inline document remains, so users can toggle policy mode without
losing their configuration. Conflicting path and inline sources are rejected
before TUN replacement. Desktop GUI uses a file picker/editor; sandboxed mobile
launchers store and pass `config_inline` because arbitrary host paths are not
portable. All surfaces validate the same EasyTier-owned YAML before starting
Leaf.

### Cross-platform startup

Use the same abstract `PlatformPolicyTun` contract on every supported target:

| Platform | Platform owner | Required bypass |
| --- | --- | --- |
| Linux | existing TUN/netns backend | socket mark + policy rule or verified physical-interface binding |
| Windows | Wintun/service backend | physical interface/route or WFP-compatible socket bypass |
| macOS | utun/Network Extension backend | NE flow/socket ownership and route exclusion |
| Android | existing VpnService | EasyTier service package/UID exclusion and/or `protect(fd)` |
| iOS/OHOS/other mobile | platform VPN extension | feature remains compile-gated until sole-TUN ownership and socket bypass pass the same tests |

Startup sequence:

1. parse and validate the envelope, policy YAML, local GeoIP/Geosite snapshots,
   group graph, UDP compatibility, and resource limits;
2. start/retain the normal EasyTier control plane and build the initial immutable
   mesh-route ownership snapshot;
3. verify native socket bypass for every enabled underlay transport and Leaf
   native actor;
4. create PolicyTunMux as the sole platform TUN owner; mesh packets are enabled
   immediately, non-mesh packets remain in a bounded REJECT/not-ready state;
5. build LeafRuntime off-path and run its local readiness checks;
6. atomically publish Leaf as the non-mesh consumer and report policy Ready.

If steps 3-6 fail, mesh remains functional and non-mesh policy traffic is
blocked with a precise status. Do not tear down the mesh or fall open.

Disabling policy mode drains/cancels Leaf, rebuilds the normal direct
VirtualNic-to-TUN ownership, and restores existing EasyTier behavior. Local
flows may restart during this explicit mode switch; mesh control-plane state is
not restarted.

### Policy document and user editing

The stable EasyTier-owned YAML contains only the concepts users need:

```yaml
version: 1

rule-sets:
  geosite:
    type: geosite
    path: rules/site.dat
    update: manual
  geoip:
    type: mmdb
    path: rules/geo.mmdb
    update: manual

proxies:
  mesh-hk:
    type: socks5
    server:
      instance-id: "11111111-1111-1111-1111-111111111111"
      virtual-ip: 10.44.0.8
    port: 1080
    via: mesh
    udp: true

  firewall:
    type: http
    server: 192.168.50.1
    port: 3128
    via: native

groups:
  overseas-chain:
    type: chain
    members: [mesh-hk, firewall]

  final-tcp:
    type: fallback
    members: [overseas-chain, mesh-hk]

  final-udp:
    type: fallback
    members: [mesh-hk, DIRECT]

rules:
  - "EXTERNAL,site:cn,DIRECT"
  - "GEOIP,CN,DIRECT"
  - "NETWORK,udp,final-udp"
  - "MATCH,final-tcp"
```

Editing UX:

- GUI provides simple pages for rule data, proxies, groups, and ordered rules;
- peer picker shows hostname/current IP but stores stable `instance-id` plus the
  current `virtual-ip` guard when available;
- advanced users edit the same YAML in a syntax-highlighted editor;
- every edit has Validate, Diff, Apply, and Roll Back actions;
- desktop watches the local file with a 500 ms debounce; GUI/mobile applies an
  explicit saved revision through RPC;
- expose generated/normalized YAML so GUI edits are not hidden proprietary
  state;
- unsupported Leaf-native fields are rejected rather than silently discarded.

### Transactional apply and automatic recovery

Every policy revision has an ID and content digest. Applying a revision:

1. parse and validate without touching the active runtime;
2. validate local rule-set files and build bounded immutable indexes;
3. resolve mesh instance/IP selectors and standard proxy references;
4. topologically sort groups and reject cycles/incompatible UDP chains;
5. construct and readiness-test a candidate Leaf runtime;
6. atomically swap the candidate;
7. drain the old runtime for a bounded interval, then cancel it;
8. retain the current and previous validated revisions/snapshots.

Invalid YAML, missing references, damaged GeoIP/Geosite files, failed readiness,
or an interrupted write leaves the last valid runtime active. If no valid
runtime has ever existed, mesh stays active and non-mesh traffic remains
fail-closed.

Runtime recovery:

- one member fails while another succeeds: per-connection fallback, then
  conservative member degradation;
- all members fail or platform network changes: enter Outage, freeze preference,
  and do not churn through nodes;
- network returns: probe the original preferred member first;
- existing TCP streams/UDP associations never migrate or replay; new sessions
  use the recovered schedule;
- Leaf task/runtime failure gets at most three supervised restarts with bounded
  backoff. After exhaustion it enters Dormant; mesh remains active and non-mesh
  remains blocked until a meaningful event/manual retry.

### Power and storm budgets

- health checks run only for groups referenced by active rules and with recent
  traffic;
- healthy interval defaults to 60 seconds; degraded half-open interval defaults
  to 30 seconds; suspend checks after 15 minutes without group activity;
- platform network callbacks are event-driven; do not poll connectivity while
  offline;
- one global recovery probe and one attempt per group/actor maximum;
- retries share one generation budget and enter Dormant after exhaustion;
- add jitter to every periodic/half-open probe;
- bound TCP connection attempts, UDP sessions, per-source sessions, DNS cache,
  rule index, breaker entries, channels, and waiting callers;
- connection-count, UDP packet-rate, and amplification signals trip only the
  affected chain/upstream breaker;
- no automatic GeoIP/Geosite downloads, no background subscription refresh by
  default, and no health probes for disabled/unreferenced groups;
- Android honors Doze/network callbacks and performs no wakeup-only periodic
  work after entering Dormant.

The acceptance report must measure disabled overhead, enabled mesh-path
overhead, idle battery wakeups, active voice-call jitter/loss, CPU/RSS, task/FD
counts, and recovery after network switching and long offline periods.

## Product Boundary

Policy proxying is an optional optimization, not a replacement EasyTier mode.
Enabling it must preserve every existing EasyTier virtual-network behavior:

- virtual IPv4 and IPv6;
- inbound and outbound TCP/UDP;
- ICMP/ICMPv6 and other L3 protocols;
- subnet proxy and exit-node routes;
- Magic DNS and public/managed IPv6;
- broadcast and multicast behavior currently supported by VirtualNic;
- OSPF, PeerConn selection, Stealth, Proxy Failover, listeners, and ACLs.

Therefore the reduced “Leaf owns TUN while EasyTier runs no-TUN” design is
rejected. It loses mesh IPv6, ICMP, non-TCP/UDP protocols, and transparent
inbound L3 behavior.

The minimum complete architecture is a narrow TUN-boundary packet owner:

```text
                         +-> unchanged EasyTier VirtualNic L3 path
system TUN -> PolicyTunMux
                         +-> Leaf transparent policy path

EasyTier L3 output ------+
                         +-> one serialized system TUN writer
Leaf output -------------+
```

`PolicyTunMux` is not a policy/rule engine. It performs one immutable destination
classification and transfers packet ownership without cloning payloads. Leaf
alone owns domain rules, GeoIP/Geosite, DNS, chains, and fallback.

When policy proxying is disabled, construction uses the existing direct
VirtualNic-to-TUN path. No mux, Leaf runtime, DNS task, rule index, proxy session,
or health checker exists.

The existing TCP/UDP data-plane API remains useful for Leaf proxy actors that
explicitly connect to a standard proxy endpoint through the mesh, but it cannot
replace the VirtualNic L3 path.

## Alternative Core Survey

No surveyed project currently satisfies all requirements while also producing
a smaller EasyTier integration:

| Candidate | Maturity | TUN/mobile | Rules/DNS/groups | EasyTier integration |
| --- | --- | --- | --- | --- |
| Mihomo | high | complete | complete | largest binary/dependency; still needs single-TUN ownership integration |
| sing-box/libbox | high | complete | complete, versioned rule sets | mature black box but not materially smaller; same single-TUN/L3 coexistence problem |
| Leaf | medium | accepts Android TUN FD | useful but less complete than Mihomo/sing-box | smallest plausible in-process Rust core; can wrap the existing EasyTier data-plane API |
| meow-rs | young | no TUN inbound currently | Mihomo-like rules/groups/DNS | compact and modular but requires a second tun2proxy component and more lifecycle glue |
| tun2proxy | high for TUN conversion | complete TUN FD support | no geosite/groups/chains | small, but solves only packet-to-SOCKS conversion |
| shadowsocks-rust | high for SS | no complete policy TUN engine | ACL/load balancing, not Mihomo policy semantics | insufficient by itself |

The core problem is not proxy-protocol implementation. On Android there is one
VpnService/TUN while EasyTier must preserve full mesh L3 behavior and the policy
engine must transparently intercept other traffic. Any embedded core therefore
needs either a single-owner packet mux or must replace EasyTier's TUN ownership
and accept loss/reimplementation of mesh L3 behavior.

Practical conclusions:

- For desktop/server-only deployment, an external Mihomo/sing-box/Leaf process
  is the smallest EasyTier change because OS routing can connect the components.
- For Android with full mesh L3, replacing Leaf with sing-box or Mihomo does not
  remove the required TUN integration; it mainly exchanges binary size and
  maturity.
- If maturity is the priority, use sing-box/libbox or Mihomo as a separately
  versioned companion and accept its size.
- If code size and Rust integration are the priority, Leaf remains the smallest
  candidate, but requires a focused spike and carries more maintenance risk.
- meow-rs should be revisited only after it has a production TUN inbound and a
  longer compatibility/security record.

## Goal

Provide an optional policy proxy for users who need domain-aware routing,
logical proxy chains, preferred exits, failover, or load balancing without
changing EasyTier OSPF costs or forcing a physical overlay path.

Transparent full-device interception is a required end state because most
applications cannot be configured with explicit proxy settings. The policy
proxy does not depend on the existing public EasyTier SOCKS feature.

The proxy chain expresses the user's preferred traffic direction. Each segment
is still transported by the existing EasyTier mesh and may use any route and
PeerConn selected by the current OSPF and transport logic.

## Current Candidate

Leaf is currently the strongest single-library candidate because it already
provides:

- an Android-compatible existing TUN file descriptor input;
- TCP and UDP dispatch;
- rule routing, DNS/fake-DNS, chain, select, failover, and try-all groups;
- Android `VpnService.protect(fd)` integration;
- public external TCP and UDP outbound handler interfaces;
- an Apache-2.0 license.

This remains a candidate, not an accepted production dependency. Pinning,
dependency auditing, lifecycle testing, and real-device validation are required
before integration.

## Required Architecture Boundary

Leaf must be an optional policy plane. It must not become part of the default
EasyTier forwarding path.

```text
Default mode:
  application -> existing EasyTier data plane

Optional policy proxy:
  application/TUN -> PolicyTunMux
                  -> mesh-owned L3: unchanged VirtualNic
                  -> non-mesh policy traffic: Leaf rules/groups
                     -> standard SOCKS/HTTP/DIRECT actors
                     -> optional existing EasyTier data-plane API for `via: mesh`
```

Fixed boundaries:

- Do not modify OSPF route calculation or encode proxy-chain state in routes.
- Do not require a physical packet path through a listed peer. A chain is a
  logical sequence of proxy endpoints selected by the user.
- Do not modify Stealth, Proxy Failover, PeerConn protocol selection, or the
  existing SOCKS behavior for users who do not enable this feature.
- Do not start a Leaf runtime, DNS server, health checker, cache, or background
  task when the optional proxy is disabled.
- A Leaf failure must not stop the EasyTier mesh.
- Prefer a separate crate/module and compile-time feature, for example
  `leaf-policy-proxy`, instead of adding Leaf types to PeerManager.

### Isolation and regression budget

The feature must have three independently inactive states:

1. not compiled: no Leaf adapter dependency or binary-size impact;
2. compiled but disabled: no runtime, listener, task, table, timer, packet
   filter, or TUN mux;
3. proxy-server-only: an optional standard Leaf SOCKS5 inbound may run on an
   exit node; no transparent TUN/netstack is required there.

Do not insert Leaf rule matching, health checks, or proxy accounting into the
ordinary PeerManager packet hot path. The adapter wraps the existing
`DataPlaneTcpStream`/`DataPlaneUdpSocket` API and adds no new peer wire protocol.

Required regression gates:

- compiled-but-disabled throughput, p99 RTT, CPU, RSS, allocations, tasks, and
  file descriptors remain within baseline noise;
- proxy-server-only idle cost is bounded and has no periodic network probes unless a
  configured client group requests health checking;
- ordinary mesh traffic bypasses Leaf even when local transparent proxying is
  enabled;
- a saturated policy relay cannot consume the queues or task budget used by
  ordinary EasyTier control/data traffic;
- stopping or crashing the policy module cannot stop PeerManager, OSPF,
  Stealth, listeners, or existing non-policy connections.

## Integration Model

Leaf owns transparent interception, rule selection, standard proxy-chain
composition, failover, load balancing, and UDP session lifecycle. EasyTier does
not add a new relay protocol or dialer API. A small Leaf outbound adapter wraps
the existing `data_plane_tcp_connect()` and `data_plane_udp_bind()` methods to
reach a standard proxy endpoint on an EasyTier virtual address/reachable
subnet. Existing OSPF/PeerConn routing is unchanged, and no system socket to a
virtual peer IP is created.

```text
Example logical chain:

Leaf -> SOCKS5 on mesh peer A -> firewall HTTP/SOCKS -> destination
```

The first proxy may be a Leaf SOCKS5 inbound running on a peer, the existing
EasyTier SOCKS portal for TCP-only use, or any separately managed proxy server.
The final actor may be a peer, firewall HTTP/SOCKS proxy, commercial proxy, or
native DIRECT. Leaf uses its existing standard protocol implementations for all
nesting.

Consequences:

- no new EasyTier wire/protobuf capability or remote service is required;
- the chain may end at a non-peer HTTP/SOCKS endpoint;
- proxy protocol UDP support is determined by Leaf and the configured standard
  actors, not by an EasyTier peer capability;
- an HTTP CONNECT actor is naturally TCP-only; a SOCKS5 actor with UDP
  ASSOCIATE can carry UDP; rules must not send voice UDP into a TCP-only chain;
- KCP, smoltcp, QUIC, UDP, TCP, FakeTCP, and relay remain transport details
  below the existing data-plane API and are not exposed in policy configuration;
- existing EasyTier `--socks` remains unchanged. Its currently disabled and
  insufficiently bound UDP ASSOCIATE implementation must not be enabled as a
  shortcut; use Leaf or another mature SOCKS5 server when UDP relay is needed.

Leaf's existing external plugin loader uses a dynamic-library/FFI boundary.
Do not use that unsafe loader inside the EasyTier process. Prefer a small
compile-time integration that directly constructs Leaf's public outbound
handler traits, or contribute a safe in-process handler registration API
upstream. Keep any required Leaf patch isolated and pinned.

The adapter selects the mesh as transport to a configured proxy endpoint. It
does not select a physical OSPF path or underlay protocol.

### Transparent policy mode

`PolicyTunMux` is the sole owner of the Android `VpnService` TUN FD. It exposes
two in-process packet endpoints: the existing VirtualNic side and Leaf's TUN
backend. Both outputs share one serialized writer. Neither consumer receives or
duplicates the raw platform FD.

Inbound packet classification order is fixed:

1. destination matches current EasyTier virtual IPv4/IPv6, exact peer address,
   advertised subnet route, public/managed IPv6 route, Magic DNS address, or
   other VirtualNic-owned prefix -> unchanged EasyTier L3 path;
2. all remaining application traffic -> Leaf, which applies domain/IP rules,
   special-range defaults, DNS, proxy chains, fallback, native DIRECT, or
   REJECT.

Mesh-route snapshots are immutable and atomically replaced from existing route
events. Longest-prefix mesh ownership wins before generic special-range rules,
so an intentionally advertised RFC1918 subnet is not accidentally forced to
local DIRECT.

Leaf installs explicit highest-priority built-in rules for common special
ranges. Initial categories include loopback, IPv4/IPv6 link-local,
limited/directed broadcast, multicast, unspecified/documentation ranges,
RFC1918, CGNAT, and ULA. More-specific EasyTier ownership is already removed by
the mux first, so an advertised private subnet still uses the mesh; unclaimed
private/LAN ranges may use local DIRECT. FakeDNS pools such as
`198.18.0.0/15` remain Leaf-owned and must not be blanket-DIRECTed as generic
benchmark space.

EasyTier underlay/listener/STUN/manual/bootstrap sockets do not depend on packet
destination exclusions. They use socket-level VPN protect/bypass at creation.
Application traffic to the same public IP remains eligible for Leaf policy;
globally excluding every advertised peer endpoint could leak unrelated traffic
on shared/CDN addresses.

### Underlay loop prevention

Do not claim safety from either endpoint exclusions or socket protection alone.
Use layered enforcement:

1. **Primary ownership bypass**: all EasyTier underlay/control sockets bypass
   the policy TUN by process/UID/package exclusion or a verified per-socket
   platform protector before bind/connect. This includes TCP, UDP, QUIC, WG,
   WS/WSS, FakeTCP where supported, STUN, hole-punch, manual/bootstrap, public
   server, DNS/bootstrap resolution, and reconnect sockets.
2. **Explicit Leaf transport**: native proxy actors use the same platform
   protector; `via: mesh` actors use the existing EasyTier data-plane API and
   never open a system socket to a virtual peer address.
3. **Dynamic endpoint audit set**: maintain the currently resolved/advertised
   peer, listener, STUN, bootstrap, and public-server endpoint IPs with bounded
   TTL. Use the set to validate socket protection coverage, detect an underlay
   packet unexpectedly entering Leaf/TUN, emit structured diagnostics, and
   trip a local circuit breaker.
4. **Fail closed on missing coverage**: policy mode may start only after the
   platform bypass implementation passes a startup probe for every enabled
   underlay transport. Unsupported transports are disabled for that mode or the
   mode fails with a precise error.

The dynamic endpoint set is not a default global DIRECT rule. Destination-only
exclusion cannot distinguish EasyTier underlay traffic from an application's
legitimate request to the same shared/CDN IP, and DNS/QUIC endpoint changes can
make such rules stale. A platform that lacks reliable process/socket bypass may
offer global endpoint bypass only as an explicit compatibility fallback with a
traffic-leak warning; it is not the normal design.

Platform expectations:

- Android: exclude/protect the EasyTier service package/UID and verify every
  native fd before use; application traffic from other UIDs still enters the
  VPN. Mesh proxy connections use the data-plane API.
- Linux: use a dedicated mark plus policy rule or bind-to-device/netns strategy;
  verify the mark survives every connector type.
- Windows: use the selected physical interface/route or WFP-compatible bypass
  at socket creation; do not depend only on destination routes.
- macOS/iOS: use Network Extension flow/socket ownership and route exclusion
  capabilities available to the containing app; fail closed where the API
  cannot provide a reliable bypass.

Proxy actors marked `via: mesh` use the existing data-plane API. Actors marked
`via: native` use platform-protected native sockets. System sockets to virtual
peer addresses are forbidden.

Users that do not enable policy mode retain the current direct VirtualNic/TUN
construction. Switching the optional feature rebuilds local TUN ownership and
restarts local flows, but does not restart or reconfigure the mesh control
plane.

## UDP Capability And Lifecycle

Leaf already includes a bidirectional UDP NAT/session manager:

- sessions are keyed by datagram source;
- uplink and downlink activity refresh the session timestamp;
- default idle timeout is 30 seconds;
- cleanup runs every 10 seconds;
- cleanup aborts the downlink task and closes the outbound send half;
- each session uses a bounded uplink channel (currently 256 entries);
- chain, select, failover, and try-all have datagram implementations;
- failover includes UDP health-check support.

Active voice calls should therefore remain alive while packets continue to
flow. UDP lifecycle management does not need to be reimplemented.

### Missing resource limits

Leaf's current NAT session registry is a plain `HashMap` without a fixed global
capacity. Before production use, add a small safety wrapper or a narrowly
scoped upstream patch:

- maximum global UDP sessions, initial proposal: 4096;
- maximum sessions per source IP, initial proposal: 256;
- bounded new-session token bucket;
- reject new sessions at capacity; never evict an active call to admit a new
  session;
- explicit cancellation of all session tasks when the optional proxy stops;
- bounded DNS cache and breaker state;
- counters for active sessions, rejected sessions, cleanup, and task shutdown.

The exact limits must be configurable internally during the spike and selected
from real-device memory and call-concurrency measurements. They are not public
configuration in the first version.

### UDP chain semantics

- UDP support belongs to each standard proxy actor, not to an EasyTier peer.
- SOCKS5 actors may support UDP ASSOCIATE; HTTP CONNECT actors are normally
  TCP-only. Leaf validates the complete chain before assigning UDP traffic.
- UDP policy traffic should remain datagram-native across UDP-capable actors.
  Do not silently carry voice UDP over a reliable stream.
- A TCP-only chain remains available for TCP rules and is excluded from UDP
  groups rather than making the underlying EasyTier peer unavailable.
- Failover normally selects an outbound when the UDP association is created.
  It cannot guarantee seamless migration of an established call because the
  public source address and NAT mapping may change.
- Existing calls are not migrated in the first version. Application ICE or its
  own reconnect logic handles recovery; new sessions use the recovered group.

## Loop Prevention And Circuit Breaking

The design assumes users choose sensible logical chains, but it must contain
configuration mistakes and runtime loops.

### Startup validation

Reject only high-confidence loops:

- a local Leaf inbound used as its own upstream;
- the same endpoint repeated in a chain where it creates direct recursion;
- an upstream resolving to the same local listening address and port;
- a peer outbound targeting the policy proxy's own inbound service.

Do not reject a chain merely because OSPF may traverse a peer more than once;
the policy layer does not own the physical mesh path.

### Runtime protection

Use bounded per-chain/per-upstream state, not a single global threshold.

Initial trigger proposal:

- more than 1000 new connections in one second; and
- failure ratio above 90% or average lifetime below 200 ms; and
- the condition persists for two consecutive one-second windows.

UDP needs separate signals because a loop may reuse a session:

- new UDP sessions per second;
- datagrams per second per chain/upstream;
- ingress-to-egress packet and byte amplification ratio;
- repeated same source/destination tuples;
- queue-full and session-cap rejection rates.

On a high-confidence trigger:

- suspend only the affected chain/upstream for 30 seconds;
- allow failover to the next healthy upstream;
- do not stop EasyTier or modify OSPF;
- use a single half-open probe after the suspension;
- keep breaker state bounded and expire inactive entries;
- emit a structured warning with chain, upstream, signal, rates, and TTL.

Thresholds must not fire from connection count alone because browsers,
downloaders, games, or benchmarks can legitimately exceed 1000 connections per
second.

### Chain depth and loop rules

- There is no semantic two-hop limit. Leaf's chain handler iterates an actor
  list and can represent more than two standard proxy hops.
- Configuration references must form an acyclic graph. Direct and indirect
  self-reference, repeated recursive groups, and a proxy endpoint routed back
  into its own inbound are rejected before startup.
- Production parsing still needs a generous resource-safety ceiling (initial
  proposal: 32 expanded actors per chain and 64 nested group references). This
  is not a routing limitation; it prevents malicious or accidental exponential
  expansion during configuration loading.
- Runtime breaker keys include the expanded chain and current actor, so a loop
  suspends only the affected path and can fail over to another group member.

### Nested groups and stable fallback

Leaf represents chain, failover, select, and other groups as ordinary outbound
handlers. A fallback group may therefore contain multiple chain groups, direct
proxy actors, or another validated group. The EasyTier-owned config loader must
topologically sort the dependency graph before constructing Leaf handlers; do
not rely on Leaf's current fixed number of repeated load passes for arbitrary
nested depth.

Example:

```yaml
groups:
  chain-a:
    type: chain
    members: [mesh-socks-a, firewall-socks]

  chain-b:
    type: chain
    members: [mesh-socks-b, external-socks]

  final-tcp:
    type: fallback
    members: [chain-a, chain-b, mesh-socks-a]

  final-udp:
    type: fallback
    members: [chain-b, mesh-socks-b]
```

The final UDP group may use a preferred complete UDP-capable chain and a simple
mesh SOCKS5 endpoint as last resort. Every expanded member of `final-udp` must
support datagrams; a TCP-only chain is excluded at validation time.

Leaf's current failover implementation periodically probes all members and
sorts the schedule by measured RTT. Defaults include a 300-second check
interval, a 6-second health-check timeout, one attempt, and a 4-second
per-connection fallback timeout. A single check can therefore reorder a path,
and the current implementation has no explicit multi-round recovery hysteresis.

The product behavior should be less sensitive and preference-first:

- configured member order is the primary preference; RTT does not continuously
  reorder healthy members;
- an individual new TCP connection or UDP association may immediately try the
  next member after its bounded connect/setup timeout, without globally marking
  the preferred member unhealthy;
- globally degrade a member only after three consecutive failed health rounds;
- recover/upgrade only after three consecutive successful health rounds and a
  minimum 30-second hold-down;
- healthy checks run every 30 seconds while the group has recent activity;
  degraded members may be probed every 10 seconds with one half-open probe;
- add 0-500 ms jitter so nodes do not probe simultaneously;
- existing TCP streams and UDP associations never migrate. Only new sessions
  use the updated schedule;
- short interruption during application reconnect is accepted. Avoid duplicating
  packets or replaying an already-established connection on another member.

TCP and UDP maintain separate health state because an HTTP chain can be healthy
for TCP while having no UDP capability. Health targets must be configurable;
do not hard-code public Google or Cloudflare endpoints as the sole evidence of
path health.

### Local network outage and network-generation handling

Failover must distinguish a member-specific failure from loss/change of the
local network. Entering an elevator, switching Wi-Fi/mobile data, changing the
default route, or temporarily losing all connectivity must not mark every proxy
member unhealthy or rotate through the entire group.

Maintain a local `network_generation` supplied by platform connectivity/route
events. When generation changes or the platform reports no validated network:

- enter group state `Outage` for an initial 3-second grace period;
- cancel/ignore health results and connection attempts created under the old
  generation;
- do not increment member failure counters;
- preserve configured order, current preferred member, and previous health;
- do not walk every fallback member for each new application connection;
- keep existing streams/UDP associations until their normal owners close them;
- rebind/protect native sockets for the new platform network before probing.

Consensus failure also enters `Outage`: when all previously usable members fail
in the same health window and there is no independent evidence that local
connectivity is healthy, freeze member state instead of degrading all members.

During `Outage`:

- new policy connections fail fast or wait behind one bounded readiness gate;
  they must not create a retry storm;
- run at most one jittered recovery probe for the group, with backoff
  `1s -> 2s -> 5s -> 10s`;
- probe the original preferred member first after connectivity returns;
- leave `Outage` only after the platform network is usable and a probe succeeds;
- restoring the original preferred member does not count as a failback switch.

### Retry budgets and dormant state

Backoff alone is insufficient because a capped periodic retry still runs
forever. Every outage generation has a fixed recovery budget:

- at most one in-flight recovery probe globally per policy runtime;
- at most one in-flight setup attempt per group and actor;
- attempts use jittered backoff `1s, 2s, 5s, 10s, 30s, 60s`;
- no more than 12 automatic probes or 10 minutes of probing per
  `network_generation`, whichever is reached first;
- application retries share the same gate and budget. They do not each start a
  new fallback walk;
- when the budget is exhausted, enter `Dormant` and stop all background network
  probes and timers for that outage.

`Dormant` is left only by a meaningful event:

- platform `network_generation` changes;
- policy/configuration changes;
- the user explicitly requests a retry;
- after at least 5 minutes, new application demand may permit exactly one
  half-open probe. Further demand is coalesced behind that probe and cannot
  increase its frequency.

All old-generation futures are cancelled and their late results ignored. New
application connections fail fast while Dormant unless they own the permitted
half-open probe. Counters and wait queues are bounded, and no failure path may
spawn an unowned retry task.

After recovery, switch away from the original preferred member only when there
is positive differential evidence: another member succeeds while the preferred
member reaches its normal three-round failure threshold. If all members still
fail together, remain in `Outage` and do not churn.

Do not rely on one public connectivity URL as the sole global-outage signal.
Combine platform network state, network generation, route/source availability,
and group-wide probe consensus. A failed captive-portal or censored public URL
must not by itself classify the device as offline.

## Configuration And User Scope

- Disabled by default.
- Ordinary mesh, SOCKS, exit-node, and subnet-proxy users see no new runtime
  component.
- First public configuration should be a separate policy-proxy section/file,
  not additions to OSPF or listener URLs.
- The policy config refers to stable peer instance IDs and/or exact virtual IPs,
  plus Leaf groups/rules. It does not refer to SOCKS endpoints.
- Loading or validating policy configuration must be transactional: invalid
  policy keeps the mesh running and preserves the last valid policy runtime.
- Do not promise full Mihomo configuration compatibility. Leaf has its own
  configuration and rule semantics.

### Configuration layering proposal

Do not expose the complete Leaf runtime configuration as the stable EasyTier
API. Use two layers:

1. a small EasyTier-owned envelope controls enablement, transparent interception,
   failure behavior, resource limits, and the policy document location/content;
2. a versioned policy document describes peer exits, groups/chains, and rules.

The envelope remains stable if Leaf is upgraded or replaced. The policy schema
may initially reuse a documented subset of Leaf semantics, but unsupported Leaf
fields must be rejected rather than silently ignored.

Initial envelope draft:

```toml
[policy_proxy]
enabled = false
mode = "rule"                 # rule | global
fail_closed = true
policy_file = "policy.yaml"   # desktop/server
# policy_inline = "..."       # GUI/mobile/RPC representation
```

An exit node does not need a new EasyTier server mode. It runs any standard
SOCKS5/HTTP proxy selected by the user. Optionally, a later packaging phase may
offer Leaf's standard SOCKS5 inbound as a convenience, without changing the
EasyTier peer protocol.

Rules:

- absence of `[policy_proxy]` is exactly equivalent to current EasyTier;
- `enabled=false` creates no Leaf runtime, TUN mux, DNS task, health checker, or
  session table;
- transparent interception is intrinsic to enabled mobile/full-device mode and
  is not confused with the existing EasyTier SOCKS portal;
- `fail_closed=true` is the safe default: policy-engine failure blocks policy
  traffic while mesh traffic remains available;
- `policy_file` and `policy_inline` are mutually exclusive;
- relative files resolve from the main EasyTier config directory, never the
  current working directory;
- GUI/RPC stores the same policy document as text/blob without translating the
  entire Leaf schema into protobuf fields in the first version.

Initial policy document draft:

```yaml
version: 1

proxies:
  peer-socks:
    type: socks5
    server:
      virtual-ip: "10.44.0.8"
      instance-id: "11111111-1111-1111-1111-111111111111"
    port: 1080
    via: mesh
    udp: true

  firewall-http:
    type: http
    server: "192.168.50.1"
    port: 3128
    via: native

groups:
  asia-primary:
    type: fallback
    members: [peer-socks, firewall-http]
    health-check:
      url: https://cp.cloudflare.com/generate_204
      interval: 30s
      timeout: 3s

  chained-egress:
    type: chain
    members: [peer-socks, firewall-http]

rules:
  - GEOSITE,cn,DIRECT
  - GEOIP,private,DIRECT
  - DOMAIN-SUFFIX,example.com,chained-egress
  - MATCH,asia-primary
```

### DNS behavior

Leaf already provides the minimum DNS mechanisms needed for rule-based
transparent proxying:

- FakeDNS include/exclude mode for retaining the original domain behind TUN
  connections;
- DNS response sniffing and IP-to-domain association;
- domain, domain-suffix, domain-keyword, external `site.dat` geosite, and MMDB
  GEOIP rules;
- `direct:` DNS resolvers for destinations whose routing result is DIRECT;
- normal proxied resolvers for destinations whose routing result is a proxy
  group;
- UDP DNS, system resolver, and DoH resolver support;
- multiple-server selection and fallback.

This is sufficient for the first version's intelligent split:

```yaml
dns:
  fake-ip: true
  direct:
    - "direct:system"
    - "direct:223.5.5.5"
  proxy:
    - "doh:dns.cloudflare.com@1.1.1.1"
```

Routing is decided from the queried domain first. DIRECT domains use the direct
resolver set; proxied domains use the normal/proxied resolver set. FakeDNS/DNS
sniffing preserves the domain when the application later connects to an IP.

Leaf does not currently provide the full Mihomo/sing-box per-domain
`nameserver-policy` and resolver-rule feature set. Do not claim complete config
compatibility. A future DNS-policy extension is justified only if the two-set
DIRECT/proxy model cannot express a real deployment.

Leaf's current FakeDNS allocator is IPv4-only (`198.18.0.0/15`). It recognizes
AAAA queries but does not allocate or return an IPv6 fake address. Full
dual-stack transparent mode must therefore choose and test one explicit policy:

1. keep IPv4 FakeDNS, return real AAAA records, and rely on DNS sniffing for
   IPv6 domain recovery; or
2. add a bounded IPv6 FakeDNS allocator and reverse map as an isolated Leaf
   patch.

Do not enable IPv4-only FakeDNS and claim complete dual-stack domain-rule
coverage without this validation.

DNS is independent from the application's transport protocol. If a selected
application chain is TCP-only (for example because it contains HTTP CONNECT),
DNS may still use DoH through that TCP chain. Raw UDP DNS must instead use a
UDP-capable chain or the configured direct resolver set.

### Geosite data ownership and updates

Geosite data is local configuration data, not an implicit online service.
EasyTier/Leaf must never force-download or auto-refresh it from a hard-coded
URL.

Supported sources:

```yaml
rule-sets:
  geosite:
    type: geosite
    path: "rules/site.dat"
    update: manual
    sha256: "optional expected digest"
```

- Releases may bundle a documented, versioned default `site.dat` snapshot.
- Desktop/server users may point to a local absolute or config-relative file.
- GUI/mobile users may import a file or explicitly request a one-time URL
  download.
- Optional subscriptions are opt-in and store their URL, interval, expected
  signature/hash policy, and last successful version. They are not enabled by
  merely referencing a geosite category.
- Startup and policy reload use the last validated local snapshot and never
  block waiting for the network.
- Download/import writes to a staging file, enforces size limits, validates the
  format and optional digest/signature, then atomically replaces the active
  snapshot.
- Keep at least one previous validated snapshot for rollback. Failed updates do
  not invalidate the running policy.
- Report source, digest, data version/time, and last update result in status;
  do not silently substitute another online source.
- Load only referenced geosite groups into a bounded immutable index and swap
  it atomically. Do not repeatedly scan the complete file on every connection.

### TCP-only final actors and UDP fallback

- `DIRECT` as the final action supports native TCP and UDP.
- A final standard SOCKS5 actor supports UDP only when its UDP ASSOCIATE path is
  enabled and validated.
- A final HTTP CONNECT actor is TCP-only unless that specific implementation
  explicitly provides a compatible datagram extension.
- UDP traffic never silently enters a TCP-only chain. Each UDP rule/group must
  define one of: a UDP-capable fallback group, native DIRECT, or explicit
  REJECT.
- A recommended policy uses separate `web` and `voice` groups even when they
  share some endpoints.

Stable EasyTier semantics:

- proxy actors are standard Leaf SOCKS5/HTTP/etc. actors rather than a new
  EasyTier-specific remote protocol;
- `via: mesh` uses the existing EasyTier TCP/UDP data-plane API; `via: native`
  uses a protected system socket. `auto` is not allowed in v1 because an
  ambiguous route can create a TUN loop;
- `RoutePeerInfo.inst_id` is the canonical persistent identity. Runtime
  `PeerId` is randomly generated when PeerManager starts and must never be
  stored as the durable policy identity;
- mesh proxy servers accept either `instance-id` or exact `virtual-ip`:
  - fixed/manual-IP deployments may use `virtual-ip`;
  - DHCP deployments should use `instance-id`, which is resolved to the
    peer's current runtime PeerId and virtual IP whenever routes change;
  - when both are present, they must resolve to the same peer during initial
    validation. `instance-id` remains the identity and `virtual-ip` is an
    expected-address guard; a later DHCP address change follows the instance
    and emits one informational event rather than disabling the exit;
  - a `virtual-ip`-only selector intentionally follows whichever peer currently
    owns that address. Because a DHCP address can later be reused by another
    instance, startup emits one identity-unpinned warning;
  - when the GUI selects an online peer, it stores both `instance-id` and the
    current `virtual-ip` by default. Advanced/manual configuration may keep only
    one selector;
  - an offline `instance-id` remains a valid but unavailable exit. It does not
    make the policy document invalid and groups may use another healthy member;
  - if both selectors later resolve to different simultaneously visible peers,
    disable only that exit and report an identity mismatch. Never silently pick
    one of the two peers;
- runtime `peer-id` may be exposed in status/debug output, but is not accepted
  as the only persistent selector because it changes when PeerManager restarts;
- hostname alone is not a valid identity because it is not guaranteed unique;
- `DIRECT` means protected native Internet access from the local device, not
  EasyTier OSPF forwarding;
- mesh TCP/UDP destinations and mesh-reachable proxy endpoints explicitly use
  `via: mesh`; no packet-level PolicyTunMux exists in the reduced design;
- group names and proxy names must be unique and references are validated before
  runtime replacement;
- chains contain standard supported proxy actors; cycles and
  duplicate recursive endpoints are rejected;
- UDP compatibility is derived from the configured standard proxy protocols
  and validated per selected group/chain at startup and on updates.

First-version transport scope:

- standard SOCKS5, HTTP CONNECT, and DIRECT actors are sufficient for the first
  spike;
- TCP chains may combine compatible SOCKS5 and HTTP actors;
- UDP rules may select only groups/chains whose complete actor sequence supports
  datagrams. HTTP CONNECT normally makes a chain TCP-only;
- Leaf's standard capability validation decides this from actor types. There is
  no EasyTier peer UDP capability or new capability advertisement;
- an incompatible UDP chain is rejected explicitly and never silently changes
  to TCP encapsulation, DIRECT, or the existing EasyTier SOCKS portal.

Configuration application is transactional:

1. parse and schema-validate;
2. resolve mesh instance/IP selectors and standard proxy actors;
3. validate references, cycles, protocol/UDP support, limits, and TUN ownership;
4. build the new runtime off-path;
5. atomically swap only after it is ready;
6. drain the previous runtime for a bounded interval, then cancel it.

An invalid update keeps the previous valid policy active and returns a precise
error. It must not restart the EasyTier instance.

## Spike Gates

Do not merge production integration until all gates pass:

1. Build Leaf as an optional dependency for Linux, macOS, Windows, and Android;
   verify disabled builds do not link or initialize it.
2. Prove the Leaf adapter can wrap existing `DataPlaneTcpStream` and
   `DataPlaneUdpSocket` to reach mesh proxy endpoints without a system socket to
   the virtual IP.
3. Prove PolicyTunMux is the sole platform-FD owner, each packet has exactly one
   consumer, both outputs use one serialized writer, and no payload clone is
   added to the mesh path.
4. Verify policy enabled/disabled parity for virtual IPv4/IPv6, inbound/outbound
   TCP/UDP, ICMP/ICMPv6, subnet proxy, Magic DNS, public IPv6, broadcast, and
   multicast before measuring proxy functionality.
5. Verify a real bidirectional voice/WebRTC-style UDP flow for at least one
   hour, including NAT keepalive and idle/resume behavior.
6. Verify a mesh SOCKS5 endpoint, an external firewall HTTP/SOCKS endpoint, a
   mixed two-hop TCP chain, and a fully UDP-capable SOCKS5 chain; reject UDP
   through an HTTP-only actor.
7. Create more than the session limit and prove memory, tasks, and file
   descriptors remain bounded without disrupting existing calls.
8. Inject a TCP connection storm and UDP datagram amplification loop; verify
   only the affected chain is suspended and the EasyTier mesh remains healthy.
9. Repeatedly start/stop the optional runtime and Android VPN service; verify no
   retained TUN FD, task, DNS listener, socket, session, or callback.
10. Compare call latency, jitter, loss, CPU, RSS, and battery against direct
   EasyTier UDP; existing SOCKS may be measured only as a historical baseline.

## Open Decisions

- Whether to vendor/pin Leaf, consume crates from git, or maintain a minimal
  upstream patch set.
- Whether Leaf will accept a safe in-process outbound registration API so the
  dynamic plugin ABI is unnecessary.
- Which Leaf features can be disabled to reduce package size and attack
  surface.
- Whether geosite-compatible data must be converted during build or loaded in
  Leaf's native rule format.
- The smallest packet-endpoint API that lets existing VirtualNic and Leaf share
  PolicyTunMux without exposing the raw TUN FD or Leaf types to EasyTier core.
- Android `VpnService.protect()` coverage for every native Leaf actor and every
  EasyTier underlay/control socket.
- Appropriate UDP session limits for mobile and desktop profiles.
- Whether UDP failover health checks should probe a neutral endpoint or a
  per-exit user target.

## Non-Goals For The First Version

- No OSPF changes or forced physical path routing.
- No seamless migration of established TCP or UDP sessions.
- No second Android VPN service.
- No implicit enablement for every user.
- No new EasyTier peer wire protocol, no replacement SOCKS implementation, and
  no new core dialer API; the Leaf adapter uses the existing data-plane API.
- No replacement of the existing EasyTier SOCKS, exit-node, or Proxy Failover
  features.
