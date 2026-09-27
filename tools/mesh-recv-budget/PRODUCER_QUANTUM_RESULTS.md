# Receive producer quantum: bounded model result

Status: **mechanism experiment completed; not a production fix**.

## Identity and scope

- [Workflow run 36287388306](https://github.com/lovitus/EasyTier/actions/runs/36287388306).
- Harness: `4bae93b42eb8a9d6b33e5cf5e71d7408874ef16e`.
- Evidence artifact: `10920749147`, 9,874 bytes.
- Archive SHA-256: `f51080d3b60cccaaef52b71fa15f12e4a0376961eb76976cef13cba624f5218b`.
- Recorded tool SHA-256: `166ecfa92fe9c82b082de794642a4900d5940f3fded812246763c6ed974c3bda`.
- Artifact digest, ZIP member safety/CRC, source, exact dependency versions,
  72 unique matrix cells and result conservation were audited.
- AMD EPYC 9V74, four logical CPUs / two physical cores on one GitHub runner.
  There are no network endpoints: this experiment performs no network I/O.
- Tokio 1.52.1, async-ringbuf 0.3.1, ringbuf 0.4.6, futures 0.3.30.
- No Core rebuild, deployment, queue-size change or production source edit.

The two-worker runtime also has the calling thread driving the inline
`block_on` downstream consumer. It is not a two-thread-total or CPU-affinity
experiment. This distinction matters when interpreting CPU and offered rate.

## Findings

The single-thread continuously-ready model loses 2.9283% of offered packets
with existing cooperative I/O budget. Explicit 64/32 service quanta remove
that loss in these model trials. This supports a burst/fairness mechanism
under that topology, not a claim of a deployed Tokio bug.

The multi-worker model does **not** become loss-free. At 128 work rounds,
median rejection is 76.6408% / 60.0457% / 63.9932% for stock / 64 / 32.
At 512 work rounds it is 94.9612% / 93.5870% / 92.3239%.
The producer offers 8.8-17.2 million packets/s while the downstream consumes
roughly 0.87-3.85 million/s. Those different offered rates prohibit treating
the rejection percentages as a matched-load network comparison. They
demonstrate that scheduling opportunities do not solve excess offered load.

Quantum 64 gives a useful model-only delivered-rate/CPU signal in some
saturated cells, but is not uniformly best. Ordinary bulk-peer probes still
drop. Tail delay does not improve monotonically: for example, the
multi-worker/512 maximum bulk-probe delay increases from 341 to 351 us with
quantum 64. The sparse peer loses no probes in any arm; that is not evidence
of loss-free ordinary traffic on the saturated peer.

All 36 paced trials deliver all packets and probes. Actual offered rate is
about 20.8-21.1 thousand packets/s, not the nominal 32 thousand ceiling.
There is no catch-up pacing. CPU samples are only 0-30 ms, including a zero
counter delta in one trial, so **no paced CPU/energy improvement is claimed**.
Paced CPU-per-packet ratios are intentionally not ranked below.

## Complete cell summary

Each row summarizes three repetitions. Rates, rejection, CPU and probe p99
are medians; probe loss is a three-repetition total; timer max is the largest
observed value. p99 is survivor-only and must be read with loss counts.
Work is synthetic arithmetic rounds, not AES or TUN processing.
CPU is whole-model seconds per million delivered packets, not Core cost.
`n/r` means the short paced CPU sample is not resolved sufficiently to rank.

| Runtime | Work | Load | Quantum | Offered Mpps | Delivered Mpps | Reject % | CPU s/M delivered | Bulk probes lost/offered | Probe p99 us bulk/sparse | Timer max us |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| current-thread | 128 | paced | stock | 0.0208 | 0.0208 | 0.0000 | n/r | 0/194 | 19/23 | 1153 |
| current-thread | 128 | paced | 64 | 0.0208 | 0.0208 | 0.0000 | n/r | 0/194 | 19/22 | 1158 |
| current-thread | 128 | paced | 32 | 0.0209 | 0.0209 | 0.0000 | n/r | 0/194 | 10/11 | 1155 |
| current-thread | 128 | saturated | stock | 2.9307 | 2.8449 | 2.9283 | 0.3503 | 350/11929 | 40/49 | 1657 |
| current-thread | 128 | saturated | 64 | 2.8491 | 2.8491 | 0.0000 | 0.3500 | 0/11929 | 21/28 | 1881 |
| current-thread | 128 | saturated | 32 | 2.8125 | 2.8125 | 0.0000 | 0.3500 | 0/11929 | 9/16 | 1940 |
| current-thread | 512 | paced | stock | 0.0210 | 0.0210 | 0.0000 | n/r | 0/194 | 46/50 | 1205 |
| current-thread | 512 | paced | 64 | 0.0211 | 0.0211 | 0.0000 | n/r | 0/194 | 45/54 | 1202 |
| current-thread | 512 | paced | 32 | 0.0210 | 0.0210 | 0.0000 | n/r | 0/194 | 23/25 | 1210 |
| current-thread | 512 | saturated | stock | 1.3343 | 1.2953 | 2.9283 | 0.7726 | 350/11929 | 90/104 | 1401 |
| current-thread | 512 | saturated | 64 | 1.2940 | 1.2940 | 0.0000 | 0.7700 | 0/11929 | 46/56 | 1701 |
| current-thread | 512 | saturated | 32 | 1.2873 | 1.2873 | 0.0000 | 0.7700 | 0/11929 | 23/31 | 1661 |
| two-workers | 128 | paced | stock | 0.0211 | 0.0211 | 0.0000 | n/r | 0/194 | 26/18 | 1166 |
| two-workers | 128 | paced | 64 | 0.0210 | 0.0210 | 0.0000 | n/r | 0/194 | 27/16 | 1161 |
| two-workers | 128 | paced | 32 | 0.0210 | 0.0210 | 0.0000 | n/r | 0/194 | 37/16 | 1168 |
| two-workers | 128 | saturated | stock | 13.3685 | 3.1228 | 76.6408 | 0.8990 | 9098/11929 | 110/67 | 1948 |
| two-workers | 128 | saturated | 64 | 10.1944 | 3.8492 | 60.0457 | 0.6758 | 7593/11929 | 92/56 | 1631 |
| two-workers | 128 | saturated | 32 | 8.7747 | 3.1595 | 63.9932 | 0.8708 | 7583/11929 | 108/62 | 1530 |
| two-workers | 512 | paced | stock | 0.0209 | 0.0209 | 0.0000 | n/r | 0/194 | 52/17 | 1187 |
| two-workers | 512 | paced | 64 | 0.0209 | 0.0209 | 0.0000 | n/r | 0/194 | 53/18 | 1155 |
| two-workers | 512 | paced | 32 | 0.0210 | 0.0210 | 0.0000 | n/r | 0/194 | 43/28 | 1169 |
| two-workers | 512 | saturated | stock | 17.1769 | 0.8655 | 94.9612 | 3.1754 | 11349/11929 | 336/177 | 1922 |
| two-workers | 512 | saturated | 64 | 15.0531 | 0.9585 | 93.5870 | 2.8068 | 11191/11929 | 314/168 | 1828 |
| two-workers | 512 | saturated | 32 | 12.5336 | 0.9569 | 92.3239 | 2.8134 | 11024/11929 | 307/163 | 1940 |

All 72 trials satisfy packet conservation, accepted identity sums,
per-peer ordering, unchanged ring capacity/reservation, and bounded
completion. They offer 36,589,824 packets and reject 14,710,587; that loss is
reported, not turned into a PASS for Core. All 288 spawned model tasks join.
The full JSONL retains per-peer accepted/rejected/probe counts and per-trial
CPU, rates, maxima and explicit-yield counts.

## Source reconciliation and next step

The inspected `peer_manager.rs` and `peers/mod.rs` blobs match the actual
Core base `3166ab672d347cdcc5a6768bc77056cd8ec38323`:
`1bde22033533344792c273211e7d885801b517c5` and
`a380c6a0d7317b631b584c53dbbc0744c5b27997`, respectively.
`create_packet_recv_chan()` uses capacity 128. The actual receive task
serially performs foreign-network handling, decryption, metrics,
decompression/ACL and the registered filters before the bounded NIC
handoff. Source structure alone does not establish which stage dominates
the saturated failure.

Do not promote unconditional yield-32/yield-64 as a loss fix. Do not enlarge
the ring, retain rejected packets indefinitely, prioritize test probes, or
weaken the original ICMP assertion. The existing actual-Core loss remains
localized to remote lossy ring admission, but this model does not distinguish
scheduler starvation from genuine downstream service cost in that run.

The next diagnosis should reuse the retained exact diagnostic Core artifact
`10920492391` and collect an endpoint-separated CPU call graph under the
failed mixed UDP/IPv4-underlay, IPv6-overlay load. Keep the original data,
probe and cleanup checks. The old packaged-artifact reader expects a
different archive layout, so the diagnostic binary must go through its
existing verified diagnostic-artifact reuse path, not be relabeled as a
formal package. Compare receive/decrypt/filter/NIC work and wakeups before
selecting a production optimization. A profile is diagnostic, not a
throughput acceptance run.

