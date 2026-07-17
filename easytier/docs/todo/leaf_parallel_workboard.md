# Leaf Parallel Candidate Workboard

This is the live execution board for batching independent Leaf/policy work into one exact candidate.
It is local execution state, not a reason to trigger a workflow by itself.

## 2026-07-17 protocol status ordering

- Workstream: stabilize the status-page protocol list without adding RPC fields or duplicating transport-priority parsing in the frontend.
- Objective: show the currently selected live connection first and the remaining live protocols in the configured transport-preference order; a transient five-second `default_conn_id` cache clear must not reorder the display while that connection remains live.
- Build-affecting: yes, Rust peer status ordering and frontend display normalization/tests.
- Evidence target: existing Rust transport-priority coverage plus frontend regression covering selected, transient zero UUID, candidate order, and active-connection removal snapshots; `.160` locked no-run/focused Rust test plus focused Vitest and frontend production build when the complete shared candidate is ready.
- Status: implementation and local Rust formatting complete; `git diff --check` passed. Remote Rust/frontend preflight remains pending until the shared worktree candidate is frozen; no build, push, or workflow dispatched.

## 2026-07-17 mobile UI wakeup reduction

- Workstream: minimal frontend-only battery optimization; no Leaf, HEV, mesh, RPC schema, or lifecycle change.
- Objective: halve active full-status polling (`1s -> 2s`), halve idle polling (`5s -> 10s`), and skip shell health/config-server/chart timers while the WebView is hidden.
- Build-affecting: frontend only.
- Evidence target: existing visibility/pause/backoff scheduler tests updated for the new cadence; focused RemoteManagement/Status tests and frontend production build on `.160` with the complete shared candidate.
- Status: implementation prepared locally; no build, push, or workflow dispatched.

## 2026-07-17 cleanup candidate: reject speculative performance fixes

- Base SHA: `be019d7182f2ac81a508102fd897134407d0c957`.
- Retained implementation: netstack `PollSender` lost-waker fix, existing mesh-owned KCP/QUIC selection, ordinary 128 KiB smoltcp buffers, HEV ownership and reserved ingress behavior.
- Removed implementation: policy mesh large-window semaphore/per-socket buffer override and portless startup proactive TCP connect.
- Retained independent work: Android captured-UID HTTP probe, policy editor changes, runtime platform notices, tests unrelated to the rejected mechanisms, and historical validation records.
- Mihomo reference reconfirmed: `common/net/sing.go::Relay` copies between kernel `net.Conn` values without an added userspace TCP window; `component/tsnet/tsnet.go::{onUse,retryStartSocks5TCP,retryStartSocks5UDP}` owns mesh listener readiness separately from policy startup.
- Evidence boundary: `lv1g2`/`lv1g3` physical IPv4/IPv6 capacity and same-artifact Linux relative controls remain valid. Android cellular-to-VPS throughput is too variable to authorize memory or startup-path changes and is retained only as functional/path evidence.
- Build-affecting status: local cleanup prepared. No build, test, commit, push, or workflow has been run for this snapshot.
- Next gate, only after explicit validation request: format locally, run the complete standard `.160` preflight once, inspect the full cleanup diff/lockfile/cfg/workflow pins, then create one candidate and one workflow pair if runtime validation is still required.

## 2026-07-17 current candidate: high-BDP mesh actor fallback

- Base artifact: exact `c7a8d8aff17e7121847782b84a4fb11c93c8d849`; Linux run `29553340531` and Android run `29553340566` passed and their checksums/build metadata matched.
- Android 4G/VPS evidence: `MATCH,DIRECT` transferred 32 MiB in `52.6715 s` (`5.096 Mbps`). The same captured Chrome UID and VPS through explicit `10.44.0.8:24443 via: mesh` did not complete in 180 seconds.
- Three-sided localization: VPS-to-KR sent and ACKed the 32 MiB body in about 3.14 seconds. KR's inner `10.44.0.8:24443 -> 10.44.0.90` socket took about 101 seconds, reported approximately `3.7 Mbps`, and was receive-window-limited for `57.6%` of busy time. Its advertised receive window was the EasyTier smoltcp data plane's fixed 128 KiB buffer.
- Linux shared-path evidence: native SOCKS transferred 8 MiB in `77.099 s` (`108802 B/s`); Leaf explicit actor transferred only `4589392/8388608` bytes in 210 seconds (`21854 B/s`) and timed out. Absolute `.37` bandwidth is not a performance baseline, but the same-host A/B confirms a shared adapter regression rather than Android-only DNS/VPN behavior.
- Browser harness boundary: the apparent post-transfer hang is not accepted as a product tail-loss result. Android kernel queues held approximately 4.8 MiB on the Chrome virtual socket and 5.5 MiB on Leaf's loopback socket while the CDP-created background page stopped consuming. Replace it with the captured-UID instrumentation HTTP stream mode.
- Reference semantics: locked Leaf is exactly `4af133266367bc6ef1d369b4b519a0a56da48760`; `leaf/src/app/dispatcher.rs` uses bounded asynchronous copy and does not create the mesh TCP window. Mihomo `/Users/fanli/Documents/mihomo-rev/common/net/sing.go::Relay` copies between kernel `net.Conn` values with half-close and likewise inserts no fixed userspace TCP window. EasyTier intentionally differs only because native mesh fallback terminates TCP in `tokio_smoltcp`.
- Candidate implementation: a 2 MiB RX/TX buffer only for policy mesh smoltcp fallback, limited to 32 simultaneous streams (128 MiB additional hard bound). KCP/QUIC streams do not consume permits; excess and ordinary streams retain 128 KiB and do not fail.
- `.160` gate: standard `scripts/leaf-remote-preflight.sh`; the existing `mesh_only_connect_never_falls_back_to_kernel` test now also proves 2 MiB mesh-only and unchanged 128 KiB ordinary capacities. Android workflow must build the captured-UID HTTP probe.
- Exact artifact evidence target: Android instrumentation DIRECT/explicit 32 MiB A/B plus KR advertised-window proof; Linux native/Leaf A/B; KCP-only, QUIC-only, native fallback, portless HEV, stop/start and RSS/FD/thread return from one batched artifact.
- Status: implementation prepared locally; formatting and `.160` preflight pending. No workflow is authorized until this complete batch passes the dispatch lock.

## Current candidate

- SHA: `5d71abed66a1ad1957834a40bc67b0d0092a95af`
- Branch: `codex/profiling-beta`
- `.160` preflight: passed; default Leaf/no-Leaf, KCP-only, and QUIC-only feature combinations compiled and the six exact mesh/prepare/lifecycle tests passed.
- Automatic Linux run: `29504862070` - in progress.
- Automatic Android run: `29504861995` - in progress.
- Same-SHA no-Leaf comparator run: `29504904098` - comparator-only job in progress; full candidate job skipped as designed.
- Comparator rationale: active disabled-mode CPU/throughput/latency/RSS/FD/thread gate.
- Known inefficiency: the current comparator workflow rebuilds the feature-on bundle.
- Local follow-up prepared in `.github/workflows/profiling-beta.yml`: `audit_comparator=true` now selects a comparator-only job with an independent concurrency group; it builds and uploads only `easytier-core-no-leaf`, does not rebuild HEV/Leaf/the feature-on bundle, and does not mutate the rolling prerelease.
- Current batched candidate: committed and pushed as `5d71abed`; transport boundary, selected-port prepare, scheduling memory, workboard, and comparator-only job share one exact snapshot.
- Final `.160` preflight: default Leaf no-run, no-Leaf no-run, KCP-only check, and QUIC-only check passed. `direct_mesh_stream_prefers_quic_then_kcp_before_native_fallback`, `prepares_then_relays_built_in_tcp_from_mesh_data_plane`, `mesh_only_connect_never_falls_back_to_kernel`, `data_plane_tcp_pingpong`, `socks_egress_guard_shutdown_waits_for_owned_task`, and `route_identity_change_cancels_only_the_old_generation` each ran as one real passing test; the no-Leaf SOCKS count test also passed.
- Reference semantics: Mihomo `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go`, `Dialer.DialContext`, `retryStartSocks5TCP`, and `retryStartSocks5UDP` keep mesh exposure and dialing inside the mesh runtime. EasyTier intentionally follows the ownership boundary rather than copying Tailscale transport internals: policy chooses an actor and prepares the managed endpoint; the existing mesh `DeferredProxySelector` alone honors `enable_quic_proxy`, `enable_kcp_proxy`, destination capabilities, readiness ACKs, health, and smoltcp fallback.
- Root cause from exact `c92b11b1` Linux artifact: portless KCP input reached `10.255.0.2:11080`, then the destination KCP proxy attempted a host-kernel connect and returned `ECONNREFUSED`; the previous unconditional HEV listener had hidden the mismatch.
- Review correction: forcing portless/UoT onto smoltcp was rejected before commit because exact evidence is approximately `941 Mbit/s` DIRECT, `478 Mbit/s` KCP, and `53.5 Mbit/s` smoltcp fallback. The replacement removes policy ownership without removing mesh acceleration or ignoring either user proxy flag.
- Port-candidate boundary: the prepare RPC returns the selected HEV port on the target virtual IP, and the userspace ingress registers all three `11080/11081/11082` candidates. An occupied `11080` therefore cannot make QUIC/KCP connect to the unrelated owner while smoltcp targets a different service.
- Confirmed design invariant: compiling `leaf-policy-proxy` registers `PolicyUdpRelayRpc` and the three smoltcp ingress candidates `11080/11081/11082` on every feature-on instance, even when local policy is disabled. This is required so a remote portless actor can select the node as an egress without requiring an unrelated local policy switch. It reserves only userspace mesh ingress state; HEV itself remains lazy. Do not repeatedly propose conditional registration as a disabled-mode optimization.
- Read-only audit finding: `KcpProxySrc::start_endpoint_only` has no external caller after `5d71abed`; only `start()` calls it, while its comment still names Policy HEV. Treat it as a stale policy-specific mesh hook pending cleanup, not as required architecture.
- Read-only audit finding: `DeferredProxySelector::connect_mesh_stream` correctly owns transport choice and has no policy dependency, but it duplicates the QUIC/KCP prepare, health, timeout, and route-restart state machine already used by `resolve_pending`. Exact runtime parity is required now; a later source cleanup should share the selection primitive rather than let the two mesh paths drift.

## Parallel workstreams

| Workstream | Description | Objective/evidence target | Build-affecting | Status |
| --- | --- | --- | --- | --- |
| Managed HEV lazy lifecycle | Replace unconditional HEV residency with an endpoint provider that starts on the first built-in TCP/UDP actor request. | No HEV PID without policy traffic; one HEV after first portless request; explicit-only does not start HEV; normal shutdown removes it. | Yes | Disabled and explicit-only gates passed; portless exposed a KCP/userspace-listener transport mismatch |
| Policy data-plane transport boundary | Replace the policy-only KCP endpoint with the existing mesh-owned QUIC/KCP selector and a narrow remote HEV prepare RPC. | User flags and destination capabilities select QUIC/KCP; failures fall back before payload; portless alone prepares HEV and propagates the selected fallback port; explicit and portless share one mesh stream API. | Yes | `.160` four feature builds and six real tests passed, including selected-port propagation; exact workflow artifact and host path/performance validation pending |
| Disabled-mode comparator | Compare feature-on/no-policy with same-SHA no-Leaf on one fixed TCP underlay. | CPU, throughput, RTT, RSS, FD, and threads within noise; no sidecar/process overhead; include the intentional three feature-on virtual ingress waiters in the measured state. | Validation only | Waiting for comparator artifact; always-on relay registration is accepted architecture, while measurable regression remains a release gate |
| Android upgrade/persistence | Upgrade the stable-signed candidate without uninstalling either EasyTier package. | Pre/post persisted-store hashes match before first start; existing instance selection/config preserved. | Validation only | Passed on `c92b11b1`: 12-file manifest and 26,624-byte tar unchanged before first start |
| Android network generation | Repeat semantic policy probes and Wi-Fi disable/enable recovery. | Same PID/TUN ownership, new network generation/DNS, captured-UID TLS recovery, no FD/task growth. | Validation only | `8b337502` passed; narrow `c92b11b1` regression pending |
| Actor path parity | Compare portless and explicit actors at one peer/target. | Same mesh selector, user flags, capability order, and fallback; 20/20 TCP; UDP echo; only portless prepares managed HEV. | Validation only | Unit selector/prepare/generation gates passed; exact artifact host matrix pending |
| Fallback + UDP lifecycle | Stack primary loss, secondary takeover, underlay outage, and concurrent UDP associations. | Fail-closed, one-second recovery, Leaf generation replacement, two-wave `+330s` FD/thread baseline. | Validation only | `8b337502` passed; lazy-start smoke regression pending |
| Mesh hook audit | Keep Leaf in rules/DNS/outbound, adapter in bridge/lifecycle, and mesh in route/overlay/KCP/smoltcp. | Policy only calls endpoint prepare; generic mesh selector owns QUIC/KCP priority, capability, health, readiness, and fallback; OSPF and platform hosts remain separate. | No | Main ownership boundary is correct; stale `start_endpoint_only` policy comment/API and duplicated selector state machine remain audit findings; exact runtime and final requirement audit pending |
| Raw UDP underlay bottleneck | Keep ordinary mesh burst/loss investigation separate from Leaf conclusions. | Fixed-underlay counters and direct baseline before any tuning. | Separate future code | Documented; not part of `c92b11b1` |
| Comparator workflow efficiency | Split same-SHA no-Leaf output from the full profiling bundle. | One no-Leaf cargo build, metadata/checksum/symbol verification, no HEV/Leaf rebuild and no rolling-release mutation. | Workflow | Batched into the next candidate through the existing dispatchable workflow; exact run pending |
| Android Policy performance/power follow-up | Investigate fixed Leaf/Policy wakeups, per-packet bridge/TUN overhead, direct priority retries, and WebView-mediated network lifecycle. | Same-SHA CPU/wakeup/syscall/queue evidence plus foreground/background Wi-Fi recovery; preserve fail-closed and first-match semantics. Detailed checklist: `android_policy_performance_power_followup.md`. | Research only; future fixes build-affecting | **Further research required.** Read-only source/runtime audit complete; no implementation authorized or started |

## Artifact-arrival execution plan

1. Verify Linux, Android, and comparator SHA/checksums/build metadata in parallel.
2. Deploy Linux artifacts to `.37/.38` while backing up and upgrading Android with `adb install -r`.
3. In parallel, run Linux no-policy sidecar absence and Android no-local-HEV startup checks.
4. Trigger one portless request and one explicit-only run to prove lazy ownership without rerunning the full already-passed matrix.
5. Run the same-bundle disabled-mode A/B and record all six resource/performance dimensions.
6. Update the release gate/report locally with exact run IDs and evidence; do not push documentation alone.

## 2026-07-16 23:40 - exact `5d71abed` combined runtime matrix

- `disabled-mode A/B`: complete. Same-SHA feature/no-Leaf artifacts showed no measurable CPU, throughput, latency, RSS, FD, or thread regression. Long throughput brackets averaged about 1592 vs 1587 Mbit/s; load CPU differed by about 0.2%; ping averaged 0.303 vs 0.309 ms.
- `explicit/portless`: complete on Linux. Both selected the same peer, target `10.255.0.2:11080`, and KCP path; repeated throughput differed only within noise. QUIC-only, KCP-only, QUIC priority, and native smoltcp fallback were all observed on the exact artifact.
- `target restart / transport fallback`: complete on Linux. Source core PID remained stable; target restart with QUIC input disabled rebuilt Leaf and selected KCP. Fail-closed counter to the physical destination remained zero.
- `chain/fallback + UDP`: complete on Linux. Healthy chain was observed as managed HEV `11080 -> peer-local HEV 28108 -> HTTP 28100`. After stopping `28108`, five TCP requests succeeded through the second `mesh-direct` member in 11-16 ms while one UDP association completed 300/300.
- `network switch + UDP`: complete on Linux. During a target restart one 400-datagram application socket recorded 23 outage timeouts and then resumed for 377 successes. The source physical-interface leak rule remained `0 packets / 0 bytes`. A fresh post-recovery association completed 50/50.
- `UDP resource return`: complete on Linux. Ten concurrent associations completed 200/200. Active FD counts were source core/Leaf `57/36` and target core/HEV `88/48`; after timeout they returned to `37/16` and `38/18`, with threads `9/4` and `9/2` and no RSS growth.
- `Android upgrade/recovery`: functional portion complete. Candidate PID `22669` and VPN network `305` survived the formal device-side Wi-Fi cycle. Underlay changed to network `309`; app events recorded `outage!4` then `309@wifi...!4` with restored DNS. Mesh and Baidu/Cloudflare TLS handshakes recovered.
- `Android resource return`: still open. Formal-cycle snapshots were RSS 271788 -> 266964 KiB, FD 402 -> 413, threads 67 -> 68. Attribute or wait out the +11 FD/+1 thread delta before closing the gate.
- `platform audit`: architecture is extensible but current implementation is not Windows-complete. Policy schema/supervisor and mesh selector are platform-neutral; `LeafPacketBridge` and process Leaf host remain Unix-gated. macOS can reuse the Unix process host outside Network Extension; Windows and macOS Network Extension require host packet/routing adapters without changing mesh protocol semantics.
- `next`: settle/attribute Android resources, decide whether a temporary backed-up Android chain/fallback configuration is necessary, uninstall only the two probe packages after final evidence, clean all Linux fixtures, then consolidate the release report. No build or workflow is needed for these runtime-only gates.

## 2026-07-16 final audit correction

- Supersedes the earlier `Android resource return: still open` line for exact candidate `5d71abed`.
- A clean Stop/Start established Android running baseline FD/thread `323/67` and stopped baseline `280/60`.
- One isolated device-side Wi-Fi cycle produced the expected transient `347/68`, fell to `335/68`, then returned to `323/67`; RSS also fell below the pre-cycle value. The previously observed `402 -> 413` comparison mixed several Wi-Fi generations, CDP, WebView and instrumentation activity and is not valid leak evidence.
- Android resource return is therefore complete: bounded delayed cleanup, no persistent FD/thread/runtime leak.
- The captured Jelly UID WebRTC test reached neither of two STUN endpoints through the current explicit `10.44.0.8:24443` SOCKS actor. Record this as the configured third-party SOCKS UDP capability boundary; do not add payload-timeout fallback.
- Linux and Android validation fixtures are cleaned. Probe APKs are uninstalled; the candidate package, user data and active VPN remain.
- Final read-only ownership/platform report: `docs/leaf_overdesign_performance_audit_2026_07_16_cn.md`.
## Candidate `089d7e0a` - USB Android independent explicit-actor lane

| Workstream | Objective | Build affecting | Evidence target | Status | Shared SHA |
| --- | --- | --- | --- | --- | --- |
| USB Android fresh baseline | Establish a device/network-independent 4G dual-stack baseline without reusing old Android data | No | VPN UID ownership, mesh peer, DIRECT TLS control | Complete | `089d7e0a` |
| Explicit port actor | Validate `10.44.0.8:24443`, `via: mesh` without confusing it with portless HEV | No | Captured-UID TLS plus peer-side `24443` observation | Complete | `089d7e0a` |
| Android diagnostic throughput | Compare policy-disabled direct SOCKS with explicit Leaf actor on one controlled object | No | Repeated 16/64 MiB samples; no CDN dependency | Complete, no general regression reproduced | `089d7e0a` |
| Earlier Android severe slowdown | Explain the original device-specific near-unusable report | No | Same-device direct SOCKS versus explicit actor A/B | Blocked only by old device availability; do not substitute USB result | `089d7e0a` |
| Linux portless queue stall | Prove and fix the permanent core-to-Leaf queue stall without changing mesh ownership | Yes, if patched | Focused lost-waker regression plus `.160` preflight | In progress | next batched candidate |
### Candidate manifest: `stack-pollsender-20260717` (uncommitted snapshot)

- Included functions: `StackBuilder::build`, `Stack::poll_send`, `Sink::poll_ready`,
  `stack::tests::full_ingress_channel_wakes_waiting_stack_sender`, and removal of the
  temporary TCP-driver `yield_now` mitigation.
- Compatibility boundary: no EasyTier mesh routing, QUIC/KCP selection, HEV endpoint,
  actor ownership, DNS, or policy rule semantics change.
- `.160` gate: `scripts/leaf-remote-preflight.sh`; one locked no-run build for
  `easytier`, `easytier-policy`, and `netstack-smoltcp`, followed by all standard
  focused filters plus the exact lost-waker regression, each with one test thread.
- Required GitHub workflows after the gate: the normal automatic Linux and Android
  profiling-beta pair only; no comparator unless a disabled-mode gate is activated.
- Linux artifact evidence target: reproduce sustained portless ingress load beyond the
  prior four-minute failure window, observe zero new core-to-Leaf queue drops, then
  crash/restart/cleanup and compare throughput.
- Android artifact evidence target: rerun captured-UID DIRECT and explicit-port actor
  TLS, then a bounded sustained transfer and stop/start cleanup on the USB 4G device.
- During `.160` wait: inspect lockfile/cfg/workflow pins and prepare Linux queue-drop
  counters. During GitHub wait: clean isolated Linux fixtures and preserve Android
  candidate data; do not mutate the in-flight snapshot.

## 2026-07-17 high-BDP explicit actor + portless HEV combined candidate dispatch lock

- Base SHA: `c7a8d8aff17e7121847782b84a4fb11c93c8d849`; intended snapshot is this base plus the complete current worktree listed by the candidate commit.
- Independent lanes: bounded 2 MiB smoltcp window for mesh-only native fallback; unchanged 128 KiB ordinary sockets; portless HEV registration/ownership boundary; policy editor portless/rule/runtime behavior; Android captured-UID HTTP byte-stream probe; audit and validation documentation.
- `.160` Rust gate: `scripts/leaf-remote-preflight.sh` passed. Exact library no-run completed; mesh-only no-kernel fallback/window isolation, UoT relay, HEV ownership, Leaf domain contract, and netstack lost-waker tests passed.
- `.160` frontend gate: Node `v22.23.1`, pnpm `9.15.4`; 3 Vitest files / 27 tests passed; `vue-tsc -b` and Vite production build passed.
- Android probe exception: Java instrumentation compilation requires the Android SDK/toolchain available in the Android workflow; this is the only pre-push target-toolchain exception and must pass before deployment.
- Workflow set: one automatic Linux profiling-beta run and one automatic Android run from the same commit. Query exact-SHA runs before any manual dispatch; do not duplicate.
- Artifact evidence: verify SHA256/build metadata first. Android 4G + lv1g2/lv1g3 collects IPv4 and IPv6 DIRECT, explicit `10.44.0.8:24443 via:mesh`, native SOCKS comparator, portless managed HEV, byte completeness, throughput, selector/transport evidence, failure recovery, concurrency cap, and FD/thread/RSS return. Linux collects same-artifact correctness, native/Leaf relative A/B, KCP/QUIC/native selection, cleanup and resource baseline; internal hosts are not absolute throughput baselines.
- During workflow wait: keep both VPS dual-stack fixed-byte HTTP endpoints ready, preserve Android app data, prepare semantic instrumentation commands, and do not mutate the immutable candidate.

## 2026-07-17 `cf7215b4` Android 4G/VPS explicit + portless diagnosis

| Workstream | Objective | Build affecting | Evidence target | Status | Shared SHA |
| --- | --- | --- | --- | --- | --- |
| Explicit actor high-BDP regression | Prove the bounded mesh-only smoltcp window removes the previous fixed-window stall without changing ordinary mesh sockets | No further change pending | Captured-UID fixed-byte HTTP over Android 4G to both public VPS targets; IPv4/IPv6; complete byte count and same-time DIRECT controls | Complete for correctness; throughput remains path-variable | `cf7215b44084441ccd64a3aad1443dc4046ab721` |
| Portless native startup race | Separate HEV/listener faults from source mesh route readiness after Android instance restart | Investigation only | Same exact artifact, target-specific policy rule, remote loopback/public pcap, stable-peer SOCKS control, repeated delayed retry, direct-tunnel timestamps | Confirmed intermittent route/tunnel-ready race; no code fix selected | `cf7215b44084441ccd64a3aad1443dc4046ab721` |
| Portless accelerated control | Verify user-enabled KCP/QUIC bypasses the unavailable native startup path without changing actor ownership | No | Two VPS fixed-byte transfers immediately after restart | Complete: 2/2 full 16 MiB | `cf7215b44084441ccd64a3aad1443dc4046ab721` |

- Performance boundary: only Android 4G ingress plus `lv1g2`/`lv1g3` public targets is accepted for this lane. `.37/.38/.160` are compiler, isolated correctness, or same-host relative controls and must not be used as Internet throughput baselines.
- Explicit actor: native and accelerated configurations completed every 16 MiB transfer to both VPS targets. Same-time results ranged from `1.975` to `27.985 Mbps`, while DIRECT itself ranged from `3.933` to `16.519 Mbps`; this is strong path variability, not a fixed Leaf cap.
- Portless native immediately after one restart failed 2/2 with local TCP established, zero HTTP bytes, and close after about 20 seconds. The two built-in connect attempts are each bounded at 10 seconds.
- The `.86` destination retained one healthy HEV process and listener. A stable `.8` mesh peer reached `.86:11080` and transferred 1 MiB in `1.227s`; therefore stale HEV, dropped userspace listener, failed VPS service, and kernel-port collision are disproved for this incident.
- During a narrowed failed probe, `.86` captured no new loopback `11080` packet and no VPS SYN. The request did not reach destination HEV. KCP/QUIC-enabled portless then completed both 16 MiB transfers (`3.548` and `3.684 Mbps`).
- Keeping the failed native configuration unchanged later produced two full 4 MiB transfers (`3.656` and `3.599 Mbps`). On a fresh restart, the Android source established a direct WG tunnel to `.86` peer `4267670262` at about `t+11s`; the probe started at `t+10s` and completed successfully. This localizes the fault to first traffic racing mesh route/tunnel readiness.
- Reference semantics: Mihomo `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go::{Snapshot,retryStartSocks5TCP,onUse}` exposes explicit readiness and retries failed mesh SOCKS availability on use with bounded exponential backoff. EasyTier currently caches a successful remote HEV prepare before proving the source native TCP data path is usable.
- Do not modify EasyTier routing, smoltcp forwarding, HEV ownership, or the always-registered three-port ingress from this evidence. Any candidate must remain inside `MeshProxyBridgeSet`/`RemoteTcpPreparation`, distinguish remote prepare from end-to-end data-path ready, preserve user `enable_kcp_proxy`/`enable_quic_proxy`, and add a restart/route-readiness regression before workflow dispatch.
- User-selected minimal candidate: no readiness state machine. `MeshProxyBridgeSet::start` performs one synchronous portless-only TCP connect after remote prepare and before Leaf starts, then drops the stream. Failure only logs and retains existing per-request retry/fallback. Explicit-port actors and EasyTier mesh internals are unchanged. Build-affecting status: implementation frozen; formatting and `.160` preflight passed.
## 2026-07-17 portless built-in actor proactive TCP connect candidate

- Status: implementation frozen; `.160` preflight passed; exact artifacts pending.
- Parent candidate: `cf7215b44084441ccd64a3aad1443dc4046ab721`.
- Build-affecting scope: `MeshProxyBridgeSet::start` performs exactly one best-effort TCP connect through the existing prepared endpoint for each portless built-in mesh actor, then immediately drops the stream. Explicit user endpoints are not probed.
- Deliberate boundary: no mesh/OSPF/smoltcp changes, no readiness state machine, no retry loop, no new timeout, no new transport selection, and no fallback semantic change. Existing `connect_remote` continues to honor the configured QUIC/KCP/native mesh selector.
- Reference: Mihomo `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go`, `Snapshot`, `onUse`, and `retryStartSocks5TCP`; EasyTier intentionally uses only one proactive connect instead of importing Mihomo's readiness/retry lifecycle.
- `.160` no-run: `scripts/leaf-remote-preflight.sh`; `cargo test --locked --no-run` completed successfully for the EasyTier, policy, and netstack library targets.
- `.160` focused evidence: the complete standard Leaf/HEV suite passed; `policy_proxy::mesh_socks_bridge::tests::proactive_tcping_is_limited_to_portless_built_in_actors` also passed against the exact compiled binary and is now part of the standard filter list.
- GitHub workflows: one automatic Linux/Android profiling-beta workflow set after the complete candidate diff is accepted; do not dispatch a duplicate run.
- Linux artifact evidence: startup probe reaches the prepared portless built-in endpoint; explicit actor remains untouched; native/KCP/QUIC selector behavior and stop/start cleanup remain unchanged.
- Android artifact evidence: on USB ADB device over 4G, repeat cold start and immediate first request against `lv1g2` and `lv1g3`; require full HTTP bytes without the observed approximately 20-second zero-byte failure. Recheck explicit actor, native portless, accelerated portless, stop/start, and process/FD/task cleanup from the same exact APK.
- Workflow wait work: prepare bounded Android probe commands, preserve application data, preflight VPS fixed-byte endpoints, and prepare Linux isolated-host cleanup without mutating the in-flight snapshot.

## 2026-07-17 high-BDP memory correction and VPS dual-stack matrix

- User rejected the previous `32 * (2 MiB RX + 2 MiB TX)` bound. The buffers are eagerly allocated with `vec![0; size]`, so the old absolute capacity was approximately 128 MiB and approximately 120 MiB above ordinary buffers; it is not acceptable for Android, iOS, or embedded Linux.
- Frozen correction: native policy fallback uses `2 MiB RX + 128 KiB TX` for at most four simultaneous streams. Absolute capacity is approximately 8.5 MiB and incremental capacity approximately 7.5 MiB. The fifth and later streams stay mesh-only with `128 KiB + 128 KiB`; they do not wait, fail, or use a kernel socket.
- Test requirement: hold four high-window streams concurrently, prove the fifth uses ordinary capacities, and preserve the existing no-kernel-fallback assertion.
- Physical `lv1g2`/`lv1g3` baseline on isolated port `24990`: single TCP IPv4 forward/reverse `7.77/8.73 Gbit/s`; IPv6 forward/reverse `8.30/8.48 Gbit/s`. Dual-stack hostname HTTP selected IPv4 automatically in both directions; forced IPv4 and IPv6 each returned the complete controlled 1 MiB object.
- UDP physical control at requested `1 Gbit/s`: IPv4 and IPv6 forward receivers reached `827/775 Mbit/s` with `16%/21%` loss; reverse senders sustained the requested rate. This is a capacity/path control, not a lossless product target.
- Existing evidence is insufficient for the requested Leaf conclusion: the prior portless run used only a `tcp6` underlay, while the prior explicit actor targeted the separate `.8:24443` peer. The new exact artifact must cover `tcp4`, `tcp6`, and dual-stack selection between the same two VPS hosts for explicit, portless native, and configured KCP/QUIC paths.

## 2026-07-17 low-memory dual-stack candidate manifest

- Intended parent: `62014be7448c9a5efdb672df227cdc812728961e`; the final commit SHA will be recorded from the commit result.
- Included implementation: one best-effort portless built-in actor TCP prewarm before Leaf startup; high-BDP native fallback limited to `2 MiB RX + 128 KiB TX` for four streams; fifth and later streams retain mesh-only routing with ordinary buffers.
- Included tests: portless-only prewarm boundary; remote prepare/relay primitive; four simultaneous large-window streams; fifth-stream ordinary-buffer degradation; no kernel fallback; standard Leaf/HEV lifecycle, UDP relay, policy-domain and netstack wakeup tests.
- `.160` gate: standard `scripts/leaf-remote-preflight.sh`, one locked library no-run build and exact direct test binaries. The complete implementation passed with the high-window test in the permanent standard filter list.
- Lock/cfg/workflow gate: `Cargo.lock`, generated files, platform `cfg`, and workflow pins must remain unchanged; run `git diff --check` and one complete candidate diff review before commit.
- GitHub workflows: one automatic Linux profiling-beta and one automatic Android policy candidate run from the same final SHA. Query exact-SHA runs and do not manually duplicate them.
- Linux/VPS evidence: verify artifact metadata and hashes; deploy isolated instances to `lv1g2` and `lv1g3`; collect physical and EasyTier `tcp4`, `tcp6`, dual-stack-selection controls; explicit actor, portless native, KCP/QUIC, IPv4/IPv6 destinations, throughput, CPU/RSS/FD/tasks, stop/start and cleanup.
- Android evidence: preserve app data and install with `adb install -r`; over LTE run same-target DIRECT, explicit actor, portless native and KCP/QUIC against both VPS targets; repeat cold-start immediate first traffic, byte completeness, IPv4/IPv6, CPU/RSS/FD/tasks and cleanup. Android is mobile-platform evidence, not the sole capacity baseline.
- Workflow wait work: prepare isolated listener ports and cleanup commands on both VPS hosts; retain the fixed-byte servers; preserve Android stores; do not mutate the in-flight SHA.

## 2026-07-17 IPv6 FakeDNS bounded/configurable candidate

- Status: implementation in progress; build affecting; not dispatched.
- Objective: enable AAAA FakeIP only when EasyTier IPv6 is enabled, keep Leaf isolated from mesh transport ownership, bound mobile/embedded memory, and permit YAML range customization.
- Mihomo parity references: `/Users/fanli/Documents/mihomo-rev/config/config.go::{parseDNS,parseIPV6}` parses `dns.fake-ip-range6`, rejects non-IPv6 prefixes, and removes pool6 when IPv6 is disabled; `component/fakeip/pool.go::{New,get}` masks the configured prefix and cycles addresses; `component/fakeip/memory.go::newMemoryStore` and `config/config.go::parseDNS` cap the in-memory pool at 1000 entries.
- Intentional difference: EasyTier uses the low-collision default `fd65:6173:7974::/64` instead of the common Mihomo example range, reserves the first four addresses, and cycles exactly 1000 slots. Invalid or too-small YAML ranges fail policy validation and Leaf construction; IPv6-disabled mode emits no AAAA FakeIP.
- Evidence target: `.160` locked no-run plus focused Leaf allocation/config tests and EasyTier policy/config tests; frontend codec test/build; exact Linux and Android artifacts later validate A/AAAA coexistence, custom range, IPv6-off NODATA, TCP/UDP reverse mapping, bounded RSS, lifecycle, and unchanged IPv4/mesh behavior.
- IPv4 extension added to the same candidate: `dns.fake-ip-range`, default `198.19.0.0/16`, first four addresses reserved (including virtual DNS `198.19.0.1`), fixed 1000-slot cyclic storage, and `/22` minimum address space. This replaces the common Mihomo/Clash `198.18.0.0/16` default without using LAN or CGNAT space; the same `.160`, frontend, Linux, and Android candidate evidence must cover both address families.
- Review corrections before preflight: fixed free-function range validation calls; moved JSON fields from the accidentally matched NF structure to TUN; restored Leaf's existing two-argument `FakeDns::new` and IPv4 `query_fake_ip` API; bounded frontend remembered-connection state to 1024; added `fd65:6173:7974::/48` to EasyTier's built-in underlay guard.
- Leaf standalone preflight exception: upstream does not commit a workspace `Cargo.lock`, so standalone `--locked` fails before dependency resolution. `PROTO_GEN=1` also regenerates unrelated lite-runtime `geosite.rs`, whose existing tests require `MessageFull`; only the generated `config.rs` is retained, then the original geosite output is restored and the exact Leaf feature set is compiled/tested without `PROTO_GEN`. The final EasyTier dependency build remains `--locked` against the committed Leaf SHA.

## 2026-07-17 dual-stack FakeDNS beta candidate manifest

- Snapshot: base `be019d7182f2ac81a508102fd897134407d0c957`, preflight code/config diff SHA-256 `68a0c118ef08e2c45ee073e1c149d11e087910a5de753bf26c72c454f5b15652` (live workboard excluded); final commit SHA is recorded after the preflighted tree is committed without source changes.
- Dependency: Leaf fork `4af133266367bc6ef1d369b4b519a0a56da48760`; exact-feature library check and `leaf/tests/fake_dns_ranges.rs` passed on `192.168.2.160` (3 tests). Standalone Leaf `cargo test --lib` remains blocked by the pre-existing GeoSite lite-runtime/full-message test mismatch; this is not hidden as candidate evidence.
- Included functions: bounded configurable IPv4/IPv6 FakeDNS pools; IPv6 opt-in following EasyTier IPv6; Android virtual-DNS reservation; direct/proxy DNS presets; IPv6 FakeIP underlay guard; GUI YAML round-trip; deterministic status ordering; bounded frontend peer cache; reduced/paused GUI polling; removal of rejected large-smoltcp-window and proactive-portless-tcping experiments; documentation and focused tests.
- `.160` gate: run `scripts/leaf-remote-preflight.sh` against the complete snapshot, including `--locked` no-run builds and focused underlay/FakeDNS/DNS tests. Run focused frontend Vitest files and the production frontend build with Node 22 in `easytier-debug-builder`.
- Lock/cfg/generated/workflow review: verify Cargo.lock pins Leaf `4af1332`, generated Leaf protobuf matches the source schema, IPv6 behavior remains feature/config gated, and profiling-beta workflow pins are unchanged.
- GitHub workflows: one push to `codex/profiling-beta`; accept only the automatic Linux profiling-beta and Android profiling-beta runs for the exact candidate SHA. Query exact-SHA runs before any manual action and do not duplicate dispatches.
- Linux evidence: optimized artifact metadata, checksum, target, symbols, and exact SHA. This beta publication does not claim a new full real-device recovery/performance matrix.
- Android evidence: workflow APK/artifact metadata, checksum, and exact SHA. This beta publication does not claim a new Wi-Fi/cellular/DNS-transition matrix.
- During `.160` wait: review lockfile, platform cfg, generated protobuf, workflow pins, release-note risks, and frontend test scope without starting another Cargo job.
- During GitHub wait: prepare the warning-first beta notes, artifact verification commands, and known-risk matrix; do not mutate the in-flight snapshot.
- Status: Leaf dependency preflight passed; frontend pnpm 9.12.1 frozen install, VPN plugin build, GUI production build, and 34 focused Vitest assertions passed. EasyTier `.160` final `--locked` no-run and focused suite passed after removing unrelated resolver drift. Ready for one immutable candidate commit/push.

## 2026-07-17 default YAML DNS correction candidate

- Snapshot: base `f8b0ca79c7ab5d3c33eedc60cb7da9d15bbb2570`, frontend diff SHA-256 `96168808eb5d6ca25445bdba6af1905007407cb3065767331eb467e45311268b` (live workboard excluded).
- Scope: correct only `DEFAULT_POLICY_TEMPLATE`; add direct resolvers `system`, `223.5.5.5`, `119.29.29.29`, `114.114.114.114`, and proxy DoH resolvers Cloudflare, Google, Quad9. Preserve backend omitted-field compatibility and all routing semantics.
- `.160` evidence: focused `policy-editor.spec.ts` and `policy-document.spec.ts`, frontend-lib build, and GUI production build with Node 22/pnpm 9.12.1.
- Workflow evidence: after `.160` passes, one push to `codex/profiling-beta`; accept only the automatic exact-SHA Linux/Android runs and update the rolling warning prerelease after artifact verification.
- Status: `.160` frontend preflight pending.

## Candidate: policy TUN / UDP hole-punch recursion (2026-07-17)

- Shared base artifact: `f8b0ca79c7ab5d3c33eedc60cb7da9d15bbb2570`; profiling run `29577571146`; Linux build ID `13b6895f6df3a66d7379bd77636aa0080bdb7e09`.
- Workstream: mark every production UDP hole-punch socket before its first send.
- Objective: prevent EasyTier underlay discovery/punch packets from re-entering Linux policy TUN and being recursively forwarded through Leaf.
- Build-affecting: yes, Rust core only.
- Status: implementation complete; remote preflight pending.
- Exact diagnosis: clean namespace syscall trace showed unmarked 28-byte UDP punch packets from EasyTier to `47.115.208.211:11010`. A temporary single-destination OUTPUT mark matched 18 packets and reduced empty-runtime synthetic policy `FlowKey` creation to zero in 8 seconds.
- Reference semantics: Mihomo `component/dialer/options.go` (`DialContext`, `ListenPacket`), `component/dialer/mark_linux.go` (`bindMarkToControl`), and `component/dialer/dialer.go` (`DefaultRoutingMark`) apply the routing mark before outbound network I/O. EasyTier intentionally uses its existing global `socket_mark`; non-policy mode remains `None`.
- Compatibility boundary: no Leaf/HEV changes, no QUIC/KCP selection changes, no packet/retry/lifecycle changes, and no per-packet work. Linux/Android/Fuchsia perform one `setsockopt(SO_MARK)` per created hole-punch socket only when configured; other platforms and `None` are no-ops.
- Focused test: `hole_punch_socket_inherits_global_socket_mark` (Linux, CAP_NET_ADMIN, ignored by default).
- `.160` gate: `scripts/leaf-remote-preflight.sh` complete batch, then run the ignored SO_MARK test directly from the exact compiled lib test binary.
- Diff/dispatch checks: inspect Cargo.lock, target cfg, workflow pins, generated files, and the complete candidate diff after preflight. No dependency/generated/workflow change is intended.
- GitHub workflows: one automatic Linux + Android candidate set only after the complete `.160` gate passes; no narrow manual dispatch.
- Linux artifact evidence: clean policy namespace idle stability, no synthetic stream/FD growth, relay route, explicit mesh SOCKS policy traffic, hole-punch/P2P recovery, stop/start cleanup.
- Android artifact evidence: preserve app data, semantic start/status, captured-UID TLS policy probe, relay-backed explicit mesh SOCKS, network/DNS change recovery, stop/start cleanup.
- Work during waits: review locked Leaf boundary and complete diff; prepare namespace/ADB probes and resource baselines without mutating the in-flight snapshot.
- Preflight result (2026-07-17): 192.168.2.160 locked no-run passed for easytier, easytier-policy, and netstack-smoltcp; the standard focused suite passed.
- Exact mark evidence: the exact EasyTier debug lib test binary ran hole_punch_socket_inherits_global_socket_mark with --exact --ignored; 1 passed, 0 failed.
- Frontend compatibility decision: when the root dns section is entirely absent, legacy policy documents inherit the current template's four direct and three proxy DNS servers. An explicitly present dns section retains field-level behavior and is not replaced wholesale.
- Frontend evidence: 192.168.2.160 pnpm 9.12.1 focused Vitest passed 24/24; frontend-lib production vue-tsc and Vite build passed. The remote Rollup optional dependency was repaired with a frozen-lockfile install; no lockfile changed.
- Dispatch status: ready after final metadata check; one batched profiling-beta push, then Linux and Android validation from the exact artifacts.

## 2026-07-18 netstack runner event-driven P0 candidate

- Status: implementation and local formatting complete; `.160` locked no-run and focused suite passed; build affecting; exact workflow artifacts pending.
- Parent snapshot: `48ae8825f627cde741bb1ff464718ac92fbafec6`; final candidate SHA is recorded only after the complete preflighted snapshot is committed unchanged.
- Confirmed runtime root cause: Android `simpleperf` attributed 95.11% of samples from one continuously running Tokio worker to `netstack_smoltcp::tcp::TcpListenerRunner`; UID network counters remained unchanged. The runner survived Leaf restart because a closed/full stack output channel made smoltcp report immediate work without any await.
- Locked dependency evidence: EasyTier `Cargo.lock` pins Leaf fork `4af133266367bc6ef1d369b4b519a0a56da48760`. Its `leaf/src/proxy/tun/inbound.rs::new_smoltcp` spawns the netstack runner without retaining the JoinHandle. This candidate intentionally leaves Leaf ownership unchanged and makes output receiver closure the runner termination protocol.
- Mihomo lifecycle reference: `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::{New,Listener.Close}` retains the TUN stack in `Listener` and explicitly closes it with the listener. EasyTier intentionally differs because the pinned Leaf API detaches the runner; the compatibility boundary is prompt `BrokenPipe` termination when Leaf drops `Stack`, with no routing, DNS, policy, HEV, QUIC/KCP, packet-format, MTU, or TCP-window change.
- Implementation: replace the internal unbounded ingress queue and separate `AtomicBool` with a bounded queue that inherits `tcp_rx.max_capacity()` (512 frames under the existing default); `handle_packet` awaits capacity and notifies after every successful enqueue. `VirtualDevice::receive` reserves output before dequeueing ingress, distinguishes output Full from Closed, and records a sticky per-poll Full state. The runner waits for real output capacity, waits event-driven on `Notify`/close when no timer exists, and consumes Tokio cooperative budget for immediate deadlines.
- Memory boundary: no new configured buffer or eager allocation is added. Internal staging is bounded to the same frame count as the existing upstream TCP queue; producer backpressure prevents unbounded retained `Vec` growth. Frame bytes remain bounded by the existing upstream IP packet source and are not copied by the queue transition.
- Focused unit evidence target: output Full preserves ingress bytes/order; bounded ingress blocks and wakes senders; output receiver drop terminates the live runner with `BrokenPipe`; repeated zero-delay polls do not starve a current-thread Tokio timer; existing `full_ingress_channel_wakes_waiting_stack_sender` remains green.
- `.160` evidence: the first no-run attempt correctly rejected a field-borrow compilation error before workflow dispatch. After expressing the disjoint output/ingress borrows explicitly, the complete standard `scripts/leaf-remote-preflight.sh` locked no-run build passed for EasyTier, policy and netstack libraries. All standard focused tests and all five netstack backpressure/lifecycle tests passed from the exact binaries. `Cargo.lock`, platform `cfg`, workflow pins, generated files and dependency metadata are unchanged; `git diff --check` and the complete candidate diff review passed.
- GitHub workflow set: one automatic Linux profiling-beta and one automatic Android profiling-beta run for the exact SHA; query exact-SHA runs and do not dispatch duplicates.
- Linux artifact evidence: TCP, UDP, FakeDNS and HEV chain/fallback correctness; idle 60-second CPU below 5% of one core; ten stop/start cycles with old TIDs, FD and tasks returning to baseline; three-run throughput median at least 95% of the parent candidate under the same topology.
- Android artifact evidence: preserve app data and install with `adb install -r`; logging off with zero file growth; captured-UID policy traffic plus TCP/UDP/FakeDNS/HEV chain/fallback; idle 60-second per-TID/UID CPU below 5% of one core and zero traffic growth; ten stop/start cycles with no old runner TID, FD/task/RSS growth; no stable throughput or functional regression against the parent artifact on the same device/network.
- Work during `.160` wait: inspect the complete lifecycle/backpressure diff, lock/cfg/workflow pins, and prepare bounded Linux/Android CPU/lifecycle probes without starting another Cargo job. Work during GitHub wait: pre-clean isolated Linux fixtures, preserve Android stores, prepare exact artifact verification and same-topology parent/candidate throughput commands without mutating the in-flight SHA.

## 2026-07-18 - Netstack runner event-driven candidate closed

- Shared candidate: `6dd2fe7e84ffd68fdb69d61088861c4c79ca7659`.
- Build-affecting status: implementation committed and pushed; `.160` locked no-run and focused tests passed; Linux run `29597762791` and Android run `29597762786` passed.
- Exact evidence: Android DIRECT and mesh-SOCKS TLS passed before/after ten cycles; the former 0.997-core/95.11% runner hotspot no longer appears. Whole Android UID is still about 0.094 core and remains a separate power follow-up.
- Linux same-window A/B: DIRECT candidate/parent `413.3/410.8 Mbps`; HEV candidate/parent `422.0/428.1 Mbps`. Functional DIRECT, HEV, chain, fallback, UDP and post-cycle replay passed.
- Linux lifecycle: ten accurate `/proc/PID/exe`-identified cycles removed every old PID; core/Leaf tasks, FD and RSS stayed bounded; final idle CPU was `0.1333%/0.0833%`.
- Cleanup: namespace, TUN, test processes, iperf servers and exact firewall rules removed; Android probes and profiler files removed; candidate VPN intentionally remains running.
- Detailed development, artifact, performance, lifecycle, compatibility and residual-risk record: [netstack_runner_event_driven_validation.md](./netstack_runner_event_driven_validation.md).

## 2026-07-18 - Netstack fairness and connection-scale follow-up

- Shared candidate: current formatted worktree; exact SHA is assigned by the candidate commit. Parent comparator is validated `6dd2fe7e84ffd68fdb69d61088861c4c79ca7659`.
- Workstreams: retain the output permit granted after backpressure so TCP/ICMP makes progress against the shared UDP sender; reduce the two smoltcp socket buffers and two Leaf Stream rings from `0x3fff * 20` bytes to 32 KiB per direction. The later dynamic Stream queue/global-byte-budget design is explicitly excluded from this candidate.
- Locked Leaf boundary: `Cargo.lock` pins `lovitus/leaf` `4af133266367bc6ef1d369b4b519a0a56da48760`; `leaf/src/proxy/tun/inbound.rs::new_smoltcp` uses this vendored stack and retains the existing detached-runner lifecycle.
- Mihomo reference: source HEAD `0a87b94845ef908c15f8495871e4cd8e33116328`, `listener/sing_tun/server.go::{New,Listener.Close}` owns and closes the selected stack. Its pinned `github.com/metacubex/sing-tun` v0.4.17 source commit `00e7bcea347af9d3b274491f77921827a678b50e`, `stack_gvisor.go::NewGVisorStackWithOptions`, fixes both gVisor TCP receive and send buffer defaults/maxima at 20 KiB and enables moderate receive buffering; `stack_gvisor_tcp.go::NewTCPForwarderWithLoopback` caps pending forwarded endpoints at 1024. Observable compatibility target is bounded per-connection memory with ordinary TCP backpressure, not identical gVisor allocation or autotuning internals.
- Intentional difference: EasyTier keeps the pinned Leaf/smoltcp architecture and uses fixed 32 KiB socket windows plus fixed 32 KiB Stream rings for this first scale candidate. This is larger than Mihomo's pinned gVisor default, has no receive-window autotuning, and may cap throughput if the local TUN/userspace scheduling loop exceeds the measured bandwidth-delay budget; failure behavior is ordinary TCP window backpressure, never packet loss or unbounded allocation.
- `.160` evidence target: locked no-run plus existing Leaf/HEV suite; permit competition, unused-permit release, 32 KiB full/empty/waker/half-close tests. GitHub and real-device evidence target: same-window parent/candidate DIRECT and HEV throughput, mixed TCP/UDP progress, 1/100/1000 Linux connection RSS/CPU/p99 and close storm, plus Android functional/lifecycle/idle-CPU regression checks.
- `.160` dispatch gate: passed twice after the complete implementation and again after the final flow-control test, using `scripts/leaf-remote-preflight.sh`. The exact EasyTier, policy, and vendored netstack library binaries passed the locked no-run build and all configured focused tests, including `capacity_wait_makes_progress_with_queued_output_sender`, `unused_reserved_output_capacity_is_released_after_poll`, `default_tcp_buffers_are_32_kib_per_layer`, `default_stream_send_buffer_backpressures_and_wakes_at_32_kib`, shutdown, output-close, and cooperative-runtime coverage.
- Candidate manifest: production changes are limited to `VirtualDevice::{wait_output_capacity,receive,transmit,release_unused_output_permit}`, `VirtualTxToken::consume`, the runner's post-poll permit release, and the four per-connection buffer allocations. No Cargo dependency/lockfile, generated protocol, platform `cfg`, workflow pin, Leaf API, routing, DNS/FakeDNS, HEV, QUIC/KCP, packet-format, or MTU change is included. The automatic profiling-beta Linux and Android workflows are the required authoritative build set; do not manually duplicate them.
- Linux artifact matrix: verify exact SHA/build metadata; run same-window parent/candidate DIRECT and HEV throughput; prove TCP progress while UDP shares the output queue; measure 1/100/1000 established connections and close storm for RSS, CPU, p99, task/FD cleanup, and packet loss. Android matrix: preserve data and upgrade in place; repeat captured-UID DIRECT/mesh-SOCKS TLS, TCP/UDP/FakeDNS/HEV chain/fallback, idle CPU, ten lifecycle cycles, task/FD/RSS cleanup, and logging-off zero-growth checks.
- Work during workflow wait: inspect artifact metadata and checksums, pre-clean isolated Linux fixtures, preserve Android stores, and prepare bounded parent/candidate commands. The in-flight snapshot must remain immutable; dynamic queues and a global byte budget remain a separate future candidate.
