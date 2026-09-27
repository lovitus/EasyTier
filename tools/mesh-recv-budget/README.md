# Locked receive scheduling models

These are isolated mechanism experiments, not Core or network throughput tests.
The existing `mesh-recv-budget` binary and its consumer-batching experiment are
unchanged. Both binaries use the existing lockfile: Tokio 1.52.1,
async-ringbuf 0.3.1, ringbuf 0.4.6, and futures 0.3.30. No dependency update is
part of this experiment.

## Producer service quantum

The preceding actual-Core experiment, run 36284039979 at harness 2956e72c,
correlated an IPv6 echo to the remote UDP receive and then lossy ring rejection
with GRO **off**. Its functional assertion remains failed. See
`../mesh-udp-flush/RING_REJECTION_LOCALIZATION.md` for the complete evidence.

`producer-quantum` asks a narrower question: can an earlier scheduling
opportunity reduce ring admission rejection without increasing ring memory,
retaining rejected packets, or giving ordinary Data probes priority?

The source reference is Core 3166ab672d347cdcc5a6768bc77056cd8ec38323:
`RingTunnelSender::try_send`, `UdpConnection::handle_packet_from_remote`, and
`PeerConn::start_recv_loop`. The model preserves the 128-entry ring and four
reserved slots, plus a bounded ring-to-downstream handoff. The downstream
channel capacity of 128 and synthetic work are experimental controls, not a
claim that the entire Core pipeline or its channel capacities are reproduced.

- Arms: existing per-operation Tokio cooperation, plus explicit service
  quanta of 64 and 32 received packets. Every arm retains `consume_budget`.
- Two runtimes: current-thread and two worker threads. The former is a
  sensitivity case, not evidence that the deployed Core has one worker.
- Two downstream costs: 128 and 512 fixed arithmetic rounds per packet.
- Two loads: continuously ready producer and 64-packet pulses separated by
  two milliseconds. There is no catch-up after a late pulse.
- Three Latin-rotated repetitions per cell, 72 trials total. Saturated arms
  offer one million packets; paced arms offer 16,384.
- One producer serves two independent rings and a shared bounded downstream.
  Peer 0 carries the bulk stream and ordinary Data probes. Peer 1 is a sparse
  ordinary Data stream. All probes follow identical lossy admission and work.
  Coprime sampling intervals and changing offsets reduce phase-aliasing with
  the tested quanta. They are not a live ping/Pong implementation.

Failure risks are part of the comparison: a yield may not schedule the
consumer; a slower consumer may remain the bottleneck; more scheduling can
raise CPU cost or reduce goodput; admission loss can move rather than vanish;
and fewer delivered probes can make survivor-only latency look better.

Each trial records offered/delivered/rejected counts, accepted identity sums
and per-peer ordering, offered/delivered probe counts, survivor p99/max delay,
timer p99/max delay, measured packet rates, process CPU seconds and CPU per
million delivered packets. Queue capacity is unchanged. All four spawned
tasks must finish and be joined. Trials have a 30-second deadline; the whole
workflow run has its existing bounded timeout. No assertion requires an arm
to improve performance or hide a negative result.

The producer simulates one cooperative I/O operation per datagram. It does
**not** execute a real socket, readiness polling, encryption, TUN I/O or GRO.
Real Tokio UDP operations already cooperate; this experiment does not claim
the deployed receiver has no fairness mechanism. `yield_now` does not promise
another task runs first, as documented by [Tokio](https://docs.rs/tokio/1.52.1/tokio/task/fn.yield_now.html).
The exact locked registration/UDP implementation was inspected separately.

## Execution and interpretation

Use the existing `Mesh natural-cohort kernel mechanism` workflow on the exact
research branch with `recv_budget_only=true`, `recv_budget_mode=producer`, and
`udp_receive_only=false`. This builds only the selected small tool. It does
not build, modify, install or deploy EasyTier Core. The workflow captures the
source SHA, lockfile, CPU identity, binary hash and complete JSONL results.

The comparison is eligible only after every trial's conservation, ordering
and bounded completion checks pass. Report all cells, including regressions.
Compare paced CPU only at matched measured rates; saturated comparisons must
show delivered rate and rejection together. CPU includes model producer,
bridges, downstream work, timer and task teardown, not a receiver-only cost.
Timers use Tokio's millisecond clock; they are fairness indicators rather
than a microsecond-accurate networking latency benchmark.

Evidence status: **72 model trials completed**, with important negative
results in the multi-worker overload cases. See `PRODUCER_QUANTUM_RESULTS.md`.
Unconditional service quanta are not accepted as a production loss fix.
The original saturated Core acceptance remains open.
