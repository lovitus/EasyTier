> [!IMPORTANT]
> This file is the architecture and compatibility record, not the current execution board.
> Use `leaf_v1_release_gates.md` for current P0 gates, `leaf_validation_journal.md` for exact-SHA evidence, and `leaf_post_v1_backlog.md` for deferred work.
> Documentation-only edits remain local and must not trigger a workflow.

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
- DNS configuration error handling was checked against Mihomo
  `config/config.go::parseNameServer` and `config/config.go::parseDNS`. Mihomo
  rejects malformed or missing resolver roles before constructing the runtime
  and preserves DNS-specific context in the returned error. EasyTier follows
  that fail-before-runtime boundary, but intentionally reports the smaller
  stable `dns.direct` or `dns.proxy` field path because its v1 schema exposes
  two resolver roles rather than Mihomo's full nameserver-policy surface.

**Status**: Linux candidate and automated test matrix validated; exact Android
candidate `38d965c2` established a Stealth-protected mesh and in-process policy
runtime on a real device. Android DIRECT/REJECT and bundled GeoIP decisions,
plus bidirectional mesh ICMP and mesh TCP while policy mode remained enabled,
have data-plane evidence. Remaining first-version blockers are lifecycle,
network-change, failure/recovery and resource-cleanup tests rather than large
rule-set performance. Windows and macOS Network Extension adapters remain
disabled rather than falling through to an unsafe generic implementation.
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
- `post-v1-optimization`: the behavior is correct and bounded for the first
  release, but measured scale or throughput remains below the project's target;
  keep the benchmark and optimization task active without delaying the first
  functional release;
- `known-limitation` / `unsupported`: deliberate product boundaries, not work
  that may silently be treated as complete;
- `done`: implementation and the named evidence both exist.

First-version release gating is intentionally narrower than completion of this
whole TODO. The first release is blocked by incorrect rule/route behavior,
unsafe VPN ownership, unrecovered network changes, loop or retry storms,
crashes, unbounded CPU/RSS/FD/task growth, broken KCP/UoT/chain behavior, or
resources that do not return to baseline after bounded cleanup. It is not
blocked by a measured but bounded performance gap that appears only with large
or unusually interleaved policies. Those performance gaps remain mandatory
`post-v1-optimization` work and retain their reproducible benchmarks, but are
implemented and validated independently after the functional candidate is
frozen. Common bundled GeoSite/GeoIP policies and ordinary custom rule sets
must still remain usable and free of catastrophic CPU, memory or startup cost
in the first release.

`post-v1-optimization` is a parallel delivery lane, not a waiver. Those rows
must remain open with their benchmark inputs and acceptance thresholds, and
optimization continues after the first functional candidate is frozen. A
failed optimization experiment reverts only its isolated optimization commit;
it must not delay or destabilize a candidate that already passes the functional,
security and lifecycle release gates. Conversely, a panic, wrong first-match
result, unbounded growth, retry storm or common-policy performance collapse is
not a non-blocking optimization and still blocks the first release.

| ID | Status | Scope | Required evidence / current blocker |
| --- | --- | --- | --- |
| BUILD-075 | failed | Exact candidate `075d3cdf`: Linux profiling, Android debug APK, macOS ARM64 DMG, full Test | Android run `29310466513` passed, but macOS run `29310489240` exposed the `MAC-COMPILE` blockers; Linux/Test results remain useful evidence but this SHA cannot be promoted |
| BUILD-CACHE | validation-pending | Candidate build latency | Profiling already cached musl targets; macOS test and Android candidate now use platform-specific target caches with failure caching. Compare the next two identical-path builds for restore/save success and wall-time improvement without profile or artifact changes |
| BUILD-NEXT | implementation-pending | Exact replacement candidate after `075d3cdf` | Commit the complete code plus this ledger as one snapshot, run format/static checks, then push the same SHA to the macOS test and profiling-beta workflows. Do not validate artifacts produced from different SHAs |
| MAC-COMPILE | implemented-validation-pending | macOS policy-enabled build exposed Linux-only socket marking and an undeclared libc path | `14a6a17f` gates TCP STUN `SO_MARK` and imports `nix::libc`; require a successful ARM64 DMG workflow for the final candidate SHA before closing |
| AND-SIGNING | implemented-validation-pending | Stable signing identity for rolling Android policy candidates | GitHub Secrets restore a dedicated candidate-only keystore; APK cert must be `14d2d885ce1bc361923a493210865f86390ffcd32eb2b555042bbd1a8b6c38e0`; after the one-time signer migration, two consecutive workflow APKs must upgrade with `adb install -r` while preserving config and VPN authorization |
| AND-VPN-OWN | done | Native persisted system-revoke marker; background restart must not reclaim VPN ownership | Android candidate run `29310466513`: three 0.67-0.87 second stop/start cycles succeeded; Clash takeover set `revoked_by_system=true`; force-stop/cold-start retained Clash ownership and showed stopped; explicit Run reclaimed VPN and cleared the marker |
| AND-VPN-COLD-SYNC | done | A persisted enabled core must not display running while no VpnService/TUN exists | Exact Android `38d965c2`: after five application-process death/cold-launch cycles, the UI consistently showed stopped with no TUN and did not silently reclaim VPN ownership. Explicit Run restored `tun0` in 0.508-0.829 seconds in all five valid samples; a separate force-stop/cold-launch recovery restored it in about 0.5 seconds. The earlier `ed0f36d3` running-without-TUN state is not present in the replacement candidate |
| AND-DHCP | done | DHCP mode must not serialize an undefined virtual IP | Android candidate run `29310466513` on `192.168.234.227`: imported config kept `dhcp=true` with no explicit IPv4; repeated starts obtained `10.245.0.2/24` and `10.245.0.3/24` without WebView or native serialization errors |
| AND-MESH-ICMP | done | Android policy-enabled mesh path after VPN revoke/reclaim | Exact `38d965c2` APK and musl peer: after restoring the previously absent remote `10.245.0.1`, Android→Linux and Linux→Android each passed 5/5 ICMP with policy mode enabled; Android mesh TCP to `10.245.0.1:25480` returned HTTP 200 in 24 ms. The earlier 5/5 loss was an invalid fixture with no remote peer, not a policy-path defect |
| AND-NETWORK-CHANGE | implemented-validation-pending | Wi-Fi roam, DHCP renew, Wi-Fi/mobile-data/airplane-mode transitions | Exact Android `38d965c2` survived a 12-second Wi-Fi loss with the same process and TUN ifindex, no policy runtime restart, bounded roughly one-per-second peer reconnect attempts, 3.2% sampled CPU after recovery and 5/5 restored mesh ICMP. Still validate repeated roam/DHCP, longer offline, mobile-data/airplane transitions, DNS generation replacement and competing VPN ownership before closing |
| UNDERLAY-BIND-REFRESH | implemented-validation-pending | Active connector bind addresses must not reuse the 60-second peer-advertisement cache after DHCP/address changes | Exact Linux `38d965c2` refreshed policy table `52000` and its source rule from `172.32.1.1` to `.3` in two seconds, but manual reconnect then retried the removed `.1` bind address until the shared `IPCollector` cache refreshed. A separate 12-second link outage cached an empty source set and blocked otherwise healthy reconnects until the same timeout. Underlay validation and connector bind selection now perform a fresh local-interface collection while peer advertisement/STUN caching remains unchanged. Validate immediate TCP/UDP/QUIC reconnect after address replacement and offline restore, bounded retries and no connector-creation CPU regression |
| AND-POLICY-UI | implemented-validation-pending | Android policy editor, import/preflight and managed rule data | Exact Android `38d965c2` CDP inspection proved the collapsible editor restores ordered `GEOIP,CN,DIRECT,no-resolve` plus `MATCH,REJECT`, keeps the no-resolve checkbox selected and preserves configuration across repeated process death. Still validate malformed diagnostics, import/export and signed replacement upgrade before closing |
| GEO-RESOURCE-UX | implemented-validation-pending | GeoSite/GeoIP should work without path, digest or first-run download fields | Exact Android `38d965c2` shows the pinned GeoSite and GeoIP snapshots as built in and verified, with no name/path/SHA input; only optional Country MMDB exposes its trusted-source URL and explicit download action. Bundled GeoIP already passed a real CN/non-CN decision. Still validate explicit same-kind override, read-only materialization failure and signed upgrade persistence |
| GEO-ONLINE-UPDATE | planned-next-version | Online replacement currently does not reliably attach the downloaded resource to the saved policy | Keep online replacement unavailable for bundled GeoSite/GeoIP in this version. The next version must make explicit update atomically replace and activate a validated snapshot, preserve the old snapshot on any failure, and cover save/restart/Android upgrade before exposing the action again |
| POLICY-OUTBOUND-UX | implemented-validation-pending | Outbound interface was an unconditional free-text field | Target-side RPC now reports platform applicability and active physical interfaces. Linux/traditional macOS use a selector and recommended route match; Android hides the field because VpnService owns the path; unsupported platforms report unavailable. Validate multiple physical interfaces, active system VPN, network changes, stale selections and old-server RPC failure |
| DESK-RELOAD | implemented-validation-pending | Transactional source-file hot reload and bounded restart | Exact Linux `38d965c2`: a forced Leaf worker death kept mesh ICMP at 50/50, recovered a new worker in 1.359 seconds, retained 9 core threads and reduced core FDs from 33 to 32; post-recovery TCP reached 1.31 Gbit/s and UoT UDP 10 Mbit/s had zero loss. A malformed `MATCH,missing-actor` edit retained the same worker/revision and working data plane, with an explicit candidate-rejected warning. Still validate repeated crash budgets, valid revision replacement and no bridge/task/FD growth over a longer loop |
| POLICY-ROUTE-REFRESH | implemented-validation-pending | Review P1: underlay bypass routes must track address/default-route changes | The supervisor now refreshes the namespace-bound routing guard on relevant events and at most every five seconds, retaining fail-closed state on error. Validate DHCP renew, interface index change, default-route loss/restore and no recursive underlay capture |
| POLICY-MAGIC-DNS | implemented-validation-pending | Review P1: Magic DNS must remain on the mesh path | `100.100.100.101/32` is included in the live mesh classifier only while Magic DNS is enabled. Validate queries before/after config patch and route snapshot replacement; domain/GEOSITE visibility remains the separate `SPLIT-DNS` limitation |
| POLICY-WRITER-FAIRNESS | implemented-validation-pending | Review P2: sustained peer traffic must not starve Leaf responses | Peer and policy packets use bounded independent queues, unbiased selection and alternating bounded drain. Saturate both directions and prove bounded loss, no starvation, no unbounded memory and unchanged mesh latency |
| POLICY-DNS-DISCOVERY | implemented-validation-pending | Review P2: loopback resolver stubs are unusable as direct Leaf DNS servers | Resolver discovery rejects loopback/link-local/multicast stubs and checks resolved systemd/NetworkManager files. Validate resolved, NetworkManager, plain resolv.conf, Android supplied DNS, empty DNS and network-change replacement |
| POLICY-DNS-SPLIT | implementation-in-progress | Domestic/direct and foreign/proxied DNS selection without resolver leakage | The working-tree candidate emits separate resolver roles. Reference semantics are Mihomo `config/config.go::parseDNS/parseNameServerPolicy` (separate validated resolver roles), `dns/resolver.go::matchPolicy/ipExchange` (ordered first-match policy, no cross-policy fallback) and `tunnel/dns_dialer.go::DNSDialer` (resolver transport follows the selected outbound and unsupported UDP fails rather than bypassing it). EasyTier intentionally exposes only direct/proxy resolver sets, not full `nameserver-policy`: domains classified DIRECT before resolution use physical/platform resolvers; domains classified to a proxy use bootstrap-pinned DoH through that exact policy actor; either set failing must fail closed without querying the other set. `DOMAIN`/`GEOSITE` can classify a domain before resolution, while a `GEOIP`-only rule necessarily cannot choose the resolver until an address exists and therefore follows the next pre-resolution match or `MATCH`; users wanting domestic/foreign split resolution must include ordered domain/GEOSITE rules and may still use GEOIP afterward for IP traffic. One intentional difference from Mihomo `respect-rules` is required for leak prevention: Mihomo may route the DNS-server endpoint by its own metadata, while EasyTier pins a proxied DoH connection to the original queried domain's first-matching non-DIRECT actor. This prevents a later IP rule for the DoH bootstrap address from silently changing that query to DIRECT; failure of the pinned actor remains fail-closed. The pin is local to DNS transport and does not alter application first-match semantics. GUI codec tests pass locally, but the Leaf pin and compiler candidate still require profiling-beta compilation and packet captures proving direct/proxy isolation, no UDP/53 proxy leak, fail-closed resolver failure, Magic DNS mesh retention and network-generation cache replacement on Linux and Android. This is separate from `SPLIT-DNS`, which covers Magic DNS visibility. |
| LNX-POLICY | implemented-validation-pending | Linux policy routing, source/mark bypass and fail-closed behavior | Exact Linux `38d965c2` isolated namespaces: policy table `52000` and source rule followed a DHCP-style address replacement in two seconds; normal SIGTERM removed the worker, TUN, split routes, rules, table and generated Leaf config. The same run exposed the separate `UNDERLAY-BIND-REFRESH` cache blocker. Still validate the fixed candidate, competing table/rule ownership, abrupt exit and physical-host regression |
| MAC-UTUN | implemented-validation-pending | Traditional macOS utun transparent adapter and packaged Leaf sidecar | DMG contains signed/runnable sidecar; v4/v6 split routes install, DIRECT and proxy paths work, normal stop reverses exact owned routes |
| MAC-ORPHAN | implemented-validation-pending | macOS parent-PID watchdog | Forced GUI/core termination removes worker within two seconds and leaves no policy route, temp config, session or repeated respawn |
| POLICY-GEODATA | implemented-validation-pending | Bundled `geosite.dat`, `geoip-lite.dat` and optional Country MMDB | GeoSite/GeoIP are pinned MetaCubeX snapshots embedded in the core and materialized only when a matching rule lacks an explicit same-kind rule set. Validate parser/category semantics, digest and atomic materialization, read-only config fallback, explicit override and no network dependency. Country MMDB remains optional; ASN MMDB remains out of v1 because no actor consumes it |
| POLICY-GEO-MATCHER-PERF | implemented-validation-pending | Indexed GeoSite/GeoIP new-session matching comparable to mainstream proxy engines | The original Leaf router consumed about 16.8 CPU seconds for 200 concurrent `GEOSITE,CN` decisions versus 0.12-0.22 CPU seconds for the `MATCH` baseline. Exact candidate `8e85c83b` reduced GeoSite to 17/19/22 ticks and GeoIP to 9/15/18 ticks, overlapping the 11-23 tick MATCH baselines, without increasing startup time or RSS. Android materialized the pinned GeoSite/GeoIP snapshots with exact digests. Final status still requires the replacement candidate after `POLICY-PREFLIGHT-MODIFIER` |
| POLICY-RULE-SCALE | post-v1-optimization | Thousands of ordered custom rules must retain first-match semantics without restoring Leaf's linear-scan cost | Exact candidate `38d965c2`, 200 concurrent tail decisions: 16K contiguous same-target suffix rules compile into one matcher and consume 9/9/12 ticks versus the 0-rule 9/9/11 baseline, with 1.19 s startup and 15.6 MiB RSS. Alternating-target suffix rules cannot currently be coalesced and consume 60/60/62 ticks at 16K. Preserve target/family/modifier/network boundaries, but do not claim zero overhead for arbitrary interleaved first-match policies. Compare a bounded ordered-block index with Mihomo before changing semantics; this bounded scale gap does not block the first functional release |
| POLICY-DOMAIN-KEYWORD-SCALE | post-v1-optimization | Large custom/GeoSite keyword sets must use a mainstream multi-pattern matcher | Exact candidate `38d965c2` incorrectly folds escaped keywords into one `RegexSet`: 5K and 16K same-target keywords consume about 4.17 CPU seconds per 200 tail decisions and reach about 59.8/117.7 MiB RSS. Replace keyword matching with a bounded Aho-Corasick or equivalent indexed matcher; keep real regex entries in bounded RegexSet chunks. Re-run 1K/5K/16K correctness, startup, RSS and CPU comparisons against Mihomo. Keep this optimization active, but do not couple it to first-release lifecycle fixes |
| POLICY-GEOSITE-LOAD-SCALE | post-v1-optimization | Selecting many or all GeoSite categories must have bounded startup time and RSS | Exact candidate `38d965c2`: all 1,473 categories targeting one actor scan once and start in 1.16 s at 25.6 MiB RSS with near-baseline match CPU. Alternating targets produce one compiled rule per category, rescan the same DAT repeatedly, take 36.8 s to start and then expose the keyword/top-level scan cost. Cache each validated GeoSite file once per config load and project selected categories into ordered matchers without changing first-match behavior; compare all-category same/alternating/mixed policies with Mihomo after the first functional candidate is frozen |
| POLICY-GEOSITE-REGEX | post-v1-optimization | GeoSite regex entries must not be silently ignored or make large keyword sets pathological | Leaf fork `08c046b4` plus build fix `73e8caa2` preserves regex-domain entries, but combining regexes with escaped plain keywords in one `RegexSet` failed the large-keyword acceptance benchmark. Separate keyword and regex engines, bound regex set compilation, validate all 364 regex entries in the pinned snapshot against Mihomo category results and reject invalid/oversized data without panic. Incorrect regex semantics remain blocking; matcher throughput tuning does not |
| POLICY-RULES | implemented-validation-pending | Ordered first-match GEOIP/GEOSITE/COUNTRY/IP/domain rules | Exact Android `38d965c2` A/B proved `MATCH,DIRECT` permits both probes, `MATCH,REJECT` blocks both, and `GEOIP,CN,DIRECT,no-resolve` permits the CN probe while the following `MATCH,REJECT` blocks the non-CN probe. Still compare domain ordering, per-rule resolve, missing category, duplicate resource and UDP actor-skip behavior with the pinned Mihomo references before closing |
| POLICY-PREFLIGHT-MODIFIER | implemented-validation-pending | Android preflight treated trailing `no-resolve` as an actor and panicked on a missing group | `ed0f36d3` extracts the actor before optional modifiers and makes unknown actor capability queries return `UnknownReference` instead of indexing. Require policy unit tests plus real Android `GEOIP,CN,DIRECT,no-resolve` preflight/start without process restart or SIGABRT |
| POLICY-NO-RESOLVE-SEMANTICS | investigating | `no-resolve` currently changes compiler merge boundaries but generated Leaf config fixes `domainResolve=false`, so GEOIP/COUNTRY/IP-CIDR rules with and without the modifier behave identically for domain destinations | Mihomo resolves lazily at the first eligible IP rule and caches the result for later rules. Add an order-preserving per-rule resolve marker in the pinned Leaf fork: rules without `no-resolve` may resolve once at their exact position; rules with it must not trigger resolution. DNS failure continues scanning; direct IP destinations never resolve. Validate domain-before-IP, IP-before-domain, mixed modifiers, one-resolution-only and Android FakeDNS paths |
| POLICY-EDITOR | implemented-validation-pending | GUI/RPC policy switch, nodes, groups, rules, fallback and rule-data controls | Desktop and Android round-trip must preserve order and explicit IDs/IPs, show validation diagnostics before start, update managed data only on explicit click and never mutate unrelated EasyTier settings |
| POLICY-CONFIG | implemented-validation-pending | Opt-in startup and configuration across CLI, TOML, RPC, GUI and Android import | Disabled/absent mode must create no policy task, route, bridge or worker; enabled mode must use the same validated document and diagnostics; malformed or unsupported platform configuration must fail policy startup without breaking the mesh |
| POLICY-CHAIN | implementation-in-progress | DIRECT, SOCKS, mesh actor, chain and stable fallback | Exact profiling-beta `05a093e2` isolated Linux tests validate the shared TCP/UDP semantics: one gated comparison may rescue only its triggering session; persistent degradation occurs only on the third spaced preferred-failed/backup-succeeded round; recovery occurs only on the third spaced preferred-success round. With both actors stopped, a 50-request TCP burst generated no additional actor packets and the UDP burst generated only one/two primary/backup packets before the bounded gate; the UDP core remained at 31 FDs, 11 threads and 16.5 MiB RSS. This proves bounded Outage behavior and no request-rate retry storm for that artifact. It does **not** close network-generation semantics: the Leaf `StableFailover` state currently has no explicit generation token, so Linux route replacement that keeps an underlay available can retain old streak evidence. Android rebuilds the in-process runtime on a changed network key, but repeated roam/DHCP and exact evidence reset still need device validation. Until the generation gap is fixed, do not claim that observations from rapid Wi-Fi/cellular changes can never combine. TCP/UDP groups share one runtime comparison gate while keeping separate health state. With remote KCP input disabled, an older exact candidate opened no new KCP sessions, TCP fell back to smoltcp at 252 Mbit/s and UoT UDP sustained 20 Mbit/s with zero loss. Still implement and validate generation-aware evidence invalidation, a real multi-hop chain, simultaneous outage/recovery, many-group retry bounds and UDP actor-skip ordering. |
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
   `POLICY-DNS-SPLIT`, `NETNS-ADAPTER` and `SPLIT-DNS` remain explicit future
   work and cannot be
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

- one member fails while another succeeds: one group-owned comparative attempt
  may rescue the current connection, but selection stays pinned until three
  differential rounds over at least 30 seconds confirm member-specific failure;
- all members fail or platform network changes: enter Outage, freeze preference,
  and do not churn through nodes;
- network returns: probe the original preferred member first; require three
  successful rounds plus a 30-second hold-down before upgrading from a genuinely
  degraded member;
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
- the group remains pinned to its current preferred member while that member is
  healthy or merely suspect. One application failure must never change the
  group selection;
- at most one group-owned comparative attempt may try the next configured
  member after the preferred member's bounded connect/setup failure. Other
  callers fail fast; they do not each walk the group;
- one comparison or outage probe has one group-level `fail_timeout` budget;
  actor count cannot multiply the timeout. Within that budget alternatives are
  tried in configured order;
- a successful comparative attempt may rescue that individual new TCP
  connection or UDP association, but it is only one differential observation
  and does not globally switch the group;
- after that rescue the group enters `Suspect`: later new sessions do not
  inherit the backup and do not each retry the full group. They fail fast until
  the next 15-second observation point, where exactly one gated request may
  compare the pinned actor with an alternative;
- globally degrade the preferred member only after three consecutive,
  independent failed observation rounds spanning at least 30 seconds, with a
  lower-priority member succeeding in each corresponding round. That same-round
  success is the minimum positive evidence that the local path is usable; a
  preferred-member failure by itself never advances failover;
- all three differential rounds must belong to one unchanged, platform-reported
  usable `network_generation`. Wi-Fi/cellular handoff, route replacement,
  airplane mode, an elevator/no-route interval, or any other generation change
  invalidates in-flight and accumulated differential evidence instead of
  rotating actors;
- any preferred-member success resets its consecutive-failure count. A round
  in which every attempted member fails is an outage signal, not a member
  failure, and does not advance the count;
- TCP and UDP classify failure to establish the configured proxy transport as
  actor-unavailable. Destination refusal, ACL/policy and other business errors
  remain outside actor health, so a dead destination cannot rotate exits;
- recover/upgrade only after three consecutive successful health rounds and a
  minimum 30-second hold-down. A successful preferred-member probe may rescue
  that one new session, but it does not immediately change the actor used by
  later sessions;
- comparisons are application-demand driven rather than periodic background
  traffic. A suspect or degraded group permits at most one comparison every 15
  seconds, which yields three independent rounds over at least 30 seconds;
- add 0-500 ms jitter before any later background health-check implementation
  so nodes do not probe simultaneously. The v1 demand-driven path must not add
  a wakeup timer merely to satisfy this future requirement;
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

- fail policy traffic closed immediately while retaining the last stable
  selection as lifecycle state where the platform adapter can preserve it;
- cancel/ignore health results and connection attempts created under the old
  generation;
- do not increment member failure counters;
- preserve configured order, current preferred member, and previous health;
- do not walk every fallback member for each new application connection;
- keep existing streams/UDP associations until their normal owners close them;
- rebind/protect native sockets for the new platform network before probing.

The current desktop candidate preserves the running Leaf runtime and its
pinned actor when both the old and refreshed underlay remain usable; it updates
the source-bypass routes in place. It stops Leaf only when the underlay becomes
unusable and starts it once after recovery. Android must rebuild the in-process
runtime when VpnService reports a different underlying network/DNS generation;
that rebuild currently starts from configured actor order rather than restoring
the previous in-memory actor. Treat Android actor-state handoff as a validation
gap: the rebuild itself must remain one-shot and must not convert the outage
into accumulated member failures or a retry storm.

Consensus failure also enters `Outage`: when all previously usable members fail
in the same health window and there is no independent evidence that local
connectivity is healthy, freeze member state instead of degrading all members.

During `Outage`:

- new policy connections fail fast or wait behind one bounded readiness gate;
  they must not create a retry storm;
- run at most one jittered recovery probe for the group, with backoff
  `1s -> 2s -> 5s -> 10s -> 30s -> 60s`;
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
member reaches its normal three-round, minimum-30-second failure threshold. If
all members still fail together, remain in `Outage` and do not churn. The final
cross-platform handoff contract must clear old-generation in-flight
observations while preserving the pinned member and its last stable health; a
generation change must never count as a proxy failure or an automatic reason to
select another member. The Android runtime-rebuild gap documented above remains
open until that state can be handed across runtimes without adding public
configuration or process-global unbounded state.

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

The working-tree candidate represents the following two-set design, but it is
not considered complete until the exact profiling-beta artifact passes packet
capture and failure validation:

```yaml
dns:
  fake-ip: true
  direct:
    - "direct:system"
    - "direct:223.5.5.5"
  proxy:
    - "doh:dns.cloudflare.com@1.1.1.1"
```

Routing is decided from the queried domain first. Destinations classified
DIRECT by a pre-resolution matcher (`DOMAIN`, `DOMAIN-SUFFIX`, `GEOSITE`, and
equivalents supported by the compiler) use the direct resolver set, while
destinations classified to a proxy use the proxied set. `GEOIP` cannot by
itself classify a not-yet-resolved domain; it remains useful after resolution,
but a policy requiring split resolution must include an earlier domain/GEOSITE
rule or rely on `MATCH`. FakeDNS/DNS sniffing then preserves the domain when the
application later connects to an IP. Until `POLICY-DNS-SPLIT` is packet-capture
validated, the candidate must not be described as leak-free domestic/foreign
split DNS.

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
  Candidate `8e85c83b` confirmed GeoSite and GeoIP matcher CPU at the MATCH
  baseline, but also proved that Leaf still traverses top-level rules in order.
- Preserve that ordered first-match contract while compacting provider-like
  blocks: only adjacent rules with the same target, matcher family, optional
  `no-resolve` state and effective network capability may share one Leaf rule.
  A target change, Domain/IP boundary, modifier change or non-mergeable matcher
  terminates the block. This gives large same-target custom domain/IP lists the
  same indexed shape as a Mihomo domain/ipcidr rule provider without reordering
  user policy. `POLICY-RULE-SCALE` owns the 1K/5K/16K acceptance benchmark.

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

## Android upgrade validation and static TUN bootstrap (2026-07-14)

- Scope: current release-candidate validation is Linux and Android only. The accidentally started macOS ARM64 run was canceled after the validation scope changed; it is not candidate evidence.
- Exact candidate: commit `08b769b48ea7274402ac1480642ff1bc5c084ea9`; Linux profiling run `29344168875` and Android policy-candidate run `29344168863` both succeeded.
- Linux evidence: the verified x86_64-musl bundle was deployed under a unique path on `192.168.2.160`, `192.168.1.37`, and `192.168.1.38`; outer/internal SHA-256 values, commit, build metadata, target, symbols, and build IDs matched. `easytier-core --version` reported `2.6.10-08b769b4` on all three hosts.
- Android evidence: `adb install -r` upgraded an existing policy-candidate installation. The installed APK SHA-256 matched `f27a8a7a2fe99cf10dd5cf6de54f0c4cde5e23dcae7e85b538e848c0cb8150b7`; package data archives were byte-identical before and after upgrade; VPN permission remained granted without prompting; cold launch did not silently reclaim VPN ownership.
- Android data-plane evidence: after direct plugin recovery, Android `10.245.0.2` and the internal peer `10.245.0.1` passed bidirectional ICMP. The full `collect_network_info` request remained pending for more than 20 seconds, which blocked automatic TUN attachment even though the core had connected to the peer.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::New` constructs the TUN listener directly from validated options and uses deferred cleanup for partial startup; `server.go::Close` releases the stack, TUN, redirect, monitors, and package manager; `server_android.go::buildAndroidRules` and `getPackageManager` isolate Android-specific integration. No directly corresponding sing-tun lifecycle test was found in the local Mihomo tree.
- EasyTier compatibility boundary: EasyTier's Android `VpnService` owns the platform TUN FD while the core remains a separate component. Therefore the adapter may bootstrap only a non-DHCP TUN from validated `virtual_ipv4`, `network_length`, and existing static/policy route derivation. DHCP still requires runtime-assigned state and fails closed. This does not change core RPC/status semantics, Leaf behavior, desktop behavior, rule ordering, or VPN permission handling.
- Implementation: `mobile_vpn.ts::applyNetworkInstanceChange` bypasses full status aggregation only when `getStaticVpnBootstrap` accepts a non-DHCP configuration. `vpn_routes.ts::getStaticVpnBootstrap` validates the IPv4 address and prefix and reuses `getRoutesForVpn`; parity tests cover static success and DHCP/malformed fail-closed behavior.
- Status: implementation complete, Linux/Android candidate rebuild and real-device upgrade validation pending. Screenshots and simulated clicks are reserved for final UI verification; routine Android checks use ADB, CDP, shell, package manager, network diagnostics, and direct Tauri/plugin calls.

## External Leaf validation lifecycle boundary (2026-07-15)

- Finding: `easytier-policy/src/leaf_process.rs::LeafProcessRuntime::start` wrote a private generated config and then awaited the external Leaf worker's `-T` validation without a deadline. A hung executable could block policy application indefinitely; an execute/spawn error returned through `?` without deleting the generated config.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/hub/hub.go::Parse` calls `/Users/fanli/Documents/mihomo-rev/hub/executor/executor.go::ParseWithBytes` and returns before `ApplyConfig` on parse failure. `/Users/fanli/Documents/mihomo-rev/config/config.go::ParseRawConfig` completes parsing before publication and uses `defer rollback()` for temporary general settings. The local Mihomo tests exercise parsing through `/Users/fanli/Documents/mihomo-rev/test/clash_test.go`, but no external validator-process timeout exists because Mihomo parses in-process.
- Externally observable semantics followed: invalid or incomplete configuration is never published; temporary validation state is rolled back on every error path; the currently active policy runtime remains untouched when candidate validation fails.
- Intentional EasyTier difference: EasyTier validates a generated configuration through the pinned external Leaf executable on desktop Linux. `run_leaf_config_validation` therefore adds a 30-second process-boundary deadline and `kill_on_drop(true)`, while `LeafProcessRuntime::start` removes the private config on timeout and execution failure. This boundary is isolated from the policy supervisor and from Android's in-process Leaf runtime.
- Tests: `config_validation_timeout_is_bounded` covers a non-returning validator; `validation_execution_failure_removes_private_config` covers cleanup when the executable cannot be started. Existing transactional supervisor tests continue to define publication behavior.
- Deferred to the next version: replacing forced child termination with a graceful SIGTERM/readiness protocol. The pinned Leaf worker API currently provides no proven cross-platform readiness handshake or bounded graceful-shutdown contract; inventing one would add lifecycle risk for little initial-release benefit. Existing parent-death handling, `kill_on_drop`, explicit wait, and config cleanup remain the safer compatibility boundary.
- Status: implementation complete; remote Linux test compilation/execution and Linux/Android candidate validation pending as part of the next batched build.
- Validation history: exact candidate `80f3ff16ae34a7d8ea1fb7f8b29a944e9cbfad89` failed Linux run `29349109228` and Android run `29349109273` during the shared `easytier-policy` test compile. The failure was confined to the new cleanup test passing `&String` where `PolicyRevision::parse` requires ownership convertible to `Arc<str>`; production code was not reached. The failed snapshot was explicitly reverted by `0166125b02ce88576b81d688a6c9ef1318bf6ae7` per rolling-beta policy. The corrected candidate passes the generated `String` by value; rebuild evidence is pending.
- Frontend validation history: rollback candidate `0166125b02ce88576b81d688a6c9ef1318bf6ae7` retained the independently committed relay-latency display and exposed a TypeScript contract gap in Android run `29349397542`: protobuf/Rust route field `path_latency_latency_first` was normalized and rendered but absent from the hand-maintained `easytier-web/frontend-lib/src/types/network.ts::Route` interface. The frontend change was explicitly reverted on the rolling branch by `4740e08b614dc129ef68b144e56556f9112830a3`. The corrected candidate adds the optional field to the stable interface rather than weakening access through `any`; existing snake_case/camelCase normalization and relay-only display tests remain the compatibility evidence.
- Rollback validation: exact full rollback `4740e08b614dc129ef68b144e56556f9112830a3` succeeded in Linux profiling run `29349772028` and Android policy-candidate run `29349772056`, proving the rolling branch returned to the previously buildable baseline before the corrected batch is republished.
- Remote diagnostic evidence: the current corrected source was synced to `192.168.2.160` and `cargo test --no-run --locked --package easytier-policy --features leaf-inprocess` was attempted in GNU debug mode with all required preflight, timeout, and log-separation rules. The first attempt exited with timeout code `124` while only updating the pinned `https://github.com/lovitus/leaf.git` dependency because the SSH invocation omitted the required reverse forwarding of the maintainer's `127.0.0.1:7890` proxy. Rust compilation never started, so that attempt is not code validation evidence. The corrected diagnostic must use `-R 7890:127.0.0.1:7890`; GitHub remains authoritative for deployable candidates.
- Corrected remote diagnostic: with `-o ExitOnForwardFailure=yes -R 7890:127.0.0.1:7890` on the same SSH command, `192.168.2.160` compiled `cargo test --no-run --locked --package easytier-policy --features leaf-inprocess` successfully in GNU debug mode in 1m38s. Direct execution of `/workspace/target/debug/deps/easytier_policy-9054d8d7e9b41fa3` passed `config_validation_timeout_is_bounded` and `validation_execution_failure_removes_private_config` independently (1 passed, 0 failed, 66 filtered out each; approximately 0.05s each). This validates the targeted lifecycle logic only; deployable Linux/Android evidence still comes from GitHub.

## Candidate cf405e80 build and artifact handoff (2026-07-15)

- Exact candidate: `cf405e8041dd58044dcdab96905ab283f57c3e97`.
- GitHub build evidence: Linux profiling run `29352190049` succeeded; Android policy-candidate run `29352189979` succeeded. No macOS workflow was started.
- Release metadata: `profiling-beta` targets the exact candidate. The Linux release asset reports size `116959214` and SHA-256 `d74d573b15ce4d26a384574aaf806ed31d08af2b29a793b369b639e7d14f1cd3`; release `SHA256SUMS.txt` reports the same digest. The Android artifact is `easytier-android-policy-candidate-aarch64-cf405e8041dd58044dcdab96905ab283f57c3e97`, artifact ID `8319127623`, reported size `31339676`.
- Download safety finding: the first Linux release download produced only `2506031` bytes with SHA-256 `b9fb6b742c59f49eceeb054fd14db7f5db070d3fb813abed2d68a350fd082756`, so validation stopped before extraction. Subsequent release/API and Actions artifact attempts encountered very low single-stream throughput or TLS handshake timeouts; bounded retries were stopped. The Android download similarly did not produce an extracted file within the bounded window. No truncated or unverified file was extracted, deployed, or installed.
- Builder capability finding: `192.168.2.160` successfully compiled and ran the targeted Rust lifecycle tests when the `7890` reverse forward was present, but its container has no Node/pnpm and the host has no `gh`; no tools were installed or container state changed to work around that boundary.
- Validation state: code compilation and targeted lifecycle tests are green; deployable Linux artifact verification/deployment and Android APK verification/upgrade remain pending until the GitHub artifact download path is healthy. The previously validated `08b769b4` deployment remains untouched.
- Download probe boundary: a bounded 1 MiB HTTP Range probe against the public Linux release asset, explicitly through `127.0.0.1:7890`, returned HTTP `000`, downloaded `0` bytes, and ended with `SSL connection timeout`. Parallel range download was therefore not started. Artifact handoff remains pending rather than weakening checksum or transport requirements.
## Candidate `cf405e80` artifact and real-device validation

Validation snapshot: `cf405e8041dd58044dcdab96905ab283f57c3e97` on `codex/profiling-beta`.

### Authoritative workflow results

- Linux profiling workflow run `29352190049`: success.
- Android policy-candidate workflow run `29352189979`: success.
- No macOS workflow was run; the current validation scope remains Linux and Android.
- The remote builder was used only for targeted GNU debug diagnostics. The deployable Linux and Android artifacts below came from GitHub workflows.

### Remote-builder diagnostic

- The first dependency-fetch attempt omitted the required reverse proxy and timed out; this was an operator error and is not validation evidence.
- The corrected SSH command used `-o ExitOnForwardFailure=yes -R 7890:127.0.0.1:7890` in the same connection as the container command.
- `cargo test --no-run --locked --package easytier-policy --features leaf-inprocess` completed successfully in GNU debug mode with all cores, incremental compilation, and the required timeout.
- Test binary: `/workspace/target/debug/deps/easytier_policy-9054d8d7e9b41fa3`.
- Direct execution of `config_validation_timeout_is_bounded` and `validation_execution_failure_removes_private_config` passed independently: one test passed in each invocation.

### Linux artifact acquisition and integrity

- GitHub's CDN repeatedly produced slow or truncated whole-file transfers. A truncated `2,506,031` byte download with SHA-256 `b9fb...2756` was rejected before extraction or deployment.
- A bounded 1 MiB HTTP Range probe through local proxy port `7890` returned `206 Partial Content` with the expected total size. The artifact was then downloaded as 16 fixed, disjoint ranges; every range length was checked before assembly.
- Artifact: `easytier-profiling-beta-linux-x86_64-musl.tar.gz`.
- Exact size: `116,959,214` bytes.
- Outer SHA-256: `d74d573b15ce4d26a384574aaf806ed31d08af2b29a793b369b639e7d14f1cd3`, matching both release metadata and `SHA256SUMS.txt`.
- `BUILD_INFO.txt` records commit `cf405e8041dd58044dcdab96905ab283f57c3e97`, ref `codex/profiling-beta`, run `29352190049`, run number `120`, target `x86_64-unknown-linux-musl`, and Rust/Cargo `1.95`.
- Internal SHA-256 values matched: core `86431a5e11078c4a9d25bf7c6dc41039d139f6015dabaff7fe96ac367df80f4e`, CLI `27b4381a764be2f4a14cbfc7d2b5cdf7bac1c65bf83999dfede12cdd782b8b93`, Leaf worker `550559e24d9ce64736e17229f66122ab74ed70f9627e270a208989dba965a5ae`.
- Local ELF inspection identifies x86-64 static PIE binaries with debug information and symbol tables. Build IDs are core `577a3aa1018f40d27fa6c4a27560f46bc44cdc21`, CLI `c83d0ef6ac82d64551ffebee1c4bc0ec5b420f2d`, and Leaf worker `698218fef6acc3860a92526ccef8261e0218beea`.

### Linux isolated-host deployment

- The exact verified archive was copied with a temporary `.part` name to `192.168.2.160`, `192.168.1.37`, and `192.168.1.38` in parallel.
- Each host verified the outer SHA-256 before rename and extraction into the isolated path `/tmp/easytier-policy-cf405e80`.
- Each host independently matched all three internal binary hashes and the exact commit, workflow run, and musl target in `BUILD_INFO.txt`.
- `easytier-core --version` returned `easytier-core 2.6.10-cf405e80` on all three hosts.
- No EasyTier service was started and no production process was modified. The old CentOS `file` utility describes the PIE as dynamically linked; this conflicts with authoritative local ELF inspection, while the exact musl artifact executes successfully on those hosts.

### Android artifact integrity and upgrade

- Artifact name: `easytier-android-policy-candidate-aarch64-cf405e8041dd58044dcdab96905ab283f57c3e97`.
- The ZIP was downloaded in 16 checked fixed ranges after a successful 1 MiB `206 Partial Content` probe.
- ZIP size: `31,339,676` bytes; all ZIP CRC checks passed.
- `BUILD_INFO.txt` records the exact candidate commit, workflow run `29352189979`, target `aarch64-linux-android`, debug profile, application ID `com.kkrainbow.easytier.policycandidate`, and the expected signing certificate.
- APK size: `87,971,123` bytes; SHA-256 `053a5dbf0fbb3ab6df024af990fdf9c1bcb056d4f31e2ba04023b18e79185a4f`.
- `apksigner` verified APK Signature Scheme v2, one RSA-3072 signer, and certificate SHA-256 `14d2d885...c38e0` matching `BUILD_INFO.txt`.
- Before upgrade, the installed candidate was running with `tun0` at `10.245.0.2/24`. Its configuration was backed up to `pre-upgrade-config.tar` with SHA-256 `a4cdf66848dbdf04b5648b014e2f9d7a31af4627bb4cbfd44e314d496f4b0b6d`.
- After upgrade, the on-device APK hash exactly matched the downloaded candidate; first-install time was preserved, last-update time changed, the old process stopped, and `tun0` was removed.
- The post-install, pre-launch configuration archive was byte-for-byte identical to the backup. Android reported `revoked_by_system=false`.
- Cold launch used the package-manager-resolved component `com.kkrainbow.easytier.policycandidate/com.kkrainbow.easytier.MainActivity` and completed in 764 ms. Launch created only the activity/WebView and foreground service: it did not silently reclaim the VPN or create `tun0`.

### Android static bootstrap and mesh evidence

- Preserved WebView local storage contained `app_mode`, `lang`, `last_network_instance_id`, and `networkList`, including the non-DHCP `10.245.0.2/24` instance and its policy rules.
- Tauri command inspection established that `run_network_instance` requires `{ cfg, save }`. Calls using incorrect argument shapes failed at argument validation and did not start the network.
- Calling `run_network_instance` with the preserved configuration and `save:false` succeeded without a manual `start_vpn` call.
- Timing from logs: post-run event at `02:46:34.025`, non-DHCP configuration loaded at `.036`, virtual IP updated at `.037`, VPN start requested at `.038`, Android plugin `startVpn` entered at `.042`, start response at `.191`, and FD/network event at `.197`. The static bootstrap therefore begins VPN setup approximately 13 ms after the event/config path and no longer waits for runtime status aggregation.
- `tun0` was automatically created as `10.245.0.2/24`. Plugin status reported `running=true`, IPv4 `10.245.0.2/24`, routes `0.0.0.0/0` and `::/0`, and `revokedBySystem=false`.
- Overlay connectivity passed in both directions: Android to `10.245.0.1` was 5/5 with 0% loss and 16.373 ms average; `192.168.1.37` to Android `10.245.0.2` was 5/5 with 0% loss and 83.107 ms average.

### Known limitations and deferred follow-up

- ADB-shell pings to public addresses succeeded, but the shell UID bypasses the application VPN. These results are explicitly invalid as policy-routing evidence and must not be cited as a Leaf policy pass or failure. A future policy validation must originate traffic from an app UID captured by the VPN or use a controlled in-app probe.
- The DOM still displayed `No Network Selected` despite a valid preserved `last_network_instance_id` and a successfully running instance. This is a frontend state-restoration defect, not a core/VPN startup failure. It is deferred to the next frontend batch to avoid forcing another full artifact build after the current safety batch passed.
- The single final Android screenshot was entirely black. Because semantic WebView inspection, process state, plugin status, logs, and network checks remained available, the screenshot is recorded as inconclusive visual evidence rather than a UI pass or failure. No simulated click was used.
- The external Leaf validation timeout and cleanup behavior has direct targeted-test evidence. End-to-end policy routing from an Android application UID remains pending.

#### Android application-UID probe investigation

- `TauriVpnService.createVpnInterface` adds every configured disallowed package plus its own `packageName` through `Builder.addDisallowedApplication`. Android's active VPN record independently confirms owner UID `10254` is excluded while ordinary application UIDs are captured. Consequently, `run-as com.kkrainbow.easytier.policycandidate` would bypass the VPN and is not valid policy evidence.
- The active VPN UID ranges were `{0-10251, 10253, 10255-20251, 20253, 20255-99999}`. Via browser UID `10207` and Termux UID `10229` are therefore included; owner/profile UIDs `10254` and `20254` are excluded as expected.
- Termux is not debuggable. Its exported `RunCommandService` requires the dangerous `com.termux.permission.RUN_COMMAND` permission, which ADB shell does not hold. The service correctly rejected the invocation; no probe ran.
- The first Via automation command also did not run because an ADB command plus arguments was incorrectly stored as one zsh scalar. This was an operator-script error and produced no device or product result.
- Corrected Via launches used an independent captured UID and no coordinate click. However, the device was PIN-locked: both activities remained `isSleeping=true`, and UI hierarchy contained only SystemUI/keyguard nodes. `wm dismiss-keyguard` could not bypass the PIN. These launches are inconclusive and are not policy evidence.
- A non-destructive root-shell check returned `Permission denied`, so root cannot safely broker the protected Termux command service. No attempt was made to grant undeclared permissions, bypass the lock screen, or guess user credentials.
- Completing this evidence requires one of: an unlocked device for semantic browser inspection, a dedicated signed probe APK whose UID is included in the VPN, or a pre-authorized command runner such as a configured Tasker/Termux integration. Until then, `GEOIP,CN,DIRECT,no-resolve` versus `MATCH,REJECT` remains unverified on Android application traffic.

## Persisted selection and captured-UID probe batch

### Runtime evidence and frontend boundary

- Android WebView CDP showed `last_network_instance_id=c17a8c16-5016-4d09-a1c3-e97c6fddcaf5`, the same ID in the persisted `networkList`, and an empty selection control displaying `No Network Selected`.
- Direct Tauri invocation of `list_network_instance_ids` returned UUID words `3246033942`, `1343638793`, `2713971068`, and `1876806389`, which encode the same `c17a8c16-5016-4d09-a1c3-e97c6fddcaf5` value. The backend and persisted configuration therefore agree; the missing selection is a GUI initialization defect.
- `easytier-gui/src/pages/index.vue` initialized `instanceId` as undefined and restored it only on a later `clientRunning` false-to-true transition. Cold startup can mount `RemoteManagement` while the client is already running, so that transition is not a reliable initialization mechanism.
- The fix resolves the normalized persisted ID synchronously when the ref is created, before `RemoteManagement` mounts. Existing transition-based restoration remains as reconnect fallback. No Leaf, policy, RPC, or VpnService lifecycle semantics are changed.

### Reference test semantics

- Mihomo `/Users/fanli/Documents/mihomo-rev/test/util.go::TCPing` creates an external client socket with bounded retry and closes it immediately after success.
- Mihomo `/Users/fanli/Documents/mihomo-rev/test/clash_test.go::testPingPongWithSocksPort` separates the client socket, proxy handshake, controlled server, and bidirectional payload assertions. Externally observable semantics: traffic evidence comes from an independent client and a controlled result, not from the proxy core's own outbound request.
- Neither the local Mihomo tree nor `/Users/fanli/Documents/singbox-withfallback` contains an Android VpnService application-UID fixture. Sing-box tests similarly construct explicit client dialers and assert concrete connection outcomes, for example `/Users/fanli/Documents/singbox-withfallback/test/shadowtls_test.go`.

### EasyTier test-infrastructure difference

- Android routes VPN traffic by application UID, while EasyTier's VpnService intentionally excludes the candidate application's own UID. A test command under ADB shell or the candidate UID is therefore outside the policy data plane even if its target is reachable.
- The new `policy-probe` Gradle module is a test-only empty APK with application ID `com.kkrainbow.easytier.policyprobe`, `INTERNET`, `debuggable=true`, and no Activity, Service, Receiver, Provider, native library, or application code.
- Validation installs the exact probe artifact and invokes a bounded socket command with `run-as com.kkrainbow.easytier.policyprobe ...`. The process then uses an independent UID included in the active VPN range without adding production code or a persistent attack surface.
- The Android policy-candidate workflow builds and verifies both APKs in one run. It rejects a wrong package ID, non-debuggable probe, launchable activity, runtime component, missing INTERNET permission, missing signer digest, or incomplete checksums.
- This is not a Mihomo/sing-box compatibility claim. It adapts their independent-client and bounded-observation test semantics to Android's UID-routed VpnService boundary.

### Pending validation for this batch

- Run the focused persisted-selection Vitest in the existing frontend-lib harness.
- Let the authoritative Android policy-candidate workflow build both APKs and verify `BUILD_INFO.txt`, signer metadata, package IDs, manifest boundary, and checksums.
- Upgrade the candidate on `192.168.234.227:5555`, verify the selection is restored before interaction, install the exact probe APK, confirm its UID is captured, and test controlled DIRECT and REJECT targets with bounded `run-as` commands.
- Remove the probe APK after evidence collection. Do not use screenshots until final visual confirmation.

### Probe candidate A result: rejected

- The exact empty probe APK installed successfully as UID `10255`; its on-device SHA-256 matched `35ae376bb6b0f1327c1598eaac4d03ae984bb2f1ae9b5e46f2c4d32a82907ae2`, and `run-as` reported the expected UID plus the `inet` supplementary group.
- Despite the correct Linux UID, `run-as` executed in SELinux context `u:r:runas_app:s0:...`. ICMP returned `sendmsg: Operation not permitted`, loopback `127.0.0.1:5555` TCP timed out, and all bounded public TCP candidates timed out while the VPN was down.
- Android's package manager exposes no shell command here to transition the no-component package into a normal application process. More importantly, changing package stopped state would not change the `runas_app` execution domain.
- Therefore candidate A cannot generate policy evidence. The failed TCP/53 and TCP/443 observations are execution-domain failures, not DIRECT or REJECT results. Keeping `run-as` would create a systematic false-negative test and is rejected.

### Probe candidate B: target plus on-demand instrumentation

- Keep the empty target APK as the independent UID and add a separate `androidTest` runner APK. Android instrumentation runs the probe code in the target application's UID/process domain rather than `runas_app`, works without UI interaction, and requires no Activity, Service, Receiver, or Provider.
- `PolicyProbeInstrumentation` accepts `host`, `port`, and a bounded `timeout_ms` (100 to 10,000 ms), opens exactly one TCP socket, closes it immediately, and reports target UID, SELinux context, target, timeout, connected state, elapsed time, and normalized error.
- A failed connection is returned as a valid observation with successful instrumentation completion; invalid arguments remain an instrumentation failure. This prevents an intentional `MATCH,REJECT` result from being confused with a broken runner.
- The workflow builds and signs both target and runner in the same Gradle invocation, verifies package IDs and instrumentation target, rejects runtime components, requires matching debug signer digests, and includes both APKs in `SHA256SUMS.txt`.
- Candidate B requires one additional authoritative Android build. Linux behavior is unchanged; the already successful `f57f5599` Linux artifact remains valid for that exact snapshot, while the next snapshot must still complete the normal rolling workflow before deployment.

### `f57f5599` persisted-selection validation result: incomplete fix

- The exact `f57f5599` candidate upgraded successfully with byte-identical preserved configuration, then cold-launched in 783 ms without silently creating `tun0`.
- CDP still showed `No Network Selected`. The saved ID remained `c17a8c16-5016-4d09-a1c3-e97c6fddcaf5`, and the backend returned the same UUID in `disabled_inst_ids`; persistence and backend identity were not the failure.
- The first fix initialized the parent ref early, but `RemoteManagement` maps that string to an object-valued PrimeVue Select model while `instanceList` initially remains empty. The Select can emit an empty model before the async list arrives, and the computed setter then clears the valid parent ID.
- Corrected boundary: ignore only an empty Select write while `list_network_instance_ids` has not produced its first response, and update `instanceList` synchronously when that response arrives. Once the first response exists, explicit user clearing remains authoritative.
- A component-level regression test manually reproduces the early empty Select emission, asserts no `update:instanceId` is emitted, resolves a disabled-instance response, and asserts the matching object becomes selected.
- This frontend correction remains local until the instrumentation candidate workflow finishes, so any workflow fix and this UI fix can share one additional build snapshot.

### `494f7be6` instrumentation build result: package gate failure

- Linux profiling run `29363296275` succeeded for exact commit `494f7be6e5b1a83837b58f1b6670758a8bc82ab0`.
- Android policy-candidate run `29363296221` completed both `Build debug APK` and `Build captured-UID policy probe`; the custom instrumentation therefore compiled successfully.
- The run failed only in `Package exact candidate`. Three consecutive silent `aapt dump badging`/signer assertions made the exact mismatched field unobservable, so the failure must not be described as an instrumentation compile failure.
- Replace the brittle runner class/target assertions against badging text with explicit `aapt dump xmltree` checks for the instrumentation element, fully qualified class, and target package. Package ID remains checked through badging. Any future mismatch prints both badging and manifest; signer mismatch prints both digests.
- The known local TypeScript test annotation was corrected from invalid `typeof INSTANCE_UUID[]` syntax to `Array<typeof INSTANCE_UUID>` before submission.
- Batch the workflow diagnostics and the RemoteManagement startup-race correction into the next exact candidate snapshot.
- Run the helper and RemoteManagement regression specs before the expensive Android/Rust build in the same workflow, so a frontend regression fails fast without consuming a full candidate build.

### `505b03ce` instrumentation device result: target attachment rejected

- Android run `29364758408` and Linux run `29364758464` succeeded for exact commit `505b03ce89400e4e6c1aa30b67729658707d14da`; focused persisted-selection tests and all artifact packaging gates passed.
- The exact candidate and probe pair installed with matching on-device hashes. The target registered as UID `10256`, the runner as UID `10257`, and package manager reported the expected instrumentation target.
- VPN-down execution failed before probe code started: `INSTRUMENTATION_FAILED` with `Instrumentation target has no code: com.kkrainbow.easytier.policyprobe`. No socket attempt occurred, so this is not DIRECT/REJECT evidence.
- Cause: the target manifest explicitly set `android:hasCode=false`. Android requires a code-capable target process even when all executable test code lives in the separate instrumentation APK.
- Candidate B revision: remove the explicit false flag and keep the default code-capable application boundary. Do not add an Activity, Service, Receiver, Provider, native library, or production business class. The runner remains the only executable probe code.
- Add a package gate rejecting an explicit false `android:hasCode` value and record `probe_target_code_capable=true` in `BUILD_INFO.txt`. This is an Android instrumentation requirement, not an EasyTier policy behavior difference.

### Hidden WebView selection finding and correction

- Reload tracing on exact candidate `197f7a88` showed `last_network_instance_id` was read correctly and the `selectedInstanceId` setter never received an empty value. This contradicts the earlier hypothesis that PrimeVue cleared the parent model.
- Runtime state was `document.hidden=true`, `visibilityState=hidden`, and `hasFocus=false` because the device remained PIN-locked. `RemoteManagement.shouldRefreshNow` intentionally suppresses all RPC refresh while hidden, including the first instance-list load; an object-valued Select cannot display the saved string until that list exists.
- A semantic CDP visibility transition (`hidden=false`, standard `visibilitychange`) immediately loaded the disabled instance and displayed `android-policy-validation-17a8c165 (c17a8c16-5016-4d09-a1c3-e97c6fddcaf5)`, stopped state, full editable config, and both policy rules. No click, screenshot, storage write, or PIN bypass was used.
- Therefore the locked/hidden `No Network Selected` DOM is expected power-saving behavior, not a user-visible restoration failure. The early parent initialization and guarded Select setter remain defensive startup-race hardening with passing component tests, but must not be claimed as the cause of this observed recovery.

### Connect-only probe result: insufficient policy evidence

- Exact `197f7a88` target/runner executed in UID `10258` and SELinux `u:r:untrusted_app:s0:...`. VPN-down TCP baselines succeeded for CN `180.101.50.188:443` (37 ms) and non-CN `104.16.132.229:443` (169 ms); several Google/Cloudflare DNS endpoints were excluded after baseline timeout.
- After EasyTier created `tun0=10.245.0.2/24`, Android's VPN record included UID `10258` in `{10255-20251}` while excluding owner UID `10254`.
- TCP connect returned success for both expected `GEOIP,CN,DIRECT` (7 ms) and expected `MATCH,REJECT` (9 ms). The TUN stack completed local TCP handshakes before an end-to-end application exchange, so connect-only cannot distinguish outbound success from policy rejection.
- Those two VPN-up connect results are explicitly not rule evidence. Extend the same on-demand instrumentation with optional TLS SNI and certificate-verified handshake. Report raw TCP and TLS phases separately; `probe_connected=true` in TLS mode requires both phases.
- Controlled validation pair: `180.101.50.188:443` with SNI `www.baidu.com` for CN DIRECT, and `104.16.132.229:443` with SNI `www.cloudflare.com` for non-CN REJECT. Both must complete TLS while VPN is down before any policy conclusion.

## 2026-07-15 next safety batch working notes

- Mihomo reference for ordered UDP fallthrough: `/Users/fanli/Documents/mihomo-rev/tunnel/tunnel.go::match` iterates rules in order, skips a matched actor when `metadata.NetWork == UDP && !adapter.SupportUDP()`, and returns only the first matched actor that supports the session network. EasyTier diagnostics must therefore stop only after an unconditional UDP-capable rule; a TCP-only actor leaves later UDP rules reachable.
- Mihomo reference for rule-data replacement: `/Users/fanli/Documents/mihomo-rev/component/resource/fetcher.go::{Initial,loadBuf}` and `component/resource/vehicle.go::FileVehicle.Read` hash the exact bytes, parse them before replacing the active strategy, and retain the prior strategy on parse failure. EasyTier reuses one bounded rule-set verifier during policy validation and immediately before Leaf config construction; a missing, empty, oversized, or digest-mismatched dependency fails runtime construction while the supervisor retains the active revision.
- Unknown-field compatibility decision: Mihomo `/Users/fanli/Documents/mihomo-rev/config/config.go::UnmarshalRawConfig` uses permissive `yaml.Unmarshal`. Sing-box `/Users/fanli/Documents/singbox-withfallback/option/options.go::Options.UnmarshalJSONContext` calls `DisallowUnknownFields`. EasyTier v1 intentionally follows the strict behavior because its supported schema is smaller and silently accepting a Mihomo field would falsely imply implementation. Failure is a stable `policy.yaml` preflight error; this is not a claim that arbitrary Mihomo configuration is accepted.
- Integrity boundary: the compile-time recheck closes changes between preflight and runtime construction, including pinned SHA-256 changes. It does not make an arbitrary user-managed path immutable after Leaf opens the generated config. A fully race-free content-addressed snapshot for custom rule data is deferred to a later version because it requires lifecycle, cleanup, disk-budget, and ownership design rather than another ad-hoc copy.
- Deferred investigation: verify whether the current HTTP client can enforce HTTPS on every redirect hop for managed rule-data updates without replacing the downloader. Do not change redirect semantics until the library behavior and Mihomo/sing-box interoperability consequences are established.
- Validation correction: Linux profiling already runs the complete `easytier-policy --features leaf-inprocess` test suite; that feature graph indirectly enabled UUID v4 and hid a default-feature lib-test compile failure in `leaf_process.rs`. Replace the random test-only UUID with `Uuid::from_u128` and add `cargo test --package easytier-policy --lib --no-default-features --no-run` before the existing full-feature suite. This is a compile-surface gate, not a second workflow or duplicate full test run.
- Final `25a35f7c46210266b1a1a92a4bbf88ed5bd21b49` evidence: remote-builder default-feature `easytier-policy` tests passed 68/68 with the required `-R 7890:127.0.0.1:7890` tunnel; Linux run `29372533028` and Android run `29372533038` succeeded. Linux artifact outer SHA-256 was `728dbe980f7d67707613ca010e460413731133bb05311900fa3c709255f4991c`, core build ID was `cb9f7761103d0401232736492103861539e4709a`, and all internal hashes passed on `192.168.2.160`, `192.168.1.37`, and `192.168.1.38` as version `2.6.10-25a35f7c`.
- Exact Android candidate SHA-256 `9faa98b6fca4412427def11295581838f8ce7988a835c28b804d18401bb28e34` preserved application data across the upgrade. Probe UID `10262` was included in VPN UID ranges while owner UID `10254` was excluded. VPN-down TLS baselines succeeded for Baidu in 276 ms and Cloudflare in 490 ms; VPN-up Baidu completed TLS in 242 ms, while Cloudflare completed only the local TCP handshake and TLS timed out in 4049 ms. Overlay ping from `.37` to `10.245.0.2` passed 5/5. Probe packages and CDP forwarding were removed; the exact candidate network remains running.

## 2026-07-15 packet and lifecycle safety batch working notes

- GeoSite reference: Mihomo `/Users/fanli/Documents/mihomo-rev/component/geodata/router/condition.go::{NewSuccinctMatcherGroup,NewMphMatcherGroup}` supports only Plain, Regex, Domain, and Full. The MPH matcher returns an error for an unsupported enum while the succinct switch has no equivalent default error, making malformed-data behavior matcher-dependent. EasyTier rejects unknown protobuf enum values during managed-data validation before Leaf sees the file; this is a strict consistency boundary, not an added rule type.
- Packet boundary: Mihomo does not own an equivalent EasyTier mesh-versus-policy TUN demultiplexer, so there is no Mihomo packet classifier semantic to copy. EasyTier validates the IPv6 fixed header and declared payload length before reading the destination and rejects empty bridge datagrams; valid packet routing and first-match policy semantics remain unchanged.
- Lifecycle reference: Mihomo `/Users/fanli/Documents/mihomo-rev/hub/executor/executor.go::ApplyConfig` serializes configuration replacement under `mux`, brackets it with `tunnel.OnSuspend`/`OnRunning`, and resets resolver connections after replacement. Sing-box `/Users/fanli/Documents/singbox-withfallback/route/network.go::{Close,notifyInterfaceUpdate,ResetNetwork}` tracks shutdown with `StopTimeout` and resets connections/listeners after interface changes. EasyTier's production Linux/Android loops already fail closed and rebuild on underlay identity changes; this batch only ensures Leaf's blocking shutdown dispatch cannot exceed the existing in-process stop deadline.
- Deferred before future use: `PolicySupervisor` is currently exported and tested but not used by the production Linux/Android recovery loops. Its concurrent `apply` calls can complete out of order, and same-digest `apply` does not check a dead runtime. Fix and validate those semantics before wiring it into production; changing it now would not improve the active runtime and could falsely imply coverage.
- Deferred low-evidence hardening: do not add arbitrary GeoSite category/value/domain-count limits beyond the existing 64 MiB entry, 16K category, and file-size bounds until Mihomo/Leaf dataset compatibility and real memory profiles establish safe thresholds.

## 2026-07-15 Android application-UID policy validation (`12cae7b7`)

- Reference semantics: Mihomo `/Users/fanli/Documents/mihomo-rev/test/util.go::TCPing` uses a bounded, independent client socket; `/Users/fanli/Documents/mihomo-rev/test/clash_test.go::testPingPongWithSocksPort` separates client, proxy, and server and requires a concrete bidirectional outcome. Sing-box `/Users/fanli/Documents/singbox-withfallback/test/shadowtls_test.go` likewise uses an explicit client dialer and concrete protocol outcome. Neither tree contains an Android `VpnService` application-UID fixture, so this probe adapts their independent-client and bounded-observation principles rather than claiming Android fixture compatibility.
- Exact candidate artifact: commit `12cae7b74b0863a2fe9f24e7058f00c44328e919`, Android workflow run `29368137997`, artifact `easytier-android-policy-candidate-aarch64-12cae7b74b0863a2fe9f24e7058f00c44328e919`, candidate APK SHA-256 `85c7c55666a266a209187fa4336e7e9fbeac2ae17283936c94ecd47c8775d31a`.
- Probe isolation: target package `com.kkrainbow.easytier.policyprobe` ran as UID `10260` in SELinux domain `u:r:untrusted_app:s0:c4,c257,c512,c768`; instrumentation package `com.kkrainbow.easytier.policyprobe.test` had a different UID. The active VPN UID ranges included UID `10260` and excluded the EasyTier owner UID `10254`.
- Controlled VPN-down baselines completed both TCP and TLS: `180.101.50.188:443` with SNI `www.baidu.com` in 271 ms, and `104.16.132.229:443` with SNI `www.cloudflare.com` in 412 ms.
- VPN-up `GEOIP,CN,DIRECT,no-resolve`: the Baidu endpoint completed TCP and TLS in 193 ms.
- VPN-up `MATCH,REJECT`: the Cloudflare endpoint completed only the local TUN TCP handshake; TLS timed out after 4040 ms with `SocketTimeoutException: Read timed out`. Its successful VPN-down TLS baseline distinguishes policy rejection from endpoint failure.
- TCP connect-only is not valid policy evidence for this TUN stack because both DIRECT and REJECT can complete a local TCP handshake. The probe therefore reports TCP and TLS phases independently and uses completed TLS as the externally observable success condition.
- Earlier empty target APK `run-as` probing was invalid because it executed in the `runas_app` SELinux domain and could not establish the required independent application network path. The first instrumentation target also declared `android:hasCode=false`; the corrected target is code-capable while retaining no runtime components.
- The previously observed hidden-WebView `No Network Selected` state was caused by the PIN-locked device keeping `document.hidden=true`; `RemoteManagement.shouldRefreshNow` intentionally deferred the first list load. A standard `visibilitychange` restored the persisted selection. This observation does not prove that the defensive parent initialization or asynchronous Select guard caused the recovery; those remain regression-tested safeguards only.
- Compatibility boundary: this validates the current Linux/Android initial policy subset and first-match behavior for the two exercised rules. It does not establish untested Mihomo fields, protocol families, DNS/FakeDNS behavior, or unsupported policy semantics.
- Deferred optimization: a probe-only Android change currently rebuilds the full Linux and Android bundles. Decoupling the probe artifact or reusing an exact candidate artifact is low-risk build-pipeline work for a later batch; changing it during this validation snapshot would add cost without improving Leaf runtime safety.

## Remote rule-data mutation lock investigation (2026-07-15)

- EasyTier source boundary: `easytier/src/rpc_service/instance_manage.rs::InstanceManageRpcService::update_policy_rule_data` holds `NetworkInstanceManager::remote_mutation_lock` while the complete bounded download and atomic replacement run. `run_network_instance`, `retain_network_instance`, and `delete_network_instance` use the same lock. This prevents current update/delete/overwrite races but lets a slow remote source delay all remote instance mutations for up to the 120-second download deadline.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/component/resource/fetcher.go::Fetcher.Update` performs `Vehicle.Read` outside `loadBufMutex`; `Fetcher.loadBuf` then serializes hash comparison, parsing, file write, state publication, and callback. Its comment explicitly treats a hash changed between read and publication as a concurrent-update condition.
- EasyTier compatibility boundary: the manager map stores instances by UUID but exposes no immutable instance generation. A lock-free download followed only by an `instance still exists` check is vulnerable to ABA: an instance can be deleted and recreated with the same UUID, path, permissions, and configuration before the old request publishes. Config/path comparison is therefore insufficient evidence that the publication still belongs to the initiating instance.
- Decision for the initial release: keep the coarse global lock. Its bounded availability cost is preferable to stale rule data being attached to a replacement instance. Do not claim parity with Mihomo's per-resource publication lock.
- Deferred candidate A: assign an immutable generation token whenever `NetworkInstanceManager::run_network_instance` inserts an instance; capture it under the mutation lock, download to an unpublished staging file outside the lock, reacquire the lock, require the same generation and permissions, then atomically publish.
- Deferred candidate B: route each `(instance UUID, resource kind)` through a sequenced update actor that owns staging and publication, while delete/overwrite invalidates the actor generation. This gives better cancellation and latest-request semantics but adds lifecycle machinery not justified before update concurrency is measured.
- Validation status: investigation only. No lock behavior changed and no new test or build is claimed.

## In-process Leaf ownership hardening working notes (2026-07-15)

- Finding: `easytier-policy/src/inprocess.rs::InProcessLeafRuntime::start` manually closed its packet FD and removed its reserved runtime ID on ordinary returns, but a panic inside `leaf::start` could bypass both operations. `spawn_late_start_reaper` also used `expect` when creating its cleanup thread; an OS thread-creation failure could panic the caller and drop the only receiver and JoinHandle while a timed-out Leaf runtime was still capable of registering late.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/hub/hub.go::Parse` returns parse errors before `ApplyConfig`; `/Users/fanli/Documents/mihomo-rev/hub/executor/executor.go::ApplyConfig` serializes publication with `mux`. Mihomo has no external or embedded Leaf start thread, so it has no equivalent late-registration cleanup owner.
- Sing-box platform reference: `/Users/fanli/Documents/singbox-withfallback/box.go::Box.Start` calls the staged `start` sequence, invokes `Box.Close` on any startup error, and returns the error; `Box.Close` walks owned lifecycle services and aggregates close errors. Externally observable semantics followed: partial startup retains an explicit cleanup owner and startup failure is returned rather than converted into a process panic.
- EasyTier implementation: `RuntimeIdReservation` and `LeafPacketFd` now own the reserved Leaf ID and raw packet descriptor for the whole blocking runtime. Their `Drop` implementations run on normal return, Leaf panic, generated-config rejection, runtime-ID allocation failure, and OS thread-spawn failure. The FD guard preserves the existing device/inode identity check so it cannot close a descriptor number that Leaf already closed and the process reused.
- Late-start difference: EasyTier must repeatedly issue shutdown because the pinned Leaf API can register a runtime after the three-second readiness deadline and an earlier shutdown is lost. The dedicated reaper remains preferred and independent of Tokio runtime shutdown. Its ownership is held behind shared state until the new thread takes it; if thread creation fails, the exact receiver and JoinHandle are moved to a Tokio blocking worker instead of being dropped or panicking.
- Tests: `runtime_id_reservation_is_released_by_drop`, `packet_fd_guard_closes_an_unclaimed_descriptor`, and `late_start_reaper_retains_and_joins_start_thread` cover the new ownership primitives and reaper join path. Existing `starts_and_stops_with_unique_runtime_and_external_packet_fd` continues to cover normal lifecycle behavior. A deterministic OS thread-creation failure injection is not added because it would require a production spawn abstraction solely for a rare error path; ownership handoff remains directly reviewable but awaits remote compilation and runtime tests.
- Remote diagnostic evidence: every SSH step used keepalive options plus `-o ExitOnForwardFailure=yes -R 7890:127.0.0.1:7890`; cargo/rustc preflight reported `CLEAR`. The first `cargo test --no-run` invocation incorrectly requested `leaf-inprocess` from package `easytier`; Cargo rejected the unknown feature immediately with exit code 101 and did not compile source. This operator error is not code validation evidence.
- Corrected remote compilation: GNU debug `cargo test --no-run --locked` selected both libraries with `easytier/leaf-policy-proxy,easytier-policy/leaf-inprocess`, all available CPU cores, test opt/debug level zero, incremental compilation, and an 1800-second timeout. It succeeded in 5m00s and produced `easytier-25b64bd194fbb9fb` plus `easytier_policy-23c562ecc6280cad`.
- Exact remote tests: direct execution of `policy_rule_data::tests` passed 5/5 in 1.19s with 1494 filtered out. Direct execution of `inprocess::tests` at `--test-threads 1` passed 6/6 in 0.06s with 68 filtered out, including real in-process Leaf start/stop, reservation Drop, FD guard Drop, and reaper ownership/join.
- Source handoff note: local `rsync` failed before transfer with macOS `setpgid: Operation not permitted`; the same five explicit files were then transferred as a non-deleting `tar` stream. GNU tar ignored only macOS extended-attribute keywords. Remote `target`, unrelated source, and container state were not modified by synchronization.
- Validation status: implementation, Rust 2024 formatting, remote joint compilation, and targeted tests are complete. GitHub Linux/Android build, artifact integrity, Linux deployment, and Android lifecycle regression evidence remain pending as part of the combined batch.
## Candidate `97dbc4d4` final Linux and Android evidence (2026-07-15)

- Exact validated commit: `97dbc4d423eff364e3213428366b78c644817f9a`; Linux profiling run `29374617141` and Android policy-candidate run `29374617119` both succeeded. No macOS workflow was started.
- Linux artifact ID `8327681950` had outer SHA-256 `4f4d020726829d9bb43d986985734fd91192dd825fd643359a7461e3aea8a3e8`; metadata, commit, Rust/Cargo 1.95, symbols, and `x86_64-unknown-linux-musl` target matched. Core build ID was `4f777d4743a70301e829df705a8852710834a4e8`; core SHA-256 was `81d5e5573335066a967b3b2e28f225fd0ae9f71f05da851da4bfd17a25e37263`, CLI SHA-256 was `8a217b260613a5206d3309277a0713f083ca710734b55235a85763f9ce7bd47d`, and the pinned Leaf worker remained `550559e24d9ce64736e17229f66122ab74ed70f9627e270a208989dba965a5ae`.
- The exact verified musl artifact was deployed to isolated paths on `192.168.2.160`, `192.168.1.37`, and `192.168.1.38`; every host independently matched metadata and internal hashes and executed `easytier-core 2.6.10-97dbc4d4`. Production processes were not modified.
- Android artifact ID `8327725027` produced candidate APK SHA-256 `6b1ccf3185b3a7c1eb069d55aa9f56ad4a59ed20b6f52c5128feb775fa334137`. The captured target probe SHA-256 was `38169e...` at UID `10264`; runner SHA-256 was `d9818d...` at UID `10265`. The candidate owner UID `10254` was excluded while probe UID `10264` was included in the active VPN ranges.
- Android configuration archives before and after upgrade were byte-identical at SHA-256 `1a9141d22f70...e87e7`. Cold launch completed in 675 ms, direct CDP/Tauri automation started preserved instance `c17a8c16-5016-4d09-a1c3-e97c6fddcaf5`, and `tun0` became `10.245.0.2/24`; no screenshot or simulated click was used.
- Controlled VPN-down TLS baselines succeeded for Baidu in 291 ms and Cloudflare in 480 ms. VPN-up `GEOIP,CN,DIRECT,no-resolve` completed Baidu TLS in 207 ms. VPN-up `MATCH,REJECT` completed only the local Cloudflare TCP phase and timed out TLS after 4052 ms, which is the expected rejection evidence given the successful baseline. `192.168.1.37` reached Android over the mesh 5/5 with 0% loss.
- Compatibility boundary: this evidence covers the hardened GeoSite enum parsing, packet-length/empty-bridge checks, shared in-process shutdown deadline, and the already supported Linux/Android policy subset. It does not claim unimplemented Mihomo fields, DNS/FakeDNS parity, or deferred `PolicySupervisor` production semantics.

## Managed rule-data redirect boundary working notes (2026-07-15)

- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/component/resource/vehicle.go::HTTPVehicle.Read` calls `/Users/fanli/Documents/mihomo-rev/component/http/http.go::HttpRequest`. The latter constructs `github.com/metacubex/http` v0.1.6 `Client` without `CheckRedirect`, so `client.go::Client.do` resolves relative `Location` values before policy evaluation, shares the request context deadline across the chain, strips sensitive headers on cross-host redirects, and `defaultCheckRedirect` stops after ten requests.
- EasyTier dependency finding: pinned `EasyTier/http_req` commit `b10aa9fc0db3067cc3d2174683a87250b80a1ea9`, `src/request.rs::Request::send`, calls redirect policy on the raw `Location`, then recursively constructs `Request::new`. Redirect hops therefore lose the configured 120-second timeout and `User-Agent`, receive a fresh default one-hour timeout, and cannot compose `Limit(5)` with per-hop HTTPS validation. This behavior is unsuitable for remotely supplied policy data and is left unchanged for the independently scoped HTTP connector.
- Implementation boundary: only `easytier/src/policy_rule_data.rs` reuses the workspace-locked `reqwest` v0.12.12 blocking client. Its resolved `Attempt::url` is checked on every hop for HTTPS, host presence, no embedded credentials, and no fragment; five redirects are allowed, one 120-second timeout covers headers, redirects, and body, HTTPS-only transport is enabled, and automatic Referer propagation is disabled. User-configured public or private HTTPS hosts remain valid; no host allowlist or private-address ban is introduced.
- Intentional Mihomo difference: Mihomo accepts the general Go HTTP client's schemes and ten-request redirect chain. EasyTier retains its pre-existing HTTPS-only source contract and five-redirect ceiling because these files control policy behavior. Failure on an unsafe hop, excessive redirects, timeout, non-success status, declared or streamed size over 256 MiB, empty body, format validation, or digest/storage operations leaves the installed rule data untouched.
- Tests: `custom_sources_are_https_urls_without_credentials_or_fragments`, `redirect_targets_preserve_url_safety_and_five_hop_limit`, and `bounded_writer_rejects_oversized_input_without_partial_write` cover the local contract. The resolved-relative-URL, shared-timeout, cross-host header stripping, and redirect engine itself remain delegated to locked reqwest behavior rather than copied into EasyTier.
- Validation status: implementation and formatting pending; remote/GitHub compilation, unit tests, artifact integrity, Linux deployment, and Android regression evidence have not yet been collected. Do not treat this working note as validation evidence.
# d875d5cf exact-candidate qualification and next underlay batch

## Exact artifact evidence

- Commit `d875d5cf164419990205624e7ada95d794828943` was built by Linux profiling run `29377915892` and Android policy-candidate run `29377915883`; both jobs completed successfully against that full SHA.
- The Linux Actions artifact and rolling `profiling-beta` release archive were byte-identical. Their outer SHA-256 was `7bd060c9ee76cc48c1e4cd3b214fc9ac119f45d5b8273694056635d4678f03eb`; outer and inner manifests passed. `BUILD_INFO.txt` recorded run 129, `x86_64-unknown-linux-musl`, Rust 1.95.0 and the exact commit. Core build ID was `c8a264afe9fb74e053b958f1790cb026abd72530`; core, CLI and Leaf worker retained debug info and symbol tables. CentOS 7 `file` described static PIE as a shared object, but `ldd` authoritatively reported `statically linked`.
- All three Android artifact hashes passed their manifest. APK Signature Scheme v2 verification passed with one signer each; the candidate certificate and the shared probe/runner certificate matched `BUILD_INFO.txt` exactly. The build recorded the exact commit, run `29377915883`, `aarch64-linux-android`, and debug profile.

## Android real-device evidence

- Device `192.168.234.227:5555` was online as `arm64-v8a`. Before upgrade, VPN preferences and WebView Local Storage/IndexedDB were archived with `run-as`. `adb install -r` preserved the original `firstInstallTime`; the persisted WebView archive remained byte-identical before the first post-upgrade start. No uninstall, clear-data, screenshot, or coordinate click was used.
- A cold start completed in 760 ms. CDP read the unchanged saved instance ID, explicit listener ports, peer, virtual address, and inline policy. Tauri native invokes used that backend-owned config; VPN permission remained granted and no replacement config was constructed.
- The captured-UID probe ran as a distinct untrusted-app UID. Mesh TCP to the peer succeeded in 20-24 ms. A CN TLS endpoint matched `GEOIP,CN,DIRECT,no-resolve` and completed certificate validation in 121-190 ms. A certificate-valid non-CN domain succeeded without VPN in 1.1 seconds, then failed its TLS handshake with the policy enabled, proving terminal `MATCH,REJECT` rather than target unreachability.
- Two API-driven stop/start cycles removed and restored the TUN without losing the saved config. Mesh ping remained healthy after restart.
- A real Wi-Fi outage was performed only after scheduling autonomous Wi-Fi recovery. Native events emitted `outage!1` with no DNS, then a recovered Wi-Fi network key carrying epoch 1 and the restored DNS list. The application PID remained unchanged, TUN routing returned, mesh ping passed 5/5, and mesh TCP, CN DIRECT TLS, and terminal REJECT behavior all passed again.

## Linux 3.10 namespace evidence

- The exact archive was copied to `192.168.2.160`, re-hashed, extracted, and checked against its inner manifest before use. Two fresh namespaces on `10.250.129.0/24` used listener bases 25300 and 25310; no existing `.1.37` or `.1.38` validation process was replaced.
- The source started the exact external Leaf worker with `MATCH,DIRECT`, explicit `eth0`, and the exact artifact path. Initial resources were approximately 16.6 MiB RSS / 9 threads for core and 5.4 MiB / 6 threads for Leaf. Rules 10899/10900, table 52000, terminal unreachable, connected route, and main-table split defaults were present.
- Ordinary lookup for `203.0.113.10` selected policy TUN while the policy mark selected table 52000 through the physical gateway. A TCP SSH banner traversed Leaf DIRECT. Mesh ping to the destination delivered 5/5 at sub-millisecond RTT.
- Killing only the exact worker PID left mesh ping at 3/3. A new generation worker appeared during the second one-second poll and policy DIRECT immediately recovered.
- Removing the physical default route stopped the worker, left only connected plus terminal-unreachable policy routes, preserved mesh ping 3/3, and failed policy traffic closed. Restoring the route created generation 3, restored table 52000, DIRECT, and mesh traffic.
- Replacing one usable gateway with another usable gateway refreshed table 52000 while retaining the same worker PID; a new policy DIRECT connection still succeeded. This is evidence against restarting Leaf for every route-set change.
- SIGTERM of the source removed core, worker, custom rules, all table-52000 routes, split defaults, TUN, and every generation temp config. The destination and both namespaces were then stopped and deleted; no EasyTier process or test namespace remained.
- `--check-config` returned status 1 with no diagnostic for both policy and non-policy pure-CLI configurations, while both real starts succeeded. This mode is not used as qualification evidence and is a separate low-priority CLI diagnostic issue.

## Reference semantics established before the next behavior edit

- Mihomo network update reference: `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go` default-interface callback flushes `component/iface/iface.go::FlushCache` and calls `component/resolver/resolver.go::ResetConnection`; it preserves the policy runtime while invalidating interface and DNS transport state.
- sing-box network update reference: `/Users/fanli/Documents/singbox-withfallback/route/network.go::NetworkManager.ResetNetwork` closes tracked connections and notifies interface listeners; `/Users/fanli/Documents/singbox-withfallback/route/router.go::Router.ResetNetwork` then resets DNS. It is intentionally stronger than Mihomo.
- Pinned Leaf `b1e33b50e37ea3b396e3cee2a1d60bb0c599655c` exposes only runtime reload and shutdown in `leaf/src/lib.rs::RuntimeManager::reload/shutdown`. Reload rebuilds router, DNS configuration/cache, and outbounds, but does not provide a network-change or close-all API. It must not be presented as equivalent to Mihomo or sing-box network reset.
- Mihomo reject reference: `/Users/fanli/Documents/mihomo-rev/adapter/outbound/reject.go::Reject.DialContext` distinguishes immediate `REJECT` (`nopConn`) from delayed `REJECT-DROP` (`dropConn`). Pinned Leaf `leaf/src/config/conf/config.rs` maps `reject` to `drop`; `leaf/src/proxy/drop/stream.rs::Handler::handle` returns an immediate error, but the Android TUN path currently exposes that as a TCP handshake followed by read timeout. This remains safe fail-closed but is not Mihomo-compatible fast rejection.

## Next-batch candidates

1. **Preferred: rich underlay transition classification.** Extend the Linux policy routing snapshot to distinguish `Unchanged`, `RoutesChanged`, `IdentityChanged`, `Lost`, and `Recovered`. Continue in-place route reconciliation for route-only gateway/metric changes, as validated above. Restart only the Leaf runtime for stable interface identity or DNS-upstream changes, preserving EasyTier, TUN ownership, mesh traffic, and policy routing. Add pure transition tests plus an integration test proving route-only refresh keeps the generation while identity/DNS changes advance it once.
2. **Rejected candidate: restart on every successful route refresh.** It is simple but the usable-gateway validation proves unnecessary downtime and connection churn with no functional benefit.
3. **Rejected candidate: call pinned Leaf reload as network reset.** It does not close tracked connections and therefore cannot establish the Mihomo/sing-box externally observable semantics.
4. **Deferred candidate: add a close-all/reset-network API to the Leaf fork.** This could preserve the runtime more precisely than restart, but it couples EasyTier to new Leaf internals and needs Leaf-side connection ownership tests. Revisit after the initial release only if restart downtime is measurable.
5. **Deferred compatibility fix: fast `REJECT`.** Fixing the Android timeout requires a Leaf/netstack boundary change and parity tests, not an EasyTier-side duplicate rule engine. Current behavior is safe and bounded but less responsive, so record it for the next version rather than expanding the initial patch surface.

The preferred initial-release boundary is therefore: retain the proven outage/recovery restart, preserve route-only refresh in place, and add a narrow identity/DNS-triggered runtime restart only after its state model and tests are isolated from Leaf internals.
## Underlay transition implementation batch

- Implemented the preferred transition model as `PolicyUnderlayTransition`: route-only changes keep the current Leaf generation, removal of a previously usable address or replacement of the interface index is an identity change, and route availability has explicit lost/recovered states.
- A failed Linux interface/index/address/route snapshot now first reconciles to a fail-closed boundary before returning the diagnostic. Enabled IPv4/IPv6 families retain split capture, a marked lookup rule, and a terminal unreachable route even while their physical default is absent.
- External Leaf startup now accepts the exact system DNS snapshot used to compile its config, and the active runtime retains that snapshot. Linux requires two matching resolver observations before applying changed/lost/recovered state, preventing one transient resolver-file rewrite from causing a restart.
- Route-only gateway replacement remains in-place by design. Stable DNS change, DNS loss/recovery, interface replacement, and removal of a previously usable source address rebuild only Leaf while EasyTier, TUN ownership, mesh classification, and policy routing remain decoupled.
- Added pure tests for route/identity/availability classification, dual-stack fail-closed boundaries, and two-observation DNS stability. The batch still requires remote Rust compilation and exact Linux namespace validation before acceptance.

## 2026-07-15 DNS and underlay transition preflight

- Implemented the next safety batch as local, decoupled policy-supervisor behavior:
  - Leaf startup can receive an explicit system-DNS snapshot without changing the existing generic process API.
  - DNS replacement, loss, and recovery require two matching observations before changing runtime state.
  - Linux underlay refresh distinguishes route-only changes from interface identity loss, address removal, loss, and recovery.
  - Route-only gateway/metric changes keep the Leaf worker alive; identity changes restart only the policy worker.
  - Missing route/address discovery reconciles the policy table to terminal fail-closed routes and keeps mark rules installed.
- Reference semantics remain the previously recorded Mihomo `listener/sing_tun/server.go`, `component/iface/iface.go::FlushCache`, and `component/resolver/resolver.go::ResetConnection`; sing-box `route/network.go::NetworkManager.ResetNetwork` and `route/router.go::Router.ResetNetwork` remain the secondary lifecycle reference. EasyTier intentionally restarts only its decoupled Leaf worker when the pinned Leaf API cannot reset DNS connections in place.
- Remote diagnostic preflight used `root@192.168.2.160` in `easytier-debug-builder`, with all CPU cores, incremental unoptimized tests, explicit timeout, separate log retrieval, and the SSH reverse proxy on remote port `7890`.
- `cargo test --no-run --package easytier --lib --features leaf-policy-proxy` succeeded in 4m34s and produced `target/debug/deps/easytier-6677514306ccf978`.
- Direct execution of `instance::virtual_nic::tests::policy_dns_monitor_requires_two_matching_observations` passed `1/1`.
- Direct execution of `policy_proxy::policy_routing::tests::` with `--test-threads 1` passed `7/7`, including the new transition classifier and per-family terminal-boundary assertions.
- This proves compilation and pure transition invariants only. Linux namespace lifecycle behavior, exact GitHub artifact identity, and Android real-device recovery remain unverified for this new snapshot and must not be claimed until the next profiling-beta build is deployed.

### 2026-07-15 exact-candidate validation findings after `aec69303`

- GitHub Linux run `29381777177` and Android run `29381777207` completed successfully for exact commit `aec69303dc985a63a80f2106fec999b9dff0accc`.
- Linux release asset SHA256 `e694e60cfc819fef88de6f7365300e759c8ed8c84db56d0bc881fdf312df9d24` matched GitHub metadata and the release `SHA256SUMS.txt`; all three inner binary hashes passed. `BUILD_INFO.txt` identifies run `29381777177`, target `x86_64-unknown-linux-musl`, Rust 1.95, and the exact commit. Core Build ID is `da11c82cb1dd40f31e439d0f25c4521fea842d47`; `.debug_info` and `.debug_line` exist and the binary is not stripped.
- Android artifact ZIP passed `unzip -tq`; candidate/probe/runner hashes and actual certificate digests matched `BUILD_INFO.txt`. Exact candidate install succeeded through explicit `adb push` plus device-side `pm install -r -t`. `firstInstallTime` stayed unchanged and the pre/post WebView Local Storage and IndexedDB archives remained byte-identical.
- Android exact candidate cold-started as version `2.6.10-aec69303~`. Captured-UID probes passed mesh TCP (`10.245.0.1:22`, 21 ms) and CN GEOIP DIRECT TLS (`223.5.5.5:443`, SNI `dns.alidns.com`, 174 ms). Non-CN `1.1.1.1:443` remained fail-closed but retained the already-deferred Leaf/netstack behavior: TCP establishment followed by TLS read timeout at 10 seconds rather than Mihomo-fast REJECT.
- Five Android semantic stop/start rounds kept the same PID. Running-state RSS stayed about 260-261 MiB, threads were 69/69/69/69/70, FDs were 345/339/328/332/333, and mesh TCP passed every round at 7-19 ms. No monotonic FD or thread growth was observed in this bounded run.
- The pre-scheduled Android Wi-Fi recovery helper failed to bring the OS Wi-Fi link back, leaving the TCP ADB device offline. This does not yet prove an application recovery regression because the operating-system link itself never returned. After manual Wi-Fi enablement, read `/data/local/tmp/easytier-aec69303-wifi-recover.log` and continue same-PID/TUN/traffic validation.
- Linux namespace startup found a release blocker before lifecycle testing: policy route installation returned `EEXIST`. `strace` proved all IPv4 bypass/capture routes succeeded and the exact failing netlink request was IPv6 `unreachable default table 52000 metric 4294967295`.
- Kernel reproduction on the CentOS 7 / Linux 3.10 host: metric `4294967295` returned `RTNETLINK answers: File exists`; otherwise-identical metric `4294967294` installed successfully as the terminal unreachable default. The next snapshot must use `u32::MAX - 1` only for IPv6 and retain `u32::MAX` for IPv4.
- Reference boundary: Mihomo `listener/sing_tun/server.go` selects default table/rule indices and passes `AutoRoute`/`StrictRoute` to `github.com/metacubex/sing-tun`; sing-box `protocol/tun/inbound.go` does the same with `github.com/sagernet/sing-tun`. Neither project layer defines an explicit terminal metric. EasyTier intentionally differs because it owns policy table 52000 and must keep marked underlay sockets fail-closed; the Linux kernel sentinel collision requires the family-specific metric documented above.
- Artifact-download process finding: direct Azure blob downloads were about 25-30 KiB/s and two `gh run download` errors were initially masked by a following `printf`. Explicit local proxy `127.0.0.1:7890`, `set -euo pipefail`, and timeout fixed release throughput. Artifact API byte-range resume produced a corrupted appended ZIP, so workflow ZIP retries must start from a fresh file and pass `unzip -tq`.
- Applied the family-specific metric fix locally and formatted it with Rust 1.95 / edition 2024. Remote incremental `cargo test --no-run --package easytier --lib --features leaf-policy-proxy` succeeded in 38.17 seconds; direct execution of the policy-routing module passed `7/7`. This is diagnostic evidence only; a new exact GitHub artifact and repeat namespace lifecycle matrix remain required.

### First-release advanced-capability exposure audit

- `easytier-web/frontend-lib/src/components/policy/PolicyEditor.vue` currently exposes all of the following as ordinary creation controls: separate direct/proxy DNS sets, native and mesh proxy nodes, an enabled-by-default UDP checkbox, chain/fallback groups, online Geo rule-data updates, editable ordered rules, and Advanced YAML. None is currently labelled experimental.
- `easytier-web/frontend-lib/src/components/policy/policyDocument.ts` intentionally parses and serializes proxy/group/DNS documents and preserves root-level extra fields. First-release scoping must not remove parser/runtime support, reject previously accepted user configuration, or silently rewrite existing advanced documents.
- Recommended first-release UI boundary: stable controls are enable/disable, DIRECT, REJECT, basic ordered domain/IP/GeoSite/GeoIP rules, Magic DNS staying on mesh, and one mesh SOCKS actor. Split-DNS, native/HTTP actors, multiple actors, UDP forwarding, chain/fallback, online Geo updates, and free-form advanced YAML remain available only after an explicit experimental-feature unlock with a clear unsupported-matrix warning.
- Candidate A (recommended): one shared `advancedPolicyFeatures` gate in `PolicyEditor.vue`; advanced panels remain visible with a warning when an existing document already uses them, preserving round-trip and edit access, but new empty configurations do not advertise or create them until unlocked. This is decoupled UI scoping and does not change backend semantics.
- Candidate B: label every advanced panel independently as experimental without gating. This is lower risk but does not prevent accidental first-release reliance and leaves the public contract ambiguous.
- Candidate C: hard-hide advanced fields. Reject this because Advanced YAML and existing saved documents would become invisible or risk lossy edits, violating user configuration ownership.
- Before implementing Candidate A, add frontend tests for: basic documents do not expose advanced creation by default; an existing advanced document is detected and preserved byte-for-byte/semantically; explicit unlock reveals controls; disabling the unlock does not delete advanced fields. Do not add this UI batch to the in-flight `67b09873` artifact validation.


### 2026-07-15 Android Wi-Fi outage recovery correction and aec69303 evidence

- Procedure correction: a wireless-ADB outage test must not disable Wi-Fi from a separate host-side ADB command. One verified device-side detached task must own the whole delayed \`disable -> outage wait -> enable\` sequence and log markers plus return codes for both Wi-Fi commands. This requirement is also recorded in the repository \`AGENTS.md\`.
- The earlier recovery helper self-deleted but produced no useful log, so it did not provide adequate proof of the re-enable step. The maintainer manually restored Wi-Fi.
- After reconnecting to \`192.168.234.227:5555\`, candidate \`aec69303dc985a63a80f2106fec999b9dff0accc\` retained the same application PID \`12917\`. \`tun0\` was present with \`10.245.0.2/24\`, and the mesh route \`10.245.0.0/24\` was restored.
- Semantic WebView inspection reported the network as running with peer \`10.245.0.1/24\`, 17 ms displayed latency, and candidate version \`2.6.10-aec69303~\`. No screenshot or simulated click was used.
- Post-recovery exact probe results: mesh TCP \`10.245.0.1:22\` connected in 19 ms; GEOIP CN DIRECT TLS \`223.5.5.5:443\` with SNI \`dns.alidns.com\` completed in 143 ms; non-CN \`1.1.1.1:443\` remained fail-closed but TCP connected before a TLS read timeout at 10043 ms. The last result confirms no policy bypass, but fast REJECT remains an explicit first-release limitation until separately fixed or scoped down.


### 2026-07-15 exact 67b09873 Linux DNS-loss blocker and platform parity boundary

- Exact candidate evidence: profiling run \`29385081559\` and Android run \`29385081583\` both succeeded for \`67b09873e220975cc230197648d626aa70f7159d\`. Linux asset SHA-256 is \`af1622bf008e9043ac5094b252451f56d592293baae1b928726080cf9e151825\`, core Build ID is \`5a3449b76205a5f5f089f43bce1a7c19d35d3daa\`, and Android candidate APK SHA-256 is \`5a8bad0e8b54ae7fe84f25153b813eaa9bd49ce3cf3a87c56048e64d0c21233a\`. Internal checksums, targets, symbols, and signing certificates matched their build metadata.
- Android exact-candidate regression passed mesh TCP (20 ms) and GEOIP CN DIRECT TLS (169 ms). Non-CN traffic remained fail-closed but retained the known slow-REJECT behavior (TLS read timeout at 10045 ms). Persisted Local Storage and IndexedDB hashes were unchanged across \`pm install -r -t\`.
- Linux exact-candidate startup on CentOS 7 / Linux 3.10 passed after the IPv6 terminal metric fix: IPv4 table 52000 uses metric \`4294967295\`, while IPv6 uses \`4294967294\`. Route metric changes and address additions retained the Leaf worker; underlay identity removal, route recovery, and stable DNS changes rebuilt it; route loss stopped Leaf while mesh remained available.
- Blocking failure: after the namespace \`/etc/resolv.conf\` was emptied, both the host file and the core mount namespace view were confirmed empty, but \`easytier_policy::system_dns_servers\` continued to \`/run/systemd/resolve/resolv.conf\`. Leaf restarted with host resolver \`202.96.134.133\` plus the policy document's existing proxy DNS. This violates namespace ownership and DNS-loss fail-closed semantics.
- Mihomo reference: \`/Users/fanli/Documents/mihomo-rev/config/config.go::parseDNS\` rejects enabled DNS with an empty \`NameServer\` and rejects an empty default resolver set. sing-box reference: \`/Users/fanli/Documents/singbox-withfallback/dns/transport/local/local_resolved_linux.go::DBusResolvedResolver.loadDefaultInterface\` and \`dns/transport/dhcp/dhcp.go::Transport.updateServers\` return errors when the selected link/DHCP response has no DNS servers. Observable compatibility semantic: do not silently substitute unrelated public or host-namespace DNS when the selected Linux network has no resolver.
- Android reference is intentionally separate: \`/Users/fanli/Documents/clashmeta-android-rev/service/.../NetworkObserveModule.kt::{onAvailable,onLosing,onLost,onLinkPropertiesChanged,notifyDnsChange,notifyNetworkRecoveryIfChanged}\` obtains LinkProperties DNS and debounces network events; \`core/src/main/golang/native/app/dns.go::NotifyDnsChanged\` updates DNS, flushes caches, resets resolver connections, and closes stale flows; \`core/src/foss/golang/clash/dns/patch_android.go::UpdateSystemDNS\` clears the system resolver on an explicit empty list. EasyTier Android obtains DNS from \`TauriVpnService.selectUnderlyingNetwork\`, emits an \`outage!epoch\` state when no DNS-bearing underlying network exists, and intentionally stops policy runtime rather than retaining stale DNS. The Linux resolver-file fix must not alter this Android path.
- Fix boundary: a readable Linux \`/etc/resolv.conf\` with usable IP resolvers is authoritative. Managed systemd/NetworkManager resolver files may be consulted only when the primary file explicitly contains a loopback resolver stub, or when the primary file cannot be read. A readable but empty/search-only/unusable non-loopback primary file is DNS Lost. Existing user-provided \`dns.direct\` and \`dns.proxy\`, including the current documented default proxy DNS, remain unchanged.


### 2026-07-15 first-release advanced capability exposure gate

- Mihomo core has no corresponding GUI exposure layer; its relevant compatibility boundary remains strict first-match policy semantics and explicit DNS/group configuration. EasyTier therefore keeps parser/runtime semantics unchanged and scopes only the visual creation surface.
- Implemented one local UI-only gate in \`PolicyEditor.vue\`. Basic mode exposes mesh SOCKS nodes, DIRECT/REJECT, ordered rules, GeoSite/GeoIP and bundled rule data. New proxy rows default to \`udp: false\`.
- Split DNS, native/UDP nodes, chain/fallback groups and custom online rule-data controls require an explicit experimental unlock. This is presentation-only state and is never serialized.
- Existing advanced documents are detected from custom DNS, native/UDP proxies, groups, UDP network rules or custom online rule data. Their controls stay visible with a warning even when the unlock checkbox is off. The parser, advanced YAML and unknown root fields remain untouched, so user-owned configuration is not deleted or normalized away.
- Added component tests for basic-mode hiding, explicit unlock/relock without YAML mutation, and existing advanced document visibility plus byte-stable preservation. Existing policy codec round-trip coverage continues to protect advanced fields.
## 2026-07-15: formal release workflow preflight for `20b1e39f`

- No formal Core, GUI, Mobile, OHOS, or Test workflow run currently exists for exact SHA `20b1e39fb25196681c84d052506cc1e3fe274e38`. The successful profiling-beta and Android policy-candidate runs are authoritative Leaf validation artifacts but do not satisfy the formal release artifact matrix.
- `core.yml`, `gui.yml`, `mobile.yml`, `ohos.yml`, and `test.yml` all support input-free `workflow_dispatch`. After maintainer real-device confirmation, dispatch each once on `codex/profiling-beta` and require every resulting run to report `headSha=20b1e39fb25196681c84d052506cc1e3fe274e38`. Do not dispatch before confirmation and do not move the ref while the chain is active.
- Historical successful Release run `29165084568` proves `release.yml::Resolve workflow run IDs` queries each required workflow with `head_sha=${GITHUB_SHA}`, accepts only successful push/workflow-dispatch runs, and then downloads Core, GUI, Mobile, and OHOS artifacts. Test is a separate quality gate and is not packaged by Release.
- Release itself accepts required `version` and `make_latest` inputs. It must run on the same immutable validated SHA after all formal runs succeed; otherwise its exact-SHA resolver intentionally fails rather than reusing another commit's artifacts.
- Historical `Validate release version` logs prove the input must exactly equal `v${easytier Cargo version}` and the tag must not already exist. Candidate `20b1e39f` still reports EasyTier `2.6.10`, while `v2.6.10` is already the latest published release; therefore this validated snapshot cannot itself be passed to formal Release. It remains the behavioral validation baseline.
- Before formal workflows, choose the next release version (normally `2.6.11`), batch the version bump with these final documentation updates, and produce one new exact candidate. Because that changes the commit and visible package version, repeat the authoritative profiling-beta/Android candidate gates and the focused Linux/Android smoke/lifecycle checks needed to prove the release SHA, then run the formal matrix only once.
- This preflight triggered no workflow and made no product-code change. The remaining gate is explicit maintainer authorization, not missing implementation.

## 2026-07-15: Leaf v1 frozen-scope release candidate closure (`20b1e39f`)

- Exact snapshot: `20b1e39fb25196681c84d052506cc1e3fe274e38` (`fix(policy): keep runtime crash recovery retryable`). Remote debug preflight compiled in 1.60 seconds and `supervisor::tests::runtime_restart_backoff_caps_until_stable_reset` passed directly (`1 passed`, 68 filtered).
- Linux workflow `29389918031` succeeded. Rolling release target commit and `BUILD_INFO.txt` both identify the exact snapshot, run, Rust 1.95, and `x86_64-unknown-linux-musl`. Archive SHA-256 is `c019ec4d1ba9fee08209e9d8bc89a79097809c82b268f776138f7ae82c3a505c`; core SHA-256 is `d14ab5a29799d58018ce8bdcf5ee5c645e2201b4e719ff0634547170b0b9d981`; worker SHA-256 is `550559e24d9ce64736e17229f66122ab74ed70f9627e270a208989dba965a5ae`; core Build ID is `88c4490007383d68a8c50a5886a9afe8a537c376`. `.debug_info`, `.debug_line`, and `.symtab` are present.
- Linux isolated validation used unoccupied underlay `192.0.2.0/24` after identifying a fixture collision between the previous `172.31.255.0/24` and host bridge `br_upnp`. Baseline mesh/DIRECT/REJECT, route metric stability, address-add stability, underlay identity restart, route-loss fail-closed with mesh preserved, route recovery, DNS stable-change restart, empty-DNS fail-closed with mesh preserved, DNS recovery, and five consecutive Leaf worker SIGKILL recoveries all passed.
- The fifth Linux crash recovered after the capped delay. Core RSS samples were `17080` KiB baseline, `17144` KiB after network/DNS recovery, and `17144/17280/17204/17200/17204` KiB after crash rounds 1-5. Threads remained 9; FD counts were non-monotonic and ended below the baseline (`37 -> 36`). Source core graceful shutdown removed its TUN, table 52000 routes, IPv4/IPv6 rules, worker, and PID-scoped temporary config; destination cleanup also passed.
- The first full-script exit after all five recoveries was a validation-fixture false failure: it globally matched historical `/tmp/easytier-leaf-*.conf` files from unrelated PIDs. PID-scoped cleanup assertions passed. Future harnesses must scope temporary ownership and must preflight namespace CIDRs against host routes.
- Android workflow `29390084604` succeeded for the exact snapshot. Artifact ZIP SHA-256 is `16481b1102c48b53a1927a23ba38848d651beecc1a2fc6458766af37318d9319`; candidate APK SHA-256 is `1cff4993856d836d2ba1b72cab66151f7de3a5cd92a69d1d08fda37de0a9ef3e`; probe and runner SHA-256 values are `e23093b104fc48c8fbff3990aefccf2d1f31ebf7032e734aef999f90d031c6e5` and `3b3e2f2afc9baeffa735fa025eb7dd31e41f726b0b123b631cd40cc01ee9d9d6`. Candidate signer remains `14d2d885ce1bc361923a493210865f86390ffcd32eb2b555042bbd1a8b6c38e0`.
- Android preserve-install retained `firstInstallTime=2026-07-14 15:25:47`. WebView Local Storage SHA-256 remained `d0fbd9fe9743f3e1c826e2ca1da87ca46d55eac911a7b6c8c62c4a8effd851bc`; IndexedDB remained `fefb5982e0d7e6e89ae454f68e1c21e3a3025951aff580b69947d360af47436a`. Cold start restored the selected network and reported `2.6.10-20b1e39f~` with the mesh peer at 4 ms.
- Android CDP semantic UI validation passed without screenshots or coordinate clicks. Basic mode hid proxy groups and custom online data; the explicit experimental gate revealed `Add group`, `Download and verify`, and the online data URL input; relocking restored the basic controls. Inline policy YAML and serialized `networkList` remained byte-identical throughout.
- Android probe UID `10272` was inside VPN range `10255-20251`, while owner UID `10254` was excluded. Mesh TCP to `10.245.0.1:24801` passed in 22 ms. `GEOIP,CN,DIRECT,no-resolve` completed Baidu TLS in 255 ms. `MATCH,REJECT` completed only the local TCP phase and closed the Cloudflare TLS handshake in 28 ms with `SSLHandshakeException: connection closed`; `probe_valid=true` and the untrusted-app SELinux domain were recorded for all probes.
- Seven Android semantic stop/start cycles all returned to `running`. RSS changed from `279592` KiB before the first cycle to approximately `267392` KiB after five cycles. Corrected entry-wise sampling around two more cycles showed FD `316 -> 341` immediately after restart, then `319` after 15 seconds; threads/tasks `66 -> 68 -> 65`; settled RSS was `264164` KiB. This is a bounded startup transient, not monotonic resource growth. The original stopped state was restored and both probe packages were uninstalled.
- Android DNS remains a separate platform path: ClashMeta Android's `Connectivity.kt::resolveDns` and `NetworkObserveModule` use `LinkProperties` and network callbacks; EasyTier continues to inject mobile network state into the in-process runtime. No Android resolver-file fallback was introduced. Wi-Fi outage was not repeated because this snapshot changes only the shared crash backoff; the prior exact-candidate Wi-Fi loss/recovery evidence remains applicable to the unchanged Android network/DNS path.
- Frozen v1 conclusion: for Linux and Android, single policy-enabled instance, DIRECT/REJECT, basic GeoSite/GeoIP/domain rules, Magic DNS mesh ownership, and one mesh SOCKS actor, no architecture-level or lifecycle release blocker remains. Split DNS, online Geo updates, HTTP actor, netns, multiple instances, chain/fallback, and high-throughput UDP remain outside the promised first-version boundary unless separately marked experimental and validated.

## 2026-07-15: Linux repeated worker crash recovery blocker (`5ec0010b`)

- Exact candidate: `5ec0010b12fe863c02386dfad614d5adb5551df4`; profiling-beta Linux workflow `29388033314`; deployed core SHA-256 `c70375df84f34b215071c2c5a125f7ce46b77a170a8e475004a9872f4af6cb97` and worker SHA-256 prefix `550559` from the verified x86_64-musl artifact.
- Isolated Linux lifecycle validation passed baseline mesh/DIRECT/REJECT, route-metric stability, address-add stability, underlay identity restart, route-loss fail-closed with mesh preserved, route recovery, stable DNS restart, empty-DNS fail-closed with mesh preserved, DNS recovery, and three worker SIGKILL recoveries. The fourth SIGKILL permanently stopped worker restart while core, TUN, mesh, and fail-closed policy routing remained alive.
- The first DIRECT fixture failure was not a product failure: validation subnet `172.31.255.0/24` collided with the host's existing `br_upnp`. The harness was moved to unoccupied `192.0.2.0/24`; gateway, mesh, and DIRECT then passed before the lifecycle run.
- Root cause: `easytier-policy/src/supervisor.rs::RuntimeRestartBudget::record_failure` returned `Dormant` after the `1s/2s/5s` stages. `easytier/src/instance/virtual_nic.rs::schedule_policy_runtime_restart` therefore made a short Leaf crash burst require a full core restart.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/component/slowdown/slowdown.go::{Do,Wait}` and `component/slowdown/backoff.go::Backoff::{Duration,Reset}`. Observable semantic followed: repeated failures remain retryable at a bounded maximum delay; success resets the backoff. Mihomo's exact `10ms..1s` timing is not copied because EasyTier is restarting an external Leaf process rather than retrying an in-process operation.
- Android remains intentionally separate. `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/util/Connectivity.kt::resolveDns` reads `LinkProperties`; `NetworkObserveModule::{onAvailable,onLosing,onLost,onLinkPropertiesChanged,notifyDnsChange,notifyNetworkRecoveryIfChanged,run}` continuously delivers network generations and DNS to the embedded core. This Linux worker-backoff fix does not read resolver files or alter Android DNS ownership.
- Fix boundary: retain EasyTier's `1s/2s/5s` stages, cap subsequent retries at 5 seconds, and retain the existing reset after 60 seconds of stable runtime or a new usable network generation. Keep fail-closed and mesh ownership unchanged. Compatibility test: `runtime_restart_backoff_caps_until_stable_reset`.

## 2026-07-15: HEV exact-candidate lifecycle and crash-ownership findings (`04660cac`)

- Exact workflows succeeded for `04660cac2691388aa3b28cddfda8b2ffdb7567ab`: Linux profiling run `29417350159` and Android candidate run `29417350155`. The Android workflow also passed the newly added `TauriVpnService` runtime-package exclusion unit tests. Linux `BUILD_INFO.txt` identifies x86_64-musl, Rust 1.95, and pinned HEV server `97e74f1068bd924e740032382cdc94ca83741ae6`; outer and all inner SHA256 checks passed, and binaries are static PIE, unstripped, with debug information and Build IDs.
- Linux namespace validation proved HEV standard SOCKS5 TCP `CONNECT` and UDP `ASSOCIATE` with the requested `0.0.0.0:0` client endpoint. The direct run returned OpenSSH through TCP and a byte-identical UDP echo. Running resources were core `14388 KiB / 9 threads / 32 FDs`, Leaf `5576 KiB / 4 threads / 11 FDs`, and HEV `260 KiB / 2 threads / 12 FDs`.
- With an unrelated process owning `11080`, HEV selected `11081`; TCP and UDP both passed. Core `SIGTERM` released `11081`, removed the HEV private directory, and left the external `11080` owner alive. This proves deterministic port fallback and listener ownership.
- The first graceful stop removed core, Leaf, HEV, TUN, table 52000, rules, and HEV files but intermittently retained the PID-scoped Leaf JSON; a second equivalent stop removed it. Root cause is Drop-only NIC task cancellation racing `Runtime::shutdown_background`. The fix explicitly aborts and joins `NicCtx` tasks, then awaits the active policy runtime before dropping the TUN container and HEV guard.
- Core `SIGKILL` caused the existing Linux-protected Leaf worker to exit, but HEV was reparented to PID 1 and kept `11080`. TUN disappeared with its FD; policy routes and named configuration remained because the dead parent could not run destructors. A new core started successfully over the stale table, did not duplicate rules, and restored TCP/UDP, proving startup reconciliation. No external cleanup daemon is needed for the first version.
- Sensitive-config boundary was tightened instead of adding crash scavenging. `easytier-leaf-worker::take_runtime_config` reads the generated JSON, unlinks it, and starts pinned Leaf with `leaf::Config::Str`; the parent also enforces absence after readiness. HEV `hev-config.c::hev_config_init_from_file` parses and closes the complete YAML before listener readiness, so the egress wrapper removes the entire private directory before returning a runtime.
- Mihomo lifecycle references remain `/Users/fanli/Documents/mihomo-rev/listener/inbound/socks.go::Socks.Close`, `listener/socks/tcp.go::Listener.Close`, `listener/socks/udp.go::UDPListener.Close`, and `component/tsnet/tsnet.go::runtime.Close`: observable close waits for owned listeners/workers. Mihomo has no external SOCKS sidecar. The additional sing-box reference is `SagerNet/sing-box@b789a2e6`, `cmd/sing-box/cmd_run_userns_linux.go::runInUserNamespaceIfNeeded`, which sets `Pdeathsig: SIGKILL`, forwards graceful signals, and waits for the owned child. EasyTier intentionally applies the Linux parent-death guarantee only to its external HEV process; Android HEV remains in-process and follows its existing joined shutdown path.
- Current UDP semantics are unchanged: HEV serves standard SOCKS5 UDP and does not default to UoT or KCP. DNS may use TCP/DoH/DoT independently. UoT/KCP remains an explicit future transport decision rather than a hidden fallback.
- Added compatibility coverage `runtime_config_is_unlinked_after_read` and `starts_worker_without_retaining_private_config_and_stops_it`. The next exact candidate must prove Linux graceful shutdown, core `SIGKILL` HEV parent-death, no named Leaf/HEV config while running, and Android in-process stop/start on the real device before this batch is release-eligible.
- Android `04660cac` real-device validation confirmed the runtime package fix before the next build: owner UID `10254` was excluded from VPN range, logcat reported `disallowedApplications: [com.kkrainbow.easytier.policycandidate]`, direct HEV TCP to `.160:22` succeeded, and standard UDP ASSOCIATE DNS returned two answers. Normal stop removed TUN and `11080` while retaining the app PID.
- The first Android port-fallback attempt selected `11081` and passed TCP/UDP, but exposed a narrower ownership violation: the readiness loop connected to the unrelated `nc` listener on `11080` before observing that the failed HEV candidate thread had exited, causing `nc` to terminate. The fix performs a no-connect bind preflight for the configured address and, for an unspecified IPv4 listener, a separate IPv6 wildcard probe. Added `occupied_candidate_is_rejected_without_connecting_to_owner`; final device validation must prove the blocker remains alive.
- Workflows `29420113157` and `29420113320` for intermediate commit `5ad94cf4` failed before producing artifacts. Linux correctly rejected an uncommitted lockfile change under `--locked`; Android compiled Linux-only `libc::getpid` because the local variable lacked a target guard. The follow-up removes the new dependency entirely, uses a narrow Linux-only libc FFI for parent-death setup, and keeps Android free of that code path.

### 2026-07-15: `61c6f313` HEV Linux/Android 精确候选最终验收

结论边界：固定提交 `61c6f313559cedce3453970e2729c6eb7035e48a` 的 HEV 集成已通过 Linux 与 Android 首版所需的精确制品、TCP/UDP、端口归属和生命周期验证。当前没有剩余的 Linux/Android HEV 专项发布阻塞；这不代表 Windows/macOS/OHOS 等平台的 EasyTier 集成已经验证，也不替代长期重复资源回基线或仍公开的高级 Leaf 能力矩阵。

精确构建与制品：

- Linux workflow [29420954296](https://github.com/lovitus/EasyTier/actions/runs/29420954296) 成功，目标 `x86_64-unknown-linux-musl`，HEV 固定为 `97e74f1068bd924e740032382cdc94ca83741ae6`，Rust `1.95.0`。
- Linux 外层 tar SHA-256 为 `d714ada314cfc06859fd32e4dba6846e07876326246d3c6c1eb3bf848b7f4d6d`；内层 `easytier-core` 为 `eeaebd3150d82c5fd1182ac81fb90307e560ab390bb05156e1b486ae591b37ac`，Leaf worker 为 `41af69169683054cbb0df1cb7d913a1df9e7f5e67644058618ccbe385d99a317`，HEV worker 为 `66b012c24de869a401fbcf2503f626da5577769a54376316010bfab21e6517a9`。
- Linux core Build ID 为 `68245d1a9aca511cf9d614ef894ee42b45d3c8f7`，`.debug_info`、`.debug_line`、`.symtab` 均存在，内外层 SHA256 清单均通过。
- Android workflow [29420954300](https://github.com/lovitus/EasyTier/actions/runs/29420954300) 成功，包括固定 HEV 静态库、持久选择定向测试、debug APK、VPN service 单元测试和 captured-UID probe。
- Android APK SHA-256 为 `a92193b96aee53feb146352ce93109916586c6d4820818b29ee0d06e114b3983`；BUILD_INFO 为 `be0d23b61ad878431ceec8f092e89f46440f4844c0309ad42e339732e7f7c155`。应用 ID 为 `com.kkrainbow.easytier.policycandidate`，签名 SHA-256 为 `14d2d885ce1bc361923a493210865f86390ffcd32eb2b555042bbd1a8b6c38e0`。

Linux namespace 实机结果（CentOS 7 / Linux 3.10 验证机 `192.168.1.37`）：

- 正常启动时 core、Leaf、HEV 分别为 9、4、2 个线程，RSS 约为 14.3 MiB、5.5 MiB、0.25 MiB；HEV 监听 11080。
- Leaf JSON 与 HEV 私有 YAML/临时目录在 listener ready 后均已删除。进程参数仍保留历史路径用于诊断，但路径本身不可再读取。
- 标准 SOCKS5 TCP 到 `192.168.1.37:22` 获得 OpenSSH banner；UDP ASSOCIATE 到本地 UDP echo 成功。
- `SIGTERM` core 后 core、Leaf、HEV、11080、TUN、策略规则、table 52000 和私有配置全部回基线。
- 使用不接受连接的独立 Python listener 占用 11080 后，blocker 在 EasyTier 启动、TCP/UDP 探测和停止后始终存活；HEV 选择 11081，且停止只释放 11081。这证明 no-connect bind preflight 不再触碰外部 listener owner。
- `SIGKILL` core 后 Leaf 和 HEV 均通过 parent-death ownership 退出，11080、TUN 和私有配置消失。策略规则/table 按既有 crash recovery 边界保留。
- 随后用 25100-25104 启动同一精确制品，成功接管残留 table，规则未重复；正常停止后规则和 table 52000 清空。

Android 实机结果（`192.168.234.227:5555`，全程保持 Wi-Fi 开启并使用 Activity/CDP 自动化，不使用截图或模拟点击）：

- 通过 `pm install -r -t` 覆盖安装；`firstInstallTime` 保持 `2026-07-14 15:25:47`，完整 `app_webview` tar 在安装前后 SHA-256 均为 `2f4730faa4f9b8f9efda4907076206239445e2686cd05d646d6202269f4cb74f`。
- 持久实例 `c17a8c16-5016-4d09-a1c3-e97c6fddcaf5` 正常启动，TUN 为 `10.245.0.2/24`。应用 owner UID 为 `10254`；VPN UID 范围为 `{0-10253, 10255-20253, 20255-99999}`，日志明确包含 `disallowedApplications: [com.kkrainbow.easytier.policycandidate]`。
- HEV 11080 上 TCP 到 `192.168.2.160:22` 成功并获得 OpenSSH banner；UDP ASSOCIATE 到 `1.1.1.1:53` 成功并获得 2 个 DNS answer。
- 正常停止后 GUI PID 保持不变，线程回到 58，TUN 与 11080 释放。
- 使用 Android 自带 `nc` 占用 11080 后，`/proc/net/tcp6` 同时显示 UID 2000 的 11080 与应用 UID 10254 的 11081；blocker PID 在 EasyTier 启动和停止后始终存活。
- HEV 11081 上 TCP 与 UDP 再次通过；停止只释放 11081 和 TUN。移除 blocker 后 11080 也释放，最终 GUI 保持运行、无 TUN、无 11080-11082，Wi-Fi 仍启用并连接。

HEV UDP 承载语义：

- 当前 HEV 只实现标准 SOCKS5 `CONNECT` 和 `UDP ASSOCIATE`，不会默认启用或协商 UoT/KCP。
- `via: mesh` 场景中，Leaf 到 HEV virtual IP 的 SOCKS UDP 数据报由 EasyTier overlay 承载，实际 peer transport 可为 EasyTier 已建立的 TCP/QUIC/WS/WG/UDP 等连接；这已经避免要求公网直接暴露 HEV UDP 端口，但协议层仍不是 UoT/KCP。
- HEV 到最终目标仍为原生 UDP。KCP 本身依赖 UDP，不能解决出口完全禁 UDP；UoT 需要明确兼容的远端，不能与普通 HEV SOCKS5 server 静默协商。
- 首版保持 HEV 标准 UDP over mesh；未来如加入 UoT，应作为支持端明确声明的可选 outbound 能力，不把 KCP 或 UoT塞进默认 HEV 路径。

剩余边界：

- HEV 的 Linux/Android 首版验收已经闭环，不需要继续为这两个平台阻塞当前候选。
- 整个 Leaf 第一版仍需按最终公开能力边界决定是否继续验证 chain/fallback、UDP/KCP/UoT、split DNS 等高级矩阵，或在首版隐藏/标为实验性。
- 长期重复 Wi-Fi/DHCP/地址切换、stop/start、Leaf/HEV crash 后 RSS、FD、线程和任务回基线仍属于独立耐久性证据，不应被本轮有限次数生命周期验证替代。
- Windows/macOS/OHOS 等平台仍需针对 EasyTier 宿主集成做构建与运行验证；本轮只证明固定 HEV fork 和当前 Linux/Android 宿主路径达到首版门槛。

### 2026-07-15: Android 第 10 轮重复 stop 暴露 WebView 队列不能拥有 VPN 关闭

精确参考与外部语义：

- Clash Meta Android `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt` 的 `runtime` 在 `finally { withContext(NonCancellable) { tun.close(); stopSelf() } }` 中由原生 service 关闭 TUN；`onDestroy()` 再请求 runtime 停止并同步等待 coroutine 退出。外部可观察语义是不依赖 Activity/WebView 事件队列完成 VPN FD ownership 清理。
- EasyTier 当前 `easytier-gui/src-tauri/src/lib.rs::GUIClientManager::{post_stop_network_instances_hook,notify_vpn_stop_if_no_tun}` 在 core 停止后只 `app.emit("vpn_service_stop", "")`；`easytier-gui/src/composables/event.ts::onVpnServiceStop` 再调用 `syncMobileVpnService()`，后者排队到 `mobile_vpn.ts::vpnOperationTail`，最终才调用 plugin `stop_vpn()`。
- 仓库已有原生 Rust plugin 路径：`tauri-plugin-vpnservice/src/mobile.rs::Vpnservice::stop_vpn` 通过 `run_mobile_plugin("stop_vpn", ...)` 调用 `VpnServicePlugin.kt::stopVpn`，后者执行 `TauriVpnService.stopByUser()`、关闭 `ParcelFileDescriptor` 并 `stopService()`。因此不需要新增组件或把 shutdown 重写到 HEV/Leaf。

精确故障证据：

- 固定候选 `61c6f313559cedce3453970e2729c6eb7035e48a` 在同一 Android GUI PID `4099` 中执行 10 轮 start/stop；每轮均完成真实 HEV TCP 和 UDP ASSOCIATE。
- 第 1-9 轮停止后线程每次回到 58、FD 每次精确回到 232；停止态 RSS 从 193560 KiB 小幅上升后在约 195888 KiB 平台化，没有线性线程/FD增长。
- 第 10 轮 core、Leaf/HEV 和 11080 已停止，但 TUN 与 Android VPN NetworkAgent 超过 11 秒仍存在，FD 为 233。日志只有全局 `Received event 'vpn_service_stop'`，没有随后应出现的 `stop vpn`/`VpnServicePlugin.stopVpn`。
- `dumpsys activity services` 显示 `TauriVpnService` 仍为 `startRequested=true`；`dumpsys connectivity` 显示 network 249、owner UID 10254 的 VPN 仍连接。`am force-stop` 后 TUN/NetworkAgent 清除且 Wi-Fi 保持连接。

修复边界：

- Android core stop hook 必须先通过 Rust `VpnserviceExt::stop_vpn` 同步关闭原生 VPN，再保留现有前端 event 作为状态同步和 native-call-failure fallback。WebView operation queue 不再拥有安全关键的 TUN FD 清理。
- 新代 start 语义不变；只有 `get_enabled_instances_with_tun_ids()` 为空时执行 native stop。存在任一启用 TUN 实例时不得关闭当前 VPN。
- native stop 失败时仍必须 emit 前端 event，避免同时丢失 fallback；命令返回 native 错误，不得静默报告成功。
- 增加 dispatcher 单元测试，覆盖存在 TUN 时不调用、无 TUN 时 native-before-frontend，以及 native 失败仍执行 frontend fallback。修复后必须用精确 Android APK 重跑同进程 10 轮资源回基线。

#### `e8f7e745` 实机预检发现 Rust mobile wrapper 命令名不匹配

- 精确 Android APK `e8f7e74549f83791ed43a6f692ff7a034bab070d` 的 workflow、签名、SHA256 与覆盖安装均通过；APK SHA-256 为 `191d6588c4a02869bf6be9463399a39d95286f42a6223521896cfee2fdb3ccb2`。
- 第 1 轮 stop 时，新增 Rust native-first 路径明确返回 `No command stop_vpn found for plugin com.plugin.vpnservice.VpnServicePlugin`。dispatcher 随后仍 emit frontend fallback，旧 JS 路径成功执行 `VpnServicePlugin.stopVpn` 并清理 TUN/HEV，所以设备没有残留，但命令正确报告失败，候选不能接受。
- 根因是 `tauri-plugin-vpnservice/src/mobile.rs::Vpnservice::{prepare_vpn,start_vpn,stop_vpn,get_vpn_status}` 把 snake_case 公共 API 名直接传给 `run_mobile_plugin`；Android `VpnServicePlugin.kt` 注册的原生命令分别为 `prepareVpn`、`startVpn`、`stopVpn`、`getVpnStatus`，iOS 已存在的 `ExamplePlugin.swift::getVpnStatus` 也使用 camelCase。
- 兼容边界：guest JS 的 `plugin:vpnservice|stop_vpn`、权限名和 Rust 方法名继续保持 snake_case；只修正 Rust wrapper 到原生 plugin method 的内部映射。修复后 direct Rust stop 必须在实机日志中进入 `stop vpn in plugin`，且 `update_network_config_state` 不再返回 command-not-found。

#### Linux `61c6f313` 10 轮重复生命周期回基线

- 在 `192.168.1.37` 的隔离 namespace 中，使用精确 musl 制品连续执行 10 轮 start、真实 SOCKS TCP、UDP ASSOCIATE、`SIGTERM` stop；listener 端口基数从 25110 每轮递增 10，所有协议端口均显式指定。
- core 每轮固定 9 线程、36 FD，RSS 范围 13992-14444 KiB；Leaf 固定 4 线程、11 FD，RSS 5540-5552 KiB；HEV 固定 2 线程、12 FD，RSS 272-280 KiB。没有线程、FD 或 RSS 递增趋势。
- 10 轮 TCP 均获得 OpenSSH banner，UDP echo 均成功；每轮 core 停止约 200 ms。
- 每轮停止后均确认无 core/Leaf/HEV 进程、11080-11082、TUN、policy rule、table 52000、Leaf JSON 或 HEV 私有目录。最终 namespace 保持干净。
- 因此 Linux 重复普通生命周期与资源回基线已达到首版证据要求；仍需单独保留长期网络切换/高并发 UDP soak，不应把本 10 轮短循环扩大解释为长期负载证明。

## 2026-07-17: exact `cf7215b4` Android 4G/VPS explicit and portless findings

- Exact commit: `cf7215b44084441ccd64a3aad1443dc4046ab721`. Linux workflow `29558109229` and Android workflow `29558109228` succeeded; artifact checksums, build metadata, commit SHA, target, symbols, APK signature, and captured-UID probe artifacts were verified before deployment.
- Android source was the USB device on dual-stack cellular `rmnet_data4`, not WLAN or an internal egress. Public fixed-byte HTTP targets were `lv1g2` and `lv1g3`. Internal hosts are excluded from absolute performance conclusions.
- Explicit `10.44.0.8:24443 via: mesh` no longer reproduces the old 128 KiB receive-window stall. Fixed-byte IPv4 and IPv6 probes completed, including repeated 16/32 MiB transfers. The exact-artifact 16 MiB matrix completed 4/4 explicit runs: native `2.310/18.004 Mbps` and KCP/QUIC-enabled `1.975/27.985 Mbps` to the two VPS targets. Same-time DIRECT was `3.933/16.519 Mbps`, proving large route/target variance and no stable protocol-wide cap.
- Portless native was initially successful for 4 MiB (`3.346 Mbps`), then after an Android instance restart failed against both VPS targets. Each failure completed the local TCP handshake, received zero HTTP bytes, and closed after about 20 seconds. The remote VPS service remained healthy.
- A target-specific diagnostic policy routed only the two VPS IPs through portless and left all other traffic DIRECT. During failure, `.86` saw neither a new loopback `11080` packet nor a public-target SYN. The HEV child remained alive and listening. A stable `.8` peer independently used `.86:11080` and transferred 1 MiB in `1.227s`; destination userspace ingress, HEV SOCKS, and public outbound were therefore healthy.
- Enabling the existing mesh-owned KCP/QUIC selector immediately made portless complete 2/2 16 MiB transfers (`3.548/3.684 Mbps`). Keeping native configuration unchanged later completed two 4 MiB transfers (`3.656/3.599 Mbps`). On a subsequent restart, source peer `1412131056` established a direct WG connection to destination peer `4267670262` at about `t+11s`; a probe started at `t+10s` completed successfully. The observed defect is first traffic racing route/tunnel readiness, not stale HEV or a permanent native fallback failure.
- Current bridge semantics explain the 20-second symptom: `MeshProxyBridgeSet::relay_socks5` gives a built-in actor two attempts; `connect_remote` gives each `data_plane_tcp_connect_mesh_only` attempt 10 seconds. `RemoteTcpPreparation::ensure` treats successful `prepare_remote_tcp_egress` as endpoint readiness, although the RPC proves only remote ownership/listener preparation, not that the source native TCP data path is ready.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go::{Snapshot,onUse,retryStartSocks5TCP}`. Mihomo reports mesh SOCKS readiness separately and retries failed listener availability on a later use with `10s..5m` bounded backoff. EasyTier must not copy Tailscale internals, but should preserve the observable distinction between prepared and data-path-ready.
- No implementation is authorized from this evidence alone. The safe design boundary is a narrow policy-adapter readiness/prewarm mechanism in `MeshProxyBridgeSet`/`RemoteTcpPreparation`; it must not alter EasyTier route selection, overlay framing, smoltcp, HEV lifecycle, or user acceleration flags. Required regression: new source runtime, remote portless prepare succeeds before native path readiness, first request waits/retries without permanent cache poisoning, route readiness completes, and the same request or a bounded retry succeeds; cancellation and generation replacement must release the waiter.
- User decision after diagnosis: do not add the readiness/prewarm state machine above. The selected minimal candidate performs one proactive portless-only mesh TCP connect after remote prepare and before Leaf startup, immediately drops a successful stream, and otherwise preserves the existing request retry/fallback. This intentionally primes the normal mesh path without changing routing, transport selection, HEV ownership, or explicit actor behavior.

### 2026-07-17: performance-fix audit correction

- The mobile-to-VPS measurements are retained as functional and diagnostic evidence, not as an absolute performance baseline. They do not justify a permanent per-stream smoltcp window increase.
- The `32 * (2 MiB RX + 2 MiB TX)` candidate and its later `4 * (2 MiB RX + 128 KiB TX)` mitigation are both rejected. Native mesh fallback returns to the established 128 KiB RX/TX buffers; configured KCP/QUIC remains the supported acceleration path.
- The single proactive portless TCP connect is also removed. It had no regression that reproduced route-not-ready after successful remote prepare, could serialize existing connect timeouts across multiple actors, and conflated remote listener ownership with source data-path readiness.
- The retained correctness fix is the vendored netstack `PollSender` wake registration for a full ingress channel. The retained tooling/UI work includes the captured-UID HTTP byte probe, policy editor behavior, platform notices, and focused lost-waker test.
- Any future portless readiness change must first reproduce the route-generation race deterministically and model readiness in the policy adapter without modifying EasyTier mesh routing, transport selection, overlay framing, or HEV ownership.
