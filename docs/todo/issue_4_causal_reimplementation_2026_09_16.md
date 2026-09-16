# Issue 4: causal findings and concrete dataplane reimplementation

Date: 2026-09-16. Refs #4 and Draft PR #5.

**Status:** completed bounded causal experiment plus a concrete engineering design. The full project performance assignment is **not** declared complete. No production reimplementation, Go Core, merge, release or deployment is claimed. Experimental omissions described below are not production fixes.

## 1. Correct the previous delivery

The maintainer correctly objected that the earlier work did not deliver the requested project-level causal diagnosis and rewrite/reimplementation plan. Finding an iperf completion defect, improving a results grader and removing a small allocation did not finish that assignment.

A previous answer also incorrectly promoted packet extraction again after its full-Core A/B had shown no material benefit. Its disposition remains **MECHANISM_VERIFIED; PERFORMANCE_BENEFIT_NOT_ESTABLISHED**. The unique-buffer control-block allocation is not a payload copy. Do not repeat that experiment as the leading remedy or claim an untested benefit for the separate TUN converter.

This document provides the missing concrete design: module boundaries, operations to remove, ownership and migration contracts, acceptance tests and an explicit full-Go alternative. It also records a new causal experiment that downgrades two more small optimization hypotheses rather than disguising them as large wins.

## 2. New causal CI result

### Reproducibility

- Run: [35041703403](https://github.com/lovitus/EasyTier/actions/runs/35041703403).
- Test/workflow commit: `8f43bb2db0d1474459cd2b3b37941b1c98de7390`.
- Source under test: `c6772dbfef2395ff96b39bd4801945d92212dffb`.
- Builds: Rust 1.95.0, GNU target, default features plus jemalloc, identical release flags/frame pointers. Stock and diagnostic are newly built comparators, not mixed with the earlier musl artifact.
- Stock Core SHA256: `b57689471e512857fc1e2a0f53c4e9b69c934b850ac1e83734671a575f4530a7`.
- Diagnostic Core SHA256: `97ad3282cd0fe9378a2151e43cd992e6bd8006d908fdc54b3bcfa0e34597a842`.
- Tool: checksum-pinned iperf 3.21; archive SHA256 `656e4405ebd620121de7ceca3eaf43a88f79ea1b857d041a6a0b1314801acdd8`.
- [Raw artifact 10426520589](https://github.com/lovitus/EasyTier/actions/runs/35041703403/artifacts/10426520589), ZIP SHA256 `355513b3dd57ae7a00d155bad87ea194a642d98b97bb93b15fcc121c36c74de1`.

The ZIP digest and all 897 manifest-listed file hashes were independently verified after download. CPU/GiB was recalculated from the recorded before/after process CPU counters and receiver bytes; PID/start identities and arithmetic matched. Raw artifacts include the generated diagnostic patch, build logs, hashes, individual results, resource samples and cleanup.

The two endpoints were on one ephemeral Linux runner in isolated veth/network namespaces. Encryption, routing and MTU settings remained unchanged between arms; no active Leaf/Mihomo/SOCKS flow or production endpoint was introduced. CPU values are both Core processes' user+system CPU seconds per received GiB over the gross transaction window, not all-machine CPU or one endpoint's cost.

### Five arms, one diagnostic binary

| Arm | Intervention | Boundary |
|---|---|---|
| stock | Unmodified Core before and after | Calibrates instrumentation/layout and environmental drift. |
| normal | Diagnostic hooks present, mask 0 | Primary same-binary control. |
| scan | Skip `entries.is_empty()` only after entry_count==0 and SOCKS disabled | Static no-flow cost intervention; not a safe general count/ownership fix. |
| metrics | Skip logical DATA `TrafficMetricRecorder::record_tx/record_rx` | Changes telemetry deliberately; other accounting, control, crypto and routing remain. Not a deployable setting. |
| both | Both interventions | Measures interaction; savings must not be added independently. |

The generator modifies only runner-local `socks5.rs`, `traffic_metrics.rs` and a guarded experiment hook in `lib.rs`. Repository production files were not changed. First-use logs prove initialization and exactly the expected intervention bits. No per-packet logging is added. Each diagnostic arm has three interleaved observations per workload; stock has two.

### Results

The workflow is **FAILED**, not green: two concurrent ping-loss gates failed. All 114 iperf transactions and 56 integrity/EOF tests completed successfully. There were 26 passing and two failing ping sessions, and all 24 activation checks passed. These are different kinds of records, not equal independent repetitions.

The failed UDP ping checks were `both`, round 4, and `metrics`, round 11. Each reports 81 transmitted / 80 received. In total there were 2,240 replies from 2,242 reported transmissions. Do not call that zero loss or silently waive the failures. This short sample does not establish whether the interventions caused the loss or whether timing/network variability did.

| Reverse saturation | Median Mbps | Combined CPU seconds/GiB | Goodput vs normal | CPU/GiB vs normal |
|---|---:|---:|---:|---:|
| UDP normal | 1157.181 | 23.491 | reference | reference |
| UDP scan bypass | 1170.135 | 23.480 | +1.12% | -0.04% |
| UDP metrics bypass | 1175.130 | 23.427 | +1.55% | -0.27% |
| UDP both | 1149.059 | 23.429 | -0.70% | -0.26% |
| TCP normal | 1929.722 | 12.674 | reference | reference |
| TCP scan bypass | 1913.152 | 12.377 | -0.86% | -2.35% |
| TCP metrics bypass | 1972.860 | 12.315 | +2.24% | -2.83% |
| TCP both | 1954.821 | 12.166 | +1.30% | -4.01% |

At fixed 100 Mbps, median CPU/GiB changes across the ablated directions/arms ranged approximately -1.56% to +1.83%, without a consistent meaningful reduction. Stock UDP saturation was 1151.73 then 1163.52 Mbps; stock TCP was 1925.74 then 1850.61 Mbps. Drift/calibration is material for small TCP effects. Three short repetitions do not establish narrow fleet-wide confidence intervals.

There were no harness exceptions. All 56 Core stop records exited with code 0 without SIGKILL escalation; maximum observed wait was about 0.164 s. Namespace deletion and root-route comparison passed. Sampled per-process peaks were 54,304 KiB RSS, 14 threads and 28 descriptors. This does not certify every internal shutdown invariant, many-peer behavior or long-term memory use.

**Decision:** neither the idle-map scan nor logical DATA accounting is demonstrated to be the major UDP mesh CPU cause. Keep flow ownership as a correctness cleanup, not the first claimed major throughput fix. Possible small TCP effects do not constitute an accepted general improvement. Never release the diagnostic bypasses.

The negative result is actionable: stop spending the main workstream on packet extraction, idle-map or telemetry micro-optimizations. Investigate/reimplement the larger packet path and the separate policy-stack scale problem. The exact split of remaining raw-mesh cost is still open; it would be incorrect to call this a complete causal apportionment.

## 3. Root-cause ledger, with evidence strength

| Problem | What is established | What is not established |
|---|---|---|
| Benchmark completion hang | Prior same-package wait-policy intervention repeatedly changed FAIL to PASS, and reversal restored FAIL. | That this explains CPU spent forwarding successfully delivered traffic. |
| Raw mesh processing/I/O cost | Measured CPU, carrier-associated efficiency differences, syscall/CPU profiles and source-proven per-packet pipeline. | A single dominant removable function or exact recoverable percentage. |
| Inactive map scan | Present in ancestor and fork; cost ablation above. | A major UDP bottleneck: the new experiment does not support that. |
| Logical DATA telemetry | Source present in ancestor, already improved by counter borrowing; new independent ablation. | A major UDP bottleneck or permission to discard telemetry. |
| Leaf connection scaling | Historical throughput roughly 868 to 444 Mbps with 1,000 idle sockets; current code still scans socket controls after smoltcp polling. | A fresh reproduction of every historical host or applicability to raw mesh TCP. |
| Duplicate policy TCP staging/window constraints | Four 327,660-byte capacities per connection; failed shrinking/chunk/budget experiments establish real tradeoffs. | 1.25 MiB physically resident for every idle connection or a universal raw-mesh bandwidth cap. |
| Leaf Unix bridge overhead | Queue/socket/runtime crossings and return construction are present, with historical native/bridge comparisons. | That two TUN names imply two serial TCP traversals in every backend. |
| Universal 400 Mbps limit/general idle leak | Not supported: prior unchanged Core exceeded 1 Gbps; short pure-mesh idle/churn was bounded. | Absence of every deployment-specific rate limit, storm or long-soak leak. |

### The architectural raw-mesh explanation

The inspected shape is:

```text
TUN receive / GSO split -> individual packet stream
 -> NIC classification/filtering and route/session selection
 -> compression/encryption -> per-connection send pipeline/ring
 -> individual UDP datagram submission

remote UDP receive -> mesh input processing
 -> decode/authentication/accounting/decompression/ACL
 -> registered packet filters -> NIC queue -> TUN writer/GRO
```

Native input can deliver several packets together, but many intermediate contracts expose one packet at a time. The Linux adapter also allocates and zero-fills a replacement buffer for each returned packet. The filter registry is held across awaited work. These are concrete reimplementation targets, not proof that every await parks, every filter allocates or every packet copies its payload. Exact async-ringbuf testing already rejected a flush/drain-barrier explanation.

Kernel packet handling, scheduling, syscalls and encryption do real work. Their inclusive profile costs cannot simply be added or called waste. A structural fix must show fewer actual crossings/allocations/wakeups or better useful batching while preserving delivered traffic, not merely a higher throughput number after removing features.

The inactive condition and logical telemetry both predate this fork. Current `traffic_metrics.rs` differs from its ancestor chiefly in the already-retained borrowed-counter fix. A mass deletion of fork-only features would not target these inherited costs.

### Policy scaling is a separate cause

Leaf's vendored `netstack-smoltcp` polls smoltcp and then iterates all application socket controls, locks them and moves bytes between socket storage and stream rings. Active traffic can pay for unrelated idle connections. Event-driven sleep fixes idle spinning, not this active O(n) work.

Fixed socket capacity can also limit the proxy segment it terminates. As a conditional calculation, 327,660 bytes at 100 ms gives a window/RTT bound around 26.2 Mbps. That is not a measured universal EasyTier limit: ordinary raw mesh does not terminate application TCP in Leaf.

## 4. Selected reimplementation direction

**Retain mesh routing/security/wire contracts. Reimplement the hot raw-packet path in bounded stages. Replace the complete policy processing boundary independently.** A full Go migration is specified below, but no Go superiority is claimed without equivalent tests.

```text
                 route / DNS / capture lifecycle authority
                                  |
                     immutable versioned DataPlanePlan
                                  |
                       one native reader per queue
                                  |
                    classify raw mesh vs policy once
                       /                       \
              raw mesh packet lane       selected policy backend
           EasyTier wire/security/       Leaf OR Mihomo OR native
             transport preserved          sing-tun candidate
                       \                       /
                    owned, generation-aware return injection
```

The target is one ownership authority and one active policy path per flow, not exactly one kernel interface at any cost. Desktop compatibility can use separate native mesh/policy TUNs when that avoids per-packet IPC. Android/Apple VPN integration requires its own common-capture adapter.

Proposed internal contracts, not compiled interfaces:

```rust
struct ReadyBatch { packets: SmallVec<[ZCPacket; 32]>, bytes: usize }
struct DataPlanePlan { generation: u64, routes: Arc<RouteSnapshot>, filters: Arc<FilterPlan> }
enum FastVerdict { Continue, Consume, Deferred(SlowPathWork) }
struct PendingBatch { /* owned unsent suffix, packet credits, byte credits */ }
```

A ready batch contains packets already available; never start a gathering timer. One packet takes the immediate path. Queue capacity is charged in packets AND bytes: a 32-packet batch must not secretly consume only one old packet credit. Control progress remains reserved. Large offload frames/virtio headers must be normalized or segmented before ordinary mesh transmission.

Activation follows `Prepared -> Ready -> Active -> Draining -> Stopped`, with explicit failure states. Validate capabilities and establish protected underlay rules before exposing capture. Publish one epoch. Stop closes admission, cancels owned workers, releases pending credits, awaits termination, closes native handles once and removes only epoch-owned route/DNS objects. Forced kill remains distinct from graceful exit. Fallback must not create a competing reader or reuse live nonce counters.

## 5. M1: flow registry correctness and O(1) inactivity

**Scope:** `gateway/socks5.rs`, `gateway/socks5/dataplane.rs`, new internal `gateway/socks5/flow_registry.rs`. This is not the major UDP performance remedy after the ablation.

Implement `register -> Lease`, `try_register`, registration-qualified removal, bulk retain/clear and `is_idle`. Each stored value has a unique registration identity. Reserve count before publication; release it after actual removal; replacement leaves the count unchanged. Old lease drop cannot delete a replacement or clear/re-register entry. Do not reset registration identities on ordinary clear.

Move every writer behind the registry before removing the all-shard fallback. Audit UDP expiry, failed connects, FFI, listener ownership, IPv4 reset and bulk removal. Acquire versus Relaxed alone is not proof. Do not let saturating-to-zero hide corrupt accounting; uncertain state must not masquerade as idle. Use the project's portable atomic facility for 64-bit IDs and handle exhaustion without reusing a live ID.

Use upstream `easytier-core/src/gateway/dataplane/flow.rs` as a behavior reference, not an architecture transplant.

**Retire:** redundant inactive map scans, unaccounted mutations and key-only owner cleanup. **Preserve:** all active SOCKS/dataplane/port-forward, ACL and modified-source behavior.

**Tests:** actual store old-owner/replacement; clear/re-register; failed try-register; publication/retain/drop interleavings; UDP bulk accounting; failed-connect cancellation; active/inactive filter cases. Then real-Core A/B in both states. Correctness and performance claims remain separate.

## 6. M2: structural raw-packet reimplementation

**Files:** `instance/virtual_nic.rs`, `instance/linux_tun_offload.rs`, `peers/peer_manager.rs`, `peer_map.rs`, `peer_conn.rs`, `traffic_metrics.rs`, `tunnel/ring.rs`, `tunnel/udp.rs`, plus small batch/plan modules. Do not import portable-core wholesale.

### M2a. Immutable plan and explicit slow work

Publish filter/route membership as versioned immutable snapshots. Preserve priority/reverse ordering and Consume/StopAndSend meaning. Borrow a plan for ready work instead of keeping registry read guards across asynchronous operations. Define when removal stops new traffic and how old objects remain alive for in-flight traffic.

Separate synchronous classification, already-resolved routes/handles and packet transformation from real DNS, connection preparation and asynchronous proxy setup. Slow work owns its packet, deadline, cancellation and generation. Resume at the correct stage: no duplicate encryption, skipped ACL or accidental delivery into a replacement policy runtime. Preserve per-flow/session ordering; do not launch a task per packet.

### M2b. Preserve ready batches end to end

Expose already-returned TUN/GSO groups without immediately destroying that ownership boundary. Process under a stable plan and move packets into bounded per-destination/session groups. Do not clone merely to form a batch. Revalidate session/generation transitions and preserve order within flows.

The UDP writer uses multi-message submission only for a real ready group. Keep the singleton path. Partial send consumes only the reported prefix; retain the unsent suffix and credits. Seal each record once; retry only unsent records after readiness. EAGAIN, cancellation, rekey and non-Linux fallback have explicit outcomes. Never concatenate unrelated peer packets into a new encrypted datagram or silently introduce a new wire protocol.

This differs from the failed writer-only sendmmsg implementation: it changes the upstream contract that supplies an existing group. It must demonstrate useful real batches. If all groups remain singletons, stop rather than adding sleeps or hiding more backlog.

Retain one clear owner per connection/queue initially. Further CPU sharding is a separate change keyed by stable session/flow identity, with control reservations and cooperative bounded bursts.

### M2c. Keep correct statistics without repeated setup

The current experiment does not identify logical telemetry as a major UDP cause. Do not make this a separate long optimization campaign or ship the diagnostic bypass.

Within the broader plan, resolve label handles during identity updates; use exact synchronous counters on the resolved path; preserve attribution on unknown-to-known transitions. A shared event timestamp can replace redundant equivalent clock reads only with unchanged activity/expiry semantics. Batch aggregation needs specified visibility and event-time rules. Inspection must not refresh lifetime.

Before adding parallel writers, address the actual storage contract: current `stats_manager.rs` uses UnsafeCell counters/timestamps, manual Send/Sync and a safe handle API whose comment assumes thread-local access. Comments are not enforced ownership. Use owner-local writes with synchronized snapshots or portable synchronized counters; preserve saturating arithmetic/reset/cleanup and test concurrent access. This is a source-level safety concern, not a diagnosed cause of the throughput result.

### M2d. Bounded buffer ownership

Measure the per-returned-packet replacement allocation in the native adapter. Recycle only after unique ownership is released; shared/fanout payloads wait for the last user. Bound pools by bytes and size classes; retain headroom/tailroom and initialized-length guarantees. Cancellation/errors reclaim credits and buffers. No eager 64-KiB allocation per small-packet slot, unbounded pool or unsafe exposure of uninitialized bytes.

**Required mechanism evidence:** group sizes at native read, processing, per-peer enqueue and actual submission; bytes retained; WouldBlock/partial sends; sampled queue residence/allocations. Bracket observers with uninstrumented runs. Acceptance requires both a mechanism change and repeated receiver-goodput/CPU improvement. Never sum inclusive profile percentages.

## 7. M3: replace the complete policy path

**Preferred Linux prototype:** compare existing native Leaf-owned TUN and native Mihomo against an exactly pinned native sing-tun/sing-box policy backend. Policy traffic remains inside that engine's native runtime. Rust mesh continues to carry raw mesh packets. Separate desktop interfaces are acceptable during this experiment.

**Retire on the new backend:** legacy AF_UNIX packet exchange, its queue/runtime crossings, duplicate return construction/queue stage, Leaf stream staging and the all-socket wrapper scan. Existing backends remain explicitly selected compatibility paths, not serial layers. A new engine behind the existing fake packet-TUN socket is not a native-fast-path experiment.

Define a versioned `PolicyPlan`: ordered rules, DNS/FakeIP, outbound chains/providers/groups, UDP behavior, protected endpoints, underlay choice, capabilities and reload semantics. Validate before activation. Unsupported semantics must reject configuration, not become DIRECT or reorder rules. Do not silently translate arbitrary Mihomo YAML into sing-box.

One authority controls desired route/capture/DNS epochs and each adapter's owned OS changes. Mesh/STUN/control/proxy sockets must not recurse into capture. Failure/reload remains fail-closed for policy traffic while preserving healthy mesh.

The reviewed sing-tun Port flow path rewrites source/selector state. It is not arbitrary raw-IP mesh forwarding. A common capture needs a supported raw diversion hook before policy NAT and a matching raw return-injection contract. That is work to implement, not an existing API established by this document. Two runtimes may not compete to read the same native descriptor.

If Leaf must be retained, the alternative is a real TCP-stack project: activity-indexed work and timer scheduling in wrapper AND lower stack, plus segmented/resizable storage, SYN-time window-scale capability, advertised windows, reassembly/SACK, zero-window/FIN and byte-budget progress. Another fixed ring size cannot solve this. The historical 128-forward-flow budget failure remains mandatory regression coverage.

## 8. Concrete full-Go alternative

A full rewrite is possible but not implemented or benchmarked here. The following is the migration plan, not an assertion that Go is intrinsically faster.

```text
go-dataplane/
  cmd/easytier-dataplane/   process/service lifecycle
  internal/wire/           exact headers, flags, serialization/version rules
  internal/session/        handshake, keys, replay, rotation, Stealth
  internal/engine/         packet owners/shards, credits, scheduling
  internal/route/          immutable route/ACL plans and epochs
  internal/native/         Linux/Darwin/Windows/mobile I/O adapters
  internal/transport/      UDP first, then required TCP/QUIC/KCP/WG/WS parity
  internal/policy/         selected native policy engine/capability adapter
  internal/metrics/        exact resolved counters and safe snapshots
  internal/control/       bounded authenticated versioned controller messages
```

**G0 — wire contract:** generate golden vectors from the pinned Rust implementation for headers/endianness, malformed input, compression, encryption/replay, handshake/capability fields and errors. Explicitly handle the known 0x20 and Noise 6/11 conflicts. Preserve valid wire contracts, not known unsafe internal implementations.

**G1 — interoperable native UDP proof:** implement the required framing, native I/O and security; test Rust-to-Go and Go-to-Rust in both directions, tampering/replay/reordering, MTU, pressure and cleanup. No security-disabled or header-omitting benchmark counts as parity.

**G2 — single data owner:** Go owns each adopted transport socket and full authenticated data/control demultiplexing. Rust remains controller initially; it cannot concurrently read the socket. The local channel carries low-rate plans and decoded control events, not every payload packet. Keep control capacity reserved. Initial cutover renegotiates sessions; never copy opaque live crypto state or reuse nonce counters after fallback.

**G3 — topology and transport completeness:** add relay/foreign-network, bootstrap/discovery, negotiation and all required transports. Test mixed versions, route churn, one-way blackholes, key changes and peer removal. A UDP-only prototype is not an all-feature replacement.

**G4 — policy/platform parity:** native policy, raw mesh diversion and platform route/DNS/VPN lifecycle. Keep legacy engines for unsupported platforms until their own gates pass. Go compilation is not platform performance/lifecycle proof.

**G5 — replacement decision:** benchmark against the corrected Rust path with equivalent features on the same hardware. Proposed thresholds: at least 20% less total CPU/GiB or 20% higher goodput at equal CPU budget on important workloads; no greater than 5% required-direction throughput regression beyond baseline uncertainty; no material latency/loss/memory/control regression. Finalize before measurement. Include Go GC and all participating processes/kernel work. These are targets, not measured wins.

Use the maintained chosen TCP stack rather than writing TCP congestion/reassembly again merely for one-language purity. Go's soft memory limit is not a byte-bounded queue design. FFI must obey cgo pointer lifetimes; no retained borrowed slices without valid ownership. A sidecar alone is neither a legal exemption nor a complete security boundary.

## 9. Platforms and retirement conditions

Shared contracts own packet classes, plans, credits, route/policy epochs and stop semantics; native adapters retain OS differences.

- Linux: native ownership, epoll, real offload/multiqueue capabilities, marks/routes/firewall rules and MTU normalization.
- macOS daemon: utun framing, kqueue/backpressure, truncation protection, sleep/wake and cleanup.
- Windows: Wintun/IOCP/AFD ownership, pressure, reconnect and stop; not Unix-FD substitution.
- Android: one VpnService capture, protected underlay sockets, app scope, revocation and network change; no competing policy VPN.
- Apple Network Extension/iOS: supported packet-flow API, entitlements, memory pressure and lifecycle; desktop utun results do not certify it.
- FreeBSD/OHOS/MIPS/MIPSel: retain compatibility until exact build/API/ABI/atomic/resource evidence exists.

Review exact licenses and distribution obligations before embedding/packaging the selected Go engine. The inspected sing-tun declares GPLv3-or-later; repository/crate license declarations differ. This document is not a legal compatibility determination.

Retire old code only when the replacement path passes and the selected build/profile no longer initializes the old engine, bridge, buffers or tasks. Do not retain a permanent third serial stack. Package-size cleanup is separate from measured runtime CPU.

## 10. Historical failures that constrain the design

| Implementation | Disposition |
|---|---|
| GRO scratch expansion 49682dbd, removed by 35c81d5b | Keep removed; historical affected-direction A/B/A approximately 336.5 -> 163.8 -> 337.1 Mbps. Preserve working offload. |
| Writer-only sendmmsg / adaptive recvmmsg | Do not repeat without changed mechanism and actual backlog. Prior real writer never formed the required batch; singleton/receive tests failed their gates. |
| Large native windows, removed in f8b0ca79 | Already removed; cannot claim the same deletion twice. |
| caf226e1 -> 4f9395a9 | Four 32-KiB rings: historical -53.8% throughput and higher idle RSS. |
| 0c8894e2 -> e26c9815; 6c92be28 -> 728a8679 | Chunk staging: historical roughly -40.3% and -38.1% throughput. |
| 271351f0 -> 062628ad | Whole-ring budget passed single flow but failed 128 concurrent forward progress. |
| 5e6d455b -> 8d4dc0f8 | Budget progress repaired but 128-KiB TCP windows still regressed throughput about 28.3%. |
| Framed Leaf PacketBatch | Keep removed: multi-host download regression and Android startup panic. This does not reject native Leaf-owned TUN. |
| Leaf-owned TUN restored f6617c51 | Retain validated native/generic/legacy behavior and host caveats; broad earlier rejection was overturned. |
| 6dd2fe7e / 8201a4a8 | Preserve event-driven runner and output fairness. They are not part of the rejected memory designs. |
| 571b6561 | Preserve lower-level Quinn truncation fix; do not restore the ineffective outer wrapper. |
| 1af984c9 | Borrowed traffic-counter fix already present; no new forecast from its historical gain. |

Historical measurements above are retained repository evidence, not newly rerun host tests. Counts of commits or lines do not prove bloat. Keep failed experiment records so the same designs are not reintroduced under new names.

## 11. Deliverables and validation required to close the assignment

M1 is a bounded ownership/correctness milestone, not the major performance lead after the new experiment. M2 structural mesh work and M3 native policy replacement are the substantive reimplementations. G0-G5 specify the full-Go branch of the decision.

The missing runtime ancestor/fork/upstream comparison must build a common feature subset at `15e5d89f`, `c6772dbf` and `286e0f4a`. Match toolchain, allocator, security, transport, MTU and workload where possible, recording unavoidable differences. Keep common comparable paths separate from best native supported paths. The completed source comparison is not this runtime experiment.

Required tests for accepted candidates:

1. Real store/ownership/lifecycle regressions, not just simplified model maps.
2. Per-change Core A/B/reversal plus individual samples and receiver CPU/goodput; observers bracketed by uninstrumented controls.
3. Raw mesh IPv4/IPv6, direct/relay, required carriers, both directions, sparse/saturated/multiple flows, actual batch and MTU/PMTUD behavior.
4. Active policy DIRECT/equivalent proxy/proxy-over-mesh, ordered rules, DNS/FakeIP and fail-closed reload.
5. Scale: 1/100/1,000/2,000 idle and 1/32/128 or feasible higher active flows. Active streams must exceed optional budget slots without deadlock.
6. Bounded latency/loss/reordering, route changes, rekey, cancellation, listener rebind and graceful versus forced shutdown.
7. Per-endpoint and total CPU/GiB, total-system CPU separately, p50/p95/p99, drops/retransmits, copies/allocations, queue bytes/residence, RSS/PSS, FD/task and release behavior.
8. Actual supported device/platform evidence before the corresponding release claim. Same-VM namespaces are not WAN/NIC or mobile certification.

Do not use sample timing inside baseline noise as acceptance. Do not remove encryption, ACL, replay, Stealth, source identity, route constraints or metrics meaning to claim a win. Stop on corruption, uncontrolled growth, control starvation or safety limits. Clean only owned resources. No automatic merge/release follows.

**Completion statement:** the missing concrete design is now supplied. The current experiment downgrades idle-map and telemetry as major UDP explanations, while the full raw-mesh stage-cost apportionment, current active-policy scale reproduction and accepted M2/M3 implementations remain unfinished. This document does not relabel those gaps as complete.

## Sources

- [Prior scoped CI report and raw-run ledger](https://github.com/lovitus/EasyTier/blob/cec1233bbd685568471e5f1927cd9cee226fb352/docs/todo/issue_4_ci_campaign_results.md).
- [Completed packet-extraction A/B](https://github.com/lovitus/EasyTier/actions/runs/35037691674).
- [Current causal workflow](https://github.com/lovitus/EasyTier/blob/8f43bb2db0d1474459cd2b3b37941b1c98de7390/.github/workflows/mesh-causal-ablation.yml), [intervention generator](https://github.com/lovitus/EasyTier/blob/8f43bb2db0d1474459cd2b3b37941b1c98de7390/tools/mesh-cause/prepare_ablation.py), [experiment](https://github.com/lovitus/EasyTier/blob/8f43bb2db0d1474459cd2b3b37941b1c98de7390/tools/mesh-cause/ablation.py).
- [Fork SOCKS filter/counts](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/gateway/socks5.rs), [ancestor](https://github.com/EasyTier/EasyTier/blob/15e5d89f70c3f64f047c48d60e67c12c74c5bebc/easytier/src/gateway/socks5.rs).
- [Fork lease ownership](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/gateway/socks5/dataplane.rs), [upstream registration-qualified flow table](https://github.com/EasyTier/EasyTier/blob/286e0f4a0d801637178d1e58def19e508c9109c2/easytier-core/src/gateway/dataplane/flow.rs).
- [Peer processing](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/peer_manager.rs), [logical telemetry](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/traffic_metrics.rs), [counter storage](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/common/stats_manager.rs).
- [Native adapter](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/instance/linux_tun_offload.rs), [NIC dispatcher](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/instance/virtual_nic.rs), [UDP writer](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/tunnel/udp.rs).
- [Leaf socket scan](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/third_party/netstack-smoltcp/src/tcp.rs), [canonical failed buffer designs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/docs/known_bugs/netstack_tcp_buffer_scaling_failed_implementations.md).
- [Canonical PacketBatch failure](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/docs/failed_attempts/FAILED_leaf_external_packet_endpoint_performance.md), [batching/counter history](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/docs/todo/core_udp_batch_optimization.md).
- [Pinned new sing-tun source](https://github.com/SagerNet/sing-tun/tree/869f0a4d76af32b9ad8cdfa821aee35489d650db), [current TUN documentation](https://sing-box.sagernet.org/configuration/inbound/tun/), [Go GC guide](https://go.dev/doc/gc-guide), [cgo ownership rules](https://pkg.go.dev/cmd/cgo#hdr-Passing_pointers).
