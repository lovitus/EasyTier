# Leaf Optional Policy Proxy Integration TODO

**Status**: Linux candidate validated; Android single-TUN candidate awaiting packaged build and real-device validation
**Updated**: 2026-07-13

This TODO is the design source of truth. Update it after each material design
discussion so implementation does not depend on chat history.

## Current implementation snapshot

The current implementation is opt-in behind `leaf-policy-proxy`; Android adds
`leaf-policy-mobile` for the in-process runtime. It remains narrower than the
final cross-platform design below:

- an absent or disabled `[policy_proxy]` envelope creates no Leaf process,
  policy task, default route, packet mux, bridge, timer, or session table. The
  Linux process-level `--policy-config` override remains supported;
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
- `PolicyNicContext` is the sole strong owner of the Linux policy-routing RAII
  guard. The periodic route updater holds only a weak reference, so task
  cancellation cannot keep source/mark rules or table-52000 routes alive after
  a graceful core shutdown;
- the pinned Leaf smoltcp backend is supplied through the vendored
  `third_party/netstack-smoltcp` crate. Its local lifecycle patches drain bytes
  already accepted by `AsyncWrite` before requesting FIN, then wake and finish
  AsyncWrite shutdown once FIN is committed. They prevent both fast remote EOF
  data loss and indefinitely retained upstream winner streams;
- KCP encapsulation keeps the existing five-way, 200 ms hedge. Every
  `KcpStream` owns its endpoint state: explicit shutdown uses the existing
  bounded FIN exchange, while drop/cancellation performs synchronous local
  cleanup and best-effort RST so successful hedge losers cannot leave remote
  SOCKS target connections alive. This remains a release blocker until beta
  resource counters return to baseline after KCP load;
- destination-relayed policy UDP exposed a separate throughput limit in the
  existing smoltcp datagram data plane. The legacy relay remains available for
  mixed-version fallback, but the new candidate requests a private,
  token-authenticated UDP-over-TCP stream after the relay RPC. Its request and
  connected-mode framing independently implement SagerNet UoT v2 (`isConnect`,
  SOCKS address, then `u16` big-endian length plus payload) without importing
  Go code. The stream uses the existing `Socks5AutoConnector`: KCP is selected
  when the local endpoint and routed destination capability permit it;
  otherwise the existing smoltcp TCP path remains available. If capability
  says KCP but its connect attempt fails or exceeds five seconds, policy UoT
  alone retries the same private stream endpoint once through smoltcp within
  the existing ten-second setup budget. Ordinary EasyTier SOCKS remains
  KCP-only after selecting KCP. Only failure of both UoT stream paths performs
  one bounded retry through the legacy datagram relay.
  The private TCP listener is per-association, unadvertised, limited by the
  existing global/per-peer association caps, protected by a random 128-bit
  token, and removed on setup timeout, cancellation, control EOF, or idle
  expiry. The destination binds the same ephemeral virtual endpoint in two
  independent TCP stacks: a kernel listener accepts KCP-proxy delivery, while
  a smoltcp data-plane listener accepts capability fallback. The first
  token-authenticated stream wins; this avoids duplicating or overriding the
  existing connector selection logic. This candidate is not accepted until
  unpaced burst, sustained UDP,
  mixed-version, KCP-disabled, cancellation, and resource-baseline tests pass;
- source policy packets enter Leaf through a dedicated fixed-capacity writer
  queue rather than calling the Unix datagram bridge with `try_send()` directly
  from the shared TUN reader. The TUN reader never awaits Leaf, a full queue is
  still dropped fail-closed, and every queued packet retains the exact bridge
  generation so worker replacement cannot replay stale traffic. The capacity
  is 4,096 packets: bounded to normal TUN MTU memory while absorbing scheduler
  bursts that previously produced losses despite substantial CPU headroom;
- UoT writes one complete `u16 length + payload` frame per async write and uses
  persistent 16 KiB readers on both ends. This preserves SagerNet framing while
  avoiding two scheduler/KCP submissions and two underlying reads per ordinary
  MTU-sized datagram. Buffers are reused and remain bounded per association;
- EasyTier-owned loopback UDP sockets at the Leaf/SOCKS bridge and destination
  SOCKS-associate boundary request 4 MiB send/receive buffers and log the
  kernel-granted values. Tuning failure remains compatible but observable;
  third-party final-hop SOCKS servers retain responsibility for their own UDP
  receive capacity;
- multi-datagram UoT coalescing is intentionally excluded. Beta profiling
  showed that it increased burst loss at the final SOCKS UDP actor without
  improving the reliable mesh path; one complete frame per write is the
  validated latency/throughput boundary;

Not yet implemented in this spike: policy file hot reload, HTTP CONNECT actor
adaptation, a bundled exit-node SOCKS5 UDP service, instance-netns worker
ownership, and desktop non-Linux TUN adapters. Proxy credentials for native and
mesh SOCKS actors are now an implementation candidate: native actors emit the
validated credentials into the private Leaf configuration, while mesh TCP and
UDP paths perform RFC 1929 authentication at the destination actor. Empty RPC
fields preserve the existing no-authentication wire behavior. TOML/RPC/GUI
envelopes and the Android single-TUN adapter are also implemented candidates
pending packaged-build and real-device validation.
These are release blockers for claiming the full plan, but they do not affect
ordinary EasyTier builds because the feature is off by default.

The Android candidate currently targets the official Tauri GUI/VpnService
path. The separate `easytier-android-jni` contrib SDK remains feature-disabled:
its host API does not yet carry policy default routes, underlying DNS, or
network-generation callbacks. Enabling the Cargo feature there without that
public host contract would expose a configuration that cannot recover after a
network change.

## Android implementation evidence

The pinned Leaf revision already provides the primitives needed by the final
Android adapter: `Config::Str`, caller-supplied TUN FDs, independently keyed
in-process runtimes, and an Android `VpnService.protect(fd)` callback for
outbound sockets. EasyTier's current mobile path already owns the sole
VpnService TUN FD in `VirtualNic::run_for_mobile()`. Therefore Android policy
mode must not create or hand the platform TUN directly to a second owner.

The implementation boundary is:

- retain the existing EasyTier mobile TUN stream and sink as the sole platform
  owner;
- reuse `PacketClassifier` and the serialized TUN writer used by the Linux
  policy path;
- connect only policy-classified packets to an in-process Leaf runtime through
  the existing packet-preserving Unix datagram bridge;
- compile Leaf from an in-memory generated config and run it on its own bounded
  runtime thread, with a unique runtime ID and explicit shutdown/join;
- exclude the actual runtime Android package name from VpnService capture
  before `Builder.establish()`, which protects both EasyTier underlay and
  in-process Leaf sockets without hard-coded package IDs. A platform variant
  that cannot guarantee package exclusion must instead wire Leaf's
  `VpnService.protect(fd)` callback and fail readiness when protection fails;
- snapshot underlying DNS servers from `ConnectivityManager`/`LinkProperties`
  before `Builder.establish()` and pass that immutable generation into policy
  config compilation;
- observe only `NOT_VPN + INTERNET` networks, coalesce Wi-Fi roam, cellular
  switch, DHCP renewal, route, address, and DNS callbacks for two seconds, and
  restart only the policy runtime when the selected physical-network signature
  changes. The VpnService TUN, EasyTier mesh, OSPF sessions, and ordinary peer
  transports remain untouched;
- follow the Clash Meta Android selection boundary: prefer Wi-Fi, Ethernet,
  USB tethering, Bluetooth tethering, then cellular; mark `onLosing` networks
  as lower priority; and include the Android `Network` handle, interface,
  addresses, routes, transport, and DNS servers in the stable generation key.
  A roam or DHCP renewal that changes any of those properties therefore
  produces one policy rebuild after debounce, while callback bursts that settle
  back to the same generation produce none;
- after a two-second debounce, treat absence of every physical network (for
  example airplane mode or an elevator) as a fail-closed outage generation:
  stop only the policy runtime, keep the TUN and mesh alive, and do not consume
  restart budget or walk all fallbacks. Recovery publishes one usable
  generation and performs one bounded policy rebuild, even if Android reuses
  the same `Network` handle and DHCP/DNS signature;
- use kernel-assigned port `0` for every private policy bridge/listener. Policy
  mode introduces no fixed local listener port that can conflict with another
  VPN application;
- respect Android's single-`VpnService` ownership rule. If another VPN takes
  ownership, `onRevoke()` closes this generation and EasyTier does not fight it
  with an automatic restart loop. A sticky restart without the original
  configuration is rejected instead of creating a default/incorrect TUN;
- on Linux, reject loopback/stub DNS endpoints such as `127.0.0.53` for the
  physical-interface-bound Leaf worker. Resolver discovery checks
  `/etc/resolv.conf`, then the systemd-resolved and NetworkManager non-stub
  files; if none contains a directly usable resolver, policy startup fails
  closed instead of entering a DNS loop;
- add IPv4/IPv6 default VpnService routes only when policy mode is enabled.
  Policy-disabled mobile startup remains byte-for-byte equivalent at the
  routing boundary.

Leaf startup is blocking and only becomes externally cancellable after its
runtime ID is registered. The wrapper must therefore validate the generated
configuration first, bound readiness waiting, transfer the bridge FD exactly
once, and never publish the policy bridge until `leaf::is_running(id)` is true.
Startup failure keeps mesh forwarding active and non-mesh traffic fail-closed.

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
config_file = "policy/default.yaml"
# config_inline = "..."        # GUI/RPC/mobile alternative
outbound_interface = "eth0"    # Linux only
# leaf_executable = "easytier-leaf-worker"
```

- absence of the section or `enabled=false` is exactly current EasyTier;
- `config_file` is resolved relative to the EasyTier config directory;
- GUI/mobile/RPC may store `config_inline` instead of a path; the two are
  mutually exclusive;
- fail-closed behavior is mandatory and internal in v1. A policy failure
  preserves mesh L3 but blocks non-mesh traffic; it never silently changes to
  DIRECT;
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
The current candidate reads the same envelope from ordinary network TOML,
protobuf/RPC, and GUI configuration. CLI/environment process overrides still
win. Relative policy paths resolve from the network TOML directory; GUI and
Android should prefer `config_inline`. Enabling the section without exactly one
document source is rejected before the instance starts.

The public Linux implementation should first auto-select the outbound interface
only when exactly one usable physical default route exists. Multiple default
routes, policy-routing ambiguity, or no usable physical default must fail with
an actionable error and require an explicit interface. Ordinary EasyTier mode
and policy mode on other platforms do not expose this Linux-only selector.

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

The final Linux adapter may make the outbound interface optional only when a
single usable default-route interface exists and it is not the EasyTier TUN,
another TUN/TAP, or a known virtual policy interface. Multiple defaults,
ambiguous VPN routes, or an interface transition are configuration errors that
show the candidates and require an explicit choice; the runtime must never
guess. An explicit CLI/environment/TOML value always wins. Other platforms use
their native VPN owner to identify and protect outbound sockets and do not
expose this Linux-only field to ordinary users.

Android reuses the single VpnService TUN owned by EasyTier. Policy packets pass
to an in-process Leaf runtime through the bounded packet bridge; mesh routes,
proxy CIDRs, public IPv6 routes, multicast, and Magic DNS remain on the
PeerManager path. The runtime package is excluded before `Builder.establish()`,
and underlying DNS servers are captured from `LinkProperties` before the VPN
becomes active. Policy mode alone adds IPv4 and IPv6 default routes.

The in-process runtime parses generated Leaf configuration before spawning,
allocates a unique runtime ID, publishes the bridge only after readiness, and
uses a late-start reaper if the three-second readiness window expires. Linux
and Android share a bounded 1/2/5-second restart budget. A fourth failure in
the same route generation becomes dormant; a route identity change retries,
and 60 seconds of stable operation resets the budget.

Magic DNS remains deliberately mesh-owned. Queries sent to its virtual address
bypass Leaf, so Leaf `DOMAIN`/`GEOSITE` rules cannot observe those names. The
runtime emits an explicit warning rather than claiming split-DNS support. This
limitation remains until a split-DNS adapter can preserve both EasyTier's
authoritative zone and Leaf FakeDNS semantics.

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

#### Mesh UDP association relay

A standards-compliant SOCKS5 server commonly binds a UDP association to the
source IP of its control TCP connection. EasyTier's TCP proxy may terminate the
control connection on the destination peer, while raw mesh UDP retains the
origin peer address; forwarding those two paths independently therefore fails
against strict servers. Neither an explicit UDP endpoint in the SOCKS request
nor a sender-only Native/KCP bypass is sufficient (both were tested and
rejected).

For a `via: mesh` actor with UDP enabled, terminate the upstream SOCKS control
session in a private policy relay on the selected destination peer. That relay
opens both the TCP control connection and the UDP socket locally, so the
upstream server observes one source identity. The origin and destination policy
relays exchange framed datagrams over ordinary EasyTier mesh L3 UDP; this does
not add a PeerManager packet type, alter transport selection, or expose a
system listener.

The association protocol is feature-private and versioned. Creation uses the
existing authenticated mesh RPC path and returns a random 128-bit token plus a
virtual UDP endpoint. Every datagram carries a version and the token; the
destination also pins the expected origin virtual IP and port. Tables are bounded,
idle-expire, close immediately with the control stream, and release their
data-plane UDP socket on cancellation, route generation change, or instance
shutdown. Invalid tokens, wrong source endpoints and oversized frames are
late datagrams are dropped without response. TCP-only actors and ordinary
CONNECT sessions retain the current KCP-preferred bridge path.

Leaf's existing external plugin loader uses a dynamic-library/FFI boundary.
Do not use that unsafe loader inside the EasyTier process. Prefer a small
compile-time integration that directly constructs Leaf's public outbound
handler traits, or contribute a safe in-process handler registration API
upstream. Keep any required Leaf patch isolated and pinned.

The adapter selects the mesh as transport to a configured proxy endpoint. It
does not select a physical OSPF path or underlay protocol.

### Transparent policy mode

`VirtualNic` remains the sole owner of the Android `VpnService` TUN FD. The
policy mux classifies packets read from that existing tunnel and exchanges only
L3 packet bytes with Leaf through the same bounded packet endpoint used by the
Linux implementation. Leaf never receives, duplicates, or closes the real
Android TUN FD, and no second TUN or VPN service is created. Both mesh and
policy return traffic share one serialized writer to the existing FD.

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

- Android: `TauriVpnService` always appends its runtime `packageName` to the
  disallowed-application set before establishing the VPN. This bypasses all
  EasyTier and in-process Leaf underlay sockets while application traffic from
  other UIDs still enters the VPN. Leaf's existing Android socket-protect hook
  remains a defense-in-depth option, not a prerequisite for v1. The service
  snapshots underlying DNS servers from
  `ConnectivityManager.getLinkProperties(activeNetwork)` before establishing
  the VPN and passes those addresses with the TUN attachment. Android exposes
  no outbound-interface setting. Mesh proxy connections use the data-plane API.
- Linux: use a dedicated mark plus policy rule or bind-to-device/netns strategy;
  verify the mark survives every connector type. If exactly one usable physical
  default-route interface exists it is selected automatically; zero or multiple
  candidates fail with an actionable request for an explicit
  `outbound_interface` rather than guessing.
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

Historical envelope draft (superseded by the implemented envelope above):

```toml
[policy_proxy]
enabled = false
mode = "rule"                 # rule | global
fail_closed = true
policy_file = "policy.yaml"   # desktop/server
# policy_inline = "..."       # GUI/mobile/RPC representation
# outbound_interface = "eth0" # Linux only; optional when one default is unambiguous
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
- Android and other mobile callers never accept `outbound_interface`; Linux
  accepts it as an override and otherwise auto-selects only an unambiguous
  physical default-route interface;
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

EasyTier's own underlay/control-plane hostname resolution is separate from
application DNS. In policy mode, its Hickory UDP/TCP sockets must carry the
same fwmark as underlay sockets; STUN and connector hostname lookup must use
that marked resolver rather than libc's unmarked resolver. Do not solve this
by bypassing every destination on port 53: that would evade Leaf's FakeDNS and
domain-rule path. Magic DNS's exact virtual address remains classified as mesh
traffic.

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
- Whether Android should additionally enable Leaf's existing per-socket
  `VpnService.protect()` callback after package/UID exclusion has passed real
  device loop-prevention tests.
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
