# Leaf Parallel Candidate Workboard

This is the live execution board for batching independent Leaf/policy work into one exact candidate.
It is local execution state, not a reason to trigger a workflow by itself.

## Current candidate

- SHA: `c92b11b10d1750cecd1f30a98471217d20fd8154`
- Branch: `codex/profiling-beta`
- `.160` preflight: passed; desktop Leaf test binary compiled and three exact tests passed.
- Automatic Linux run: `29497882709` - passed.
- Automatic Android run: `29497883758` - passed; persisted data retained and exact probe passed.
- Same-SHA no-Leaf comparator run: `29497919676` - queued/in progress.
- Comparator rationale: active disabled-mode CPU/throughput/latency/RSS/FD/thread gate.
- Known inefficiency: the current comparator workflow rebuilds the feature-on bundle.
- Local follow-up prepared in `.github/workflows/profiling-beta.yml`: `audit_comparator=true` now selects a comparator-only job with an independent concurrency group; it builds and uploads only `easytier-core-no-leaf`, does not rebuild HEV/Leaf/the feature-on bundle, and does not mutate the rolling prerelease.
- Next batched candidate: uncommitted transport-boundary correction plus this workboard, scheduling memory, and comparator-only job.
- Final `.160` preflight: default Leaf no-run, no-Leaf no-run, KCP-only check, and QUIC-only check passed. `direct_mesh_stream_prefers_quic_then_kcp_before_native_fallback`, `prepares_then_relays_built_in_tcp_from_mesh_data_plane`, `mesh_only_connect_never_falls_back_to_kernel`, `data_plane_tcp_pingpong`, `socks_egress_guard_shutdown_waits_for_owned_task`, and `route_identity_change_cancels_only_the_old_generation` each ran as one real passing test; the no-Leaf SOCKS count test also passed.
- Reference semantics: Mihomo `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go`, `Dialer.DialContext`, `retryStartSocks5TCP`, and `retryStartSocks5UDP` keep mesh exposure and dialing inside the mesh runtime. EasyTier intentionally follows the ownership boundary rather than copying Tailscale transport internals: policy chooses an actor and prepares the managed endpoint; the existing mesh `DeferredProxySelector` alone honors `enable_quic_proxy`, `enable_kcp_proxy`, destination capabilities, readiness ACKs, health, and smoltcp fallback.
- Root cause from exact `c92b11b1` Linux artifact: portless KCP input reached `10.255.0.2:11080`, then the destination KCP proxy attempted a host-kernel connect and returned `ECONNREFUSED`; the previous unconditional HEV listener had hidden the mismatch.
- Review correction: forcing portless/UoT onto smoltcp was rejected before commit because exact evidence is approximately `941 Mbit/s` DIRECT, `478 Mbit/s` KCP, and `53.5 Mbit/s` smoltcp fallback. The replacement removes policy ownership without removing mesh acceleration or ignoring either user proxy flag.
- Port-candidate boundary: the prepare RPC returns the selected HEV port on the target virtual IP, and the userspace ingress registers all three `11080/11081/11082` candidates. An occupied `11080` therefore cannot make QUIC/KCP connect to the unrelated owner while smoltcp targets a different service.

## Parallel workstreams

| Workstream | Description | Objective/evidence target | Build-affecting | Status |
| --- | --- | --- | --- | --- |
| Managed HEV lazy lifecycle | Replace unconditional HEV residency with an endpoint provider that starts on the first built-in TCP/UDP actor request. | No HEV PID without policy traffic; one HEV after first portless request; explicit-only does not start HEV; normal shutdown removes it. | Yes | Disabled and explicit-only gates passed; portless exposed a KCP/userspace-listener transport mismatch |
| Policy data-plane transport boundary | Replace the policy-only KCP endpoint with the existing mesh-owned QUIC/KCP selector and a narrow remote HEV prepare RPC. | User flags and destination capabilities select QUIC/KCP; failures fall back before payload; portless alone prepares HEV and propagates the selected fallback port; explicit and portless share one mesh stream API. | Yes | `.160` four feature builds and six real tests passed, including selected-port propagation; exact workflow artifact and host path/performance validation pending |
| Disabled-mode comparator | Compare feature-on/no-policy with same-SHA no-Leaf on one fixed TCP underlay. | CPU, throughput, RTT, RSS, FD, and threads within noise; no sidecar/process overhead. | Validation only | Waiting for comparator artifact |
| Android upgrade/persistence | Upgrade the stable-signed candidate without uninstalling either EasyTier package. | Pre/post persisted-store hashes match before first start; existing instance selection/config preserved. | Validation only | Passed on `c92b11b1`: 12-file manifest and 26,624-byte tar unchanged before first start |
| Android network generation | Repeat semantic policy probes and Wi-Fi disable/enable recovery. | Same PID/TUN ownership, new network generation/DNS, captured-UID TLS recovery, no FD/task growth. | Validation only | `8b337502` passed; narrow `c92b11b1` regression pending |
| Actor path parity | Compare portless and explicit actors at one peer/target. | Same mesh selector, user flags, capability order, and fallback; 20/20 TCP; UDP echo; only portless prepares managed HEV. | Validation only | Unit selector/prepare/generation gates passed; exact artifact host matrix pending |
| Fallback + UDP lifecycle | Stack primary loss, secondary takeover, underlay outage, and concurrent UDP associations. | Fail-closed, one-second recovery, Leaf generation replacement, two-wave `+330s` FD/thread baseline. | Validation only | `8b337502` passed; lazy-start smoke regression pending |
| Mesh hook audit | Keep Leaf in rules/DNS/outbound, adapter in bridge/lifecycle, and mesh in route/overlay/KCP/smoltcp. | Policy only calls endpoint prepare; generic mesh selector owns QUIC/KCP priority, capability, health, readiness, and fallback; OSPF and platform hosts remain separate. | No | Source boundary corrected and feature-matrix compiled; exact runtime and final requirement audit pending |
| Raw UDP underlay bottleneck | Keep ordinary mesh burst/loss investigation separate from Leaf conclusions. | Fixed-underlay counters and direct baseline before any tuning. | Separate future code | Documented; not part of `c92b11b1` |
| Comparator workflow efficiency | Split same-SHA no-Leaf output from the full profiling bundle. | One no-Leaf cargo build, metadata/checksum/symbol verification, no HEV/Leaf rebuild and no rolling-release mutation. | Workflow | Batched into the next candidate through the existing dispatchable workflow; exact run pending |

## Artifact-arrival execution plan

1. Verify Linux, Android, and comparator SHA/checksums/build metadata in parallel.
2. Deploy Linux artifacts to `.37/.38` while backing up and upgrading Android with `adb install -r`.
3. In parallel, run Linux no-policy sidecar absence and Android no-local-HEV startup checks.
4. Trigger one portless request and one explicit-only run to prove lazy ownership without rerunning the full already-passed matrix.
5. Run the same-bundle disabled-mode A/B and record all six resource/performance dimensions.
6. Update the release gate/report locally with exact run IDs and evidence; do not push documentation alone.
