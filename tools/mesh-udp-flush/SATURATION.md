# Pure mesh bottleneck diagnosis

Current cursor: run 36074709548 completed the uninstrumented saturation lane.
Profile recovery run 36080205599 passed with the same executable bytes:
2,568 CPU samples, both endpoints represented in each capture, no reported
sample loss, and clean namespace/process teardown. The original failed
profile remains failed. TUN observation run 36081104653 also passed, reusing
artifact 10841125115 for four bounded 64 MiB transfers without a rebuild.
Next step: distinguish receive-buffer capacity, naturally available batch
size and GRO eligibility before proposing any production receive change.
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

### Exact candidate TUN observation

Run: https://github.com/lovitus/EasyTier/actions/runs/36081104653
Harness: 085bef3ac302daf11a2a8c2d6396f8b316526e8f.
The same GitHub runner hosts client/server namespaces connected by a veth;
IPv4 UDP mesh, AES-GCM, Stealth disabled, inner TUN MTU 1360 with a 10-byte
virtio header. Each direction transfers 64 MiB. No physical-host claim.

| Mode | Direction | Receiver successful write calls | Writes above 1370 bytes |
| --- | --- | ---: | ---: |
| legacy | upload | 51317 | 0 |
| legacy | download | 51315 | 0 |
| GSO | upload | 51318 | 0 |
| GSO | download | 51314 | 0 |

All recorded TUN writes returned their requested byte count. Both endpoints'
thread and TUN fd identities remained unchanged; trace overrun, commit
overrun and dropped-event counters were zero. All entry/exit pairs selected
for TUN writes were matched. Four transfer/trace lanes, integrity, UDP echo,
ICMP, diagnostic activation and cleanup passed; root routes were unchanged.
Twenty cleanup records were clean, with no forced kills.

Artifact 10841254098, complete ZIP SHA-256:
`7df04bc5a002cac1d68989b07dd0aee7c62f138df1629b7eee35a571f39878cb`.
The complete digest and all 84 internal manifest entries were checked.
Raw evidence remains outside Git. Instrumented rates are not performance
acceptance data and must not be pooled with the earlier saturation samples.

This supports a concrete remaining receive-side target: this workload did
not produce larger TUN writes even with outer UDP GSO enabled. It does not
establish why GRO failed to combine packets or promise a gain from larger
buffers. Keep the prior large-scratch regression as a negative result.

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

### Capacity mechanism and historical fixture correction

Exact Core base c6772dbf: both UDP receive loops use `buf.split()`;
bytes 1.9.0 `split_to(len)` sets the returned capacity to `len`.
Ring decrypt truncates the AEAD tail in place. The TUN sink then slices off
outer headers, without reserving space. A full-size frame therefore has
only the short AEAD tail as spare capacity, not the receiver allocation's
8 KiB or the old standalone probe's assumed 4 KiB per packet.

Locked tun-rs 2.8.7 refuses TCP coalescing when the destination capacity is
insufficient; it deliberately does not allocate. Thus capacity is a concrete
barrier on this path, even when several eligible segments form a batch.
This does not establish actual cohort sizes or quantify a safe fix's gain.

The previous 4 KiB Vec fixture is a synthetic model, not a faithful model
of UDP split-slice ownership. Its historical timing results remain recorded
but cannot prove the production capacity behavior. A new capacity-only mode
reuses its valid TCP generator and the locked GRO implementation, holds
bytes/order/cohort constant and varies only the head buffer capacity.
It checks output payload length conservation and merge/no-merge behavior
for 1/2/4/8/32 segments. It is not a byte-integrity or throughput acceptance
claim, not a production patch, and not permission to restore the reverted
64 KiB scratch optimization. Results are pending the small-tool CI run.

Capacity-only run 36081657966 at a94cfe7f did not execute the experiment.
The new job omitted mold although inherited Cargo configuration passes
`-fuse-ld=mold`; dependency build-script linking failed with exit 101
(`collect2: fatal error: cannot find ld`). This is a harness prerequisite
failure, not a failing GRO contract. Original artifact and logs are retained
privately. Do not claim the capacity matrix passed. The required harness
repair is installing mold as the existing kernel-mechanism job does.

Follow-up run 36081873242 at cc63dc75 installed mold successfully, but
compilation then rejected the new probe's direct access to GROTable.to_write
(E0616, private field). No matrix rows ran. The dependency exposes apply_gro,
but not its emission-index list; do not change dependency visibility to make
the probe compile. The existing tool observes expanded frame lengths instead.
A repair should assert merged head length and payload bytes through public
buffers, without reporting inferred emission indices as measured output.
Both failures are harness errors, not negative production GRO results.

### Capacity contract completed

Run https://github.com/lovitus/EasyTier/actions/runs/36082188054 passed at
`d9db50e4d95ae8bcf3a1d2ae98f4606b5d744b81`. All 20 cohort/capacity combinations
executed through public apply_gro and buffers. The fixture uses 1,320-byte
TCP payloads, valid IPv4/TCP checksums, ordered same-flow ACK segments and
28-byte modeled AEAD tail capacity. It uses Vec with an exact capacity to
model the source-derived slice bound; it does not execute bytes 1.9.0 or
Core encryption and is not a full packet-path or cross-platform test.

| Head capacity | Maximum observed segments in head, cohort up to 32 |
| --- | ---: |
| 1398 bytes (1370-byte frame plus 28-byte tail) | 1 |
| 4096 bytes | 3 |
| 8192 bytes | 6 |
| 65545 bytes | 32 |

Singletons stayed singletons. Merged head payloads exactly matched the
corresponding concatenated original payloads; the other input payloads
remained intact. These are observed head sizes, NOT a measured output-frame
list or throughput result. No dependency visibility was changed.
Artifact 10842292695 complete archive digest was verified:
`6ef1cf6fe7012f776ef8e66708db4887b65df64b3ff5cb21f854d5f8aeafd597`.

Current next step: one isolated actual-Core comparison of unchanged versus
bounded head capacity, with natural cohort histogram, expanded-frame counts,
uninstrumented throughput/CPU and cleanup. Prefer testing 4/8 KiB bounded
reusable storage before considering larger buffers. Preserve singleton
behavior and do not add sleeps/yields, change ordering or modify crypto.
The old 64 KiB scratch regression is still a reason to require actual-Core
A/B evidence before adopting anything. The earlier small-tool failures remain
failures; this successful run does not retroactively reclassify them.
