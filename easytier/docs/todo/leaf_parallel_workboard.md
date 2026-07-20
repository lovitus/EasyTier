# Leaf Parallel Candidate Workboard

This is the live execution board for batching independent Leaf/policy work into one exact candidate.
It is local execution state, not a reason to trigger a workflow by itself.

## 2026-07-20 v3.0.0 lossless recovery

- Recovery base: exact validated `f6617c5136672016951adb0f79ab0daec7ba7112`.
- Invalid candidate: `defaa44290c73726b9ab4775f1405ac01de0f84e`; Core and Test failed and the candidate omitted the accepted GUI/Geo work. It is diagnostic evidence only.
- Archives: invalid candidate, GUI/Geo WIP, PollSender WIP, rejected AGENTS edit and all three stash parents plus an ordinary combined stash snapshot are pushed and independently fetched with matching commit/tree IDs.
- Reconstruction: PORT-RANGE, platform/32-bit/API fixes, profiling history, Geo backend, compact GUI and WIP documentation are split into logical commits on `codex/v3.0.0-recovery`.
- PollSender: archived intermediate runner is superseded by the bounded event-driven implementation already present in `f6617c51`; evidence is recorded in `docs/release/v3.0.0_recovery_ledger.md`.
- Readiness result: the exact `.160` three-node case passed without the `defaa442` test hunk; the hunk is rejected rather than carried into the recovery candidate.
- Dispatch lock: no candidate push until the recovery tree, version metadata, release notes, `.160` Rust/frontend gates and the six-revert public bridge all match one immutable tree.

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

- Failed scale candidate: `caf226e1318f59ba7716f1d6f11cf07c8d5bd27f`; validated parent comparator: `6dd2fe7e84ffd68fdb69d61088861c4c79ca7659`. Linux workflow `29621525512` and Android workflow `29621525499` succeeded, and the exact artifacts passed SHA/run/target/HEV-pin verification. Android real-device installation was intentionally stopped after the authoritative Linux throughput gate failed.
- Workstreams in the failed snapshot: retain the output permit granted after backpressure so TCP/ICMP makes progress against the shared UDP sender; reduce the two smoltcp socket buffers and two Leaf Stream rings from `0x3fff * 20` bytes to 32 KiB per direction.
- Locked Leaf boundary: `Cargo.lock` pins `lovitus/leaf` `4af133266367bc6ef1d369b4b519a0a56da48760`; `leaf/src/proxy/tun/inbound.rs::new_smoltcp` uses this vendored stack and retains the existing detached-runner lifecycle.
- Mihomo reference: source HEAD `0a87b94845ef908c15f8495871e4cd8e33116328`, `listener/sing_tun/server.go::{New,Listener.Close}` owns and closes the selected stack. Its pinned `github.com/metacubex/sing-tun` v0.4.17 source commit `00e7bcea347af9d3b274491f77921827a678b50e`, `stack_gvisor.go::NewGVisorStackWithOptions`, fixes both gVisor TCP receive and send buffer defaults/maxima at 20 KiB and enables moderate receive buffering; `stack_gvisor_tcp.go::NewTCPForwarderWithLoopback` caps pending forwarded endpoints at 1024. Observable compatibility target is bounded per-connection memory with ordinary TCP backpressure, not identical gVisor allocation or autotuning internals.
- Rejected semantic/performance boundary: fixed 32 KiB smoltcp windows are not compatible with the current EasyTier/Leaf scheduling pipeline. In one immediate same-namespace A/B, the parent DIRECT three-run median was `871.9 Mbit/s`; the candidate median was `402.6 Mbit/s`, a `53.8%` regression with zero retransmits in both. The sender congestion/window evidence fell from `335280` to `43560` bytes. This disproves the assumption that local TUN RTT alone makes 32 KiB performance-neutral.
- Scale evidence: both snapshots established all 1/100/1000 connections. At 1000 idle connections the parent Leaf used `1,310,748 KiB VmSize / 32,376 KiB RSS`; the 32 KiB candidate used `176,968 KiB VmSize / 50,568 KiB RSS`. The candidate reduced virtual reservation by `86.5%` but increased idle RSS by about `18 MiB`, because the old large allocations remain mostly lazily backed while the smaller allocator class eagerly touches more pages. Both returned to 11 Leaf FDs after the close storm; p99 connect was `13.1 ms` parent and `12.5 ms` candidate; 30-second idle Leaf CPU was about `0.20%` and `0.23%` of one core respectively.
- Concentrated-poll evidence: with 1000 established idle connections, the parent sustained about `444.1 Mbit/s` TCP while a simultaneous `20 Mbit/s` UDP stream completed with zero loss, versus an unloaded parent median of `871.9 Mbit/s`. This confirms the documented O(n) active-socket scan remains a separate high-connection CPU/throughput boundary.
- `.160` dispatch gate: passed twice after the complete implementation and again after the final flow-control test, using `scripts/leaf-remote-preflight.sh`. The exact EasyTier, policy, and vendored netstack library binaries passed the locked no-run build and all configured focused tests, including `capacity_wait_makes_progress_with_queued_output_sender`, `unused_reserved_output_capacity_is_released_after_poll`, `default_tcp_buffers_are_32_kib_per_layer`, `default_stream_send_buffer_backpressures_and_wakes_at_32_kib`, shutdown, output-close, and cooperative-runtime coverage.
- Revert/replacement manifest: commit a literal revert of `caf226e1`, then retain only `VirtualDevice::{wait_output_capacity,receive,transmit,release_unused_output_permit}`, `VirtualTxToken::consume`, the runner post-poll permit release, and their two deterministic fairness/release tests. Restore all four existing buffer sizes exactly. The next memory candidate must preserve the measured smoltcp receive window and target only the duplicate Leaf Stream staging layer with demand allocation, per-connection bounds, a global byte budget, and bounded creation channels.
- Validation boundary: do not claim that fixed 32 KiB buffers reduce real idle RSS or preserve throughput. The replacement fairness-only snapshot requires the complete `.160` gate before one push; the dynamic-queue candidate is separate and must include budget reclamation, Pending/waker, close/drop, half-close, bounded-channel, 1/100/1000 active/idle RSS, and same-window throughput evidence before acceptance.

## 2026-07-18 rejected Stream-staging memory candidates

- Canonical known-bug and failed-implementation record: [`netstack_tcp_buffer_scaling_failed_implementations.md`](../known_bugs/netstack_tcp_buffer_scaling_failed_implementations.md). This workstream is closed as an implementation failure; do not revive a rejected buffer candidate from the shorter workboard notes alone.

- Validated comparator: fairness/event-driven `8201a4a8270a173949e8fa0cf994ac7328aa46b2`, Linux run `29622735384`, Android run `29622735405`. Same-window unloaded median was `868.0 Mbit/s`; 1000 idle Leaf memory was `1,310,816 KiB VmSize / 32,300 KiB RSS`, p99 connect `13.86 ms`, and close returned to 11 FDs.
- Rejected chunk candidate `0c8894e280528f382d6689131990fef9e22466e6`: Linux `29623951590`, Android `29623951560`. Demand `VecDeque<Bytes>` queues reduced 1000-idle Leaf memory to `673,940 KiB VmSize / 26,080 KiB RSS`, but unloaded median fell to `518.3 Mbit/s` (`-40.3%`). Literal revert `e26c9815`.
- Rejected restored-capacity chunk candidate `6c92be286802a5f10ebaa65318e5e830218832f4`: Linux `29624958371`, Android `29624958377`. Restoring the old `0x3fff * 20` limit still produced only `536.9 Mbit/s` (`-38.1%`), isolating allocation/free and permit hot-path cost rather than queue capacity. Literal revert `728a8679`.
- Rejected lazy-ring candidate `271351f03a7ee9532ec7d8ab44f248ed6c5d69b3`: Linux `29626363000`, Android `29626363004`. `.160` locked no-run and all focused tests passed. Unloaded receive was `928.9 / 932.7 / 929.6 Mbit/s`, zero retransmits. At 1000 idle it improved Leaf to `670,864 KiB VmSize / 24,252 KiB RSS`, p99 `11.78 ms`, returned to 11 FDs, and used about `0.23%` of one core while idle. Same-window 1000-idle TCP/UDP A/B did not regress (`256.4/19.74 Mbit/s` candidate versus `247.5/19.76` comparator; zero TCP retransmits and zero UDP loss).
- Rejection reason: the required 128-parallel-flow budget-saturation test exposed a send-side liveness failure. Candidate reverse/receive completed 128 streams in `8.00s` at `344.4 Mbit/s`; forward/send did not complete and was interrupted at `39.69s`. The exact comparator completed the same 128-stream forward test in `8.00s` at `634.0 Mbit/s`. Evidence is checksummed under `/private/tmp/netstack-271351f0-validation`. This disproves the block-budget design despite its normal-path throughput and memory wins. Literal revert `062628ad`.
- Do not revive a finite whole-ring send budget without proving progress when active stream count exceeds the block count. A replacement must not make connection completion dependent on another connection releasing its entire staging ring; the 128-stream forward and reverse saturation cases are mandatory pre-acceptance gates.

- Rejected progress-safe adaptive candidate `5e6d455b513a2aba5e06db2737944dda2a8b7a0d`: Linux run `29628076254`, Android run `29628076268`. The design gave every active direction an unconditional 32 KiB base ring and budgeted only full 320 KiB expansions, eliminating the whole-ring progress dependency. `.160` locked no-run and all 18 netstack tests passed. Exact 128-parallel forward and reverse tests both completed in `8.00s` at `624.4` and `579.9 Mbit/s`, so the prior saturation deadlock was fixed.
- Rejection reason: reducing each smoltcp RX/TX window to `128 KiB` produced unloaded reverse `624.4 / 659.3 / 658.5 Mbit/s` (median `658.5`), while the exact same-window `8201a4a8` comparator produced `893.2 / 918.0 / 923.4 Mbit/s` (median `918.0`). The `28.3%` relative loss fails the `824.6 Mbit/s` gate and proves 128 KiB is still below the measured local TUN/netstack BDP. Evidence is checksummed under `/private/tmp/netstack-5e6d455b-validation`. Literal revert: `8d4dc0f8`.
- Fixed-window tuning is closed as a candidate direction. A local, never-pushed 256 KiB experiment (`78278eb3`) was withdrawn by literal revert (`225b0a22`) before workflow dispatch: a universal size cannot preserve high-BDP behavior, and continuing with 512 KiB/1 MiB trials would only move the same unsupported threshold. At 10 Gbit/s the receive-window BDP is approximately 1.25 MiB at 1 ms RTT, 12.5 MiB at 10 ms, and 62.5 MiB at 50 ms.
- Locked smoltcp `0a926767a68bc88d5512afefa7529c5ecdade4ea`, `src/socket/tcp.rs::{SocketBuffer,Socket::new}`, stores RX/TX in fixed `RingBuffer`s and chooses the receive window scale from the initial capacity during construction. It exposes capacity queries but no established-socket storage replacement or safe growth API. Dynamically allocating only the Leaf Stream staging layer therefore cannot remove the smoltcp window ceiling; true autotuning would require a separately designed smoltcp segmented/resizable buffer plus SYN-time scale negotiation, advertised-window, reassembly, close, and pressure tests.
- The measured 1000-idle comparator used `1,310,816 KiB VmSize` but only `32,300 KiB RSS`; the large zero-filled socket allocations are predominantly lazily backed. The 32 KiB candidate reduced virtual reservation while increasing RSS by about 18 MiB. Therefore virtual size alone is not evidence of an actionable memory fault, and no buffer rewrite is justified for the runner P0.
- Accepted scope remains the event-driven/fairness snapshot `8201a4a8270a173949e8fa0cf994ac7328aa46b2`, which changes channel lifecycle, backpressure, and cooperative scheduling while preserving all existing TCP buffer/window semantics. High-BDP TCP autotuning and the O(n) active-socket scan are independent future projects and must not be bundled into this lifecycle fix.
- Final safe remote snapshot `6c377afee8303d10f0f5e37f6ab165c97838f156` has byte-identical `device.rs` and `tcp.rs` content to `8201a4a8`. Its Linux profiling workflow `29628775508` and Android candidate workflow `29628775467` both completed successfully. The local 256 KiB experiment and its revert have zero net source difference and were never pushed; no additional buffer candidate or real-device installation is authorized by this workstream.

## 2026-07-18 Shadowsocks/UoT v2 candidate manifest

- Shared EasyTier candidate SHA: `1eb6f191cb049b56afd8c399adf0c37c92ecfa86`; `.160`
  dispatch gate passed and only one batched push is permitted.
- Locked Leaf base: `4af133266367bc6ef1d369b4b519a0a56da48760` from the pre-change
  `Cargo.lock`; pinned UoT candidate: `742ad65c441f9d60279916b82628b810efbd48fb`.
- Scope: `ProxyKind::Shadowsocks`, bool/string UDP compatibility, strict cipher/password
  validation, minimal JSON compiler module, Leaf Shadowsocks native UDP plus UoT v2
  datagram mode, existing mesh-chain/fallback composition, documentation and report.
- Explicitly unchanged: EasyTier mesh/HEV/DNS/rules/group/fallback implementations and
  Leaf detached runtime lifecycle.
- `.160` no-run: `scripts/leaf-remote-preflight.sh`, after the Leaf fork commit is pinned
  and generated protobuf plus `Cargo.lock` are present.
- Focused policy tests: UDP mode compatibility; SS validation; SOCKS UoT rejection;
  native/mesh-chain/fallback Leaf actor compilation.
- Focused Leaf tests: UoT v2 domain/IPv4/IPv6 byte vectors; lazy request; packet boundary;
  short-buffer alignment; JSON/protobuf mapping; existing Shadowsocks/chain tests.
- Current evidence: `.160` standard `scripts/leaf-remote-preflight.sh` exact-pin `--locked`
  no-run passed twice, including the final API/compatibility correction; the final incremental
  compile took 31.69 seconds and every focused test passed. The four new policy filters each
  ran one test. Leaf UoT test binary ran five tests and passed after a documented
  test-harness-only workaround for the pre-existing Tokio-macros and GeoSite lite-protobuf
  failures.
- GitHub workflows: one automatic Linux profiling beta and Android candidate pair for the
  exact EasyTier SHA; query existing runs before any manual dispatch.
- Linux evidence: Gust `v3.2.9-porty8` SS TCP, SS UDP, sings/UoT, native/mesh chain,
  fallback/fail-closed, stop/start, FD/task/RSS baseline, throughput/CPU/loss. Primary
  performance matrix uses the two 10Gbps dual-stack public validation nodes with separate
  IPv4, IPv6 and dual-stack runs; `10.20.0.65` is not a mainland-IPv4 client baseline.
- Android evidence: exact upgraded artifact with preserved data, captured-UID policy probe,
  TCP/UDP/UoT, native/mesh chain, lifecycle and bounded traffic/resource evidence.
- Build-wait work: prepare Gust configs, isolated ports, host cleanup, checksums, semantic
  Android commands and result collectors; do not mutate the in-flight snapshot.
- Prepared interoperability environment: both 10Gbps dual-stack nodes have checksum-verified
  Gust `v3.2.9-porty8` (`gost` SHA-256 `46ebef5815c6918f1c6e6102cc22a1af5398e92eee4070c05bddd62825c21647`).
  The server node uses isolated `28388/TCP+UDP` for `ss+ssu` and `28389/TCP` for `sings`;
  existing services and firewall state are unchanged.

## 2026-07-18 Shadowsocks/UoT v2 候选收口

共享候选 SHA：`1eb6f191cb049b56afd8c399adf0c37c92ecfa86`

| 工作流 | 目标 | 构建影响 | 证据目标 | 状态 |
|---|---|---:|---|---|
| Shadowsocks actor | 最小插件化接入四个受控 cipher 与严格字段校验 | 是 | .160 focused tests、Linux/Android 精确 artifact | 完成 |
| UoT v2 | 支持 `off/native/uot-v2`，保留 bool 兼容 | 是 | sing-box TCP/UDP/UoT interop、IPv4/IPv6 目标 | 完成 |
| chain | 复用 mesh SOCKS actor 后接 peer-local SS，不增加 `via: mesh` SS 语义 | 否 | mesh -> SS -> UoT，IPv4/IPv6 underlay | 完成 |
| fallback | 首成员不可用时切换到可用 UoT actor | 否 | dead-first HTTPS/TCP/UDP | 完成 |
| 性能 | 比较 raw、native、UoT、chain 与 v4/v6 | 否 | 10 Gbps 双栈 lv1g2/lv1g3 | 完成 |
| 生命周期 | 资源回基线与空闲 CPU | 否 | 5 次 stop/start、60 秒空闲 | 完成 |
| Android UoT 实包 | captured UID 产生确定 UDP/UoT | 否 | sing-box 服务端 UoT 日志 | 暂缓：设备按维护者要求撤离；不得用 TCP/Chrome 缓存冒充 |
| 环境清理 | 清除 namespace、规则、进程和专用端口 | 否 | 所有 remaining 计数为 0 | 完成 |

最终判断：实现、Linux 双栈功能、UoT interop、chain、fallback、性能和资源证据已闭环；Android 构建与标准 SS TLS 已闭环，Android UoT 实包按新的设备边界明确留空。文档更新不单独触发 workflow。

## 2026-07-18 Trojan/VMess/VLESS 插件候选

共享候选 SHA：`bfbe4de5129298b1c15ea3a7e1132e376bfcc811` 与 `a36343304a34f1510a63a0d66002012ed0ec6fa2` 均被真实 VLESS 互操作否决；Leaf 无-flow修复 `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb` 已通过 `.160` 独立编译和 3/3 精确测试，EasyTier 替换候选的 `--locked` no-run 与完整默认 focused suite 也已通过，待提交和 artifact。

| 工作流 | 目标 | 构建影响 | 证据目标 | 状态 |
|---|---|---:|---|---|
| 协议 schema | 严格接收 Trojan、VMess AEAD、VLESS 及 TLS/WS 字段 | 是 | `.160` config focused tests、未知字段 fail-closed | `.160` 通过 |
| Leaf 编译 | 私有 TLS/WS/protocol actors 封装为稳定公开 tag | 是 | `.160` compiler test、精确 JSON actor 顺序 | `.160` 通过 |
| mesh 组合 | 复用现有 mesh SOCKS actor 作为 chain 第一跳 | 否 | 三协议 direct 与 mesh-prefixed chain | direct 已开始；mesh 待修复 artifact |
| 前端 | YAML 往返、可视化协议/UUID/cipher/TLS/WS Host | 是 | `.160` Vitest 与 production build | 29/29 + build 通过 |
| Linux 功能 | TCP、UDP、DNS、fallback、stop/start、资源回基线 | 否 | `.37/.38` 与受控公网服务端 | `bfbe4de5` Trojan/VMess direct 通过；VLESS WSS 根因为 Leaf 强制 Vision，无-flow修复待 artifact |
| 双 VPS 性能 | 同条件 sing-box 对照，分别 direct/mesh、IPv4/IPv6/双栈 | 否 | `lv1g2/lv1g3` 三次中位数、CPU/RSS | sing-box IPv4 基线已采集；候选对照待修复 artifact |
| Android | 本批不使用已撤离设备，不伪造实机证据 | 否 | workflow 构建证据；实包待设备恢复 | 受设备边界阻塞 |

候选清单：后端四个新 Leaf feature、三个窄协议编译器、一个 crate-private TLS/WS 编译层、严格字段校验、UDP capability、前端编解码与编辑器、默认注释示例、聚焦测试和文档。`.160` 使用 `scripts/leaf-remote-preflight.sh` 完成一次 `--locked` no-run 与全部默认 focused tests；前端在同一 builder 使用 Node 22 跑两个 policy Vitest 文件和 production build。只有两条预检都通过才提交并推送一次候选；自动 Linux/Android workflow 构建同一 SHA。构建等待期间准备临时节点配置、sing-box 对照配置、隔离端口、清理命令和结果采集器，不修改在途快照。

明确边界：锁定 Leaf 不支持 Shadowsocks 2022；真实凭据只进入远端临时文件。Trojan fingerprint/smux/Brutal、VLESS flow/Reality/XUDP/XHTTP、WebSocket early-data 不进入本候选，必须根据互操作证据另行决定，不能静默接受。

`bfbe4de5` 的 Linux/Android workflow `29646685998/29646686016` 均成功，Linux artifact 的外层与包内 SHA256、`BUILD_INFO`、musl target 和 Build ID 已核对。双 VPS 真实节点证明 Trojan direct、VMess WS direct 可用且来源地址来自远端代理；VLESS WSS 在 EasyTier 中超时/空响应，而 sing-box 在保留及移除 early-data 两种配置下均成功，排除了 early-data 边界。`a3634330` 增加 `http/1.1` ALPN 后生成配置正确但仍失败，进一步定位到锁定 Leaf `742ad65c` 无条件发送 `xtls-rprx-vision`，与 Mihomo 仅在显式 flow 时启用 Vision 的语义不符。fork `36ba707f` 改为标准无-flow请求/响应，`.160` 独立 integration test 3/3 通过；主仓库锁 pin 后的标准 preflight 也通过，artifact 与真实节点矩阵尚待完成。

替换候选清单：仅包含 Leaf pin `36ba707f`、既有 WSS ALPN 修复和验证文档；不新增公共配置或数据面逻辑。`.160` 已完成主仓库 `--locked` no-run 与完整默认 focused suite；随后只推送一次候选，由自动 Linux/Android workflow 构建同一 SHA。等待期间准备 lv1g2/lv1g3 共享 `/slab2` 的单次 `ncat + tar` 接收、`udp:true` 临时配置、direct/mesh/forced-v4/forced-v6/fallback/stop-start/资源与三次中位数矩阵，不修改在途快照。

## 2026-07-18 代理端点 FakeDNS 自举回环修复候选

- 状态：实现完成，`.160` 标准 `--locked` no-run 与完整 focused suite 已通过；构建影响；共享候选 SHA 待提交后记录。
- 被否决 artifact：`de3e03887917dea4765dc83bb5f21db6b266df19`，Linux/Android workflow `29649710067/29649710096` 均成功且 Linux artifact 校验完整，但域名形式的 VLESS WSS 真实流量失败。
- 交叉验证：同主机、同节点、同目标、相邻时间的 sing-box 完成 64 MiB，约 512 Mbit/s；EasyTier 12 秒零字节。临时 sing-box 服务端上的 plain VLESS、VLESS+WS、VLESS+WSS 均由同一 EasyTier artifact 完成 64 MiB，约 275-295 Mbit/s。CDN 强制 IPv4、强制 IPv6 配置也都通过，约 272/291 Mbit/s。
- 精确根因：域名配置下 Leaf worker 实际向 FakeIP `198.19.0.4` 和 `fd65:6173:7974::4` 发起大量并发 TCP SYN；默认 `dns.direct: [system, ...]` 被编译为 `direct:system`，而 TUN 接管后的系统解析入口已是 Leaf FakeDNS，导致代理端点 bootstrap 查询回到 FakeDNS。
- Mihomo 参照：`hub/executor/executor.go::updateDNS` 独立设置 `resolver.ProxyServerHostResolver`，`component/dialer/dialer.go::parseAddr` 用它解析代理服务器地址，不经过 FakeIP service。sing-box `common/dialer/dialer.go::NewWithOptions` 同样为 domain server address 构建 resolve dialer。
- 修复边界：只在 `easytier-policy/src/leaf_config.rs::compile_dns_servers` 把 `system` 展开成宿主在 TUN 接管前传入的底层 DNS IP，去重并输出 `direct:<IP>`。不修改 Leaf、VLESS、TLS/WS、mesh、HEV、路由、FakeDNS 或代理组逻辑；无平台 DNS 时不退回 `direct:system`，保持 fail-closed。
- `.160` 目标：标准 `scripts/leaf-remote-preflight.sh` 一次 `--locked` no-run 和完整 focused suite；新增精确测试 `expands_system_dns_to_captured_platform_servers_for_proxy_bootstrap`。
- Artifact 目标：一次自动 Linux/Android workflow 集；Linux 精确 artifact 重跑域名/强制 v4/v6 的 Trojan、VMess、VLESS direct 与 mesh、UDP、fallback、stop/start、资源及性能矩阵。每个失败均以同主机、同节点、同目标的 sing-box 相邻窗口交叉验证。
- 首次候选 `1059c21d88d06d10c9c965750269484dbc7dcbcf`：Linux workflow `29651804523` 的完整 policy suite 为 87/88，通过的新行为与旧快照断言冲突；唯一失败仍期待 `direct:system`，实际为捕获的 `direct:1.1.1.1`。运行时代码、编译和新增精确测试均通过。修正候选只更新该断言并把稳定配置快照测试加入 `.160` 默认过滤器；Android `29651804525` 独立保留其实际结果，不用它替代 Linux artifact。

## 2026-07-19 owned-TUN/GSO final disposition

| Workstream | Objective | Build-affecting | Evidence target | Status | Shared candidate |
|---|---|---:|---|---|---|
| Linux owned-TUN hard gate | Compare confirmed GSO fast path with same-artifact legacy on `.160` | no | Three untraced runs per mode, dual-TUN ownership, offload flags, RSS and cleanup | **FAILED: download ratio 0.8424 < 0.95; candidate reverted** | `8b48153acc286c70c70faf8a2e4d1cb3c015be05` |
| Revert preflight | Return runtime/config/UI/proto surface to the pre-experiment boundary | yes | `.160` `--locked` no-run plus focused suite on complete revert snapshot | **PASSED: 4m11s no-run and all configured focused tests** | four auditable revert commits |
| Revert artifacts | Publish one auditable rolling-beta revert candidate | yes | Exact-SHA Linux/Android workflows only after `.160` gate | **READY: one automatic workflow set after the audit-doc commit** | four auditable revert commits plus audit docs |

Wait-time work for the revert candidate is limited to diff/lockfile/cfg/workflow-pin inspection and host cleanup. No third performance architecture, newer-kernel benchmark, or Android performance run is authorized by this failed candidate.

## 2026-07-19 cross-kernel correction after the safe revert

| Workstream | Objective | Build-affecting | Evidence target | Status | Candidate |
|---|---|---:|---|---|---|
| `.160` CentOS 3.10 A/B | Detect regressions on the oldest supported validation kernel | no | Three untraced runs per mode | **FAILED for fast path: download ratio 0.8424** | `8b48153a` |
| lv1g2 Debian 4.19 A/B | Repeat on independent 10 Gbps VPS and newer kernel | no | Three untraced runs, GSO flags, dual-TUN counters, cleanup | **PASSED: download +53.2%, upload +35.8%, RSS +2.3%** | `8b48153a` |
| lv1g3 Ubuntu 5.4 A/B | Confirm newer-kernel result on a second VPS | no | Three untraced runs, TUN flags, dual-TUN counters, cleanup | **PASSED: download +87.9%, upload +76.3%, RSS +1.3%** | `8b48153a` |
| Safe revert | Keep product branch free of the disputed candidate while evidence is reviewed | yes | `.160` preflight and exact-SHA Linux/Android workflows | **PASSED at `c5051e1f`; temporary state** | `c5051e1f` |
| Cross-kernel decision | Decide whether to restore the candidate with old-kernel legacy fallback | yes | User decision, then one batched candidate/preflight/workflow set | **USER UNDECIDED** | none |

The cross-host results supersede the earlier full-rejection conclusion. Documentation remains local until it accompanies a build-affecting decision; it must not trigger a documentation-only workflow.

## 2026-07-19 expanded `.37/.38/KR` matrix

| Workstream | Objective | Build-affecting | Evidence target | Status | Candidate |
|---|---|---:|---|---|---|
| `.37` exact-artifact A/B | Add an independent CentOS 7 / 3.10 result without touching `etns_scale` | no | Three untraced runs per mode, GSO, dual TUN, RSS, cleanup | **PASSED: download +34.3%, upload +81.3%, RSS -4.8%** | `8b48153a` |
| `.38` exact-artifact A/B | Add a second independent CentOS 7 / 3.10 result | no | Three untraced runs per mode, GSO, TUN accounting, RSS, cleanup | **PASSED: download +38.0%, upload +30.6%, RSS -4.8%** | `8b48153a` |
| KR exact-artifact A/B | Validate Debian 5.10 while preserving production EasyTier/TUN/iperf | no | Three untraced runs per mode, GSO, dual TUN, RSS, cleanup | **PASSED: download +80.7%, upload +39.0%, RSS +11.2%** | `8b48153a` |
| KR false-positive audit | Explain the excluded first run without hiding a product failure | no | Preserve RA `expires` diff and prove production/candidate ownership | **CLOSED: only volatile RA expiry changed; fresh matrix passed** | `8b48153a` |
| Six-host decision | Separate a host-specific negative result from platform/kernel policy | yes | Five passes, one `.160` failure, no unsupported kernel heuristic | **USER UNDECIDED: recommend restoring explicit opt-in feature without kernel gate** | safe revert remains `c5051e1f` |

All three new hosts verified the exact archive and package metadata before execution. Accepted runs were host-state clean and candidate-owned resources returned to baseline. The raw per-run values, comparator medians, watcher evidence, exclusions, remote evidence roots, and cleanup state are recorded in `UNDECIDED_leaf_linux_owned_policy_tun_cross_kernel.md`. No documentation-only workflow is authorized.

## 2026-07-19 user-approved restoration

| Workstream | Objective | Build-affecting | Evidence target | Status | Candidate |
|---|---|---:|---|---|---|
| Exact implementation restore | Restore only the validated `8b48153a` non-document tree | yes | Tree equivalence for runtime/config/UI/proto/scripts; no new heuristics | **COMPLETE: tracked paths and both new script blobs exactly match `8b48153a`** | local restoration snapshot |
| Corrected evidence | Preserve all six hosts and replace the premature full-rejection conclusion | no | Cross-kernel validation document and workboard | **COMPLETE** | local restoration snapshot |
| Restoration preflight | Apply the standard `.160` dispatch lock once to the complete batch | yes | `--locked` no-run plus configured focused suite | **PASSED: 33.41s no-run and all configured focused tests** | local restoration snapshot |
| Restoration artifacts | Produce one exact-SHA Linux/Android workflow set | yes | Automatic profiling-beta workflows after preflight | **READY: one push after this documentation update** | local restoration snapshot |

During the `.160` wait, only tree-equivalence, lockfile, platform `cfg`, generated proto, workflow-pin, and candidate-scope inspection are allowed. Do not mutate the in-flight snapshot or start another build.

Candidate manifest: the build-affecting tree is byte-equivalent to `8b48153acc286c70c70faf8a2e4d1cb3c015be05`, including Leaf revision `a5bb6a31df2c62200be052b61ca01b01ea5e3c25`. The candidate adds only the corrected six-host evidence and restoration decision. Required workflows are the single automatic Linux profiling-beta and Android policy candidate runs. Existing exact-artifact Linux evidence is `.160`, `.37`, `.38`, lv1g2, lv1g3, and KR; Android is a non-Linux build/regression boundary because the restored fast path is Linux-only.

## 2026-07-19 generic dual-TUN `87301ee0` cross-host audit

This non-build-affecting lane ran while the restored `f6617c51` workflows were building. It did not mutate the in-flight candidate and must not trigger another workflow.

| Workstream | Objective | Build-affecting | Evidence target | Status | Candidate |
|---|---|---:|---|---|---|
| Exact historical artifact | Prove that every host uses the same pre-GSO dual-TUN build | no | Workflow metadata, archive/package SHA-256, build ID | **PASSED: run `29682690040`, archive `32127dfe...`, build ID `efd4e765...`** | `87301ee0` |
| Five-host generic A/B | Determine whether the `.160` regression generalized across machines | no | Three interleaved legacy/generic runs on `.37`, `.38`, lv1g2, lv1g3, and KR | **PASSED on all five: download +29.5% to +76.8%; upload +19.5% to +58.2%** | `87301ee0` |
| Generic path identity | Exclude accidental use of the later GSO implementation | no | Netns-internal `tun_flags` on all five hosts | **PASSED: all report `0x1001`** | `87301ee0` |
| Resource and cleanup | Detect idle storms, RSS growth, leaked process/netns/TUN, or production interference | no | 20% idle-CPU abort, RSS medians, host snapshots, final ownership audit | **PASSED: RSS -3.3% to -12.0%; no candidate residue; protected state retained** | `87301ee0` |
| Historical decision correction | Separate a real host-specific negative from a design-wide failure | no | Six-host aggregate including the original `.160` evidence | **CLOSED: `.160` remains negative; prior general rejection was false** | `87301ee0` |
| Restored candidate artifacts | Build the already approved GSO/generic/legacy hierarchy | yes | Exact-SHA automatic Linux and Android workflows | **IN PROGRESS: Linux `29688959030`, Android `29688959035`** | `f6617c51` |

The old generic candidate is not a new restoration target. Its audit validates the middle fallback already present in `f6617c51`: `fast-GSO -> fast-generic -> legacy`. Raw runs, comparator medians, observer exclusions, archive hashes, and cleanup evidence are recorded in `leaf_linux_owned_policy_tun_cross_kernel_validation.md`. Documentation remains local until it accompanies a later code/release snapshot or the maintainer explicitly requests a documentation push.

## 2026-07-19 PacketBatch cross-host rejection audit

| Workstream | Objective | Build-affecting | Evidence target | Status | Candidate |
|---|---|---:|---|---|---|
| Historical artifact recovery | Reuse the exact continuous-body PacketBatch artifact without rebuilding | no | Workflow metadata, ZIP/bundle/package SHA-256 and `BUILD_INFO` | **PASSED: run `29677226981`, exact `39dd4d2f`** | `39dd4d2f` |
| Five-host A/B | Test whether `.160` alone caused the PacketBatch rejection | no | Three interleaved legacy/batch runs on `.37`, `.38`, lv1g2, lv1g3, KR | **FAILED on all five download gates: ratios `0.946/0.922/0.714/0.693/0.829`** | `39dd4d2f` |
| Interference audit | Separate host noise, sampler failure and product behavior | no | Stable adjacent legacy runs, isolated retry for KR sampler `SIGPIPE`, host cleanup | **CLOSED: KR probe retried cleanly; cross-host regression remains** | `39dd4d2f` |
| Resource safety | Detect idle storm, leaked process/netns/TUN, or production interference | no | Idle CPU <=20%, TUN bounds, shutdown, final host audit | **PASSED: no candidate residue; protected state retained** | `39dd4d2f` |
| Historical conclusion | Decide whether the PacketBatch rollback was another dual-TUN false rejection | no | Original `.160` plus five independent hosts and Android panic | **CONFIRMED FAILURE: rollback remains correct; original single-host evidence was incomplete but not directionally false** | `39dd4d2f` |

The detailed medians, archive hashes, lv1g3 variance, KR excluded sampler attempt, cleanup evidence, and corrected scope are appended to `FAILED_leaf_external_packet_endpoint_performance.md`. This audit is documentation-only and must not trigger a workflow.

## 2026-07-19 policy editor accuracy and compactness lane

- Description: reconcile visual policy menus with the validated EasyTier policy schema and current locked Leaf outbound support.
- Objective: remove the invalid HTTP outbound, show protocol-correct UDP controls, expose FakeIP ranges, add examples to free-form fields, and keep node/group/rule lists compact until explicitly edited.
- Build-affecting status: local implementation and focused tests added; not yet preflighted or pushed.
- Evidence target: `.160` focused `policy-editor.spec.ts`, `policy-document.spec.ts`, policy parser tests covering defaults/HTTP rejection, and one frontend production build from the complete snapshot.
- Current status: implementation complete locally; validation intentionally not started while the candidate remains unfrozen.
- Shared candidate SHA: none; the last immutable runtime candidate remains `f6617c5136672016951adb0f79ab0daec7ba7112`.
- Follow-up correction: do not compile the temporary bundled GeoSite/GeoIP category list into the frontend. The core now generates a versioned sidecar from the active rule-data file, keys it by file size and the configured/index SHA-256 identity, refreshes it after managed updates, and exposes it through the management API. The frontend reloads only when the instance/path/SHA identity changes and keeps searchable editable selectors with virtual scrolling; a missing index degrades only the suggestion list, not typed rules or policy startup.
