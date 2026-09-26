# Combined packaged Core comparison: partial gains, saturation gate failed

## Current disposition

[Run 36260893872](https://github.com/lovitus/EasyTier/actions/runs/36260893872)
is **FAIL**, not accepted or retried to green. The completed fixed-load phase
shows a measured CPU reduction. The later IPv6 saturation control-progress
assertion failed. These are separate results; neither erases the other.

The secure-relay ownership fix retains its independent red/green and bounded
acceptance in [RELAY_GUARD_PACKAGED.md](RELAY_GUARD_PACKAGED.md). The new failure
is direct UDP with Stealth off, not that secure-relay workload. No production
fix, rollback, queue enlargement or assertion relaxation follows from this
single failed sample.

## Revisions and evidence

- Harness: `dc28066955ec7a71a95f1120a379ffb98fc400a2`.
- Baseline: `c6772dbfef2395ff96b39bd4801945d92212dffb`, artifact `10899308750`.
- Candidate: `3166ab672d347cdcc5a6768bc77056cd8ec38323`, artifact `10911199419`.
- Both existing optimized packages: x86_64 musl, jemalloc, no Leaf/policy,
  Rust 1.95.0. No Core rebuild or diagnostic overlay in this comparison.
- Baseline Core SHA-256:
  `b59903879732a9f1b868e7a279aec076355877ff7b70a9372636aba13913fd23`.
- Candidate Core SHA-256:
  `46ab646adbac5eaea1c008f23b1b2340b25ac6ffb80018b1d5b41d6ec404fb86`.
- Evidence artifact `10912970641`, 1,443,438 bytes; complete ZIP digest and
  CRC checked locally:
  `8c30bda2a64a37724a9afb8dc15b0e6b3b3365e3ef1427f76447737cd3af4782`.

Both endpoints are network namespaces on one GitHub Ubuntu 22.04 runner:
AMD EPYC 7763, four logical CPUs, Linux 6.8.0-1064-azure. Underlay is an
IPv4 veth/UDP connection; overlay is IPv4 or IPv6, one mesh TUN per endpoint,
MTU 1360, AES-GCM, compression disabled. No Mihomo/Leaf traffic is involved.
This is not two physical hosts, a WAN result or original-host acceptance.

## Completed fixed-load phase

24/24 cases passed, three interleaved samples per version, inner family and
Stealth mode, each with upload and download. The offered rate is 200 Mbit/s;
observed goodput is approximately 192 Mbit/s for both versions. This is not
a bandwidth ceiling. CPU is the sum of both Core processes per delivered GiB.

| Inner family | Stealth | Direction | Baseline CPU s/GiB | Candidate CPU s/GiB | Reduction |
| --- | --- | --- | ---: | ---: | ---: |
| IPv4 | off | upload | 34.88 | 22.08 | 36.70% |
| IPv4 | off | download | 34.08 | 21.12 | 38.03% |
| IPv4 | on | upload | 37.76 | 24.00 | 36.44% |
| IPv4 | on | download | 37.12 | 24.32 | 34.48% |
| IPv6 | off | upload | 33.60 | 21.76 | 35.24% |
| IPv6 | off | download | 34.40 | 21.12 | 38.60% |
| IPv6 | on | upload | 35.52 | 22.40 | 36.94% |
| IPv6 | on | download | 36.16 | 23.04 | 36.28% |

Whole-host CPU/GiB also decreased in all eight groups. These are measured
combined effects in one run, not sums of incremental percentages from other
runners. The separate syscall trace is excluded from performance metrics.

## Saturation: completed IPv4 samples, incomplete IPv6 matrix

Three IPv4 samples per version and direction completed before the failure:

| Direction | Baseline median Mbit/s (range) | Candidate median Mbit/s (range) | Baseline / candidate CPU s/GiB |
| --- | ---: | ---: | ---: |
| upload | 1128.66 (1128.61-1129.67) | 1540.48 (1475.21-1554.41) | 24.59 / 16.06 |
| download | 1107.57 (1001.81-1115.21) | 1533.82 (1389.89-1554.60) | 24.56 / 15.89 |

These completed IPv4 samples do not make the whole saturation phase PASS.
IPv6 has only one complete baseline and one complete candidate case before
the second candidate case fails. Do not publish a complete three-sample
IPv6 median, omit the failed sample, or pool this with older accepted runs.

## Exact failure and counter evidence

Case `saturation/08-candidate-v6-stealth0`, download:

- The 1 GiB bulk transfer completed at 1506.45 Mbit/s. Its raw result is
  retained, but no accepted transfer metric was recorded after the failure.
- Concurrent ICMP: 20 sent, 19 received; sequence 17 missing, 5% loss.
  The unchanged assertion is `ICMP progress failed`.
- The upload in the same case returned all 20 ICMP probes.
- Kernel UDP receive/send error counters and interface error/drop counters
  did not increase during the failed transfer window.
- Server underlay TX and client underlay RX both recorded 833,986 packets
  and 1,200,908,907 bytes; client UDP InDatagrams increased by 833,986.
- Remote ICMPv6 counters show 19 requests received and 19 replies generated;
  the client received 18 replies within the counter window. Ping starts
  before the snapshot, so this window excludes its first request.
- The ICMP counters suggest a missing return-path packet after remote reply
  generation. They do not identify an individual ciphertext or prove which
  userspace stage lost it. Zero kernel errors alone do not prove ring loss.
- GSO/GRO change inner packet accounting. Do not compare TUN packet counts
  one-for-one with underlay datagrams.

Across all recorded phases: 33 completed suites out of 34, 68 independent
payload-integrity checks, 1,020 UDP echoes, 1,359 of 1,360 ICMP replies.
All 360 tracked children exited zero without forced kills, 68 namespaces
were removed, and 34 root-route comparisons were unchanged. The 68 Core
logs have a maximum size of 5,083 bytes, with no already-encrypted warning
or panic. This is not long-duration memory or resource acceptance.

## Source and historical reconciliation

Current `ZCPacket::is_lossy()` classifies all `PacketType::Data`, including
encapsulated ICMP/TCP, as lossy. `RingSink::try_send()` rejects data at
capacity minus four reserved slots. `force_send()` only attempts a push;
it does not evict an old packet. UDP receive drops a rejected Data packet
with a trace-level message. Warn-level logs cannot exclude that path.

[Historical packet-stage run 36088594844](https://github.com/lovitus/EasyTier/actions/runs/36088594844)
correlated one missing IPv6 echo with an actual `ring_reject` fingerprint
on the pre-integration implementation. That establishes an existing loss
mechanism, not the cause of sequence 17 in this new uninstrumented run.
The detailed evidence and negative experiments remain in
[SATURATION.md](SATURATION.md).

Do not repeat rejected `recv_many`, `recvmmsg`, writer-yield, large-buffer or
ICMP-priority proposals based only on this failure. Moving the drop boundary
or increasing queues does not establish lower CPU or loss-free forwarding.

## Bounded next diagnostic, not a production patch

The existing packet-stage overlay is reused on exact source `3166ab67` in
the existing UDP experiment workflow. It builds one no-Leaf diagnostic
Core/CLI with the comparator's target, features and optimization settings.
It does not rebuild the accepted production artifact or run five formal
workflows. Diagnostic binaries are retained separately for reuse.

At most six inner-IPv6, direct-UDP, Stealth-off cases run, each with the same
1 GiB upload/download and unchanged ICMP assertion. Stop at the first
failure. TUN sequence capture and bounded encrypted-payload fingerprints
cover send, UDP receive, receive-ring rejection, peer receive and NIC enqueue.
Per-Core logs retain the 2 MiB cap, and trace overflow invalidates evidence.
Instrumented throughput is not performance evidence. If loss does not
reproduce, the original failure remains unresolved rather than becoming PASS.

Production queue bounds, scheduling, ciphertext, route selection and TUN
behavior are unchanged. Original-host, WAN, sustained resources and mixed-flow
acceptance remain open. There is no merge, deployment or release from this report.

### Diagnostic setup failure, not a Core result

Run `36262853021` at harness `7c10eccb52febbfb1bd75c46afeb2929a8d74e19`
failed before compilation or traffic. The observation overlay applied, but
Rust 1.95 lacked the rustfmt component. The new diagnostic job had omitted
the component-install step already used by the historical experiment job.
The failed log is retained; zero traffic cases ran. The correction installs
that component explicitly and changes neither the Core source nor assertions.
The next run is a repaired diagnostic execution, not a retry of the failed
uninstrumented A/B to obtain a passing result.
