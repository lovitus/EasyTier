# Pure mesh bottleneck diagnosis

Current cursor: run 36074709548 completed the uninstrumented saturation lane.
Profile recovery run 36080205599 passed with the same executable bytes:
2,568 CPU samples, both endpoints represented in each capture, no reported
sample loss, and clean namespace/process teardown. The original failed
profile remains failed. TUN observation run 36081104653 also passed, reusing
artifact 10841125115 for four bounded 64 MiB transfers without a rebuild.
Run 36082689922 has now completed the isolated actual-Core comparison:
8 KiB head capacity improved throughput and CPU/GiB over capacity zero.
Current gate: replay 36086239105 failed in both capacity-zero legacy and
8192/GSO cases. Loss is not established as a head-capacity regression.
Next step: correlate receive-ring rejection with missing ICMP sequences;
do not change buffering, retry policy or the zero-loss assertion first.
This Linux fixture is not full acceptance.
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

## Actual-Core bounded-head experiment prepared

The disposable-source overlay compares capacities 0/4096/8192 in one
candidate binary, with the previous UDP GSO diagnostic unchanged. The stock
binary and unpaced load generator are reused from artifact 10841125115;
only the new diagnostic Core is built. Nine interleaved rounds provide three
samples per capacity and direction with the existing 1 GiB workload.

The overlay keeps one reusable allocation per TUN sink, promotes at most one
eligible TCP frame per naturally available multi-packet flush, and skips
singletons and frames already large enough. It never waits to form a batch,
changes packet ordering before writes, or changes receive/crypto/routing.
Allocation recovery follows its pointer after GRO swaps rather than selecting
an arbitrary large input. Counters record cohort sizes, copied bytes, expanded
batches, maximum frame and lost scratch; all three modes include counters.
Expanded-batch counters are conservative (larger than the original maximum),
not exact output packet counts. Cancellation/teardown remains existing sink
ownership, not a new runtime. Missing metrics or lost scratch fail the run.

Failure modes being tested: no useful cohorts, copy cost exceeding syscall
savings, payload corruption, stuck writes/ICMP, resource residue, and buffer
reuse failure. The existing integrity, UDP echo, transport, metric and cleanup
checks remain mandatory. No result or production acceptance is claimed yet.

Current execution: run 36082689922, harness 837349f8, dispatched with
`tun_capacity=true`. The bounded 600-second status wait returned timeout,
not a workflow failure; no terminal result has been obtained. Keep this
same run/artifact identity on continuation. Do not dispatch a duplicate or
claim throughput/CPU results before the existing run completes.

## Actual-Core result: bounded head capacity benefits this workload

Run https://github.com/lovitus/EasyTier/actions/runs/36082689922 completed
successfully at harness `837349f8f5031d5c877b90edb6f195b6f9615af9`.
Core base remains c6772dbf plus the isolated UDP and TUN overlays. Candidate
Core SHA-256: `d5a10eedde39a37913db9d4a7d8f35d18848d9451cf93f6d57c1085bb42b33ca`.

Both endpoints are isolated client/server namespaces on the same GitHub
Ubuntu 22.04 runner: AMD EPYC 7763, Linux 6.8.0-1064-azure. Underlay veth,
IPv4 UDP mesh, AES-GCM, Stealth off, compression none, inner TUN MTU 1360.
This is neither two physical hosts nor a WAN/10 GbE benchmark. All modes
use outer UDP GSO and the same candidate bytes, with only head capacity
changed. Nine interleaved rounds, one unpaced GiB per direction per round;
three samples per capacity and direction, without perf/trace instrumentation.
The common diagnostic counters are enabled in every mode.

| Capacity | Upload samples Mbps | Download samples Mbps | Median up/down Mbps | Median two-Core CPU s/GiB up/down |
| --- | --- | --- | --- | --- |
| 0 | 1370.55, 1370.89, 1381.62 | 1123.16, 1379.37, 1405.52 | 1370.89 / 1379.37 | 17.91 / 17.52 |
| 4096 | 1431.93, 1458.65, 1461.01 | 1442.44, 1466.33, 1450.73 | 1458.65 / 1450.73 | 17.04 / 17.02 |
| 8192 | 1508.54, 1507.80, 1555.20 | 1574.61, 1564.65, 1564.27 | 1508.54 / 1564.65 | 16.21 / 15.75 |

Relative to capacity zero, 4 KiB throughput improved 6.40/5.17 percent;
8 KiB improved 10.04/13.43 percent with CPU/GiB down 9.49/10.10 percent.
The low baseline download sample is retained, not discarded. Do not combine
these medians with another runner's earlier GSO results as a new measured
combined speedup.

Across each mode's six endpoint lifetimes (including functional checks):

| Capacity | Flushes | Singleton flushes | Mean cohort | Promoted heads | Expanded batches | Copied bytes | Maximum frame | Scratch lost |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 400865 | 41682 | 15.30 | 0 | 0 | 0 | 1370 | 0 |
| 4096 | 401879 | 33889 | 15.11 | 367990 | 247443 | 350608056 | 4006 | 0 |
| 8192 | 441706 | 35561 | 13.26 | 406145 | 294984 | 416512406 | 8022 | 0 |

Thus natural cohorts exist; this workload was not restricted to singleton
flushes. Capacity promotion produced larger frames and improved measured
CPU/throughput despite copying. Expanded batches are not output-write counts.
The storage addition is one 4/8 KiB reusable buffer per experimental TUN sink,
not per TCP connection. End-RSS and lifetime high-water sums stayed around
60-62 MiB across modes; differences are not a demonstrated memory reduction
or long-duration leak result. High-water sums are not simultaneous peaks.

Eight existing UDP contract tests, 18 transfer/digest/half-close lanes,
nine UDP-echo checks, ICMP progress, actual transport/activation, 18 TUN
metric files and all nine cleanups passed. Routes were unchanged in all
nine runs; no scratch was lost. The exact candidate has not yet established
Stealth-enabled, mixed-flow/interactive, physical-host, IPv6 or long-running
acceptance for this receive change. Production source remains untouched.

Artifact 10842834881 is 130825152 bytes. Published outer SHA-256:
`04aadac13d18425a73d26c9cde857df03a93e17b42d606de10589a18a12b1fc0`.
Only selected evidence was downloaded by byte range (539584 bytes); all 34
selected files matched the internal manifest. The full outer ZIP was not
locally downloaded/rehash-verified. Raw files remain in private evidence.

Exact-artifact compatibility follow-up uses `profile_only=true,tun_compat=true`
with artifact 10842834881 and no Core compilation. It covers the Cartesian
product of capacity 0/8192, Stealth off/on, UDP legacy/GSO: eight endpoint-pair
lifetimes and sixteen 256 MiB directional transfers. Existing integrity,
UDP echo, ICMP progress, Stealth activation, scratch recovery and teardown
checks remain unchanged. Short compatibility measurements are not three-run
performance evidence. IPv6, mixed-flow and physical-host claims remain open.

### Exact-artifact compatibility result

Run https://github.com/lovitus/EasyTier/actions/runs/36085051623 passed at
harness `32ac890f64c8cc99de82be1244eb90ddc1ed268d`, reusing the exact
837349f8 candidate artifact without compilation. Capacity 0/8192 x Stealth
off/on x legacy/GSO completed all eight combinations: sixteen directional
256 MiB transfers and digest/half-close checks, eight UDP echo checks,
ICMP progress, thirty-two peer observations, sixteen TUN metric records,
and all process/namespace cleanup. All four root-route comparisons matched.
Stealth-on writer metrics explicitly observed enabled outer sealing; it was
not inferred solely from configuration. No scratch allocation was lost.

The 8 KiB mode expanded batches with and without Stealth, in both legacy
and GSO modes. The zero-capacity control never promoted or expanded frames.
These short runs close this combination check, not three-sample performance
acceptance, mixed-flow fairness, IPv6, physical-host or duration testing.

Artifact 10842419482 is 127160 bytes. Its full archive SHA-256 and all 235
internal manifest entries were verified:
`bf99b3318416af043dea33186240914787b0105b26a5fb510d4381a076ddf8d6`.
Current next step: exercise the existing candidate on inner IPv6 and mixed
flows using bounded fixture extensions, not another Core build. No production
adoption or release has been made.

Extended compatibility preparation: reuse the same artifact with
`profile_only=true,tun_compat=true,extended_tun=true`. Core's existing
`--ipv6` option assigns inner ULA addresses; no manual TUN address injection
or production changes. Underlay is still IPv4. Both IPv4 and inner IPv6
run opposite-direction TCP transfers concurrently on separate ports, with
each result checked and CPU normalized by both payloads. Existing digest/
half-close and UDP echo use the selected address family. ICMP progress and
all prior cleanup/activation gates remain. This is a short coexistence
check, not a latency fairness bound, IPv6-underlay or durability proof.

### Extended compatibility failed: ICMP loss remains an adoption gate

Run 36085716874 at 5a1e4d6a failed without relaxing any assertion. Completed
capacity-zero IPv4/inner-IPv6 mixed cases with Stealth off/on, and capacity
8192 IPv4 Stealth-off mixed case. Capacity 8192, Stealth off, inner IPv6,
GSO failed ICMP progress: sequence 9 was missing (19/20 replies, 5% loss)
during concurrent opposite-direction 256 MiB transfers. Both bulk transfers
completed. The preceding legacy round passed; remaining cases did not run.
This is NOT complete extended acceptance and NOT a proven causal regression
from head capacity based on one observation. Do not publish this candidate.

Failure cleanup was clean; scratch_lost and pending_on_drop were zero for
both endpoints. Existing logs/metrics do not establish the packet-loss layer.
Preserve exact binaries and unchanged zero-loss assertion for six interleaved
0/8192 replays with per-namespace protocol and interface counters. Each failed
invocation remains failed; replay aggregation must exit nonzero if any fails.
This diagnostic does not increase socket buffers or modify production code.

Artifact 10844195055 complete archive digest and 387 internal files verified:
`04072d757fd310d9fdf2dcce79c07a8bf37901e5c161bde01645d027177bb370`.

### Bounded replay: loss also occurs without either optimization

Run 36086239105 reused the exact executable from run 36082689922; no
rebuild. Harness 0396b444 interleaved capacities 0,8192,8192,0,0,8192,
with legacy then GSO, inner IPv6, Stealth off and concurrent opposing
256 MiB transfers. The aggregate remains FAIL, preserving every assertion.

- Replay 0, capacity 0, legacy download: 2/20 ICMP replies missing.
- Replays 1 through 4: passed their selected checks.
- Replay 5, capacity 8192, GSO download: 1/20 ICMP replies missing.

Both failing intervals have zero delta in UDP RcvbufErrors and interface
error/drop counters. Veth transmit/receive byte and packet deltas match
between endpoints. Receiver ICMP echo-reply counts equal received requests,
but received requests are fewer than sender requests. These aggregate
counters narrow the investigation; they do not locate an individual lost
packet. Ping starts before the counter snapshot, so counter deltas cover
19 requests rather than the complete 20-packet ping.

A passing replay-5 legacy upload has 21 UDP RcvbufErrors but zero ICMP loss.
Therefore neither absence nor presence of that counter alone explains the
failed ICMP gate. No performance comparison may treat these failed runs as
accepted compatibility evidence.

Source inspection identifies an existing candidate loss boundary:
UdpConnection::handle_packet_from_remote uses RingSink::try_send for
ZCPacket::is_lossy(), which classifies PacketType::Data rather than the
inner transport protocol. RingSink rejects at the reserved-capacity
boundary before total capacity is exhausted. This is a hypothesis needing
packet-correlated evidence, not permission to remove bounded queues or
change prioritization. Production source remains unchanged.

Artifact 10843962028 complete archive digest and 523 internal entries verified:
`c5fa8f28818c243ec1a56760c97d1ffc2a5f124085edbbdf9ee08f9037815b2b`.

### Sequence capture outcome: observation readiness defect, no reproduced loss

Run 36087085329 at 2ac127bb reused the same Core executable. Aggregate FAIL:
first capacity-zero replay stopped at `ICMP capture not ready`, with empty
capture logs. The new harness checks readiness without waiting for the
capture-ready event; its existing UDP warmup is not a readiness guarantee.
This is a harness defect, not Core failure. Do not retry unchanged blindly.

The other five cases completed both modes/directions (20 transfers). Both
TUN endpoints captured matching ICMP IDs and sequences 1..20 for all of
these transfers, with zero capture-kernel drops. Original loss did not
reproduce under observation; this neither clears the earlier gate nor
proves receive-ring rejection. Capture rates are not performance evidence.

Artifact 10844366449 and 533 internal entries verified; full SHA256:
`6fc23ac38c0c8e3c76d0877c412527d651b90de9a888c19c5f7c84ab1e0af5eb`.
Next prerequisite: bounded event-driven capture readiness, preserving
failure/cleanup evidence and the original zero-loss assertion. No production
buffer or priority change is supported by these observations yet.
