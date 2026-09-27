# Linux mesh TUN: reserve the GRO head for data

Status: implementation prepared; actual-Core regression and artifact acceptance
are pending. This is not a release or a resolution of the mesh performance goal.

## Source and measured mechanism

- Production parent: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
- The parent combines the existing bounded Linux TUN head with the separately
  validated secure-relay destination guard. Neither change is replaced here.
- Dependencies stay locked: `tun-rs 2.8.7`, `bytes 1.9.0`; no dependency update.
- [Locked standalone experiment](https://github.com/lovitus/EasyTier/actions/runs/36290522251):
  identical packet bytes and order, IPv4/IPv6, four TCP control kinds and three
  positions. All eight leading-control pairs changed from three emissions to
  two; the sixteen other pairs retained two emissions. Control-only batches,
  packet bytes and allocation ownership survived.
- That experiment used a standalone selector and the real locked GRO library.
  It is mechanism evidence, not an executed regression in EasyTier Core or a CPU
  improvement measurement. Do not count it as either.
- [Mixed actual-Core profiles](https://github.com/lovitus/EasyTier/actions/runs/36289395042)
  contained 950 TUN-write stacks in 4,896 raw samples (19.40%). This includes
  kernel TCP delivery and is neither wholly removable work nor an additive CPU
  saving estimate. Both endpoints sent and received concurrently.

## Minimal change and preserved behavior

Only `LinuxTunOffloadSink`'s existing head selection changes. Payload-bearing
ACK/PSH-ACK TCP packets may use the one reusable 8 KiB buffer. Pure ACK, SYN, RST
and FIN packets remain in their original buffers and still reach the existing
GRO/write path. Header length is checked before payload eligibility. The locked
library still owns checksum, sequence, options and merge validation.

There is no new queue, allocation bound, batching delay, runtime option, protocol,
route, encryption or non-Linux path. Existing buffer identity recovery, partial
write error propagation, cancellation ownership and legacy fallback are unchanged.
The optimization does not reorder the input batch or replace the library's GRO
ordering policy.

## Failure modes and regression contract

- A control packet must not consume the only useful merge buffer.
- Rejecting a packet for promotion must not reject/drop it from the write path.
- IPv4/IPv6 control placement must preserve all bytes and multiplicities after
  real GRO/GSO round-trip, with no allocation growth.
- An all-control batch must leave both packets and the reserved buffer untouched.
- Existing tests for shared slices, checksum rejection, prepend relocation,
  partial writes and cancellation continue unchanged.

The new table-driven Core test is
`gro_head_skips_control_packets_without_changing_packet_bytes`. Its regression
predicate is two emissions for control plus two adjacent data segments. The
unchanged parent selects a leading control and emits three. Actual execution on
the unchanged and corrected production implementations remains required; source
inspection and the standalone result do not satisfy that gate.

## Acceptance and rollback boundary

1. Establish behavioral red/green on the actual Core test without changing test
   assertions or adding a bespoke workflow for this predicate.
2. Run the existing Linux TUN lifetime/error regressions and normal static gates.
3. Build one optimized, no-Leaf candidate and reuse the existing parent artifact.
4. Compare identical endpoint topology, IP family, load and repetitions. Record
   achieved rate, CPU/GiB, control latency/loss, retransmissions, RSS and cleanup.
   Keep traced activation separate from untraced performance measurements.
5. Do not merge or publish on mechanism-only evidence. If actual benefit is
   negligible or correctness regresses, withdraw only this selector patch.

The independent receive-ring overflow and earlier saturation ICMP failures remain
open. This change must not be represented as solving them or the pre-existing
fork-wide bandwidth/CPU problem. No new original-host, WAN or all-platform result
is claimed.
