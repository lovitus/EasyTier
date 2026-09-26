# Packaged Linux TUN-head integration: measurements and remaining relay blocker

Evidence date: 2026-09-26.

Current production source remains `6e90dbf102e5c93d56c531b87eea1858c4a8e61e`,
stacked on `3a1f3d9fc840a37a7649448a042bf44035bfaf7b`. PRs #9 and #10 are draft,
unmerged and unreleased. This document changes no production source or workflow.

## Current acceptance state

| Scope | State |
| --- | --- |
| Exact-source Test and formal static gates | PASS, with disclosed existing-test annotations |
| Direct fixed-rate and unpaced Core comparison | PASS within the topology below |
| Mixed parent/candidate and IPv6-underlay compatibility | PASS |
| Short idle and cleanup observation | PASS within the recorded windows, not a leak-freedom claim |
| Three-peer relay traffic and cleanup | FUNCTIONAL_PASS |
| Stealth relay log/resource behavior | FAIL: large per-packet warning output on both parent and candidate |
| Original reported host, WAN, sustained resources and mixed-flow overload | OPEN |

Do not treat workflow SUCCESS as overall resource acceptance. No further relay
load run should be started before the redundant next-hop encryption path and a
bounded log-growth stop condition have been addressed. No source change has yet
been made for that newly identified issue.


[Performance run 36234743171](https://github.com/lovitus/EasyTier/actions/runs/36234743171) passed using the unchanged packaged harness `55508eff5cab2955c482576a97c0ed7634d76646`. This is actual Core, not the small allocation probe.

Baseline is the already-optimized UDP candidate `3a1f3d9fc840a37a7649448a042bf44035bfaf7b`; candidate is `6e90dbf102e5c93d56c531b87eea1858c4a8e61e` from draft PR #10. Both packages were built once; neither was rebuilt for this comparison. Their outer archive, inner package, Core identity and BUILD_INFO were checked by the existing CI harness before traffic.

Endpoints: isolated client/server network namespaces on one GitHub-hosted Ubuntu 22.04 / Linux 6.8.0-1064-azure runner, AMD EPYC 7763, four visible CPUs. One mesh TUN per Core, MTU 1360, direct IPv4 UDP underlay, IPv4/IPv6 inner TCP, AES-GCM. No Leaf/Mihomo, no physical link or WAN inference. Each row below is the median of three interleaved runs per arm; CPU is the sum of both Core processes' CPU-seconds per GiB.

### Fixed-rate observation

Observed application rate remained approximately 192 Mbit/s in both arms. Transfers were 64 MiB with Stealth off and 32 MiB with Stealth on. CPU/GiB decreased in all eight family/direction/Stealth cells, by 14.46%-18.75% relative to the UDP-only parent.

| Inner family | Stealth | Direction | Parent CPU s/GiB | New CPU s/GiB |
| --- | --- | --- | ---: | ---: |
| IPv4 | off | upload | 24.16 | 20.32 |
| IPv4 | off | download | 24.64 | 20.32 |
| IPv6 | off | upload | 23.36 | 19.36 |
| IPv6 | off | download | 24.00 | 19.84 |
| IPv4 | on | upload | 26.56 | 22.40 |
| IPv4 | on | download | 26.56 | 22.72 |
| IPv6 | on | upload | 25.60 | 20.80 |
| IPv6 | on | download | 25.92 | 21.76 |

### Unpaced observation

One GiB per transfer, Stealth off; same topology and three interleaved samples per arm. These throughput samples were not under perf or strace.

| Inner family | Direction | Parent Mbit/s | New Mbit/s | Parent CPU s/GiB | New CPU s/GiB |
| --- | --- | ---: | ---: | ---: | ---: |
| IPv4 | upload | 1370.36 | 1530.98 | 17.97 | 16.05 |
| IPv4 | download | 1359.48 | 1476.15 | 17.97 | 15.98 |
| IPv6 | upload | 1328.89 | 1478.93 | 18.32 | 16.42 |
| IPv6 | download | 1342.95 | 1507.25 | 18.28 | 16.34 |

Median throughput improved 8.58%-12.23%; CPU/GiB decreased 10.37%-11.07%. IPv4 download had overlapping throughput ranges (parent 1168-1363, candidate 1345-1539 Mbit/s), so these medians are bounded observations, not a guarantee for every sample. Do not add these percentages arithmetically to the earlier UDP result from another run.

### Function, ownership and resources

37 completed suites; 74 successful bulk transfers and 74 separate digest/half-close checks; 1110 UDP echoes; 1480/1480 ICMP replies. All 74 Core exits were clean, all 74 namespaces removed, and all 37 root-route checks were unchanged. No lost-head warning or panic was found in these Core logs. The separate trace suite is excluded from the throughput table.

Paired RSS ranges overlap: parent 52.25-56.71 MiB; candidate 52.38-55.88 MiB. Both arms show 26-29 FDs and 13-14 threads per Core at the recorded transfer boundaries. This establishes neither a memory reduction nor long-duration leak freedom. The code adds one reusable 8 KiB allocation per Linux offload sink, not per flow or packet.

The exact-source [Test run 36231652201](https://github.com/lovitus/EasyTier/actions/runs/36231652201) passed all four new actual TUN/GRO regressions and 1772 partition tests, with the previously disclosed unrelated PASS/LEAK and SLOW annotations retained.

### Immutable evidence

- Candidate build run: [36233922318](https://github.com/lovitus/EasyTier/actions/runs/36233922318), no-Leaf comparator artifact `10903497520`.
- Candidate artifact ZIP SHA-256: `3b3c99386546baf5271831e39e8981f660871d062f5f853d677455103efadae0`.
- Candidate Core SHA-256: `5d2451bdca191a143a49338f6b837011ced6ca3cf9c02cbbe179aa5436a77fd1`; Build ID `451c290585134e9d1b4f88099fc2033170eadb15`.
- Parent reused artifact `10898758495`, Core SHA-256 `b0d61a1911898412434fbbfb5bc15d8ca7dcd766f0710769de0c3f6968c8c1a6`.
- Both are x86_64 Linux musl, jemalloc-only/no Leaf, Rust 1.95.0, with the same optimized symbol-bearing comparator build settings.
- Result artifact `10903808482`, ZIP SHA-256 `186bf553835d868646ef0dac60215fcc5569eed8838902235380b656e7e0e47e`, verified after download. One transient report-download EOF was retried without rerunning any workflow or changing source.


## Exact mixed-version and IPv6-underlay evidence

[Run 36235807927](https://github.com/lovitus/EasyTier/actions/runs/36235807927)
used the same frozen artifacts and harness. All 24 cases passed: eight
IPv4-underlay mixed placements and sixteen IPv6-only-underlay cases, with
IPv4/IPv6 inner traffic and Stealth off/on. Parent/parent, parent/candidate,
candidate/parent and candidate/candidate placements are distinguished by their
actual binary hashes in `results.jsonl`; this is not a claim about all upstream
versions.

There are 48 digest/half-close checks, 720 UDP echoes, 960/960 ICMP replies, 48
clean Core exits, 48 removed namespaces and 24 unchanged root-route checks.
The 96 peer observations report exactly 32 `udp` and 64 `udp6` values.

Artifact `10903624143` has downloaded/verified ZIP SHA-256
`d305b43d4d0552cdd6fe3efd9c65db703c5d514086f3b3d0e0927df4cc754621`.

## Short idle/profile evidence is from a different runner CPU

[Run 36235812507](https://github.com/lovitus/EasyTier/actions/runs/36235812507)
ran on **AMD EPYC 9V45**, four visible CPUs, Ubuntu 22.04 / Linux
6.8.0-1064-azure. The throughput comparison above ran on EPYC 7763. Both arms
within each run are matched, but absolute rates or CPU costs must not be merged
across these hosts. Profiled transfer rates are not additional throughput
acceptance.

Each pair completed a 2 GiB upload and download. Eight 30/60-second idle windows
were collected before and after traffic. Aggregate two-Core CPU, with 100%
meaning one CPU, was:

| Arm | Before traffic | After traffic | End-of-window paired RSS |
| --- | --- | --- | --- |
| Parent | 0.133%-0.167% | 0.117%-0.200% | 49.68-50.24 MiB |
| Candidate | 0.133% | 0.150%-0.200% | 50.29-51.20 MiB |

Observed FDs settled to 25-26 per Core after traffic. Both arms went from 13
threads/Core before traffic to 14 at the end; this short observation is not
proof that all retained tasks or allocations disappear while the process lives.
All four Core processes exited cleanly, all four namespaces were removed, and
both root-route checks stayed unchanged. Four digest/half-close checks, 60 UDP
echoes and 80/80 ICMP replies passed.

All four recordings contain both expected Core PIDs: 4403 samples in total,
zero reported lost samples. The flat self samples are only pointers for later
research, not proof of a new bottleneck: candidate SOCKS packet-filter self
samples total 3.12%-3.29%, AES update routines 4.26%-5.41%, and `tcp_gro`
0.85%-0.87%. No additional production change is justified solely by these
fractions.

Artifact `10904457128` has downloaded/verified ZIP SHA-256
`535f0f80d919aaee7abd05bc4f471ee0860394e00c5f4595c713349bf17650af`.

## Relay passed traffic but exposed a real log/resource failure

[Run 36236617653](https://github.com/lovitus/EasyTier/actions/runs/36236617653)
completed 24 cases: IPv4/IPv6 underlay/inner families, Stealth off/on, and six
parent/candidate client-relay-server placements. Endpoints have no direct L3
route and relay kernel forwarding is disabled. The records contain 48
no-direct-path checks and 192 relay-route observations; traffic is not inferred
from an online peer flag.

All 48 digest/half-close checks, 720 UDP echoes, 960/960 ICMP replies, 72 Core
exits, 72 namespace removals and 24 root-route checks passed. **These functional
results do not close resource acceptance.**

The result artifact was 1,190,447,548 bytes. Inspection of its ZIP central
directory showed actual log growth, not embedded Core binaries. Log members in
Stealth-on cases total **2,757,617,020 uncompressed bytes**. The largest single
log is 116,717,756 bytes. Sampled all-parent and all-candidate endpoint logs
both repeatedly emit:

```text
WARN easytier::peers::encrypt::ring: packet is already encrypted
```

Each such record also formats the entire `ZCPacket` payload. The all-parent
case's client log is 116,427,523 bytes; the all-candidate counterpart is
115,428,104 bytes. Stealth-off sampled endpoints do not show this stream.
Thus this is not introduced by the 8 KiB TUN-head change. The earlier relay
artifact `10900851933` was also about 1.20 GB; its prior functional PASS must
not be interpreted as a log/resource acceptance result. Its internal logs were
not re-extracted in this follow-up, so its exact per-version warning provenance
is not asserted here.

### Source mechanism and smallest next fix

At the exact current source:

- `PeerSessionTunnelFilter::before_send` in
  `easytier/src/peers/peer_conn.rs` checks that the source is this peer, but does
  not check that `hdr.to_peer_id` equals the directly connected next-hop peer.
- `RelayPeerMap::send_msg` already applies the end-to-end destination session.
- The connection filter then enters the unrelated next-hop session, including
  its lock/nonce/encryptor path. `RingCipher::encrypt_with_nonce` sees the
  encrypted flag, writes the large warning and returns `Ok(())` without
  re-encrypting. This explains functional delivery alongside log amplification.
- The receive filter already checks destination ownership. The four encryption
  implementations have similar already-encrypted warning guards; suppressing
  all of them would hide the caller ownership problem rather than fix it.

The exact upstream commit
[`425a24273b192399d3de3509fd868382f8ec89cc`](https://github.com/EasyTier/EasyTier/commit/425a24273b192399d3de3509fd868382f8ec89cc)
contains the corresponding send-side destination guard and an invalid
next-hop-session regression fixture. This is the B01 behavior discussed in
issue #2, now backed by a separately observed runtime symptom.

The next production batch should only move the existing source/destination
ownership check before taking the existing session mutex and add the missing
destination comparison. Keep StdMutex, connection-local Stealth, wire format,
all encryption/error handling and session GC unchanged. Do not pick ArcSwap or
B02 activity-aware GC as an incidental fix.

Required evidence remains: an unchanged-baseline failure for the next-hop
ownership regression; relay ciphertext retained byte-for-byte even if the
next-hop session is invalid; ordinary directly addressed encryption/decryption
still working; and a bounded exact-artifact relay case showing delivery
without per-packet warning growth. These are requirements, not completed tests.

### Evidence retrieval and operational mistakes retained

Artifact `10904178832` has GitHub-reported ZIP SHA-256
`b892d14cc84c319834858f2ddae4f83fe5501a2e0587fbdf6e8058f4a5b44b9d`.
The oversized full download was terminated. The **whole ZIP hash was not locally
verified**. HTTP Range retrieval obtained its directory and 101 selected small
evidence members in 2,083,260 bytes; ZIP member CRCs and individual SHA-256s were
checked. Three log prefixes were retrieved separately in 707,004 range bytes;
prefixes do not claim whole-file CRC or hash verification. Original logs stay
private; no payload dumps are copied into Git or issues.

The first relay dispatch, run `36235810037`, was cancelled before any job.
The research workflow groups concurrency by branch with
`cancel-in-progress: false`; dispatching three modes did not create independent
queues, and the middle pending run was displaced. After the surviving profile
run completed, relay alone was dispatched once. This was an operator scheduling
mistake, not a Core failure. Future modes on this harness branch must be
scheduled one at a time; no workflow source change or repeated Core build was
made to recover.

Two local extraction assumptions also failed visibly (a nonexistent root
summary and an optional stderr member pattern). Existing per-case files were
then consumed; neither error caused a CI rerun or a change to test assertions.

## Current task cursor

- Frozen source and artifacts: parent `3a1f3d9f`, candidate `6e90dbf1`, listed
  above. No operational host has been upgraded by this batch.
- Completed: production regression/static gates, exact direct performance,
  IPv6/mixed-version functional coverage, short idle/profile, relay traffic and
  cleanup.
- Not completed: Stealth relay resource acceptance, original-host/WAN,
  long-duration resources, mixed-flow overload, other architectures.
- Unique next action: implement/review only the B01 destination ownership guard
  with the required regression and bounded log-growth evidence. Do not repeat
  completed TUN experiments, add receive queues, suppress all warnings, or
  broaden this into session GC/ArcSwap or policy-engine work.
