# TCP control-head correction: actual Core evidence and mixed-load boundary

## Frozen source and actual regression result

Parent: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
Unchanged-runtime input: `d0cf2e0b36b7831cfb81ea0000af9a3fd4c18894`.
Corrected candidate: `6dce45e745da2b796e10266423f9a8d0f9cb28fe`.
The two regression files have identical SHA-256:
`7ff93a89a573553065662bff1eabc9e6eca95180e2989c42e26bd611d1054ed6`.

[Baseline Test](https://github.com/lovitus/EasyTier/actions/runs/36292331111)
fails the actual Core regression on IPv4 ACK-first: three emissions instead
of two. The failure is a runtime assertion, not a compile/setup error.
[Candidate Test](https://github.com/lovitus/EasyTier/actions/runs/36292333849)
passes the identical regression in 0.021 seconds and all four unchanged TUN
byte/allocation, prepend/error, partial-write/no-replay and cancellation/drop
tests. The correction only selects the existing bounded scratch buffer for
payload-bearing ACK/PSH-ACK TCP. It does not discard control packets, add a pool,
enlarge queues or change packet bytes, routing, encryption or policy behavior.

This proves the regression correction, not an end-to-end performance gain.

## Full-suite failure is still open

The corrected full Test workflow is **FAIL**, not accepted. Its subnet-proxy
partition passes 255 of 256 cases. The failing case has native TUN, no public
relay, source KCP/QUIC enabled, destination KCP/QUIC disabled and both input
protocols enabled. It expires in `subnet_proxy_test_tcp` at
`easytier/src/tests/three_node.rs:1398`. Earlier subnet TCP/UDP exchanges succeed;
the final listener accepts but the connector does not print its post-connect
marker before the deadline. This identifies the observed phase, not the cause.

The unchanged-runtime run passes all 256 subnet cases; that same case takes
2.410 seconds. All three trees share test blob
`26f652b2c669a09f493e796669961c5458438ce3`. Unchanged test source and one baseline
pass do not prove flakiness or exclude a runtime regression. No assertion,
deadline or expectation was weakened and no failure was waived or retried.

The candidate non-three-node partitions report 1326 passing tests, including
one nextest LEAK annotation on the existing Leaf worker lifecycle test. Retain
that annotation; it does not establish Core memory growth or leak freedom.

## Same-package mixed-flow experiment

Optimized no-Leaf candidate build:
[36294320946](https://github.com/lovitus/EasyTier/actions/runs/36294320946),
existing `profiling-beta.yml` with `audit_comparator=true`. This mode does not
publish or move release tags. Reuse parent artifact `10911199419`; do not rebuild
the baseline. At this note's preparation, artifact/performance results are pending.

The existing `artifact_pair` JSON accepts optional `mixed_flow: true`. The
default remains the existing single-flow matrix. Both package/source/hash checks
remain mandatory. The resolved choice is saved in `provenance.json` and each
phase's run records. There are no workflow-definition or production-code edits.

The selected experiment reuses `lab.py` without modifying it:

- Fixed load: simultaneous opposite-direction flows, 100 Mbit/s cap and 64 MiB
  per flow; 200 Mbit/s is the aggregate configured cap, not measured goodput.
- Existing IPv4/IPv6 inner-family, Stealth and three interleaved sample matrix.
- Existing 120 timestamped ICMP replies must cover each complete fixed transfer,
  including its secondary flow, with at least 20 replies during load.
- Core/host CPU accounting already divides by the bytes from both completed
  flows. The secondary flow has its own completion/byte-count assertion.
- Saturation retains the existing 1 GiB per-flow transfer size and strict
  ICMP/integrity/cleanup assertions, now with opposite-direction bulk traffic.
  Its existing 20-probe window is not claimed as full-transfer coverage.
- The separate GSO activation trace remains single-flow and is excluded from
  performance comparisons. Compatibility/relay/profile-only inputs cannot be
  silently combined with the new mixed-flow choice.
- Any failure stops its phase and retains raw evidence. Neither a later pass
  nor this different workload erases the earlier saturation-loss result.

Both endpoints are disposable namespaces on one GitHub host. Record its CPU,
kernel, both binary identities, actual rates, CPU/GiB and RSS before interpreting
results. No physical-host, WAN, original-device or whole-project speedup claim
is supported by this setup. No merge, production deployment or release follows
from this research run. Full-suite and receive-ring acceptance remain open.

## First experiment stopped on a missing historical CLI artifact

[36295269376](https://github.com/lovitus/EasyTier/actions/runs/36295269376)
failed during preparation: GitHub returned HTTP 404 for CLI artifact
`10895437417`. Both Core-package validation loops had completed, but no traffic
case ran. This is an artifact-availability failure, not performance evidence.
The failed run remains failed; no Core was rebuilt and no integrity gate bypassed.

The replacement is the preserved integrated diagnostic artifact `10912354766`
from run `36263145990`, with complete ZIP SHA-256
`b6c7fd95e007339644d16a104bc9a70a1e5f449f86937cddaa0c02d3f46be165`.
Its root `easytier-cli` has independently measured SHA-256
`b462eb22b7a9d54b0eab6c69892c4af6c5f499d78a7ce492087963f2a92b27e0`.
Only that CLI is extracted for use; its diagnostic Core is not substituted for
either comparison endpoint. The integrated diagnostic source uses `3166ab67`;
the CLI source and `easytier/src/proto` are unchanged between the old base
`c6772dbf` and that integrated parent. The two endpoint packages, source SHAs,
new regression, traffic cases and assertions remain frozen. Results of the
corrected preparation and actual mixed-load execution were pending at that
checkpoint. The terminal results below supersede that pending status.

## Exact-package result: no measured fixed-load CPU benefit

The corrected comparison [run 36295644707](https://github.com/lovitus/EasyTier/actions/runs/36295644707)
finished with **FAIL**, not PASS. All 24 fixed-load cases and the separate
trace case passed. The first saturation case, using the unchanged baseline,
lost one ICMP reply. The runner stopped at that failure; candidate saturation
was not reached. No deadline, loss assertion, or traffic requirement was relaxed.

### Frozen inputs and evidence

| Item | Identity |
| --- | --- |
| Harness | `a0b3194db3ef876eb187617ea8c6913b50fec2da` |
| Baseline source | `3166ab672d347cdcc5a6768bc77056cd8ec38323` |
| Baseline artifact | `10911199419`, run `36258711567` |
| Baseline ZIP SHA-256 | `cf19f8fc26a00eb42ec0cd194c954663a1415d2a05eee8519dd82fdbea98f545` |
| Baseline Core SHA-256 | `46ab646adbac5eaea1c008f23b1b2340b25ac6ffb80018b1d5b41d6ec404fb86` |
| Candidate source | `6dce45e745da2b796e10266423f9a8d0f9cb28fe` |
| Candidate artifact | `10923363812`, successful build run `36294320946` |
| Candidate ZIP SHA-256 | `f0dcff52b93946b1f2ad5c204f23206dddea567a7feb2d8905ce9c1b74d2f697` |
| Candidate Core SHA-256 | `c536dd4dcbbb782ca61b63def86897c9237c0c6f8901d2ffdd49cddc042914d3` |
| Candidate Build ID | `8dcf37891a153f34342d4d4600ad949c16474d7b` |
| Result artifact | `10923464977`, 1,250,392 bytes |
| Result ZIP SHA-256 | `317b72fdc9dc127968354c8d0300e386825c59872d2ffa13a3156f441503bf5b` |

The result archive's complete ZIP hash was independently checked. This workflow
does not publish a per-file `manifest.json`; no per-file manifest verification is
claimed. Runner preparation checks the actual endpoint packages and Core hashes
and records their provenance. The first local analysis incorrectly expected such
a manifest and stopped before producing metrics. Analysis was corrected to the
existing archive contract; no workflow or runtime was changed for that analysis.

### Endpoints, workload and aggregation

Both endpoints are isolated network namespaces on the **same GitHub runner**,
connected by a veth underlay. This is not a physical-host, WAN, or mobile result.
The runner reports AMD EPYC 7763, four logical CPUs, two cores with two threads
each, and Linux `6.8.0-1064-azure`. Both packages are optimized Rust 1.95.0
`x86_64-unknown-linux-musl` Core builds with jemalloc and no Leaf policy features.

The underlay is UDP/IPv4 with AES-GCM. Inner IPv4/IPv6 and Stealth off/on are
separate cases. Two simultaneous opposite-direction flows each transfer 64 MiB
at a configured 100 Mbit/s, for a configured aggregate cap of 200 Mbit/s.
Each arm/family/Stealth combination has three interleaved case samples; each
sample contains two primary-orientation epochs. The six epochs are not treated
as six independent samples.

CPU seconds/GiB sums both Core processes and divides by delivered bytes from
both simultaneous flows. The two epochs are byte-weighted per case. The table
reports the median and range of the three case values. Throughput uses the
full-transfer measurement windows, not the configured rate.

| Inner IP | Stealth | Baseline CPU s/GiB, median (range) | Candidate CPU s/GiB, median (range) | CPU change | Aggregate Mbit/s, baseline -> candidate |
| --- | --- | --- | --- | --- | --- |
| IPv4 | off | 20.28 (19.88-20.44) | 20.28 (19.80-20.44) | 0.00% | 194.1914 -> 194.1770 |
| IPv4 | on | 22.64 (22.48-22.68) | 22.64 (21.92-22.72) | 0.00% | 194.1157 -> 194.1138 |
| IPv6 | off | 20.04 (19.92-20.32) | 20.12 (19.76-20.16) | +0.40% | 194.2225 -> 194.1812 |
| IPv6 | on | 21.80 (21.76-21.84) | 21.92 (21.68-22.40) | +0.55% | 194.3078 -> 194.1297 |

Sample ranges overlap. Rate differences are at most 0.092%. These results show
neither a meaningful CPU improvement nor a meaningful CPU regression. They do
not support presenting the control-head correction as a throughput optimization
or the main bottleneck fix. Pair RSS ranges were 51.92-55.25 MiB on baseline and
51.79-55.12 MiB on candidate; this is not evidence of a memory reduction or
leak freedom.

The fixed phase delivered 6 GiB. Its 48 full-transfer control windows received
5,760/5,760 ICMP replies, including 2,544 replies during load. Every window
bracketed the transfers and contained at least 20 in-load replies. Maximum
observed ICMP RTT was 1.21 ms on baseline and 0.993 ms on candidate; this alone
does not establish a latency improvement.

### Saturation failed on baseline; candidate remains unmeasured

Case `saturation/00-baseline-v4-stealth0` completed both 1 GiB bulk flows in each
of its two epochs, delivering 4 GiB total. The upload epoch received 20/20 ICMP
replies. The download epoch received 19/20; sequence 16 was missing.

Observed primary/secondary rates were 705.861/699.212 Mbit/s in the upload epoch
and 688.312/682.446 Mbit/s in the download epoch. This is approximately
1.37-1.41 Gbit/s aggregate for the baseline only. The remaining eleven saturation
cases did not run. There is no candidate saturation result to compare.

The exact missing packet was not traced in this run. An earlier, different
IPv6 sample proved a receive-ring rejection, but that is not proof of the cause
of this IPv4 sequence-16 loss. The new loss occurred on the unchanged parent
package, so this occurrence cannot have been introduced by the control-head fix.
Saturated receive loss remains an open investigation, not a waived gate.

### Functional and resource boundaries

Across the fixed, trace and failed saturation phases, 52 integrity checks and
26 UDP echo batches of 30 datagrams completed. All 52 Core process exits were
zero; all 52 namespaces were removed. The 392 cleanup records contained no
reported anomalies, forced kills or remnants. Core logs totalled 201,175 bytes,
with a largest individual log of 6,123 bytes; there was no observed log storm.
The separate trace recorded 5,659 successful GSO submissions and is not included
in the performance medians.

All 104 link-counter pairs reported zero link errors/drops. Protocol counters
were not universally zero: `Ip.OutNoRoutes` increased by 167 in fixed load,
10 in trace and 9 in saturation; their cause is not attributed here.
`Tcp.RetransSegs` increased by 21 in fixed load and 1,345 in saturation. The
104 protocol-counter pairs are retained rather than dismissed as unrelated.

## Disposition and next research boundary

- The actual-Core regression demonstrates the narrow control-head eligibility
  defect and its correction. It does not establish a material performance gain.
- Keep the correction isolated and unmerged. Do not expand it into queue,
  allocation, scheduling or protocol changes to manufacture a positive result.
- The candidate's separate full Test failure remains unresolved; the one subnet
  TCP-connect timeout is neither proved flaky nor waived.
- Return to measured receive admission/consumer costs and kernel I/O costs.
  Reuse existing profiles and source evidence before selecting another experiment.
- No merge, release, production deployment, saturated acceptance or overall
  mesh-performance completion is claimed by this report.
