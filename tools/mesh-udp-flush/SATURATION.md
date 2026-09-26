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

### Readiness repaired; successful workload is not complete capture evidence

Run 36087535015 at 81febf47 passed six suites and 24 directional mixed-load
transfer checks using unchanged Core bytes. Capture startup now waits for
an explicit listening event with a ten-second bound. No Core rebuild.

Independent artifact inspection found a separate tail-completeness gap:
seven of twelve paired captures contain 78 records, despite tcpdump reporting
80 received by filter and zero kernel drops. Both endpoints omit only the
last ping's sequence-20 request/reply; ping itself received all 20 replies.
The other five paired captures contain all 80 records. Observed keys match
between endpoints, with maximum observed RTT 2.548 ms, but truncated traces
cannot establish complete per-packet coverage or a latency upper bound.

The original intermittent loss did not reproduce. Neither CI success nor
zero capture-kernel drops clears the previous failed gate. Before further
loss replay, require capture drain/completeness rather than a fixed delay.
Do not infer a Core queue repair from this observer defect.

Complete archive SHA256:
`2d5bdc00f6910c9a534d787294340dba41ff6e8299cdeb0e79539092c712a3cc`.

### Complete capture reproduces reply-path loss

Run 36087998291 at d85f6770 retains FAIL. The first five suites completed
with complete 80-record paired captures. The sixth suite, capacity 8192,
legacy mode, Stealth off, inner IPv6 mixed upload, reproduced 1/20 ICMP loss.
This is not an outer UDP GSO-only failure: GSO calls are zero in this case.

ICMP ID 24162 sequence 3 is captured as a request at both TUN endpoints.
Endpoint 1 captures its reply at timestamp 1790304904.664075, but endpoint 0
never captures that reply. Endpoint 1 has 40 records, endpoint 0 has 39;
both report equal captured/filter-received counts and zero capture-kernel
drops. The original ping assertion fails. This establishes a missing reply
between peer TUN egress and local TUN ingress, not failure to generate the
reply and not the previous capture-tail defect.

Neither endpoint reports an increment in UDP receive-buffer errors during
this transfer. Both writers have zero GSO calls, EAGAIN and pending-on-drop;
TUN scratch_lost is zero. These aggregate counters do not distinguish Core
receive-ring rejection from other mesh processing or TUN delivery failures.
Do not change queue capacity/backpressure based on this alone.

Artifact 10843988454 and 570 internal entries verified; complete SHA256:
`420fa84617c4b55cb5800db01c430e54aa8c8c5da765331bfef7880cd743cd68`.
Next measurement should observe the existing receive-ring rejection boundary
and correlate a lost packet across Core stages, rather than repeat unchanged
captures. Production adoption remains unapproved; prior measured CPU and
throughput improvements remain fixture-specific evidence, not acceptance.

### Packet-stage diagnostic candidate in progress

Commit dd061adb adds bounded, opt-in observations only to disposable CI source:
ICMP identity before encryption, ciphertext payload fingerprint after encryption,
UDP receive / ring rejection / peer receive fingerprints, and decoded ICMP
identity before NIC enqueue. Queue behavior and packet contents are unchanged.
Per-process output is capped at 8192 records; overflow invalidates evidence.
The fixture is inner IPv6 echo (104 bytes) with the pinned 28-byte AES-GCM tail.
This is not a general protocol classifier or production feature.

Run 36088594844 builds this candidate once, reuses stock/load-generator artifacts,
and then executes the bounded interleaved localization batch. Local syntax,
format and workflow gates passed. The authorized status helper reached its
600-second observation timeout; it did not report a terminal workflow failure.
Unique next step: resume status observation of run 36088594844 and inspect its
artifact. Do not redispatch or rebuild because this observation timed out.

### Packet-stage evidence confirms receive-ring rejection for one lost echo

Run 36088594844 at dd061adb built the diagnostic candidate successfully;
eight existing contract tests passed. The localization batch remains FAIL:
trace-2-8192 legacy download loses ICMP echo ID 47227 sequence 10.

Correlated records for ciphertext payload fingerprint `2328586c6fc92eb0`:

1. Endpoint 0: encrypted_tx, plaintext identity (128, 47227, 10).
2. Endpoint 1: udp_rx with the same fingerprint.
3. Endpoint 1: ring_reject with the same fingerprint.
4. Endpoint 1: no peer_rx or nic_enqueue for that echo; no TUN delivery.

The sender TUN captures the request. Receiver TUN does not. Captures report
79/78 records respectively, matching their filter-received counts, and zero
capture-kernel drops. Neither Core trace overflows. This sample directly
identifies existing UDP receive-ring rejection, not a NAT, network-interface,
TUN kernel, or reply-generation failure. It does not retrospectively prove
the cause of every earlier uninstrumented loss.

The exact source uses a 128-slot receive ring with four slots reserved for
non-Data traffic. is_lossy classifies all PacketType::Data, not just inner
UDP. PeerConn consumes that ring and awaits a separate 128-slot MPSC send;
PeerManager then performs decrypt, metrics, decompression, ACL/pipeline and
NIC delivery. This bounds the next performance question: service rate and
backpressure along that chain, not ICMP-specific prioritization. Do not
remove either bound, silently enlarge buffers, or classify encrypted payload
as ICMP to make the test pass. Diagnostic throughput is not acceptance data.

Artifact 10845561111: 57 selected evidence files verified against its internal
manifest; only 688005 ZIP-range bytes fetched. Full archive digest was not
locally verified. Raw selected files and retrieval provenance remain private.
Unique next step: reconcile prior receive-path experiments with the now-proven
ring bottleneck, then choose a bounded scheduling/consumption experiment that
measures throughput, CPU, timer/Pong fairness and loss together. No production
rollout or extra replay of the unchanged diagnostic is justified.

### Receive optimization selection after source reconciliation

Do not replay the rejected writer-side yield or large 64 KiB scratch approach.
The natural-cohort probe models a synchronous I/O owner, not Core's receive
ring -> PeerConn -> MPSC -> PeerManager -> NIC chain. Its successful kernel
mechanism results do not establish receive-task scheduling correctness.

Upstream 86222771c59652bcce0098b5f9eaf19dbfa2c988 changes host egress from
awaited send to try_send/drop. That protects shared-router liveness but moves
the drop boundary; it is not a demonstrated bandwidth/CPU fix and should not
be adopted just to remove receive-ring rejections from this experiment.

Dependency reconciliation found Core Cargo.lock at Tokio 1.52.1, while the
existing standalone probe lock uses 1.53.1. A new scheduling comparison must
use the Core version; previous probe results must retain their own version.
Inspection of the local exact Tokio 1.52.1 source confirms recv() consumes
cooperative budget and returns one semaphore permit per item. recv_many()
consumes one budget unit per bounded batch and returns permits together.
This is a concrete mechanism to test, not proof of gain. An unbounded
try_recv loop would bypass this fairness contract and is not the proposal.

The next small-tool experiment should compare recv() against recv_many(8)
and recv_many(32), using the same bounded queues, packet order and payload
work, with explicit per-item/per-batch budget alternatives. Measure timer
and control-packet latency, loss, throughput and CPU together; test both
single-thread and multi-thread scheduling. Report extra locally held batch
capacity rather than claiming memory bounds are identical. No Core edit
until this mechanism has evidence; no extra full Core build for the model.

### Locked receive scheduling model: modest mechanism gain, loss remains

Run 36091355557 / 7dc84a0c passed the standalone model's order, payload and
accepted/delivered accounting assertions in 24 trials. Core was not built.
Tokio 1.52.1, async-ringbuf 0.3.1 and futures 0.3.30 follow the Core lock.
The model has a 128-slot ring with four reserved slots, a 128-slot MPSC,
synthetic per-packet CPU work and a one-million-packet unpaced producer.
It is not actual UDP, crypto, NIC throughput or a no-loss acceptance test.

Three-run medians, units million delivered packets/s and CPU s/million:

| Runtime | Consumer | Goodput | CPU | Data drops / million offered | Control p99 us |
| --- | --- | ---: | ---: | ---: | ---: |
| current-thread | recv | 2.568 | 0.392 | 31248 | 8 |
| current-thread | batch8 | 2.664 | 0.372 | 31248 | 7 |
| current-thread | batch32 | 2.676 | 0.372 | 31248 | 6 |
| current-thread | batch32 + per-item budget | 2.689 | 0.372 | 31248 | 6 |
| two-workers | recv | 3.209 | 0.861 | 755914 | 69 |
| two-workers | batch8 | 3.377 | 0.844 | 705462 | 54 |
| two-workers | batch32 | 3.347 | 0.771 | 668590 | 51 |
| two-workers | batch32 + per-item budget | 3.297 | 0.812 | 678507 | 53 |

All trials delivered reserved control messages; maximum observed control delay
was 89 us. Timer p99 values are about 1.0 ms and maximum observed overshoot
1.961 ms. Short trials and process CPU tick quantization limit precision;
these are observations, not production fairness bounds or proof of a stable
10-percent CPU reduction. Local batch storage is 1280/5120 bytes for 8/32
model packets, versus 160 bytes for one model packet, excluding Vec metadata.
Real ZCPacket payload retention would differ and must not use these byte sizes.

No arm eliminates ring loss. In single-thread mode the identical drop count
reflects the model's producer budget versus reserved-ring boundary; in the
multi-thread mode the unconstrained producer overwhelms the chain. This is
not a prediction of Core loss rates. The roughly 4-5 percent goodput change
is a modest mechanism signal, not sufficient evidence for a production patch
or a claim that batching solves the proven Core rejection. A paced offered-load
comparison and longer CPU measurement are needed before selecting this change;
do not trigger a full Core rebuild based only on these short saturation trials.

Artifact 10845801728 complete ZIP digest verified:
`d6cd257b5f2828b1f04e51fb7b12ec544a1dc51c25c6c1a06b0400c17fd2ae20`.

### Longer and paced scheduling comparison: do not adopt recv_many now

Run 36091675062 / c5c3af4d completed 48 model trials (three repetitions,
four consumer modes, two runtime types, saturated and pulse64 supply).
Saturation uses five million offers. Pulse64 waits at least one millisecond
between 64-packet offers without catch-up; measured actual supply is about
31k packets/s, NOT a claim that a 64k/s network rate was achieved. Accepted
packet order/payload/count assertions passed. No Core was built or changed.

Within this run, current-thread saturated batch8/32 retains a roughly 4-5%
goodput benefit. On two workers, recv is 2.380M delivered packets/s versus
2.353M batch8 and 2.266M batch32. CPU s/million is 1.234 versus 1.250/1.288.
These reverse the earlier short-run direction; do not pool different runner
runs or report only the favorable samples. At pulse64 supply, all modes have
zero data/control drops but no distinguishable CPU saving: current-thread
medians 0.547 CPU s/million throughout, two-worker recv/batch8 both 1.329.
CPU tick granularity still limits low-duty estimates.

Decision: do not introduce recv_many or change production cooperative budget
based on this model. It has not shown a robust multi-thread performance win
and does not resolve overload rejection. Stop expanding this model just to
obtain a favorable result. Existing GSO and bounded TUN-head gains are separate
actual-Core experiments and are neither confirmed nor invalidated by this
negative scheduling result. The exact receive-ring drop evidence remains.

Artifact 10845887193 complete ZIP SHA256 verified:
`9ebf1823f53ff47864ded0d69e1f808e69948ed28fe97d76f03f55c7614f5389`.
Next performance investigation should return to measured actual-Core hot-path
costs, preserving current queue bounds and control-plane behavior, rather
than treating loss relocation or a synthetic batching win as a Core repair.

### Actual-Core self-cost reconciliation after the scheduling rejection

Revisited the legacy upload/download reports from run 36080205599, without
new builds or traffic. Sum only the top-level self-overhead rows, never the
indented inclusive call chains. The displayed rounded rows sum to 99.03%
(upload) and 98.66% (download), so these are approximate reported shares,
not exact counts or exhaustive attribution.

| Self-cost grouping | Upload % | Download % |
| --- | ---: | ---: |
| Kernel symbols | 56.32 | 54.55 |
| Non-kernel symbols | 42.71 | 44.11 |
| Recognizable crypto symbols | 5.80 | 5.62 |
| Recognizable allocation/copy symbols | 2.82 | 2.36 |
| Recognizable async/scheduler symbols | 9.86 | 12.63 |
| Socks5Server filter symbols | 2.42 | 1.48 |

Only kernel/non-kernel form a partition. The other labels are conservative
symbol-name buckets, can overlap, and miss inlined or unresolved work.
Kernel share includes transport, TUN, scheduling and other kernel work;
it is NOT equivalent to the cost removable by batching UDP receives.
These samples do not justify changing crypto or removing the SOCKS filter.
The inspected filter also has a stale-entry-count safeguard; eliminating
its map check merely because it appears in a profile is not safe evidence.

Next bounded mechanism experiment: compare nonblocking single-datagram
receive with bounded recvmmsg at the socket boundary, not recv_many at the
MPSC boundary. Preserve datagram boundaries, source addresses, truncation
reporting, queue capacity and control-plane policy. Report actual batch
occupancy, syscall count, delivered bytes, loss, CPU, idle wakeups and
control/timer latency at paced and saturated supply. Keep this in a small
standalone tool before considering a Core overlay. No production edit is
approved by this cost grouping alone, and no new throughput claim is made.

Reference: https://man7.org/linux/man-pages/man2/recvmmsg.2.html
Use nonblocking readiness rather than the recvmmsg timeout argument: the
manual documents that timeout checking can leave a partially filled batch
blocked indefinitely. Bound each receive drain and preserve cancellation;
do not wait for a batch to fill. This mechanism is Linux-specific and must
not be presented as a cross-platform fix. Reuse existing GSO/TUN evidence
separately and retain the receive-ring rejection as an unresolved overload
behavior, not evidence that receive syscall batching fixes it.

### Real UDP receive syscall experiment: insufficient benefit for Core edits

Run 36092642667 / c93dbbcc380581f03f18b76cb2125471fd91502f succeeded.
This compiled only udp_receive, not Core. Eighteen trials compare recvmsg(1)
with nonblocking recvmmsg(8/32), three repetitions in alternating order,
with two-second saturated or pulse64/1ms supply. Two threads exchange real
loopback UDP datagrams: 1400-byte data and 64-byte control markers. Every
received datagram checks length, source, sequence and payload. Receive CPU
uses CLOCK_THREAD_CPUTIME_ID; timing includes validation and drain overhead.
All arms report the same effective SO_RCVBUF of 524288 bytes.

Three-trial medians:

| Supply | Max batch | Delivered Mbit/s | Receiver CPU s/GiB | Packets/nonempty syscall |
| --- | ---: | ---: | ---: | ---: |
| saturated | 1 | 7080.52 | 0.8617 | 1.000 |
| saturated | 8 | 7088.26 | 0.8837 | 1.353 |
| saturated | 32 | 7008.67 | 0.9082 | 1.339 |
| paced | 1 | 617.65 | 0.9072 | 1.000 |
| paced | 8 | 620.18 | 0.8757 | 1.429 |
| paced | 32 | 619.83 | 0.9354 | 1.325 |

All paced arms lost zero datagrams and control markers. Saturated loss is
NOT zero: batch1 lost 2615/953/2076 data-or-control datagrams; batch8 lost
26/0/0; batch32 lost 1924/510/1853. Control losses were 2/2/1, 0/0/0 and
2/0/2 respectively. Successful execution means integrity/accounting
assertions passed, not loss-free operation. No Core reserved-ring control
policy exists in this model. Control p99 is conditional on delivered
markers, so do not call it guaranteed latency or ignore missing markers.

Batch8 saturated receiver CPU/GiB increased about 2.6%; batch32 about 5.4%.
The paced batch8 reduction is about 3.5%, without enough evidence to justify
production complexity. Low natural occupancy explains why a max batch of
32 does not mean 32 packets/syscall. This single-send loopback workload does
not prove behavior with GRO/GSO or a real NIC. It also does not model Tokio
fairness, encryption, ring rejection, cross-platform behavior or WAN limits.
The 100ms idle prelude yielded five 20ms poll waits in every arm; that is the
explicit tool timeout policy, not evidence of an event-driven Core idle fix.

Decision: no production recvmmsg patch from these results. Avoid further
model tuning merely to obtain a positive result. Existing actual-Core UDP
GSO and bounded TUN scratch gains remain the stronger candidates; their
mixed-load acceptance remains open. A future receive experiment needs an
actual-Core profile or observed receive burst distribution supporting it.

Artifact 10846481166 was only 8569 bytes; full ZIP SHA256 verified:
`23ad45efd0a1647a37c66656f7e213f64c86d44365b12ef965cc85d45d9f2765`.
Source SHA inside artifact matches the dispatched SHA. No large Core matrix
was downloaded. Initial local rustfmt failed because rustup was absent
from PATH; adding the existing HOME/.cargo/bin resolved it, then the normal
pre-commit checks passed. One workflow observation and one artifact metadata
request encountered transient GitHub transport failures; the same run was
observed/retrieved again, never rebuilt or redispatched.

### Original symptom host: current low-load observation, not a load acceptance

On 2026-09-25, a bounded read-only 30-second observation of the original
lab-laptop's already-running Core completed without replacing or restarting
it. It reports 3.0.16-4-391c191c, Linux 6.8.0-40 x86_64. Current executable
SHA256 is `5c3986318f81b4c6fb3df71a8099ed69a5d7590a2ad5af97ddc58d77dc8d838a`.
This differs from the earlier profiling artifact hash; the version string
alone must not be used to claim identical build flags or artifact identity.
No CLI arguments, credentials or private host addressing are published.

perf stat measured 735.25ms task-clock over 30.002576930s (about 2.45% of one
CPU), 7094 context switches, 911 migrations and 198 page faults. Independent
process user+system counters increased by 80 ticks at 100Hz across a 30.05s
snapshot window. These windows differ slightly; do not force identical CPU
numbers. PID and process start tick stayed identical. Threads remained 13.
RSS increased from 48196 to 48832 KiB; one short interval cannot establish
or exclude a memory leak.

TUN deltas were RX 15133 bytes/98 packets, TX 10718 bytes/79 packets, with no
TUN error/drop increment. This is LOW LOAD, not zero traffic: management
traffic and background peers remained active. It does not reproduce idle
full-core usage at this time and does not answer saturated CPU/GiB or the
historical slow-Wi-Fi question. No load, interface change, firewall change,
service kill or deployment was performed. Private raw evidence is retained
outside Git. Current issue comments were reconciled through pagination;
no new independent team approval was present in the latest five comments.

The existing actual-Core fixture exposes no rate-control flag; do not
pretend the standalone paced syscall model validates a paced mesh path.
Next acceptance work must continue with the measured GSO/TUN candidate,
keeping original saturation/zero-loss failures visible, rather than
substituting this quiet production snapshot or unrelated synthetic PASS.

### Next exact-artifact comparison: existing paced fixture, not weaker acceptance

The lab's default path already uses paced_probe.py at 200 Mbit/s. The prior
statement about no rate-control flag did not mean no paced implementation
existed. Reuse that implementation; do not create another load framework.
The standalone sender's IPv4-only socket and unstripped bracketed IPv6
literal prevented using it for the known inner-IPv6 mixed-flow scene.
Only that fixture address-family handling is extended. An optional
--paced-mbps (default unchanged at 200) carries the same cap to both senders
and checks the reported cap. Unpaced scenarios keep their old semantics.

The paced_tun workflow input reuses artifact 10842834881, including its
existing binary checksum checks, with no Core/toolchain rebuild. Compare
200 and 600 Mbit/s PER FLOW, two opposite-direction flows, capacities 0 and
8192, legacy/GSO, three interleaved repetitions. Actual measured rates, not
the nominal cap or their sum, govern interpretation. Keep zero ICMP loss,
content/half-close, UDP echo, peer transport, scratch ownership and cleanup
assertions unchanged. No packet tracing is enabled during CPU measurement.
Original unpaced failures remain FAIL regardless of this additional lane.
No new source-string/mocked tests or red/green workflow framework is added.

### Exact candidate, paced inner-IPv6 mixed load: positive CPU result

Run 36093630841 succeeded at harness 0cb5c1fbc0090fcad683ca52535336dd61c2a6f2.
It reused the previously built Core candidate
`d5a10eedde39a37913db9d4a7d8f35d18848d9451cf93f6d57c1085bb42b33ca`
from artifact 10842834881; no Core was rebuilt. Hardware: one GitHub VM,
AMD EPYC 7763, four visible CPUs. Both endpoints are isolated namespaces
on that VM with veth IPv4 UDP underlay and inner IPv6 TCP. AES-GCM enabled,
Stealth off, TUN MTU 1360. No WAN, physical NIC or cross-platform claim.

12 suites, 24 Core-pair lifecycles, 48 primary and 48 concurrent reverse
transfers completed. Also passed 48 integrity/half-close checks, 24 UDP
checks of 30 echoes each, and all 48 ping runs (960/960 echo replies).
All Core exits were zero without forced kill; scratch_lost summed to zero.
The root-route invariance assertion stayed enabled. Combined Core RSS
snapshots ranged 60,940,288-63,303,680 bytes across all arms; this is not a
per-arm memory saving or long-term leak measurement. No tracing ran during
CPU comparisons.

Each row below pools six primary measurement windows (three repetitions,
two directions; each window includes a concurrent reverse flow). CPU/GiB
is the sum of both Core process CPU deltas divided by both delivered flows'
bytes. Conventional median averages the two middle samples for six values.
Measured Mbit/s is for the primary flow, NOT aggregate throughput.

| Per-flow configured cap | Arm | TUN scratch | Actual primary Mbit/s median | Core CPU s/GiB median | CPU sample range |
| ---: | --- | ---: | ---: | ---: | --- |
| 200 | legacy | 0 | 192.08 | 31.20 | 30.96-31.44 |
| 200 | GSO | 0 | 192.15 | 22.20 | 21.84-22.40 |
| 200 | legacy | 8192 | 192.10 | 29.60 | 29.28-29.84 |
| 200 | GSO | 8192 | 192.20 | 18.12 | 18.00-18.32 |
| 600 | legacy | 0 | 500.95 | 24.52 | 24.24-24.80 |
| 600 | GSO | 0 | 518.66 | 19.84 | 19.44-20.08 |
| 600 | legacy | 8192 | 508.72 | 23.52 | 22.96-23.76 |
| 600 | GSO | 8192 | 524.14 | 17.28 | 17.04-17.76 |

At the nearly identical ~192 Mbit/s primary rate, GSO plus 8KiB scratch
reduces Core CPU/GiB by 41.9% versus legacy/zero, and scratch alone adds an
18.4% reduction over GSO/zero. At the higher configured cap, combined
reduction is 29.5% and incremental scratch reduction 12.9%, but actual
rates differ: none achieves the configured 600 Mbit/s. Do not call this an
exactly matched 600 Mbit/s experiment or a new saturated-bandwidth record.
Within each rate/arm, CPU sample ranges support the direction rather than
one favorable outlier. This is useful actual-Core evidence, unlike the
negative standalone receive models.

Boundary: this closes the paced IPv6 mixed-load comparison only. The prior
unpaced ICMP failures remain FAIL, including the baseline ring rejection.
It does not establish a production-safe fix for every overload condition,
other platforms, actual physical hosts, or Stealth in this new workload.
Next work should reconcile the two positive overlays into one bounded
production proposal with activation/fallback/lifetime review, then validate
the exact candidate on the original hardware without replacing production
first. Do not re-open recv_many/recvmmsg based solely on these results.

Evidence artifact 10845874849 is about 1 MiB, full ZIP SHA256 verified:
`0a8a3dbb38bd0c5e59215c3905223d291aba70c1038b451f0412bdfb3b218152`.
Artifact harness SHA and reused binary hashes match the selected candidate.

### Production-boundary source audit: do not copy diagnostic overlays verbatim

Inspected adapter.rs.in and tun_capacity.rs.in, then reconciled the locked
registry tun-rs 2.8.7 checksum
`ea75f145e8f32c72b1afdf137f2181810b0232be9930519e8d82071b4a3b3bdf`.
This is source analysis, not new runtime validation.

1. UDP capability fallback is absent in the diagnostic writer. send_group
   propagates a non-Interrupted error; send_frames propagates it; forward
   terminates with IOError. A kernel/device that cannot accept UDP_SEGMENT
   therefore loses the writer instead of retaining legacy sends. This is a
   production admission blocker, not a regression discovered in a shipped
   feature. Existing successful GSO tests do not cover unsupported kernels.
2. Preserve each Frame's already-sealed bytes while handling such rejection.
   Any permitted fallback must send only the rejected group's unchanged
   datagrams in order, never reseal them, never replay previously completed
   groups, and never retry an ambiguous short successful submission. A
   per-writer capability decision is sufficient; no global capability
   manager, new queue or background probe is justified. EAGAIN remains
   readiness-driven; permanent errors must not trigger an unbounded loop.
   Exact fallback errno policy still requires kernel/reference evidence;
   do not silently classify every network error as lack of GSO support.
3. Preserve the diagnostic ownership bound: 32 staging + 64 ring + 32 wire
   slots, versus the legacy 128-slot ring. Do not retain a 128-slot ring
   AND add staging/wire slots while claiming unchanged memory bounds. Packet
   ownership bounds are not total RSS bounds; payload sizes and temporaries
   still matter. Control and gate-key traffic remain single datagrams.
4. TUN scratch reclamation is supported by the exact dependency implementation:
   coalesce_tcp_packets checks target capacity before resize/extend and may
   swap buffer elements for prepend. Matching the allocation pointer after
   send_multiple, rather than assuming a fixed vector index, is appropriate.
   BytesMut owns the allocation in the future; cancellation/drop releases it.
   Missing reclamation must disable this optimization rather than allocate
   repeatedly, panic or turn it into a pool. Existing measured scratch_lost=0
   is evidence for tested paths, not an exhaustive cancellation guarantee.
5. TUN error handling is DIFFERENT from one UDP sendmsg. tun-rs's
   async_device/unix/mod.rs::send_multiple loops through write candidates,
   can keep going after an individual write error, and finally returns Err.
   Some packets may already have been delivered. Never replay the whole TUN
   batch on Err as a supposed legacy fallback; that can duplicate traffic.
   Preserve existing error propagation and reclaim owned memory only.
6. Diagnostic environment switches, process-global OnceLock mode, namespace
   assertions, panic on environment values, per-drop JSON/files and detailed
   counters are not production configuration/lifecycle APIs. A production
   proposal must use existing configuration ownership and cfg boundaries;
   do not expose these experimental flags as a supported public interface.
7. The dependency's tcp_gro also copies packet bytes into a temporary Vec.
   This is a source fact, not a measured dominant cost or authorization to
   fork tun-rs. Do not add that separate optimization to this delivery batch.

Remaining proof before adoption: bounded unsupported-GSO fallback with exact
wire/order checks; true backpressure/cancellation cleanup; unchanged fallback
on non-Linux paths; isolated exact-artifact measurement on original hardware.
The paced CPU improvement is retained, but none of these requirements is
marked passed by that result. Production source remains unchanged.

### Bounded GSO fallback implementation prepared (uncommitted/unvalidated)

adapter.rs.in now retains a per-writer gso_disabled flag and a diagnostic
fallback count. Only grouped send errors EINVAL/EIO/ENOPROTOOPT/EOPNOTSUPP
select individual sends for the rejected group's first frame and all later
frames. Already completed groups are never restarted; Frame bytes are not
resealed. Other errors propagate; a short successful submission is not
replayed. Packet/staging/ring bounds, grouping, protocol and TUN behavior
are unchanged. This is still a runner-local experimental overlay, not a
shipped production patch.

Kernel/reference research: Linux v6.8 net/ipv4/udp.c::udp_send_skb frees the
GSO skb and returns EINVAL for sk_no_check_tx; ordinary non-GSO sends follow
the checksum-disabled send branch. It also rejects unsupported checksum/
transform conditions with EIO. Quinn's current quinn-udp/src/unix.rs turns
off segmentation following EIO/EINVAL; this is supporting precedent, not
code imported into this project or proof of every errno on every kernel.
A raw-content fetch returned HTTP429; the kernel evidence was obtained from
the corresponding GitHub v6.8 source page, not inferred from that failure.

- https://github.com/torvalds/linux/blob/v6.8/net/ipv4/udp.c
- https://raw.githubusercontent.com/quinn-rs/quinn/main/quinn-udp/src/unix.rs

Prepared real-socket regression: set SO_NO_CHECK on a test-only IPv4 socket;
prove grouped send actually returns EINVAL; then use send_frames with a
single-send prefix, a group and a single-send suffix. Receive exact frame
bytes/source/order twice, require only one capability fallback/GSO attempt,
and check no extra datagram remains. No fake sender replaces the tested
object. The overall case has a three-second bound. This new case has NOT
been compiled/run, and has no red/green evidence yet; do not commit it as
validated or count it toward the eight previously passed contracts.

Local Rust parser/format rendering and maintained pre-commit checks passed.
Neither establishes kernel behavior or a new artifact. The next required
work is the unchanged-send_frames red and updated-send_frames green run,
followed by the existing exact-Core fixture checks for the new artifact.
No TUN whole-batch fallback, production source modification, deployment or
release was made in this step.

### Integration seam reconciliation while candidate-test policy is unresolved

Source search in this experimental tree found no implemented
experimental_features/exp_feature/enable_udp_gso hooks. Earlier discussion
of an --exp-feature interface is not evidence that this tree supplies one.
Do not promise to reuse a nonexistent switch or introduce a general feature
registry merely for this optimization.

Current UDP ownership has two concrete construction points:
UdpTunnelListenerData::handle_new_connect and
UdpTunnelConnector::build_tunnel. Each creates separate 128-slot send and
receive rings and a connection-local Stealth state. UdpConnection::new
owns an AbortOnDrop forward task and reports termination through its close
channel. A future integration must update both SEND-side factories together,
leave the receive ring unchanged, and preserve that close/abort ownership.
Per-writer GSO-disable state naturally lives inside forward_from_ring_to_udp;
it needs neither GlobalCtx ownership nor a process-global OnceLock. Shared
listener sockets do not imply that one peer's fallback must disable every
other peer's route. Recreating a connection starts a fresh capability trial.

This identifies a small internal seam, not an approved default-on rollout.
No runtime config schema, public constructor signature, CLI option, GUI
control, receiving-ring policy, or persistent capability state was changed.
The fallback WIP and its new regression are still uncommitted and unrun.
The requested narrow exception for staging an unvalidated experimental SHA
on GitHub has not yet received an explicit reply; do not interpret an
unsubmitted suggested answer as approval or claim a workflow is running.

### Candidate staging authorization

The maintainer explicitly approved staging this unvalidated experimental batch on
GitHub so the existing workflow can compile and test it. This supersedes the
pending-authorization note above, not the validation requirements. Approval does
not authorize merging, production deployment, or release. The new real-kernel
GSO-rejection regression remains unvalidated until the workflow supplies evidence;
prior passing contracts do not cover it.

### Capability fallback candidate CI handoff

- Candidate: `b0d88e17c84d56736cc9f87d4bd1cae533e0b8f3`.
- Local pre-commit checks passed before the experimental staging commit.
- Run `36208946731` failed before compilation: download-artifact found zero
  artifacts for historical run `36074709548`, so artifact `10841125115` could not
  be reused. This is an artifact-availability failure, not a regression result.
  Failure collection also reported absent output directories because setup had
  not reached their creation. No test result was produced.
- Run `36209030843` uses the same candidate with default inputs, rebuilding the
  existing stock/control and UDP candidate rather than depending on that artifact.
  This does not replace the pending bounded-TUN capacity comparison.
- The mandated status helper reached its 600-second observation timeout without
  reporting a terminal result. CI outcome and the new regression remain UNKNOWN.
- Unique next step: collect the terminal outcome of run `36209030843`; do not
  dispatch another build merely because observation timed out. Red/green evidence
  is still pending; no production deployment, merge, or release occurred.

The next bounded observation of run `36209030843` also ended with helper exit
124 (`status=timeout`), not a terminal CI verdict. No replacement run was
started. Compilation, regression and performance outcomes remain unverified;
the unique next action remains collecting that run's terminal evidence.

### Verified result: fallback candidate b0d88e17

Run `36209030843` completed SUCCESS on exact candidate
`b0d88e17c84d56736cc9f87d4bd1cae533e0b8f3`. The preceding timeout notes
were observation timeouts and are superseded by this terminal evidence.
Artifact `10895437417` was downloaded and its full ZIP digest verified:
`e321fcb6e1fe1b1937f98fdc170f0865510effd23ab1fa004312520916647a06`.
Extracted binaries were independently hashed against the included identities:

- Stock Core: `c35374e171aa6de98e9f8844325ddef9c5c5d376214c4678249354792e5898da`.
- Candidate Core: `0b97f81ca1ed46dd860a345de764b8430aff62995b3ceaf6f40cc4a9a3626c99`.
- Production source base remains `c6772dbfef2395ff96b39bd4801945d92212dffb`;
  this is the disposable UDP overlay, not a merged production implementation.

Environment: Ubuntu 22.04, Linux 6.8.0-1064-azure, AMD EPYC 7763, four
logical CPUs (two cores). Both endpoints are network namespaces on this one
GitHub runner, joined by veth: client underlay `192.0.2.1`, overlay
`10.88.0.1`; server underlay `192.0.2.2`, overlay `10.88.0.2`. Both underlay
and overlay are IPv4. This is neither WAN nor original-host validation.
The offered rate is capped at 200 Mbit/s; measured rates around 192 Mbit/s
are not a bandwidth ceiling.

Stealth-off medians, separately measured upload/download; CPU is the sum
of both Core processes per delivered GiB. Stock has two samples per direction;
other modes have three. Stealth-on compatibility samples are not pooled here.

| Mode | Upload Mbit/s | Download Mbit/s | Upload CPU s/GiB | Download CPU s/GiB |
| --- | ---: | ---: | ---: | ---: |
| Stock | 192.316 | 192.327 | 34.24 | 33.68 |
| Same-candidate legacy | 192.389 | 192.407 | 34.24 | 34.08 |
| Staging only | 192.398 | 192.365 | 34.40 | 33.60 |
| GSO | 192.335 | 192.319 | 24.00 | 24.80 |

GSO lowers Core CPU/GiB by 29.9% upload and 27.2% download against the
same-candidate legacy path at matched throughput. Staging alone does not
show a meaningful improvement. Paired Core RSS observations range from
60,948,480 to 63,700,992 bytes across all arms; this is not proof of an
arm-specific memory reduction or long-duration leak freedom.

Functional evidence:

- Nine contract tests passed, including
  `issue4_flush_real_kernel_rejection_falls_back_without_replay`.
- Fourteen two-Core rounds; 28 bulk transfers and 28 integrity checks passed.
  Six bulk transfers have Stealth enabled, 22 disabled.
- 420 UDP echo datagrams passed; 560/560 concurrent ICMP probes returned.
- All 28 Core process exits were zero, with no forced kill; namespace cleanup
  succeeded and the harness recorded unchanged root routes.
- Peer records remained UDP. GSO activation was present including Stealth-on
  outer framing. Metric parsing had no errors.
- Normal lab writers recorded zero capability fallbacks. Forced rejection is
  covered by the separate real-socket contract, not by these performance arms.

Remaining gates: the regression has a verified green result but no execution
of the identical regression on the pre-fix implementation yet. Its explicit
raw-kernel EINVAL assertion is useful negative-path evidence, not a substitute
for that red/green requirement. No merge or deployment is authorized by this
result. The bounded-TUN comparison and original-host performance acceptance
remain separate outstanding work; previous unpaced ICMP failures remain FAIL.

Current cursor: no CI run is live for this batch. Preserve this exact artifact.
Next close the pre-fix red/green evidence using the existing experiment path,
without weakening assertions, introducing a new workflow, or widening the
production patch. Then reconcile the minimal implementation with current Core
before requesting production integration; do not restart rejected receive
batching experiments.

### One-time pre-fix behavioral control dispatched

Negative-control branch `codex/issue4-gso-fallback-red`, commit `f2123fd3`,
restores only `send_frames` from `0cb5c1fb` while retaining the exact green
candidate's test module and diagnostic field declarations. Source test-suffix
SHA-256 is `c4a0964f236caa5f6337dbc0a7f3c41f61d61756ae2e43c64ca2e5d40d7f62fa`;
restored function SHA-256 is
`654ee2a0ba41a05937b3f9ebe91206a0302bc90beb7c340af0bcde3baec7936a`.
No test assertions or workflow files changed. Pre-commit and template rustfmt
syntax checks passed. Run `36211303677` uses the existing experiment workflow.
Only an executed regression failing on the old sender's propagated EINVAL,
with the other eight contracts passing, qualifies as the expected negative
control. An infrastructure or compilation failure does not qualify.

The green candidate and production branches are unchanged. This negative
control must not be merged/deployed/released. Current unique next step is to
collect run `36211303677`; do not rebuild the successful green candidate.

### Closed: one-time pre-fix regression control

Negative-control run `36211303677` completed FAILURE at
`f2123fd3083079516435a04f49d1f61a8fea1c9b`. This is the intended test failure,
not a compilation or infrastructure failure: all nine tests executed, eight
passed, and only `issue4_flush_real_kernel_rejection_falls_back_without_replay`
failed. The preserved original error is `Os { code: 22, kind: InvalidInput,
message: "Invalid argument" }`, returned by the restored pre-fix sender and
unwrapped by the unchanged test. Exit code was 101. The sender counters recorded
one successful prefix datagram, one rejected GSO attempt and zero fallbacks.

The same regression passed with the fix in run `36209030843` (nine passed).
Together these runs close the behavioral red/green requirement for this
specific regression. The green execution preceded the one-time reverted-code
control; this is not a claim about chronological red-first development.
No test assertion was weakened and no workflow was changed for the control.
The failing negative-control branch remains non-shipping evidence.

### Integration reconciliation and remaining narrow checks

The inspected canonical tracked HEAD is still
`c6772dbfef2395ff96b39bd4801945d92212dffb`, identical to the experiment base.
There are no committed UDP/TUN changes to reconcile between those revisions.
An unrelated untracked research document was left untouched; this observation
is not permission to stage or remove other work.

Issue #8 records an existing small Tokio fixture that extracts `group_end`,
`send_group` and `send_frames` verbatim. That fixture should be used for further
send-path error/readiness checks instead of another cold Core compilation.
Its dependency versions include the exact Tokio 1.52.1 used by this Core base.
Its current fixture containers lack the new writer-local downgrade fields and
its MTU checks still assume EINVAL. Prior run `35699982270` already proves the
runner returns EMSGSIZE for that MTU case. Preserve that failure and correct
any future fixture expectations explicitly against the observed kernel contract,
not by accepting arbitrary errors. The checksum-disabled EINVAL case remains
a distinct capability-rejection scenario. No fixture assertions were changed
in this evidence batch.

The existing workflow is push-triggered, not workflow_dispatch. Do not invent
a dispatch command for it. A future related fixture batch should use its actual
entry point, preserve IPv4/IPv6 EAGAIN/cancel/shared checks, and separately prove
ordinary-send error propagation and unaffected writer state. This must not
relabel full Core owner-drop, relay, mixed-version, physical IPv6 or endurance
coverage as completed.

Current cursor: both green and negative-control runs are terminal; none is live.
The fallback regression is closed. Production adoption remains unapproved and
unvalidated. Next bounded work is the existing lightweight fixture's compatibility
and remaining writer-local failure contracts, not another performance model or
rebuild of the already-green Core candidate.
