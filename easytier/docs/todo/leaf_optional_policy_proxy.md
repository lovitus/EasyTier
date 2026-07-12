# Leaf Optional Policy Proxy Integration TODO

**Status**: transparent proxy required / spike required  
**Updated**: 2026-07-12

## Goal

Provide an optional policy proxy for users who need domain-aware routing,
logical proxy chains, preferred exits, failover, or load balancing without
changing EasyTier OSPF costs or forcing a physical overlay path.

Transparent full-device interception is a required end state because most
applications cannot be configured with explicit proxy settings. Explicit local
SOCKS support is useful for development and diagnostics, but is not the final
product boundary.

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
  application/TUN -> Leaf rules and groups
                  -> existing peer SOCKS endpoint or EasyTier peer outbound
                  -> existing EasyTier OSPF/PeerConn transport
                  -> exit
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

## Integration Options

### Phase 1: existing peer SOCKS endpoints

Use Leaf as a local rules/groups engine and configure existing EasyTier SOCKS
listeners as its upstreams.

Advantages:

- almost no EasyTier core modification;
- validates rules, logical chains, failover, and UDP behavior before adding a
  new internal API;
- can run as an optional companion process on desktop/server platforms.

Limitations:

- each chain hop must expose a reachable and appropriately protected SOCKS
  endpoint;
- extra SOCKS framing and local socket traversal;
- SOCKS UDP ASSOCIATE behavior must be validated end to end;
- Android full-device mode still needs ownership of the existing VPN TUN FD.

Current EasyTier behavior is more specific:

- the public EasyTier SOCKS portal currently leaves `allow_udp=false`, so UDP
  ASSOCIATE is rejected even though the embedded SOCKS implementation contains
  protocol support;
- existing EasyTier UDP port forwarding is not equivalent to SOCKS5 UDP
  ASSOCIATE;
- Phase 1 can therefore validate TCP nesting, rules, and TCP failover, but it
  cannot be treated as a complete UDP-chain solution without an additional
  change;
- production transparent UDP should prefer the internal `PeerExitDialer`
  datagram interface rather than expanding the public SOCKS server merely to
  connect the two components.

### Existing SOCKS TCP transport selection

Leaf's SOCKS chain creates nested logical SOCKS connections. When one of those
connections enters the existing EasyTier SOCKS portal, EasyTier independently
selects its current TCP connector for that requested destination:

1. if the destination resolves to the EasyTier virtual network, KCP proxy is
   enabled, an endpoint exists, and the destination peer allows KCP, the SOCKS
   CONNECT uses the existing KCP path;
2. otherwise, a virtual-network destination uses the existing smoltcp packet
   path;
3. a destination outside the virtual network, a loopback destination, or a
   destination with no resolved mesh peer uses a kernel-native TCP connect on
   that SOCKS node.

Example logical chain:

```text
Leaf -> peer A SOCKS
     -> A asks for peer B SOCKS virtual address
        -> A-to-B leg: KCP when eligible, otherwise smoltcp over the mesh
     -> B asks for the public destination
        -> B-to-Internet leg: native kernel TCP
```

KCP and smoltcp describe the SOCKS CONNECT implementation inside EasyTier;
they do not replace the underlying PeerConn selection. Their overlay packets
still use the currently selected QUIC/UDP/TCP/etc. PeerConn according to the
existing transport policy.

The current SOCKS KCP path is intentionally KCP-only after it is selected: a
KCP connect failure returns a SOCKS error rather than falling back to smoltcp or
native. Leaf may then apply its own group/chain failover to another logical
upstream, but it must not assume transparent KCP-to-smoltcp fallback within the
same EasyTier SOCKS CONNECT.

### Phase 2: local EasyTier peer outbound adapter

Only if Phase 1 proves useful, add a narrow internal abstraction:

```rust
trait PeerExitDialer {
    async fn connect_tcp(peer_id: PeerId, destination: Destination) -> Stream;
    async fn bind_udp(peer_id: PeerId, destination: Destination) -> Datagram;
}
```

Leaf implements an external outbound handler over this abstraction. The
adapter must use EasyTier internal streams/datagrams rather than a system socket
to a virtual IP, preventing traffic from re-entering the platform TUN.

Leaf's existing external plugin loader uses a dynamic-library/FFI boundary.
Do not use that unsafe loader inside the EasyTier process. Prefer a small
compile-time integration that directly constructs Leaf's public outbound
handler traits, or contribute a safe in-process handler registration API
upstream. Keep any required Leaf patch isolated and pinned.

This API selects a logical exit peer. It does not select a physical OSPF path or
underlay protocol.

### Phase 3: optional Android full-device policy mode

Only after the adapter and UDP lifecycle pass validation, allow Leaf to consume
the Android `VpnService` TUN FD. There must still be exactly one VpnService and
one TUN interface. Starting a second VPN is prohibited.

The FD must have exactly one packet reader and one serialized writer. Leaf and
the current EasyTier VirtualNic must never concurrently read or write the same
TUN FD.

The preferred architecture is a small `PolicyTunMux` at the existing mobile
TUN boundary:

```text
                         +-> existing EasyTier VirtualNic packet path
Android TUN -> PolicyTunMux
                         +-> Leaf packet/netstack input

existing EasyTier output -+
                           +-> one serialized Android TUN writer
Leaf packet output --------+
```

`PolicyTunMux` classifies only by a read-only, atomically replaced prefix
snapshot:

- EasyTier virtual addresses and current mesh/subnet routes go to the existing
  VirtualNic path;
- traffic selected for transparent policy proxying goes to Leaf;
- classification does not inspect or modify OSPF costs, peer paths, Stealth, or
  PeerConn protocol selection;
- each packet has one owner and is delivered to exactly one ingress;
- both producers share one bounded writer and preserve packet boundaries.

This requires a narrow Leaf integration seam that accepts packet input/output
channels instead of directly owning a raw TUN FD. The change should be proposed
upstream or maintained as a small isolated patch. Leaf must not receive the raw
FD when `PolicyTunMux` is active.

On Android, EasyTier's own application sockets may be excluded/protected from
the VPN. A Leaf system socket targeting an EasyTier virtual IP can therefore
bypass the TUN and fail to enter the mesh. Peer-chain outbounds must use the
internal TCP/UDP `PeerExitDialer`; system sockets to virtual peer addresses are
not an acceptable production implementation.

Applications that do not need full-device policy routing may continue using a
local SOCKS entry and must not pay the Leaf TUN-engine cost. Users that do not
enable the optional policy proxy retain the current direct VirtualNic-to-TUN
wiring without `PolicyTunMux` or Leaf runtime initialization.

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

- A chain remains datagram-native only when all required handlers support an
  unreliable datagram transport.
- Leaf may carry UDP over a reliable stream when a handler supports that mode.
- If a required chain actor has no compatible datagram implementation, setup
  must fail explicitly rather than silently bypassing the actor.
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

## Configuration And User Scope

- Disabled by default.
- Ordinary mesh, SOCKS, exit-node, and subnet-proxy users see no new runtime
  component.
- First public configuration should be a separate policy-proxy section/file,
  not additions to OSPF or listener URLs.
- The policy config refers to peer IDs or named peer SOCKS endpoints and Leaf
  groups/rules.
- Loading or validating policy configuration must be transactional: invalid
  policy keeps the mesh running and preserves the last valid policy runtime.
- Do not promise full Mihomo configuration compatibility. Leaf has its own
  configuration and rule semantics.

## Spike Gates

Do not merge production integration until all gates pass:

1. Build Leaf as an optional dependency for Linux, macOS, Windows, and Android;
   verify disabled builds do not link or initialize it.
2. Prove TCP and UDP external handlers can reach a selected EasyTier peer
   without opening a system connection to the peer virtual IP.
3. Prove `PolicyTunMux` has one FD reader/writer, exact packet ownership,
   bounded queues, dynamic prefix-snapshot updates, and no packet duplication.
4. Verify mesh ICMP and non-TCP/UDP L3 traffic remain on the unchanged
   EasyTier VirtualNic path while transparent TCP/UDP policy traffic uses Leaf.
5. Verify a real bidirectional voice/WebRTC-style UDP flow for at least one
   hour, including NAT keepalive and idle/resume behavior.
6. Verify SOCKS UDP through one peer, a two-hop logical chain, failover, and
   failure when a chain actor lacks datagram support.
7. Create more than the session limit and prove memory, tasks, and file
   descriptors remain bounded without disrupting existing calls.
8. Inject a TCP connection storm and UDP datagram amplification loop; verify
   only the affected chain is suspended and the EasyTier mesh remains healthy.
9. Repeatedly start/stop the optional runtime and Android VPN service; verify no
   retained TUN FD, task, DNS listener, socket, session, or callback.
10. Compare call latency, jitter, loss, CPU, RSS, and battery against direct
   EasyTier UDP and existing SOCKS UDP baselines.

## Open Decisions

- Whether Phase 1 is sufficient or a peer-pinned internal adapter is justified.
- Whether to vendor/pin Leaf, consume crates from git, or maintain a minimal
  upstream patch set.
- Whether Leaf will accept a safe in-process outbound registration API so the
  dynamic plugin ABI is unnecessary.
- Which Leaf features can be disabled to reduce package size and attack
  surface.
- Whether geosite-compatible data must be converted during build or loaded in
  Leaf's native rule format.
- Android ownership rules for the TUN FD and `VpnService.protect()` callback.
- Exact packet-channel API required to run Leaf behind `PolicyTunMux` without
  giving Leaf ownership of the raw TUN FD.
- Appropriate UDP session limits for mobile and desktop profiles.
- Whether UDP failover health checks should probe a neutral endpoint or a
  per-exit user target.

## Non-Goals For The First Version

- No OSPF changes or forced physical path routing.
- No seamless migration of established TCP or UDP sessions.
- No second Android VPN service.
- No implicit enablement for every user.
- No new proxy protocol implementation in EasyTier.
- No replacement of the existing EasyTier SOCKS, exit-node, or Proxy Failover
  features.
