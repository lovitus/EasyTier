# Leaf Optional Policy Proxy Integration TODO

## Mandatory reference policy

All Leaf/policy-proxy behavior must be designed and reviewed against the local
Mihomo implementation configured in `AGENTS.md`; sing-box is the secondary
reference when Mihomo lacks the feature or platform behavior differs.
Implementation notes and tests must identify the referenced files/functions and
the observable semantics being preserved. This includes ordered rule matching,
DNS/FakeDNS, GeoIP/GeoSite loading and updates, caches, hot paths, proxy groups,
failover/recovery, loop prevention, lifecycle, and errors. EasyTier-specific
differences are allowed only when required by the mesh/Leaf architecture and
must document their reason, compatibility boundary, failure behavior, and
validation evidence. Unknown behavior is an investigation blocker, not a reason
to invent a new semantic.

Current implementation references:

- transactional policy loading was checked against Mihomo
  `hub/executor/executor.go::ApplyConfig` and
  `component/updater/update_geo.go::{UpdateGeoIp,UpdateGeoSite}`. Mihomo
  serializes application under its executor lock and validates Geo data before
  replacing the stored file. EasyTier intentionally builds a complete Leaf
  candidate off-path, atomically publishes it only after readiness, and retains
  the previous runtime on failure because dropping non-mesh traffic during an
  invalid file edit would violate the policy-plane availability contract;
- Android VPN ownership was checked against Clash Meta for Android
  `app/.../util/Clash.kt::startClashService` and
  `service/.../TunService.kt`. VPN permission acquisition remains an explicit
  user action. EasyTier additionally persists a system-revoked marker because
  its mesh instance may remain enabled after Android transfers TUN ownership;
  WebView reload or process synchronization must not silently reclaim it.
- macOS outbound binding and transparent-route ownership were checked against
  Mihomo `component/dialer/bind_darwin.go` and
  `listener/sing_tun/server.go`. Both implementations bind native egress
  sockets with `IP_BOUND_IF`/`IPV6_BOUND_IF`. EasyTier intentionally keeps its
  existing TUN and installs only two IPv4 and optional two IPv6 split-default
  routes; the private guard records each successful route and rolls it back in
  reverse order. It does not copy Mihomo's full route-set or auto-redirect
  engine.

**Status**: Linux candidate and automated test matrix validated; exact Android
side-by-side candidate established a Stealth-protected mesh and in-process
policy runtime on a real device. DHCP-empty and native persisted VPN-revocation
fixes plus the macOS utun/process-sidecar adapter are implemented and pending a
single replacement candidate build and repeat validation. Windows and macOS
Network Extension adapters remain disabled rather than falling through to an
unsafe generic implementation.
**Updated**: 2026-07-14

This TODO is the design source of truth. Update it after each material design
discussion so implementation does not depend on chat history.

## Active implementation ledger

Status may move to `done` only with the evidence named in the last column.
Implementation without a matching build or real-device result remains
`validation-pending`. New findings must receive an ID here before code changes
start, so parallel platform work cannot disappear into chat history.

Status meanings:

- `investigating`: the symptom is real, but the fault boundary has not been
  isolated; do not implement a speculative fix;
- `implementation-pending`: the required behavior and acceptance boundary are
  known, but production code is incomplete;
- `implemented-validation-pending`: production code exists, but the exact
  artifact has not passed the required failure/recovery or real-device matrix;
- `blocked`: a required platform primitive or validated design is missing;
- `known-limitation` / `unsupported`: deliberate product boundaries, not work
  that may silently be treated as complete;
- `done`: implementation and the named evidence both exist.

| ID | Status | Scope | Required evidence / current blocker |
| --- | --- | --- | --- |
| BUILD-075 | failed | Exact candidate `075d3cdf`: Linux profiling, Android debug APK, macOS ARM64 DMG, full Test | Android run `29310466513` passed, but macOS run `29310489240` exposed the `MAC-COMPILE` blockers; Linux/Test results remain useful evidence but this SHA cannot be promoted |
| BUILD-CACHE | validation-pending | Candidate build latency | Profiling already cached musl targets; macOS test and Android candidate now use platform-specific target caches with failure caching. Compare the next two identical-path builds for restore/save success and wall-time improvement without profile or artifact changes |
| BUILD-NEXT | implementation-pending | Exact replacement candidate after `075d3cdf` | Commit the complete code plus this ledger as one snapshot, run format/static checks, then push the same SHA to the macOS test and profiling-beta workflows. Do not validate artifacts produced from different SHAs |
| MAC-COMPILE | implemented-validation-pending | macOS policy-enabled build exposed Linux-only socket marking and an undeclared libc path | `14a6a17f` gates TCP STUN `SO_MARK` and imports `nix::libc`; require a successful ARM64 DMG workflow for the final candidate SHA before closing |
| AND-SIGNING | implemented-validation-pending | Stable signing identity for rolling Android policy candidates | GitHub Secrets restore a dedicated candidate-only keystore; APK cert must be `14d2d885ce1bc361923a493210865f86390ffcd32eb2b555042bbd1a8b6c38e0`; after the one-time signer migration, two consecutive workflow APKs must upgrade with `adb install -r` while preserving config and VPN authorization |
| AND-VPN-OWN | done | Native persisted system-revoke marker; background restart must not reclaim VPN ownership | Android candidate run `29310466513`: three 0.67-0.87 second stop/start cycles succeeded; Clash takeover set `revoked_by_system=true`; force-stop/cold-start retained Clash ownership and showed stopped; explicit Run reclaimed VPN and cleared the marker |
| AND-DHCP | done | DHCP mode must not serialize an undefined virtual IP | Android candidate run `29310466513` on `192.168.234.227`: imported config kept `dhcp=true` with no explicit IPv4; repeated starts obtained `10.245.0.2/24` and `10.245.0.3/24` without WebView or native serialization errors |
| AND-MESH-ICMP | investigating | Android policy-enabled mesh path after VPN revoke/reclaim | External IPv4 and DNS passed, but the first `10.245.0.1` ICMP run lost 5/5 packets. Confirm the remote TUN/peer state, then A/B the same Android config with only policy mode disabled before assigning a root cause |
| AND-NETWORK-CHANGE | implemented-validation-pending | Wi-Fi roam, DHCP renew, Wi-Fi/mobile-data/airplane-mode transitions | Repeated transitions must update DNS/network generation, stop without restart storms while offline, recover once after connectivity returns, preserve mesh routes, and not reclaim VPN after another app owns it |
| AND-POLICY-UI | implemented-validation-pending | Android policy editor, import/preflight and managed rule data | Validate the collapsible editor without coordinate automation, config import/export, diagnostics, manual update buttons and persistence across process death and signed APK upgrade |
| GEO-RESOURCE-UX | implemented-validation-pending | GeoSite/GeoIP should work without path, digest or first-run download fields | The editor now reports the pinned bundled GeoSite/GeoIP snapshots as built in and keeps Country MMDB optional. Empty built-in rows remain editor-local and are not serialized. Validate default presets, explicit same-kind override, Android upgrade persistence and no path/SHA prompts |
| GEO-ONLINE-UPDATE | planned-next-version | Online replacement currently does not reliably attach the downloaded resource to the saved policy | Keep online replacement unavailable for bundled GeoSite/GeoIP in this version. The next version must make explicit update atomically replace and activate a validated snapshot, preserve the old snapshot on any failure, and cover save/restart/Android upgrade before exposing the action again |
| POLICY-OUTBOUND-UX | implemented-validation-pending | Outbound interface was an unconditional free-text field | Target-side RPC now reports platform applicability and active physical interfaces. Linux/traditional macOS use a selector and recommended route match; Android hides the field because VpnService owns the path; unsupported platforms report unavailable. Validate multiple physical interfaces, active system VPN, network changes, stale selections and old-server RPC failure |
| DESK-RELOAD | implemented-validation-pending | Transactional source-file hot reload and bounded restart | Valid edit publishes a ready candidate; malformed edit retains old revision; worker kill follows 1/2/5-second budget; no bridge/task/FD growth |
| POLICY-ROUTE-REFRESH | implemented-validation-pending | Review P1: underlay bypass routes must track address/default-route changes | The supervisor now refreshes the namespace-bound routing guard on relevant events and at most every five seconds, retaining fail-closed state on error. Validate DHCP renew, interface index change, default-route loss/restore and no recursive underlay capture |
| POLICY-MAGIC-DNS | implemented-validation-pending | Review P1: Magic DNS must remain on the mesh path | `100.100.100.101/32` is included in the live mesh classifier only while Magic DNS is enabled. Validate queries before/after config patch and route snapshot replacement; domain/GEOSITE visibility remains the separate `SPLIT-DNS` limitation |
| POLICY-WRITER-FAIRNESS | implemented-validation-pending | Review P2: sustained peer traffic must not starve Leaf responses | Peer and policy packets use bounded independent queues, unbiased selection and alternating bounded drain. Saturate both directions and prove bounded loss, no starvation, no unbounded memory and unchanged mesh latency |
| POLICY-DNS-DISCOVERY | implemented-validation-pending | Review P2: loopback resolver stubs are unusable as direct Leaf DNS servers | Resolver discovery rejects loopback/link-local/multicast stubs and checks resolved systemd/NetworkManager files. Validate resolved, NetworkManager, plain resolv.conf, Android supplied DNS, empty DNS and network-change replacement |
| LNX-POLICY | implemented-validation-pending | Linux policy routing, source/mark bypass and fail-closed behavior | Isolated namespace/container tests plus physical-host mesh regression; include stale cleanup, competing table/rule ownership, abrupt exit, underlay change and route restoration |
| MAC-UTUN | implemented-validation-pending | Traditional macOS utun transparent adapter and packaged Leaf sidecar | DMG contains signed/runnable sidecar; v4/v6 split routes install, DIRECT and proxy paths work, normal stop reverses exact owned routes |
| MAC-ORPHAN | implemented-validation-pending | macOS parent-PID watchdog | Forced GUI/core termination removes worker within two seconds and leaves no policy route, temp config, session or repeated respawn |
| POLICY-GEODATA | implemented-validation-pending | Bundled `geosite.dat`, `geoip-lite.dat` and optional Country MMDB | GeoSite/GeoIP are pinned MetaCubeX snapshots embedded in the core and materialized only when a matching rule lacks an explicit same-kind rule set. Validate parser/category semantics, digest and atomic materialization, read-only config fallback, explicit override and no network dependency. Country MMDB remains optional; ASN MMDB remains out of v1 because no actor consumes it |
| POLICY-GEO-MATCHER-PERF | implemented-validation-pending | Indexed GeoSite/GeoIP new-session matching comparable to mainstream proxy engines | The original Leaf router consumed about 16.8 CPU seconds for 200 concurrent `GEOSITE,CN` decisions versus 0.12-0.22 CPU seconds for the `MATCH` baseline. The pinned EasyTier Leaf fork replaces 112,008 linear suffix conditions with label-boundary hash lookups and merges 7,500 GeoIP CIDRs into sorted ranges with binary search. Re-run the exact A/B workload and reject the patch if GeoSite remains a material CPU hotspot or changes first-match semantics |
| POLICY-RULES | implemented-validation-pending | Ordered first-match GEOIP/GEOSITE/COUNTRY/IP/domain rules | Compare ordering, `no-resolve`, missing category, duplicate resource and UDP actor-skip behavior with the pinned Mihomo references; preflight and runtime must produce the same result |
| POLICY-EDITOR | implemented-validation-pending | GUI/RPC policy switch, nodes, groups, rules, fallback and rule-data controls | Desktop and Android round-trip must preserve order and explicit IDs/IPs, show validation diagnostics before start, update managed data only on explicit click and never mutate unrelated EasyTier settings |
| POLICY-CONFIG | implemented-validation-pending | Opt-in startup and configuration across CLI, TOML, RPC, GUI and Android import | Disabled/absent mode must create no policy task, route, bridge or worker; enabled mode must use the same validated document and diagnostics; malformed or unsupported platform configuration must fail policy startup without breaking the mesh |
| POLICY-CHAIN | implemented-validation-pending | DIRECT, SOCKS, mesh actor, chain and passive fallback | TCP and UDP matrices including final non-mesh SOCKS, actor failure, simultaneous network outage, restore hysteresis, bounded retry and no fallback oscillation |
| POLICY-KCP-UOT | implemented-validation-pending | Existing KCP-preferred mesh data plane and private UoT fallback | KCP available/unavailable/mixed-version tests; SOCKS KCP-only semantics unchanged; UDP over KCP/UoT throughput/loss; KCP state, target sockets, FDs and tasks return to baseline |
| POLICY-UDP-LOSS | investigating | Policy UDP throughput remains below TCP and prior 50 Mbit/s runs observed loss | Separate Leaf/TUN, UoT framing, KCP, destination relay and test-actor loss with counters and profiling; do not mask loss by unbounded buffers or silently downgrade to smoltcp |
| POLICY-LOOP-GUARD | implemented-validation-pending | Dynamic peer endpoint exclusion and retry-storm containment | Validate endpoint refresh, self/peer/mesh CIDR exclusions, more than 1000 attempted connections per second, malformed chains, worker crash and system-TUN coexistence without CPU/FD/KCP growth |
| POLICY-LEAF-EMBED | investigating | Android in-process Leaf global runtime registry and shutdown ownership | Stress multiple start/stop and late-registration cancellation; prove unique runtime IDs, no FD double-close, no blocking call on async workers, and no process-global state leak between instances |
| POLICY-PERF | validation-pending | Disabled-mode overhead and enabled-mode CPU/memory/latency | Profiling comparison against the validated pre-policy binary on the same hosts and traffic traces; report TUN copy/syscalls, Leaf queues, KCP/UoT, RSS, tasks, FDs, latency and battery/idle wakeups |
| WIN-ADAPTER | blocked | Windows transparent adapter | Leaf cannot bind an interface on Windows and no verified Wintun/WFP bypass adapter exists; remain compile/runtime gated |
| MAC-NE | blocked | macOS Network Extension adapter | NE owns route/socket protection differently; traditional utun implementation must not be enabled in `macos-ne` builds |
| IOS-ADAPTER | blocked | iOS Network Extension transparent policy adapter | Requires an NE-owned packet-flow adapter and protected outbound sockets; do not reuse the traditional macOS route implementation or advertise policy mode until implemented and device-validated |
| MULTI-INSTANCE | known-limitation | More than one policy-enabled network in one core process | Process-global route ownership and Leaf runtime assumptions permit one active policy instance; additional networks keep the normal EasyTier path. A future design must isolate route tables, runtime IDs, ports and cleanup before lifting this limit |
| NETNS-ADAPTER | known-limitation | Policy mode inside an EasyTier instance netns | Desktop policy startup rejects configured instance netns because worker, route and outbound-interface ownership are not yet namespace-complete; do not silently run the worker in the host namespace |
| SPLIT-DNS | known-limitation | Magic DNS and Leaf domain/GEOSITE visibility | Current design keeps Magic DNS on mesh, so Leaf cannot observe those query names; requires a dedicated split-DNS adapter |
| DEP-RELOAD | known-limitation | Rule-data-only file changes | Source YAML digest drives reload; managed GUI updates the source reference and requests save/restart, but an externally replaced dependency alone is not hashed every five seconds |
| HTTP-ACTOR | unsupported | HTTP CONNECT actor | Pinned Leaf build has no outbound HTTP actor; reject rather than advertise a false fallback capability |

## Current execution queue

Work is batched by dependency so one expensive build validates several completed
modules without hiding failures:

1. `BUILD-NEXT`: finish static review and produce one exact candidate containing
   the macOS compile fix, stable Android signing workflow and this ledger.
2. `MAC-COMPILE` and `AND-SIGNING`: establish usable macOS/Android artifacts;
   do not start feature expansion until both artifacts can be repeatedly
   installed and launched.
3. `AND-MESH-ICMP`, `POLICY-ROUTE-REFRESH`, `POLICY-MAGIC-DNS`,
   `POLICY-WRITER-FAIRNESS` and `POLICY-DNS-DISCOVERY`: close correctness risks
   before throughput tuning. The Android ICMP result must first be repeated
   against a remote node with a verified TUN address.
4. `GEO-RESOURCE-UX`, `POLICY-OUTBOUND-UX`, `POLICY-GEODATA`, `POLICY-RULES`, `POLICY-EDITOR`,
   `POLICY-CONFIG`, `POLICY-CHAIN` and `POLICY-KCP-UOT`: validate the
   user-visible policy contract as one coherent functional matrix.
5. `AND-NETWORK-CHANGE`, `DESK-RELOAD`, `LNX-POLICY`, `MAC-UTUN`,
   `MAC-ORPHAN`, `POLICY-LOOP-GUARD` and `POLICY-LEAF-EMBED`: run abnormal
   lifecycle, route ownership and resource-leak tests.
6. `POLICY-UDP-LOSS` and `POLICY-PERF`: profile only after correctness is
   stable; keep or revert optimizations based on exact-artifact evidence.
7. `GEO-ONLINE-UPDATE`, `WIN-ADAPTER`, `MAC-NE`, `IOS-ADAPTER`, `MULTI-INSTANCE`,
   `NETNS-ADAPTER` and `SPLIT-DNS` remain explicit future work and cannot be
   implied by a Linux/macOS-traditional/Android pass.

Workflow for every ledger item:

1. record the finding and acceptance evidence in this table;
2. implement a bounded module behind the existing opt-in feature/platform gate;
3. format and statically review the whole batch before one candidate push;
4. build the exact commit through the relevant GitHub workflows;
5. validate the same artifact under normal, failure, recovery and resource-load
   conditions;
6. mark `done` only after attaching run IDs, artifact hashes and real-device
   results; otherwise keep it pending or revert the failed candidate commit.

## Current implementation snapshot

The current implementation is opt-in behind `leaf-policy-proxy`; Android adds
`leaf-policy-mobile` for the in-process runtime. It remains narrower than the
final cross-platform design below:

- an absent or disabled `[policy_proxy]` envelope creates no Leaf process,
  policy task, default route, packet mux, bridge, timer, or session table. The
  Linux process-level `--policy-config` override remains supported;
- desktop enabled mode requires `bind_device=true`, an explicit physical
  `--policy-outbound-interface`, no configured instance netns, and one active
  policy instance per process. If a process hosts additional networks, they
  keep the ordinary NIC path rather than failing or inheriting policy routes;
- `easytier-leaf-worker` is a pinned, separately supervised Leaf process. It
  validates Linux `SO_BINDTODEVICE` or the macOS interface index without
  sending traffic, uses at most four worker
  threads, and is restarted at most three times per unchanged endpoint
  generation;
- Linux sidecars use `PR_SET_PDEATHSIG`; macOS sidecars receive the exact
  parent PID and stop from a one-second watchdog if reparented. This keeps
  abrupt GUI/core termination from leaving an orphan Leaf runtime on either
  desktop adapter;
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

## Known interaction: exit nodes and manual routes versus the policy classifier

`collect_policy_mesh_routes_for` builds the classifier's mesh-route snapshot
from the local virtual subnet, Magic DNS's fake IP, `proxy_cidrs`, and public
IPv6 routes. It does not include `exit_nodes` or manual `routes`. Combined with
`NicCtx::do_forward_nic_to_peers_task` classifying before forwarding, this
produces two concrete effects when policy proxy is enabled alongside those
features on the same node:

- if this node lists remote `exit_nodes`, non-mesh-destined traffic is
  classified `Policy` before it can reach the exit-node forwarding branch in
  `do_forward_nic_to_peers`. The exit node is not used; traffic instead exits
  through Leaf's own rules. The same applies to any manual route whose CIDR
  covers non-mesh destinations (for example `0.0.0.0/0`): it coexists in the
  kernel routing table with the policy proxy's split-default `/1` captures but
  never wins the longest-prefix match, so it becomes inert rather than
  conflicting;
- if this node sets `enable_exit_node=true` and relays another peer's traffic
  through its unmarked gateway sockets (`gateway/tcp_proxy.rs`,
  `gateway/udp_proxy.rs`, `gateway/icmp_proxy.rs`), that egress traffic is
  captured by this node's own split-default routes, re-enters its own TUN, and
  is classified `Policy` a second time before finally leaving through Leaf's
  marked, bypass-table-routed egress. This is one bounded extra hop, not
  unbounded recursion, and uses the same bounded Leaf input queue and mesh
  bridge admission limits as ordinary policy traffic; it is not a resource
  leak. It does mean the other peer's relayed traffic is subject to this
  node's Leaf domain/rule/FakeDNS configuration.

Neither interaction produces a routing loop, a crash, or unbounded resource
growth. This was discussed with the maintainer on 2026-07-13: both effects
match expectations and are accepted as-is. No further handling, classifier
change, or documentation-visible warning is planned for this interaction.

File-backed policy hot reload is now an implementation candidate: it hashes the
bounded source first, builds and readiness-checks a complete replacement while
the old runtime remains active, atomically publishes the replacement, and
retains the old revision with bounded retry/log suppression when validation or
startup fails. Inline/RPC policy changes still use the existing explicit
save-and-rerun path. Not yet implemented in this spike: HTTP CONNECT actor
adaptation, a bundled exit-node SOCKS5 UDP service, instance-netns worker
ownership, and desktop non-Linux TUN adapters. Proxy credentials for native and
mesh SOCKS actors are now an implementation candidate: native actors emit the
validated credentials into the private Leaf configuration, while mesh TCP and
UDP paths perform RFC 1929 authentication at the destination actor. Empty RPC
fields preserve the existing no-authentication wire behavior. TOML/RPC/GUI
envelopes and the Android single-TUN adapter are implemented candidates; the
replacement candidate must repeat packaged-build and real-device validation.
These are release blockers for claiming the full plan, but they do not affect
ordinary EasyTier builds because the feature is off by default.

The Android candidate currently targets the official Tauri GUI/VpnService
path. The separate `easytier-android-jni` contrib SDK remains feature-disabled:
its host API does not yet carry policy default routes, underlying DNS, or
network-generation callbacks. Enabling the Cargo feature there without that
public host contract would expose a configuration that cannot recover after a
network change.

The rolling Android policy candidate is deliberately packaged as
`com.kkrainbow.easytier.policycandidate`, signed by the repository's stable
candidate key, and labelled `EasyTier Policy Candidate`. It can coexist with a
maintainer's normal EasyTier installation and subsequent exact snapshots can
upgrade it in place. This is a validation-only packaging boundary: ordinary
debug and release builds keep `com.kkrainbow.easytier`. The workflow verifies
both application ID and signing-certificate digest before publishing the APK.
This replaces the previous unusable arrangement where every GitHub runner
generated a different Android debug key for the production package ID.

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
  configuration is rejected instead of creating a default/incorrect TUN.
  `VpnService.prepare()` remains an explicit-run ownership check: Android shows
  its authorization UI after the first installation only when permission is
  actually absent; normal EasyTier stop/start cycles do not request permission
  again. A native persisted revoke marker prevents WebView/process restart from
  reclaiming the VPN in the background, and a later explicit Run clears it only
  after Android reports that EasyTier owns VPN permission;
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

### Modular development and candidate selection

Do not continue adding policy lifecycle branches directly to `VirtualNic`.
Keep `VirtualNic` responsible only for creating the platform packet endpoint,
classifying mesh-owned packets, and attaching/detaching one policy controller.
Move the remaining orchestration behind these internal boundaries:

1. `PolicyFrontend`: parse, normalize, preflight, statically simulate, and
   manage immutable rule-data snapshots. It has no Leaf or EasyTier runtime
   types and is shared by CLI, RPC, GUI, and mobile.
2. `PolicyController`: own candidate construction, atomic publication,
   platform/network-generation changes, bounded restart state, and shutdown.
   Linux and Android use the same state machine rather than maintaining two
   copies in `virtual_nic.rs`.
3. `PolicyEngineFactory`: construct an engine from a validated revision,
   packet endpoint, DNS snapshot, and resolved mesh actors. Leaf process and
   in-process Leaf remain separate implementations of this interface.
4. `PlatformPolicyAdapter`: own only platform routing, protected/native socket
   behavior, rule-data storage, and network-change notifications. It cannot
   inspect policy rules or Leaf configuration.
5. `MeshActorResolver`: translate stable instance/IP selectors to the existing
   EasyTier SOCKS/UoT data plane without exposing PeerManager to the policy
   engine.

Alternative implementations are allowed only at one of these seams. Do not
maintain several complete policy stacks. The profiling build may compile a
small `policy-experiments` feature with a non-persistent selector such as
`ET_POLICY_EXPERIMENT_ENGINE=leaf-process|leaf-inprocess|replay`; release builds
must omit this selector and contain only the accepted platform default. Packet
bridge, writer scheduling, UDP relay, or DNS experiments use the same pattern:
each candidate implements one shared trait and receives identical bounded
packet traces and lifecycle events.

Every candidate must pass the same conformance harness before traffic testing:

- malformed policy and damaged rule data leave the previous revision active;
- mesh packets and Magic DNS never enter the policy engine;
- startup, cancellation, network-generation changes, late completion, and
  restart-budget exhaustion have deterministic expected states;
- TCP, UDP ASSOCIATE, KCP-backed UoT, credentials, fallback, and actor identity
  produce the same externally visible results;
- queues, tasks, sessions, FDs, and retry state return to their baseline;
- packet replay records correctness, drops, allocations, CPU time, and
  wakeups, then real-device profiling decides the winner.

Candidate code that loses validation is removed or reverted rather than left
as a permanent public option. This keeps experimentation parallel while the
shipping configuration and long-term maintenance surface remain singular.

### Public configuration envelope

All launch surfaces map to one configuration:

```toml
[policy_proxy]
enabled = true
config_file = "policy/default.yaml"
# config_inline = "..."        # GUI/RPC/mobile alternative
outbound_interface = "eth0"    # desktop policy mode; use en0 on macOS
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

Implemented desktop process-runtime launch surfaces:

```text
CLI:
  easytier-core --config config.toml \
    --policy-config policy/default.yaml \
    --policy-outbound-interface eth0 # use en0 on macOS

Environment:
  ET_POLICY_PROXY_CONFIG=/path/to/policy.yaml
  ET_POLICY_OUTBOUND_INTERFACE=eth0

Advanced worker override:
  --policy-leaf-executable /path/to/easytier-leaf-worker
  ET_POLICY_LEAF_EXECUTABLE=/path/to/easytier-leaf-worker
```

For the spike, supplying `--policy-config` or `ET_POLICY_PROXY_CONFIG` enables
policy mode. Omitting both disables it completely. Linux and the traditional
macOS utun backend require the physical outbound interface. The worker override
is for packaging and testing; normal macOS packages place
`easytier-leaf-worker` beside the GUI executable.
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
| macOS | existing utun backend (implemented); Network Extension remains gated | `IP_BOUND_IF`/`IPV6_BOUND_IF` plus owned split-default routes |
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
  # GeoSite and GeoIP are omitted here: matching rules use EasyTier's pinned
  # bundled snapshots unless an explicit same-kind rule set is supplied.
  country:
    type: mmdb
    path: rules/country-lite.mmdb
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
  - "GEOSITE,CN,DIRECT"
  - "GEOIP,CN,DIRECT,no-resolve"
  - "NETWORK,udp,final-udp"
  - "MATCH,final-tcp"
```

Editing UX:

- GUI provides simple pages for rule data, proxies, groups, and ordered rules;
- peer picker shows hostname/current IP but stores stable `instance-id` plus the
  current `virtual-ip` guard when available;
- advanced users edit the same YAML in a syntax-highlighted editor;

#### GUI authoring and preflight delivery order

Implementation status (2026-07-14): the first two layers are now wired into the
shared GUI as an ACL-style collapsed **Policy Proxy** panel. The enable switch
is inside the panel; disabled users do not see or initialize the editor. The
inline visual editor covers mesh/native SOCKS5 actors, ordered chain/fallback
members, ordered common rules, bundled Geosite/GeoIP DAT and optional Country MMDB, three bounded rule
presets, and the same advanced YAML text. YAML is the only persisted source;
invalid YAML suspends visual write-back instead of restoring an older model.
The existing config validation RPC now invokes the runtime policy parser and
returns structured warnings before Save/Run. The core embeds pinned MetaCubeX
GeoSite and GeoIP snapshots, verifies and materializes only the missing kinds
used by a policy, and leaves an explicit same-kind rule set untouched. Country
MMDB remains an optional user-managed resource. Online replacement of bundled
snapshots is deferred to `GEO-ONLINE-UPDATE`. Peer picker and the
side-effect-free simulator remain explicit follow-up items; the UI does not
fake these capabilities client-side.

The validated EasyTier policy YAML remains the only source of truth. The GUI
must never expose or persist generated Leaf configuration. Deliver the editing
surface in three small layers rather than cloning Mihomo's full configuration
UI:

1. **Core preflight first.** Saving and running both call the same
   `easytier-policy` parser used at runtime. The result is structured by field,
   actor/rule index, severity, and stable error code. It validates rule-set
   files and digests, references, cycles, expanded chain limits, UDP actor
   capability, and rule syntax. A failed preflight blocks Run but does not
   destroy the last saved/running valid policy.
2. **Visual actors and ordered rules.** Proxy rows provide a mesh peer picker
   (display hostname/current address, persist `instance-id` and optional exact
   virtual-IP guard), native address, credentials, and UDP capability. Group
   rows edit only the supported `chain`/`fallback` type and ordered members.
   Rules use an ordered table with drag/reorder and an advanced raw-rule cell.
   Unsupported health knobs are shown as fixed runtime behavior, not invented
   as fields that the current schema cannot consume.
3. **Local rule-data manager.** `GEOSITE` and `GEOIP` use pinned bundled
   MetaCubeX snapshots without a first-run download or persisted path. Advanced
   YAML may override either kind with one explicit local rule set. Country MMDB
   stays optional and user-managed. Bundled data has no automatic update,
   subscription, ASN database or source fallback in this version; safe online
   replacement remains a separate next-version task.

The editor also provides a side-effect-free policy simulator. Given a domain
or IP, TCP/UDP, and optional destination port, it reports the matched rule,
expanded actor chain, UDP eligibility, and final DIRECT/REJECT/proxy action.
This is static policy evaluation, not a connectivity health check. A separate
optional **Test selected exit** action may resolve the current mesh identity and
perform bounded TCP/UDP probes, but network failure must not make an otherwise
valid document unsavable.

File and inline editing remain mutually exclusive. Switching from the visual
editor to YAML serializes the same document deterministically; returning to the
visual editor is allowed only after preflight succeeds, so unknown or malformed
fields are never silently discarded. Secrets remain masked in the form and are
excluded from diagnostics and generated previews.

#### Quick policy presets

The ordinary GUI flow must not start from an empty rule table. It offers small
EasyTier-owned presets based on the familiar MetaCubeX GeoX layout, while
generating only syntax supported by the validated policy schema and Leaf:

- **China direct, overseas preferred:** private/local and mesh-owned traffic is
  excluded first; `GEOSITE,CN` and `GEOIP,CN` use `DIRECT`;
  `GEOSITE,geolocation-!cn` and the final rule use the selected overseas
  fallback group.
- **Selected services:** optional switches add well-known categories such as
  GitHub, Google, Telegram, YouTube, Netflix, and Spotify before the broad
  country rules. Every enabled category is checked against the imported
  Geosite snapshot before the revision can be applied.
- **Global policy:** private/local and mesh-owned traffic remains excluded,
  while all remaining TCP and UDP traffic uses the selected groups.
- **Direct except selected:** only checked categories use the policy group;
  the final action is `DIRECT`.

The wizard asks for the preferred and fallback actors separately for TCP and
UDP. It marks TCP-only actors clearly and previews that a matching UDP session
will continue to the next configured rule, exactly as Mihomo does. It can offer
`DIRECT` or `REJECT` as an explicit UDP fallback and never changes a UDP
decision into TCP-only forwarding. The generated order is only an editable
preset: the persisted `rules` list is authoritative, and neither EasyTier nor
Leaf assigns an implicit priority to `NETWORK`, Geosite, or GeoIP rules.

The generated baseline is deterministic and inspectable. A typical country
split uses Geosite for domain categories and GeoIP DAT for IP categories;
Country MMDB remains an explicit third matcher for ISO country rules:

The visual editor does not serialize paths or digests for the bundled GeoSite
and GeoIP snapshots. Advanced YAML can still supply an explicit same-kind rule
set, which takes precedence. The example only declares optional Country MMDB:

```yaml
rule-sets:
  country:
    type: mmdb
    path: rules/country-lite.mmdb
    update: manual
    sha256: "<verified digest>"

rules:
  - "GEOSITE,CN,DIRECT"
  - "GEOIP,CN,DIRECT,no-resolve"
  - "COUNTRY,JP,overseas,no-resolve"
  - "NETWORK,udp,overseas-udp"
  - "GEOSITE,geolocation-!cn,overseas"
  - "MATCH,overseas"
```

The three matcher families are ordered alternatives, not an intersection. Leaf uses
first-match semantics: the first matching rule selects the actor and evaluation
stops. The persisted order is entirely user-defined: EasyTier does not force
Geosite before GeoIP/Country, GeoIP before Geosite, or `NETWORK,udp` into a fixed
position. The example first lets Chinese domain and IP rules select `DIRECT`,
then applies the explicit JP Country MMDB rule and sends the remaining UDP
traffic to `overseas-udp`; moving any rule changes the result exactly as the
displayed order implies. `GEOIP,<tag>` always reads a category from
`geoip-lite.dat`; `COUNTRY,<ISO>` always reads `country-lite.mmdb`; and
`GEOIP,lan` is a built-in Mihomo-compatible special-network matcher. There is
no automatic DAT-to-MMDB fallback, so a rule never changes meaning according
to which files happen to exist. None of these sources
replaces the protected mesh/private route snapshot, and a final rule remains
mandatory so an unknown destination never receives an implicit action.

If every target is UDP-capable, the user may omit the explicit `NETWORK,udp`
rule and let the same ordered domain/IP rules handle both transports. For a
TCP-only target, the EasyTier-to-Leaf compiler adds `network=tcp` to that rule's
existing condition in Leaf's JSON router representation. TCP still observes
the original rule at the original index; UDP does not select that actor and
continues to the next configured rule. An explicit `NETWORK,udp` rule targeting
a TCP-only actor is therefore an ineffective branch and preflight reports it,
but it does not reorder the document. `MATCH`/`FINAL` is compiled as a normal
TCP/UDP network condition instead of Leaf CONF's special FINAL mechanism, so
even unconditional rules retain their configured position. This reproduces
Mihomo's ordered “matched actor lacks UDP support, continue scanning” behavior
without modifying Leaf or inventing a system priority.

Platform-owned mesh routes, the exact Magic DNS address, and configured proxy
CIDRs remain in `PolicyTunMux`'s mesh snapshot and therefore precede this rule
document without duplicating volatile routes into YAML. The preset additionally
generates explicit local/LAN exclusions needed for destinations that are not
owned by the mesh. The preview labels these rules as protected defaults; users
may inspect them, but unsafe removal requires the advanced editor and a
preflight warning.

The resource page offers three fixed MetaCubeX `latest` release assets only as
explicit one-shot update actions. It does not copy Mihomo's `geox-url`
auto-download or subscription behavior. A requested file is downloaded to
staging, bounded by size, format-checked, hashed, and atomically promoted; the
GUI then writes the local path and digest shown above. Runtime startup is
offline and never follows a `latest` URL.
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
  TCP-only. EasyTier derives the expanded actor capability before compiling
  protocol-aware Leaf rules.
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
mesh SOCKS5 endpoint as last resort. A TCP-only chain is not selected for UDP;
matching continues with the next configured rule or UDP-capable fallback.

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
  - GEOIP,private,DIRECT,no-resolve
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

### Geo rule-data ownership and updates

Geo rule data is local configuration data, not an implicit online service.
EasyTier/Leaf never downloads or refreshes it merely because policy mode starts.

Supported sources:

```yaml
rule-sets:
  geosite:
    type: geosite
    path: "rules/geosite.dat"
    update: manual
    sha256: "EasyTier-generated digest"
    source-url: "optional trusted custom HTTPS mirror"
  geoip:
    type: geoip
    path: "rules/geoip-lite.dat"
    update: manual
    sha256: "EasyTier-generated digest"
    source-url: "optional trusted custom HTTPS mirror"
  country:
    type: mmdb
    path: "rules/country-lite.mmdb"
    update: manual
    sha256: "EasyTier-generated digest"
    source-url: "optional trusted custom HTTPS mirror"
```

When `source-url` is absent, EasyTier uses its built-in MetaCubeX URL. A manual
update writes a private temporary file, enforces the size limit, parses the
resource, computes the digest, atomically replaces the managed file and only
then commits `path`/`sha256` to the policy document. A failed update preserves
the previous file and document state.

The recommended interoperable defaults follow the MetaCubeX example data
layout without copying its automatic-download behavior:

Reference: <https://wiki.metacubex.one/example/conf/#__tabbed_1_1> and the
versioned assets published by <https://github.com/MetaCubeX/meta-rules-dat>.

- `GEOSITE,<tag>` always uses `geosite.dat`;
- `GEOIP,<tag>` always uses `geoip-lite.dat`, except built-in `GEOIP,lan`;
- `COUNTRY,<ISO>` always uses `country-lite.mmdb`;
- `GeoLite2-ASN.mmdb` is not exposed because the pinned Leaf matcher does not
  support ASN rules and expanding it in EasyTier would add a second complex
  matcher with no current user-facing rule contract;
- the three upstream `latest` URLs are fixed one-shot GUI update sources, not
  runtime dependencies. A successful update records the local path, digest,
  and size in the edited YAML only after the staged file validates and is
  atomically published.

This keeps the familiar `GEOSITE,cn`, `GEOSITE,geolocation-!cn`,
`GEOIP,private`, and explicit `COUNTRY,CN` model while preserving EasyTier's
offline-first contract. The recommended starting order is private/LAN and
mesh-owned destinations first, then China DIRECT rules, then non-China policy
groups, and finally `MATCH`.

- Desktop/server users may point to a local absolute or config-relative file.
- GUI/mobile users may explicitly request one of the three fixed one-time
  updates after the instance configuration has been saved.
- Startup and policy reload use the last validated local snapshot and never
  block waiting for the network.
- Download writes to a private staging file, enforces file/category/CIDR limits,
  validates the expected format, computes its digest, and atomically replaces
  the active snapshot. Failed updates preserve the current file and YAML.
- Load only referenced GeoIP DAT categories into the generated immutable Leaf
  rule list. Geosite and Country MMDB continue through Leaf's native external
  matcher and are not reparsed per connection by EasyTier.
- This keeps file I/O and protobuf parsing out of connection and packet hot
  paths. EasyTier pins a minimal Leaf fork that indexes the expanded data:
  suffix/full domains use case-insensitive label-boundary hash lookup, keywords
  retain their ordered substring semantics, and IPv4/IPv6 CIDRs are merged into
  sorted ranges queried by binary search. This is intentionally smaller than
  Mihomo's succinct/MPH matcher while removing the same linear-scan bottleneck.
  `POLICY-GEO-MATCHER-PERF` remains validation-pending until the exact baseline,
  GeoSite and GeoIP workloads are rerun with the profiling beta artifact.

### Overseas egress validation topology

Two existing overseas hosts are assigned local-only validation aliases
`overseas-egress-a` and `overseas-egress-b`. Their real hostnames remain in the
git-excluded local environment and must not appear in repository logs,
fixtures, reports, or scripts.

Validation uses them as independent mesh SOCKS exits and then as an ordered
fallback group. It must cover:

- `GEOSITE,cn` and private/mesh destinations remaining DIRECT;
- `GEOSITE,geolocation-!cn` and selected country GeoIP categories using the
  preferred overseas exit;
- failure of the preferred exit falling back to the second exit without
  aggressive switching during a general network outage;
- TCP, native SOCKS UDP ASSOCIATE, EasyTier KCP-backed mesh SOCKS, and UoT;
- recovery after the preferred exit returns, using the existing conservative
  health window rather than migrating established flows.

### TCP-only final actors and UDP fallback

- `DIRECT` as the final action supports native TCP and UDP.
- A final standard SOCKS5 actor supports UDP only when its UDP ASSOCIATE path is
  enabled and validated.
- A final HTTP CONNECT actor is TCP-only unless that specific implementation
  explicitly provides a compatible datagram extension.
- UDP traffic never silently enters a TCP-only chain. It continues with the
  next configured rule; policies should end their UDP path with a UDP-capable
  group, native DIRECT, or explicit REJECT so the fallback is visible.
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
- UDP rules select only groups/chains whose complete actor sequence supports
  datagrams. HTTP CONNECT normally makes a chain TCP-only, so UDP skips a rule
  targeting it and keeps scanning in document order;
- EasyTier derives this capability from actor types while compiling the ordered
  JSON rules. There is no EasyTier peer UDP capability or new capability
  advertisement;
- an incompatible actor is never silently changed to TCP encapsulation or the
  existing EasyTier SOCKS portal. The selected fallback is the next matching
  compatible rule written by the user.

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
   mixed two-hop TCP chain, and a fully UDP-capable SOCKS5 chain; prove UDP
   skips an HTTP-only actor and selects the next configured compatible rule.
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
