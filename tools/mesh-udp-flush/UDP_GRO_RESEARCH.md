# GSO supply and UDP GRO receive: bounded mechanism research

Status: prepared, not executed. No production Core change, merge or release.
The original combined saturation failure remains open.

## Why this differs from the rejected recvmmsg experiment

The exact uninstrumented performance comparison is documented in
[COMBINED_PACKAGED.md](COMBINED_PACKAGED.md). Its separate diagnostic sendmsg
capture in run `36260893872` records 5,823 successful calls, no sendmsg errors
and no unparsed call records. Of those, 5,796 have UDP_SEGMENT metadata and
63,260 iovecs/datagrams: mean 10.914 per GSO call, median 8, maximum 32.
The remaining 27 calls have one iovec. Timing under tracing is excluded from
performance conclusions. Each GSO iovec is a frame in the inspected source
`3166ab67`, so these counts describe actual submitted groups, not a configured
maximum presented as observed occupancy.

The prior standalone recvmsg/recvmmsg trial used individual sends and only
1.3-1.4 datagrams per nonempty syscall. Its negative decision remains valid
for that experiment; it did not test receive offload against actual GSO
supply. This is a distinct hypothesis, not repeated tuning until a pass.

Linux [udp(7)](https://man7.org/linux/man-pages/man7/udp.7.html) documents
UDP_GRO as the receive counterpart to UDP_SEGMENT, returning segment size
as ancillary metadata. The Linux v6.8
[receiver selftest](https://github.com/torvalds/linux/blob/v6.8/tools/testing/selftests/net/udpgso_bench_rx.c)
reads that metadata as a native int. The upstream correction
[`436864095a95`](https://github.com/torvalds/linux/commit/436864095a95fcc611c20c44a111985fa9848730)
explains why treating it as a u16 breaks big-endian systems. These are API
references, not imported kernel code or a cross-platform performance claim.

## Exact experiment boundary

- New standalone Rust binary `udp_gro` in the existing probe crate; unchanged
  dependencies and Cargo.lock, no Core compilation.
- Existing `mesh-natural-cohort.yml`, `udp_receive_only=true`,
  `udp_receive_mode=gro`; other experiment modes retain their previous entry.
- Two loopback UDP endpoints, separately IPv4 and IPv6, on a hosted Linux
  runner. One producer and one receiver thread; no private host or WAN.
- Sender uses UDP_SEGMENT with at most eight datagrams, matching the observed
  median group. Both comparison arms have exactly the same sender.
- Receiver compares GRO off/on using recvmsg, identical socket receive
  buffers and one reusable 64 KiB userspace buffer. No recvmmsg or larger
  Core ring. Up to 32,768 bounded control-latency samples per trial.
- Three interleaved repetitions per family, paced/unpaced supply and GRO
  off/on: 24 trials. Each supplies for two seconds, with bounded drain and a
  six-second trial deadline; workflow execution is capped at 120 seconds.
- Datagrams have sequence, timestamp and deterministic payload. Every
  delivered datagram is checked for length, all payload bytes, exact source,
  duplicates/order and truncation. 333-byte tails exercise a short final
  segment; 64-byte controls are not appended to preceding data groups.
- CPU uses CLOCK_THREAD_CPUTIME_ID for both producer and receiver. Output
  includes delivered goodput, sent/received/lost counts, control loss and
  delivered-control latency, syscall counts, actual aggregate size and polls.

## Failure modes and decision rules

Missing or truncated ancillary metadata, wrong segmentation/source/payload,
unsupported socket options, a deadline failure or a requested GRO mechanism
that never activates fail the run. No silent fallback is counted as GRO.
All loss remains explicitly reported, including saturation loss. A workflow
pass means accounting and data checks completed, not a loss-free Core path.

The first decision is whether materially lower receiver CPU/GiB survives
both families and paced supply without hiding loss or control degradation.
Short timings and sender-limited loopback goodput require caution. A smaller
syscall count alone is not success. If the mechanism is not useful, stop here.

Even a positive result is not Core acceptance: GRO can increase the burst
delivered to an existing bounded ring. Any later integration must preserve
per-datagram Stealth/crypto/framing, fairness and current queue capacities,
and must repeat exact-Core control-progress and cleanup checks. This tool
does not exercise those semantics, Tokio scheduling, real NIC offload, old
kernel fallback or any non-Linux platform.

The 100 ms idle prelude and 20 ms poll timeout are explicit tool mechanics,
not evidence about event-driven Core idle power. No new Core configuration,
buffer budget, dependency or production public API is introduced.
