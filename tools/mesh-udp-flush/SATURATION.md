# Pure mesh bottleneck diagnosis

Current cursor: run 36074709548 completed the uninstrumented saturation lane.
Profile recovery run 36080205599 passed with the same executable bytes:
2,568 CPU samples, both endpoints represented in each capture, no reported
sample loss, and clean namespace/process teardown. The original failed
profile remains failed. Next step: profile_only=true plus tun_trace=true,
reusing artifact 10841125115 for four bounded 64 MiB transfers, not a rebuild.
Production Core and the unresolved GSO rejection assertion are unchanged.

## Receive-side follow-up

The recovered profiles put inclusive TUN flush stacks at 26-29 percent of
remaining GSO CPU samples. These include kernel receive processing; the
wrapper's self cost is small. This does not justify removing the wrapper.
Evidence: https://github.com/lovitus/EasyTier/issues/4#issuecomment-5824935812

An older retained syscall capture has 26,118 successful receiver TUN writes
per transfer in four traces, none above 1,370 bytes. This is an older binary
and paced workload, not evidence of current GSO candidate merge behavior.
The new observation records syscall entry/exit, exact TUN fd and thread
identities, and trace loss counters on the existing exact candidate. Rates
and CPU from traced transfers are diagnostic only. Changed identities or
lost records fail the observation rather than silently undercounting.

Do not revive the previously reverted large GRO scratch buffer, infer
merge failure solely from this old capture, or conflate transport GSO with
inner TCP GRO. No production receive-path change is included here.

## Completed saturation measurement and remaining profile recovery

Harness 1639ec4534eb824ce88e7300b5fa1453839c4f25 measured same-binary legacy
1125.65/1130.53 Mbps versus GSO 1400.46/1412.15 Mbps (three samples per
direction). Two-Core CPU seconds/GiB were 24.36/24.29 versus 17.51/17.37.
This is approximately 25 percent higher single-flow throughput and 28 percent
lower CPU/GiB on this runner, not a WAN or production acceptance claim.
Eight contract tests and the saturation lane's integrity, ICMP, transport,
metric and cleanup gates passed. The whole run remains FAIL for profiling.
Detailed evidence: https://github.com/lovitus/EasyTier/issues/4#issuecomment-5824616876

The failed recording contains metadata but no PERF_RECORD_SAMPLE records.
The recovery uses perf's documented fd control with --delay=-1, enable/ack
before load and stop/ack after load, rather than signal termination. Exit 0
and actual samples for both Core PIDs are required; 255 is not accepted.
The original recording is not repaired or reclassified as successful.
Reference: https://github.com/torvalds/linux/blob/v6.8/tools/perf/Documentation/perf-record.txt

The profile-only job checks the archived binary and probe hashes. The probe
source comes from the current checkout and must match the old source hash;
it is not compiled again. The Core and probe executable bytes are unchanged.
Profiled rates remain excluded from throughput comparison.

## Established evidence: do not restart the baseline investigation

The earlier claim that no upstream comparison or Stealth A/B existed was
incorrect. The parent issue already records both experiments:

- Release comparison: https://github.com/lovitus/EasyTier/issues/4#issuecomment-5691724544
  Upstream 2.6.4 measured 810.36/813.77 Mbps and 32.80/32.44 two-Core
  CPU seconds/GiB; fork 3.0.16-4 with Stealth disabled measured
  919.45/914.25 Mbps and 28.88/28.48. Each direction has three samples.
  These are same-host namespace measurements, not WAN evidence. The
  releases have different TUN offload backends and are not an identical
  toolchain/source causal comparison. They do not show a general fork
  throughput regression in that workload.
- Stealth intervention: https://github.com/lovitus/EasyTier/issues/4#issuecomment-5692616218
  On the same fork artifact, enabling Stealth reduced throughput by
  10.11/10.29 percent and increased CPU/GiB by 13.67/12.95 percent.
  This does not explain all mesh processing cost and is not a recommendation
  to disable security. It is a separate experiment, not additional samples
to pool into the release comparison.
- Existing profiles: https://github.com/lovitus/EasyTier/issues/4#issuecomment-5699311395
  Cost spans kernel UDP/IP, crypto, peer receive, queues, TUN and routing.
  Short samples do not establish a new busy loop or justify optimizing
  a small hash-function percentage. Do not add inclusive stack percentages.

Higher end-of-transfer RSS was observed in the fork release comparison.
Sustained memory growth, a leak and its cause have not been established.
Neither a snapshot nor VmHWM alone can establish them.

The remaining question for this batch is the actual Core saturation gain
from the already-built GSO candidate and the remaining endpoint costs.
Do not rebuild upstream or restart the release/Stealth comparisons without
a new conflicting observation. Do not expand GSO error-branch work before
establishing this benefit. No result is claimed for the new harness yet.

## Exact input and scope

The intended reuse of artifact 10678889726 from run 35692418170 is not
available: the artifact API returns 404 and that run's artifact listing is
empty, despite repository read/write access. Run 35114295729 also lists no
artifacts. Do not dispatch a workflow with these unavailable inputs.

A retained older archive identifies harness e87186d8b2ebfd02652e12d64c9a00d7ce37bb08
and SHA256 579afb2bbd2928a99805cb5b6cc119580b793ae412e94b6bc524c232c2a4c558.
The source comparison confirms it lacks the dedicated shutdown metric files
added later. It is not substituted for the intended candidate, and the
observation gate is not weakened to reuse it.

Consequently one rebuild uses the existing Core flush workflow, unchanged
Core base c6772dbfef2395ff96b39bd4801945d92212dffb and current diagnostic
overlay. The workflow's saturation input selects the new measurement batch
instead of repeating the fixed-load matrix. It preserves the eight existing
contract tests and records new binary identities. Old CPU savings remain
historical evidence, not results for these rebuilt artifacts.

Both endpoints are network namespaces on one GitHub-hosted Linux runner.
The underlay is a veth pair, IPv4 UDP, with unchanged AES-GCM, MTU and
offload settings from the fixed-load experiment. This does not establish
physical-host, WAN, IPv6, relay, TCP-transport or cross-platform performance.

Measure direct veth first, then stock brackets and three interleaved legacy
and GSO samples in both directions. Each transfer is an unpaced 1 GiB with
a bounded deadline. Preserve integrity, UDP echo, ICMP progress, actual
transport, diagnostic activation and cleanup checks. Failure is not PASS.

Record per-Core CPU and RSS before/after, plus each process's lifetime VmHWM.
The high-water marks include startup and earlier work in that round; their
sum is not a simultaneous memory peak. This does not measure allocations
or prove absence of a leak. Whole-host CPU includes the load generator.

Run perf separately against both Core PIDs. Profiled rates must not enter
throughput comparisons. Preserve raw data and per-PID symbol reports.
If perf is unavailable or denied, report that gap rather than treating an
empty profile as evidence. Direct-veth throughput and runner CPU capacity
must be considered before attributing a limit to Core.

The report must distinguish send syscalls, receive work, crypto, copying,
allocation and scheduling from actual samples. GSO savings alone do not
prove the overall bottleneck is solved. No production fallback, wire,
routing, TUN or crypto change is authorized by this diagnostic.
