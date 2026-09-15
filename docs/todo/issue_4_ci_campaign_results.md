# Issue 4: tested mesh performance findings and repair gates

Status: scoped Linux CI campaign completed; diagnosis recorded; no Core optimization, Go rewrite, production deployment, merge or release performed. Refs #4 and Draft PR #5. Tests ran 2026-09-15 UTC; evidence consolidated 2026-09-16. This report supersedes the earlier SOURCE_REVIEW_ONLY status for these specific experiments, not for untested upstream integrations.

## Executive decision

The campaign separates a **reproducible benchmark-tool completion fault** from **real encrypted-mesh CPU/I/O cost**. It disproves a universal approximately 400 Mbps ceiling in the tested Core. It does not prove a general memory leak, an idle busy loop, or that Rust is the cause.

The strongest causal result is server-side: the Ubuntu 22.04 packaged iperf performs a ten-second select wait from Nread on a nonblocking data descriptor. A scoped wait-policy intervention, with Core and the packaged binaries unchanged, repeatedly changes FAIL to PASS; removing it restores FAIL. Do not patch Core FIN handling to conceal this application behavior.

For the remaining performance work, preserve the Rust Core and test one measured packet-ownership/I/O boundary. Native sing-tun or a Go reactor remains a separately benchmarked option, not a demonstrated fix for pure mesh. No complete equivalent Go implementation was tested.

## 1. Reproducible inputs

| Item | Value |
|---|---|
| Measured Core source | `391c191c3d8b477c3b7e8ef19e87ae3cba5c9504` |
| Binary SHA256 | `95c5724367ce4ab5a33688f871cc1531c50d84e78389e0c77d4d17ea99b5dc71` |
| Build ID | `73e2433803eb3f9c07aa8024283765468a634a55` |
| Archive SHA256 | `9c7d8a5dc1987491c79e4e1a2576911f18807f7f2ac80570a9675c0291ea2445` |
| Original optimized artifact run | `31561399937` |
| Harness base | `c6772dbfef2395ff96b39bd4801945d92212dffb`, codex/current |
| Initial harness | `25b9fa4ea0e502dc310baa0539057bb3d9ee6a1f` |
| Final targeted harness | `2f659bed0cd29fed97a06345946624b87c2af7e0` |
| Evidence-only aggregation | `eae09463b4ac5900417b5aff8c92f1a96b3ed91d` |
| Built upstream iperf 3.9 | `1f8fb13297f3e3e40169ebcd12e171167e394473` |
| Dependency contract | async-ringbuf 0.3.1 |

The [391c191c...c6772dbf comparison](https://github.com/lovitus/EasyTier/compare/391c191c3d8b477c3b7e8ef19e87ae3cba5c9504...c6772dbfef2395ff96b39bd4801945d92212dffb) has three commits and ten changed workflow/instruction/release-documentation/script files, with no Core production-source or Cargo dependency changes. The measured binary is relevant to the target source, without claiming different commit identities or build artifacts are identical.

The two ephemeral Linux runner images each used two namespaces on one VM, joined by veth, without namespace external default routes or production network identity. Encryption remained enabled; Leaf/Mihomo/GOST application workloads were inactive. CPU/kernel environments differ across allocations, including the same image at different times. Results are not an OS comparison or a WAN promise.

Goodput is receiver payload. CPU/GiB means combined **both-Core user+system CPU seconds / received GiB** over the documented gross transaction window, excluding iperf CPU and not measuring all machine CPU. Combined core equivalents are CPU seconds/wall seconds, not physical-core counts or a single endpoint. Native rows' zero Core CPU means no Core process, not free native networking. RSS/FD/thread values are sampled, not complete kernel/heap limits.

## 2. Run ledger and retained failures

| Run | Outcome |
|---|---|
| [34995727266](https://github.com/lovitus/EasyTier/actions/runs/34995727266) | Broad matrix completed on both images. 22.04 retained 19 iperf failures; 24.04 completed all 76 network/fixture records. Ring fixture had a linker-configuration setup failure and no result. Overall red. |
| [34997288873](https://github.com/lovitus/EasyTier/actions/runs/34997288873) | Targeted simulation steps ran, but artifact upload failed on both jobs. Zero archives were recovered from this run; log-only measurements are not pooled into the tables below. |
| [34998192877](https://github.com/lovitus/EasyTier/actions/runs/34998192877) | Main targeted lab lane completed on both images. Separate 22.04 control lane retained four reproduced completion failures. A green main-lane gate does not turn them into PASS. |
| [35034887986](https://github.com/lovitus/EasyTier/actions/runs/35034887986) | Downloaded/digest-verified four first/final-run artifact archives and aggregated them without rerunning workloads. |

Consolidated coverage: **242 workload records, 219 successful and 23 failed**. There are 34 successful integrity/half-close records representing **826 fixture connections**, including 800 serial-churn connections. The remaining 208 are iperf records, including observation/intervention cases. These are bookkeeping counts, not equal-duration independent statistical repetitions. All 23 failures are retained iperf completion cases, not misreported throughput results.

The [consolidated artifact](https://github.com/lovitus/EasyTier/actions/runs/35034887986/artifacts/10422374721) contains summary.json, original-results.jsonl, results.csv and an SHA256 manifest of 1,198 source files. ZIP SHA256: `e6d4ad2a54826326cd0289fd26d5eb52e2b3d503261ac7cbe44eba94cea69ed9`. Configured retention is 30 days; original archives have shorter retention. Preserve needed evidence before expiry.

**CI gate correction:** the final diagnostic workflow's current success gate reads `lab/results.jsonl`, not the separate `control/results.jsonl`. Infrastructure completion, ordinary correctness, expected causal-reproducer outcomes, and cleanup must be graded independently in the next harness revision. A successful diagnosis may include a deliberately reproduced failure; that application transaction must still be labelled FAIL.

## 3. Causal completion diagnosis

Ubuntu 22.04 installed iperf3/libiperf0 `3.9-1+deb11u1ubuntu0.1`; Ubuntu 24.04 installed `3.16-1build2`. Version text alone is insufficient provenance. The independent upstream 3.9 comparator is a separate exactly pinned build.

The first matrix had **two native-veth failures without any Core running**, as well as mesh failures. Independent content/EOF tests passed. The final 22.04 UDP-mesh normal-mode control sequence was:

| Original | Observe-only | Change matching wait | Repeat change | Observe-only restored | Original restored |
|---|---|---|---|---|---|
| FAIL | FAIL | PASS | PASS | FAIL | FAIL |

The [test-only select shim](https://github.com/lovitus/EasyTier/blob/2f659bed0cd29fed97a06345946624b87c2af7e0/tools/mesh-ci/select_diagnostic.c) changes only positive-timeout select calls containing one read FD, no write/error set, and a nonblocking descriptor. It calls the real select with timeout zero rather than fabricating readiness. It is attached only to the iperf server, not Core or the machine globally. Observer output resolves `caller=Nread`, `fd=5`, `timeout=10.000000`; change counters are nonzero only in intervention mode.

The same fixed-rate native controls passed. A strace-attached mesh observation also passed, demonstrating timing disturbance; it is not counted as ordinary uninstrumented performance. The built upstream 3.9 completed two repetitions of all four role/payload-direction combinations for both carriers on both final-run images.

**Conclusion:** the packaged receive-wait policy is a high-confidence causal contributor to the reproduced completion fault. Data-only waiting delaying return to the separate-control-connection event loop is the strongly supported mechanism. Exact downstream patch-line attribution and all possible long-flow races are not claimed complete.

**Repair:** pin a maintained security-patched measurement tool/package that passes control/data and EOF regressions. Preserve this failing package as an isolated reproducer. Do not deploy the diagnostic LD_PRELOAD globally, remove production timeout protections, downgrade production to old upstream 3.9, or change mesh close semantics simply to obtain a green iperf result. [Ubuntu's package/security record](https://ubuntu.com/security/notices/USN-7970-1) is provenance, not independent proof of the experimental mechanism.

## 4. Bandwidth and CPU

### Initial completed single-stream saturation results

| Image | Carrier | A-to-B median Mbps | B-to-A median Mbps | Successful repeats |
|---|---|---:|---:|---|
| 22.04 | UDP | 1070.6 | 1088.9 | 3/3 each |
| 22.04 | TCP | 1619.1* | 1691.8 | *1/3 forward; 3/3 reverse |
| 24.04 | UDP | 1883.0 | 1882.4 | 3/3 each |
| 24.04 | TCP | 2848.1 | 2846.9 | 3/3 each |

The starred number is one successful observation, not a valid three-run median. Failed cases remain excluded. These results reject a universal approximately 400 Mbps ceiling in this binary/configuration. They do not reject every deployment-specific rate limit or explain every laptop constraint.

Native completed references were about 32–33 and 48–49 Gbps respectively, with different packetization/aggregation and no mesh encryption path. Do not derive a product-regression multiplier from that ratio. Do not attribute image differences solely to Ubuntu/kernel version. The same 24.04 image's later allocation measured roughly 1.07–1.18 Gbps UDP instead of approximately 1.88 Gbps, with unchanged Core.

### Final uninstrumented 100 Mbps reverse controls

| Image | UDP combined CPU seconds/GiB, before/after | TCP combined CPU seconds/GiB, before/after |
|---|---:|---:|
| 22.04 | 31.23 / 30.52 | 20.30 / 20.59 |
| 24.04 | 25.02 / 25.16 | 16.16 / 16.73 |

This is a within-job carrier-associated efficiency difference, not proof that one syscall explains it. It is not a recommendation to make TCP universal on lossy WANs. Initial fixed-rate aggregate groups sometimes combine ordinary and later offload/delay rows because the first harness lacked phase labels; they must not be mistaken for a pure rate-response curve. The explicit final before/after phases above avoid that grouping error.

### I/O evidence

Counts-only diagnostic strace at a matched nominal 50 Mbps, across both Core processes:

| Image / carrier | sendto | recvfrom | epoll_pwait | write | writev |
|---|---:|---:|---:|---:|---:|
| 22.04 UDP | 22625 | 23896 | 17412 | 23747 | 0 in selected summary |
| 22.04 TCP | 4 | 7360 | 5317 | 23326 | 3009 |
| 24.04 UDP | 19994 | 20979 | 15525 | 21138 | 0 in selected summary |
| 24.04 TCP | 4 | 7055 | 4745 | 22832 | 2662 |

These are perturbed diagnostic counts, not precise uninstrumented datagram/syscall ratios. Not every read/write is attributed to a TUN FD. Receive-error counts remain in raw summaries; successful submission is not delivery. Trace wait time is not CPU time.

Software cpu-clock profiles (about 1K/820 samples, zero recorded loss) identify broad kernel wakeup/unlock, syscall entry, TUN, packet allocation, hardware AES-GCM, copy, timing and mesh-dispatch costs. They do not apportion all avoidable CPU or justify summing inclusive percentages. Core system time is material, but necessary kernel processing is not automatically waste.

The inspected [UDP writer](https://github.com/lovitus/EasyTier/blob/391c191c3d8b477c3b7e8ef19e87ae3cba5c9504/easytier/src/tunnel/udp.rs) submits individual optionally sealed datagrams. [Linux TUN offload](https://github.com/lovitus/EasyTier/blob/391c191c3d8b477c3b7e8ef19e87ae3cba5c9504/easytier/src/instance/linux_tun_offload.rs) already exists and replaces/zero-fills RX buffers per returned packet. Actual natural-batch histograms, queue residence and allocation attribution remain missing.

### Kernel-feature A/B/A

| Image | Condition | CPU seconds/GiB at 200 Mbps | Saturation Mbps |
|---|---|---:|---:|
| 22.04 | on | 30.4802 | 1062.2 |
| 22.04 | off | 32.6117 | 1042.6 |
| 22.04 | on restored | 30.8355 | 1062.8 |
| 24.04 | on | 24.6118 | 1179.9 |
| 24.04 | off | 26.0427 | 1072.8 |
| 24.04 | on restored | 24.6833 | 1152.8 |

Disabling tso/gso/gro raised fixed-load CPU/GiB by roughly 6% relative to the two A values. This short comparison favors preserving the working offload path, not an exact universal percentage. Flags off **does not mean legacy Core adapter**; virtio/native ownership is unchanged. It does not justify restoring the rejected GRO-scratch implementation.

## 5. Resources and ruled-out hypotheses

Largest sampled per-Core RSS: **66,932 KiB (~65.4 MiB)** across artifact-backed lanes; initial 22.04 peak: 31,156 KiB (~30.4 MiB). Maximum sampled thread count 14 and FDs 33 per process. Connected-idle observations ranged from below tick resolution to approximately 0.00375 combined core equivalents. Serial churn snapshots showed no persistent FD/thread increase; RSS was stable or declined in the cited before/after pairs. Namespace/root-route cleanup passed.

These short/simple-topology observations do **not** establish a general leak, nor do they rule out all long-soak, multi-peer, kernel-buffer or production-only problems. Supervised cleanup can escalate to kill, so cleanup success is not proof of graceful Core shutdown without escalation.

The exact async-ringbuf 0.3.1 contract probe found send immediately ready before consumer polling, with one queued item; dependency `poll_flush` is immediately ready. Synthetic variants produced the same 1,563 consumer groups for 100,000 items. Therefore **a ring-flush drain barrier is not the batching cause**. This does not prove real Core batching.

Do not repeat:
- writer-only drain/sendmmsg without proof of natural backlog: the [historical real-Core candidate](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/docs/todo/core_udp_batch_optimization.md) never triggered it;
- larger GRO scratch: [real A/B/A rejection](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/docs/todo/core_gro_gost_ipv6_candidate_manifest.md) remains relevant;
- counter-borrow optimization already present in [measured traffic_metrics.rs](https://github.com/lovitus/EasyTier/blob/391c191c3d8b477c3b7e8ef19e87ae3cba5c9504/easytier/src/peers/traffic_metrics.rs);
- bigger queues/windows or arbitrary workers without matched sparse-load, fairness and memory evidence.

## 6. Ranked repairs and Go decision

1. **Measurement repair now.** Pin tool/library provenance; separate normal correctness, causal reproductions and infrastructure; attach physical-direction and phase metadata; keep failed transfers out of capacity/efficiency tables. Keep content/EOF independent of tool completion.
2. **One packet-boundary observation, then a bounded patch.** Measure actual TUN/GSO groups through mesh handling, per-peer queue, UDP submission and receiver TUN output. Identify where a natural batch is broken. Preserve a proven batch without a timer, new wire format, unlimited backlog or control starvation; handle partial submissions, EAGAIN, cancellation, order and non-Linux fallback. Validate on real Core, not a prefilled queue.
3. **Buffer recycling only after allocation attribution.** Preserve headroom/tailroom, unique/shared buffer ownership, encryption, initialization, cancellation and byte-bounded pooling. Do not turn every small-packet slot into a permanent oversized allocation or restore rejected eager windows.
4. **Small metadata/filter changes only for a newly measured operation.** Preserve ACL/order/reload/generation/metric meaning; old inherited wins are not current forecasts.
5. **Native-reactor consolidation as a separately gated architecture experiment.** Aim to remove existing per-packet handoffs rather than add a third runtime. Compare against the fastest correct existing topology, not the slowest bridge.

**Go is possible, but not the evidence-backed first repair.** No Go Core was implemented or benchmarked here. Translating the same packet-per-task/queue/syscall architecture preserves most of its work. Go's GC introduces explicit CPU/memory tradeoffs and its memory limit is soft, so allocation/ownership still matter ([official Go GC guide](https://go.dev/doc/gc-guide)). That is not proof Go must be slower either.

The useful comparison is a narrow Rust-versus-Go native packet owner/reactor with identical wire packets, encryption/Stealth, MTU, routing/source identity, fair control progress, cancellation and resource limits. If it wins materially, migrate by tested boundaries. A full rewrite must additionally reproduce peer discovery, relays, protocol negotiation, platform routes/DNS/VPN ownership and policy compatibility. Do not promise parity from a single throughput benchmark.

A native sing-tun policy experiment remains separate. Its inspected Port forwarding uses source/selector rewriting, not a transparent arbitrary-IP mesh bypass; its native stack needs native device integration. Preserving policy NAT semantics is not the same as preserving raw mesh identities. Existing GPL/combined-work and cross-platform distribution constraints remain adoption gates.

## 7. Limits and next deliverable

This campaign did not test real WAN/Wi-Fi, full loss/reordering conditions, every MTU, large peer counts, overnight memory behavior, IPv6, KCP/QUIC, active policy engines, mobile/desktop non-Linux targets, MIPS, or a Go implementation. Simulated delay was a bounded smoke, not realistic WAN certification. Per-stage batch/queue/allocation attribution is still incomplete. No new production Core speedup has been demonstrated.

The next deliverable is a **specific instrumented packet-ownership boundary and one causally justified real-Core A/B/A candidate**, not another general rewrite plan. Fix benchmark provenance before treating tool failures as product faults. Continue checking security, receiver delivery, sparse traffic, timers/Pongs, memory, stop and interoperability; do not disable encryption to claim a win.

Full test recipe: [initial fixture](https://github.com/lovitus/EasyTier/blob/25b9fa4ea0e502dc310baa0539057bb3d9ee6a1f/tools/mesh-ci/diagnose.py), [targeted follow-up](https://github.com/lovitus/EasyTier/blob/2f659bed0cd29fed97a06345946624b87c2af7e0/tools/mesh-ci/followup.py), [final diagnostic workflow](https://github.com/lovitus/EasyTier/blob/2f659bed0cd29fed97a06345946624b87c2af7e0/.github/workflows/mesh-ci-diagnosis.yml), [aggregation recipe](https://github.com/lovitus/EasyTier/blob/eae09463b4ac5900417b5aff8c92f1a96b3ed91d/.github/workflows/mesh-ci-evidence-report.yml). Run/job logs and raw artifact rows are the source of numerical results; no private laptop archive was silently incorporated.

No merge/release follows automatically. Full local repository preflight was unavailable, and test-only CI is not a substitute for release/platform gates. This evidence-only document must not retrigger the completed benchmark matrix.