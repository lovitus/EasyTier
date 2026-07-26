# Core UDP receive batching optimization

Status: PROBE ONLY; PRODUCTION IMPLEMENTATION NOT AUTHORIZED BY EVIDENCE

Date: 2026-07-26

## Evidence and hypothesis

Exact public-host tracing of the existing Core UDP transport observed roughly
258,000 `recvfrom` calls and 82,000 `sendto` calls in the sampled interval.
Source inspection at baseline
`35c81d5b62d3b5aed033f8f259ae24ae4f8680bc` confirms that the main connected
UDP receive loop awaits `recv_buf_from()` once per datagram.

The narrow hypothesis is:

> On Linux, one readiness event followed by bounded `recvmmsg(MSG_DONTWAIT)`
> may reduce receive syscalls and receiver CPU when multiple underlay datagrams
> are already queued, without changing packet contents, source identity,
> ordering, transport selection, or other platforms.

This does not imply that batching is useful. The prior GRO scratch experiment
proved that a valid isolated mechanism can still provide no real Core benefit.

## Probe stage

The standalone `tools/core-udp-batch-probe` must run on `192.168.2.160` before
any EasyTier source change.

Required evidence:

- repeated runs, not one best result;
- bursts 1, 2, 4, 8, 16, and 32;
- fixed 1,200-byte UDP payload;
- actual receive syscall count;
- receiver-thread CPU time;
- payload, packet order, source IP, and source-port verification;
- bounded runtime and receive buffer;
- no packet loss or silent truncation.

Initial acceptance requires:

- burst 1 CPU cost no more than 5% above Tokio `recv_from`;
- bursts 4-32 reduce receiver CPU by at least 15%;
- actual packets per receive syscall scale materially above 1;
- all correctness checks pass in every run.

If those conditions fail, stop. Do not modify EasyTier.

## Production stage, only after probe acceptance

The smallest acceptable implementation would be:

- Linux-only and initially behind an experimental feature;
- keep the existing Tokio socket creation, binding, mark, IPv4/IPv6, stealth,
  handshake, and connection ownership;
- replace only the established data receive primitive with a bounded
  readiness-driven batch reader;
- batch capacity no greater than the existing packet/ring capacity;
- no timer, fixed sleep, extra worker, unbounded queue, or packet reorder;
- drain immediately after each readiness notification;
- preserve every datagram's source `SocketAddr`;
- on unsupported syscall or initialization failure, use the existing
  per-datagram receive loop;
- cancellation and socket close must wake and terminate the same task;
- non-Linux builds must compile the unchanged legacy path.

The fallback contract is:

`Linux recvmmsg receive -> existing Tokio recv_from receive`

There is no fallback from a partially consumed batch. Every successfully
received datagram must be delivered once before the next readiness wait.

## Production rejection conditions

Reject the implementation if any of these occur:

- sparse burst CPU regression above 5%;
- source address, packet order, or stealth behavior changes;
- added coalescing latency;
- packet loss or truncation under socket pressure;
- cancellation delay or leaked task/FD;
- no repeatable Core CPU or throughput gain on the public dual-stack pair;
- a gain appears only with a forced synthetic burst unavailable in real Core
  traffic.

`sendmmsg` is explicitly out of scope. It requires a separate probe because
outbound packets may target different peers or require different framing and
failure accounting.
## 2026-07-26 standalone mechanism probe

The first three `.160` debug runs preserved every 1200-byte datagram, sequence,
source address, and source port. Median receiver-thread CPU per packet compared
with the current Tokio one-datagram receive loop was:

| queued burst | `recvmmsg` packets/syscall | CPU change |
| ---: | ---: | ---: |
| 1 | 1 | +60.7% |
| 2 | 2 | +6.0% |
| 4 | 4 | -28.5% |
| 8 | 8 | -36.0% |
| 16 | 16 | -39.9% |
| 32 | 32 | -49.0% |

Therefore an unconditional `recvmmsg` replacement is `REJECTED`: it violates
the singleton-load regression gate. The next probe keeps the existing Tokio
single receive path below 64 packets per 5 ms and enables batching only for a
sustained high-rate flow. It exits batch mode after a 32-call window carries at
most 40 packets. These thresholds are experimental and cannot enter production
until low, high, and low-high-low simulations show bounded transition behavior.

The adaptive receive experiment is also `REJECTED`. Across three runs:

- Low traffic stayed entirely on the existing single-packet path.
- Continuous high traffic achieved only 1.028-1.046 packets per receive syscall.
- The mode changed to batch and back 1,587-1,618 times per 160,000 packets.
- The mixed low-high-low case achieved only 1.015-1.038 packets per syscall and
  changed mode about 800 times.

Packet rate alone does not prove that the kernel receive queue contains a useful
batch. Adding thresholds or longer hysteresis would hide rather than remove this
uncertainty. No receive-side production change is justified by this evidence.

The next independent hypothesis is send-side batching only. A userspace writer
queue can reveal whether more packets are already present without another
syscall: await one item, drain up to 31 immediately available items, preserve the
existing single-packet send path for one item, and call `sendmmsg` only for an
actual multi-packet batch. This must be rejected unless singleton behavior is
identical and high-load syscall/CPU reduction is repeatable.

The three-run `.160` send-side probe passed its mechanism gates:

| load | path | median packets/syscall | median wall ns/packet | median CPU ns/packet |
| --- | --- | ---: | ---: | ---: |
| one packet/ms | current single send | 1.000 | 1,078,688 | 38,730 |
| one packet/ms | drain then conditional batch | 1.000 | 1,080,893 | 38,169 |
| queued high load | current single send | 1.000 | 17,965 | 17,940 |
| queued high load | drain then conditional batch | 31.994 | 5,540 | 5,539 |

At low load the candidate never invoked `sendmmsg`; median wall time changed by
+0.2% and receiver-thread CPU by -1.4%. At queued high load it reduced send
syscalls from 160,000 to 5,001 and wall/CPU time per packet by about 69%. Every
1200-byte datagram arrived in sequence with no loss in the bounded simulation.

This is evidence for the primitive, not yet for EasyTier. Before touching Core,
the exact established UDP writer must be shown to have an existing bounded queue
that can be drained without changing cancellation, backpressure, destination,
error, or packet-order semantics. A cross-host probe must then cover partial
send, nonblocking `EAGAIN`, and a real public 10 Gbps IPv4/IPv6 pair.

The exact candidate inspection found an existing `AsyncHeapRb<ZCPacket>` with
capacity 128 in both established UDP directions. The writer currently awaits one
`RingStream` item and calls `send_to` once. No new queue, timer, worker, or
cross-connection scheduler is required.

The static debug cross-host probe then passed on the public 10 Gbps dual-stack
pair:

| network | packets | payload | successful send syscalls | packets/syscall | sender payload rate | result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| IPv4 | 64,000 | 1200 bytes | 2,000 | 32.000 | 134.5 Mbit/s | exact sequence PASS |
| IPv6 | 64,000 | 1200 bytes | 2,025 | 31.605 | 135.3 Mbit/s | exact sequence PASS |

The IPv6 run required 25 extra successful syscalls, exercising partial batch
completion; the unsent suffix resumed without duplicate, loss, or reorder. An
initial approximately 300 Mbit/s debug-receiver run lost nine packets after
sequence 59,638 and was rejected as receiver saturation evidence, not attributed
to `sendmmsg`. Reducing the bounded probe to approximately 135 Mbit/s closed both
address families cleanly.

Production scope is now limited to the Linux established UDP writer:

- Await the first ring item, then nonblocking-drain at most 31 existing items.
- Preserve the original single-item `UdpSocket::send_to` call.
- Use nonblocking `sendmmsg` only for an actual multi-item batch.
- Preserve the exact non-Linux loop without prefetching.
- Keep per-datagram header conversion and stealth sealing in ring order.
- Retry only an unsent suffix after partial completion or `EAGAIN`.
- Do not change receive, handshake, queue capacity, control packets, or protocol.

## Real Core result: production candidate rejected

The two-file production prototype compiled for GNU and musl, and its five
focused tests passed. A static musl Core was then run on the public pair with a
forced UDP underlay and stealth enabled.

The optimization did not trigger in real Core traffic:

| workload | observed send syscalls | `sendmmsg` calls |
| --- | --- | ---: |
| four TCP streams over mesh, about 187 Mbit/s sender result | 911 `sendto` in the initial narrow trace | 0 |
| 1200-byte UDP high load | 73,402 `sendmsg` + 14,955 `sendto` | 0 |
| 64-byte UDP high-PPS load | 72,594 `sendmsg` + 4,077 `sendto` | 0 |

The latter traces included `sendto`, `sendmsg`, and `sendmmsg`. Even when the
debug receiver was saturated and dropped traffic, the established writer did
not find a second item waiting in its per-connection ring. The real producer and
writer remain effectively lockstep; the userspace queue condition demonstrated
by the standalone probe does not occur in these Core paths.

Status: `FAILED_NO_REAL_CORE_EFFECT`. Remove the `ring.rs`/`udp.rs` production
prototype. Keep the standalone tools and this evidence so the same idea is not
reintroduced without proof that a future architecture actually forms writer
backlog. The evidence does not justify a workflow or optimized release
candidate.

## Cross-host receive follow-up: also rejected

A separate static probe compared `recvfrom` and `recvmmsg` on the same public
IPv4 path:

| offered case | receive mode | result | receive syscalls | receiver CPU |
| --- | --- | --- | ---: | ---: |
| 64,000 x 1200 bytes, about 70 Mbit/s | `recvfrom` | PASS | 64,000 | 241.1 ms |
| 64,000 x 1200 bytes, about 70 Mbit/s | `recvmmsg` | PASS | 3,945 | 233.7 ms |
| 64,000 x 1200 bytes, about 135 Mbit/s | `recvfrom` | PASS | 64,000 | 222.8 ms |
| 64,000 x 1200 bytes, about 135 Mbit/s | `recvmmsg` | FAIL, 72-packet gap | N/A | N/A |

At the passing rate, `recvmmsg` averaged 16.2 packets per syscall but improved
thread CPU by only about 3.1%, below the 15% acceptance gate. At the higher rate,
the single receive path passed while the batch path lost packets. This evidence
does not justify unconditional, sampled, or rate-adaptive receive batching in
Core.

An all-syscall trace of a forced-UDP Core run showed that syscall wall time is
not a CPU profile: `epoll_pwait` and `futex` represented 86.9% because waiting
across all runtime threads is accumulated. The active data calls were 33,789
`sendmsg` and 36,803 TUN `read` calls in the bounded run, each about 5.5% of
reported syscall time. No further code change may be chosen from these counts
without user-space symbol or bounded stage timing evidence.

## Post-batching Core hot-path investigation

Base source: `35c81d5b62d3b5aed033f8f259ae24ae4f8680bc`.

The existing `hotpath-cpu` feature was built first. Container-level CPU sampling
was blocked by `perf_event_paranoid=2`; no sysctl or container capability was
changed. Function timing was still usable, but it included async wait time and
reduced the same isolated two-node throughput to 379-449 Mbit/s. The plain
optimized-dev musl Core reached about 1.035 Gbit/s. Hotpath timing throughput is
therefore rejected as production performance evidence.

Two controlled parameter hypotheses were rejected before changing production
code:

- Four Tokio runtime threads produced only 276-297 Mbit/s, versus 403-426
  Mbit/s with the default runtime in the instrumented build. It also increased
  CPU per byte. Do not enable `multi-thread` as a throughput optimization.
- Disabling encryption did not improve the instrumented median over default
  AES-GCM. ChaCha20 appeared faster in that ordered run, but the result is
  confounded by instrumentation and run order. Do not change the default cipher
  or protocol compatibility from this evidence.

Host-root system-wide sampling succeeded without changing kernel policy. A
five-second, zero-loss sample of the plain Core under a 4 GiB fixed upload
resolved the leading user-space costs as:

| symbol or group | sample share |
| --- | ---: |
| AES-GCM hardware routines | approximately 14-15% |
| musl cancellable syscall entry | 9.44% |
| `memcpy` | 5.50% |
| `PeerManager::send_msg_by_ip` | 2.02% |
| `quanta::Instant::now` | 1.74% |
| deep `Vec` clone alone | 1.10% |

The profile also contained string/key cloning, allocation/deallocation, metric
label hashing, and `TrafficMetricRecorder` work. Source inspection found a
specific avoidable operation: every resolved peer-cache hit cloned two
`CounterHandle` values, and each handle clone deep-cloned its diagnostic
`MetricKey`, label vector, and strings before updating the same shared
`MetricData`.

The standalone `metrics_hotpath` probe models that exact ownership shape with
the same `quanta` clock. Five runs of two million updates on `.160` produced:

| path | median ns/sample |
| --- | ---: |
| clone two handles and touch twice | 1290.05 |
| borrow handles and touch twice | 419.44 |
| borrow handles and share one timestamp | 279.23 |

Only the first step entered the production experiment. The resolved cache-hit
path now borrows `TrafficCounters` while its DashMap read guard is alive.
Cache-miss and instance-ID-upgrade paths still clone owned handles before their
entry guards are released. Timestamp frequency and precision are unchanged.
The implementation changes no packet, route, protocol, metric name, label,
cache invalidation, or platform behavior.

`.160` evidence:

- GNU `easytier --lib` no-run: PASS.
- Existing `traffic_metrics::tests::`: 3 passed, 0 failed.
- Patched x86_64-musl Core: static PIE with debug info, build ID
  `b451c6ae1a161b0b9af83e4d5f53b7f744da91c4`.
- Baseline binary SHA-256:
  `cdcb730fd5763db3695686d62b1783f0439917bd51f0af01f786e5aee0f2d226`.
- Candidate binary SHA-256:
  `a27440cabd6a0b3b6bd4ae26235e7ddca74a91a125af9f662970175f748153ca`.

The real Core A/B used the same isolated namespaces, forced UDP underlay, two
1 GiB uploads per process lifetime, and interleaved order
baseline/candidate/candidate/baseline:

| result | baseline mean | candidate mean | change |
| --- | ---: | ---: | ---: |
| payload throughput | 1.025 Gbit/s | 1.063 Gbit/s | +3.63% |
| sender CPU ticks per 2 GiB | 3148.5 | 3033.0 | -3.67% |
| receiver CPU ticks per 2 GiB | 3303.0 | 3184.5 | -3.59% |

RSS stayed within ordinary run noise (about 26.5-27.5 MiB). Every transfer
reported the exact requested byte count. Status:
`ACCEPTED_FOR_CANDIDATE_PREFLIGHT`.

The shared-timestamp variant remains deferred. Its likely additional whole-Core
gain is small and it would expand the change into `StatsManager`; it is not
justified in this batch. The accepted cache-borrow change is deliberately
limited to one production file and does not justify changing encryption,
runtime threading, UDP batching, or cross-platform APIs.

## Candidate dispatch manifest

Intended build snapshot:

- Borrow resolved per-peer traffic counters instead of deep-cloning their
  handles on every packet.
- Preserve all rejected UDP batching implementations as documentation and
  standalone probes only.
- Include the standalone probe crate, its lockfile, and this investigation
  record; no other production source is part of the candidate.

Pre-push gates:

- `.160` GNU `easytier --lib` no-run build and all three traffic-metric tests:
  PASS.
- `.160` x86_64-musl Core build and isolated baseline/candidate A/B: PASS.
- `.160` standalone `metrics_hotpath` build and execution: PASS.
- `.160` all standalone probe binaries `--locked` build: PASS.
- Lockfile, platform `cfg`, credentials, whitespace, generated/binary files,
  and complete candidate diff review: PASS.

Required GitHub work:

- One rolling `profiling-beta` build for the complete immutable snapshot.
- No separate workflow for the documentation or probe tools.
- Android candidate is not required for this Linux performance-only evidence;
  the generic Rust unit behavior remains covered by formal platform workflows
  when this change later enters a release candidate.

Planned artifact evidence:

- Verify profiling asset checksum, `BUILD_INFO`, commit SHA, build ID, symbols,
  and target.
- Reuse the same bounded functional and throughput matrix; do not reopen the
  rejected batching hypotheses.

Work during the workflow wait:

- Prepare artifact verification and bounded deployment commands.
- Review only independent documentation and release evidence; do not mutate the
  immutable candidate.
