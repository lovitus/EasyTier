# GSO supply and UDP GRO receive: bounded mechanism research

Status: 43 Core contracts and the actual-Core off/on comparison completed.
CPU/goodput benefit is measured; saturation retransmission and latency trade-offs
remain unresolved. Workflow success is not production acceptance.
No production Core change, merge or release.
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

## Result: 24 completed trials, meaningful CPU saving but no saturation clearance

[Run 36267284991](https://github.com/lovitus/EasyTier/actions/runs/36267284991)
succeeded at exact source `8cb045fda9c83d62ecb434ceaf7de259d34d2862`.
Both endpoints were local UDP sockets on the same GitHub Ubuntu runner:
Intel Xeon 6973P-C, four logical CPUs / two cores, Linux
`6.8.0-1064-azure`, x86_64. IPv4 and IPv6 were separate trials, not a WAN
path or a mixed-family connection. No Core process was built or run.

The locked probe build took 10.83 seconds. The artifact records probe SHA-256
`ea7607c420cd8a9b3866fe69dbd9ccaf5eab1490c9392b2651f79faee774913c`.
This is the runner's binary-hash record, not a separately downloaded and
rehashed executable. Evidence artifact `10914064389` is 9,875 bytes; its full
ZIP SHA-256 and all eight members' CRCs were verified:
`9cb7e50a0ee5d008dde74f434cfd7cec1751ddb7cf53a565c2f8fb62c3eb8f51`.
Its source SHA matches the run. All eight arm/family/load groups have exactly
three distinct repetitions. Effective SO_RCVBUF is 524,288 bytes in every
trial. Build output and Cargo.lock are retained with the evidence; the tool
does not instantiate a Tokio runtime.

### Paced supply: three-trial medians

CPU units are seconds per delivered GiB. Total CPU includes the standalone
producer and receiver, not two EasyTier Core processes.

| Family | GRO | Delivered Mbit/s | Receiver CPU | Total CPU | Delivered-control p99 median, us |
| --- | --- | ---: | ---: | ---: | ---: |
| IPv4 | off | 637.33 | 1.2097 | 1.8999 | 36 |
| IPv4 | on | 649.83 | 0.5760 | 1.0013 | 32 |
| IPv6 | off | 642.55 | 1.0864 | 1.6657 | 32 |
| IPv6 | on | 652.03 | 0.5844 | 0.9660 | 42 |

All twelve paced trials have zero data loss and zero control loss, across
1,389,888 delivered datagrams. Receiver CPU/GiB falls 52.38% on IPv4 and
46.21% on IPv6; total producer-plus-receiver CPU/GiB falls 47.30% and 42.01%.
GRO delivers about 7.732 datagrams per nonempty call versus exactly one
without GRO; maximum observed receive batch is eight. Short final segments
and single controls passed the same length/source/payload checks.

Receiver CPU sample ranges are 1.1532-1.2250 versus 0.5732-0.6197 on IPv4,
and 1.0086-1.0918 versus 0.5102-0.6037 on IPv6. These support the direction
within this short mechanism trial, not a precision guarantee on deployed
Core. Relative sleeping makes the paced offered rate slightly dependent on
sender work; rates differ by about 1-2%, so they are not exactly equal-load
or externally clocked trials.

IPv6 control latency did not remain identical: p99 samples move from
31/35/32 to 50/42/41 microseconds. Do not describe this as no control-latency
regression. No control was lost at this paced load, but the ten-microsecond
median increase needs evaluation in an actual Core scheduling context.

### Saturation: overload remains visible

| Family | GRO | Delivered Mbit/s median | Receiver CPU/GiB median | Sent, all three trials | Received, all three trials |
| --- | --- | ---: | ---: | ---: | ---: |
| IPv4 | off | 9,939.81 | 0.8643 | 12,956,160 | 5,350,204 |
| IPv4 | on | 21,238.08 | 0.4045 | 24,310,208 | 11,474,324 |
| IPv6 | off | 10,197.24 | 0.8425 | 15,215,808 | 5,475,005 |
| IPv6 | on | 21,533.70 | 0.3990 | 32,552,192 | 11,606,045 |

Every saturation trial loses data and controls. Offered traffic also rises
substantially with GRO, so the approximately doubled delivered goodput is
not a like-for-like offered-rate comparison and absolutely not a loss-free
Core throughput claim. GRO is not congestion control and does not repair
the separately observed Core receive-ring rejection. No missing packet was
waived or filtered from these totals.

## Disposition and next boundary

The paced result supports a narrowly scoped actual-Core experiment, rather
than discarding receive offload based on the earlier individual-send model.
It does not yet authorize a production receive-path change or release.
Before an overlay, reconcile the socket owners and all receive call sites;
GRO must never be enabled on a socket still read by an unaware handshake,
STUN or other datagram consumer. Split using validated metadata before
per-datagram Stealth/authentication, and preserve source identity, packet
ownership, existing queue bounds and cooperative scheduling.

The next candidate must be experimental and compared against the retained
`3166ab67` binary. Do not claim the 46-52% receiver mechanism saving as a
Core saving, introduce an unbounded pending queue, or use this result to
clear run `36260893872`'s missing IPv6 echo. Original-host acceptance also
remains outstanding; this evidence is hosted-runner loopback only.

## Isolated Core adapter: ownership audit and pending acceptance

Status: EXPERIMENT ONLY. The frozen Core remains
`3166ab672d347cdcc5a6768bc77056cd8ec38323`; the earlier combined
saturation failure is not cleared by the standalone mechanism result.

Audited ownership at that exact source:

- `StunClientBuilder::stop` aborts and joins its receive tasks. The successful
  `get_udp_port_mapping_with_socket` path awaits it before returning the socket.
- `UdpSocketArray::add_new_socket` publishes a punched socket and then breaks
  its read loop. Subsequent array operations only send or remove that entry.
- `UdpConnectAttempt` finishes SACK negotiation before `build_tunnel` creates
  its data receiver. Failed handshakes returned to the punch array never
  enable this experiment's GRO option.
- `UdpTunnelListenerData::do_forward_task` is the listener's sole reader;
  its STUN replies and hole-punch forwarding use send-only socket clones.
- The adapter therefore enters only the listener data loop and post-SACK
  connector data loop. It is not added to generic bind, STUN, or SACK reads.
  This audit does not claim arbitrary external callers may share raw readers
  with an established tunnel.

The disposable adapter copies each segment into the existing `BytesMut` spare
capacity before existing framing, Stealth authentication and ring submission.
It does not put references to a large shared allocation into queued packets.
The scratch cost is exactly 65,536 bytes per enabled receive owner, plus small
metadata; it is not memory-neutral. Queue capacities, MTU, send batching,
connection selection and wire format are unchanged. Unsupported GRO uses the
original reader. Ready buffered segments consume Tokio cooperative budget.
Truncated/invalid ancillary batches are discarded rather than parsed as packets.
Ordinary datagram truncation retains the existing output-capacity boundary.

The experiment adds no production option, dependency or public API. Its
`ET_ISSUE4_UDP_GRO=on/off` selector and shutdown-only counters exist only in a
GitHub runner overlay. The same binary is compared interleaved, using unchanged
`lab.py` integrity, ICMP, resource and cleanup assertions. Tests include a real
kernel negative control that bypasses splitting, IPv4/IPv6 boundaries and short
tails, interleaved source/control datagrams, zero-byte datagrams, bounded queued
allocations and current-thread cooperative progress. Existing UDP/Stealth and
hole-punch listener tests run with the adapter enabled.

Compiler/contracts subsequently passed in run `36271631477` (39 UDP and four
hole-punch tests). Pending gates: actual Core GRO occupancy, fixed-load CPU/RSS,
saturated and opposite-direction mixed load. The first failure stops its phase
and remains evidence. No production benefit, loss fix, WAN result, unsupported
platform benefit or release acceptance is asserted before these gates finish.

### First isolated Core attempt: validation environment failure

Run `36268964745`, harness
`4e14d18a5e87f361303c82e566d91b7ef6a50e2c`, completed with **FAIL**.
The immutable base remains `3166ab672d347cdcc5a6768bc77056cd8ec38323`.

- The optimized Core test library compiled. The selected UDP suite passed
  **39/39** in 22.89 seconds, including the three receive-adapter contracts.
- IPv4 and IPv6 kernel negative controls each returned one **11,533-byte**
  aggregate without splitting, rather than the required 1,400-byte first
  datagram. The split path preserved all tested datagram bytes/boundaries.
  This is an explicit splitter-bypass negative control, not a claim that the
  unchanged GRO-disabled production baseline violates datagram boundaries.
- Both selected plain/Stealth hole-punch handshake cases passed. The wider
  `hole_punch_listener_` filter also selected two UPnP integration cases.
  Both failed in `UpnpIntegrationEnv::new`, before constructing the listener,
  at `ip link add name br_upnp type bridge`: **Operation not permitted**.
- The experimental workflow omitted the formal Test workflow's privileged
  namespace/UPnP fixture setup. This is a real validation-environment defect,
  not a passing run, not an established GRO regression and not an excuse to
  waive or remove the two tests.
- The standalone Core/CLI build and all actual-Core performance comparisons
  were skipped after this failure. There is **no actual-Core GRO throughput,
  CPU or RSS improvement result** from this run.

Evidence artifact `10915795267` is 102,856 bytes. Its full ZIP SHA-256 is
`24e466fe648336defccefb496f3936d617f20fd03e016caa926056b8513b3dd6`.
All 13 ZIP members passed CRC checks and all 12 manifest entries matched their
SHA-256 values. Raw logs remain private. The prior combined saturation failure
is still open.

The narrowly scoped correction is to use the existing formal Test environment:
UPnP/iptables dependencies, bridge netfilter settings and privileged test-binary
execution. Preserve every current assertion and the failed run; do not modify
Core, the receive adapter, test expectations or the peer/queue policy to address
this setup error. Permission to correct this newly introduced workflow defect
has been requested separately. No automatic retry has been dispatched.

### Fixture-only continuation

The continuation corrects only the isolated workflow environment using the
formal Test job's existing prerequisites: bridge/UPnP/iptables packages,
`br_netfilter` settings, loopback IPv6 fixture, and privileged test execution.
Cargo still compiles and caches as the runner user; Cargo's target runner
executes only the resulting test process with `sudo -E`. The exact Core source,
receive adapter, both test filters, every test expectation and all lab assertions
are unchanged. The first failed run remains the diagnostic record; this is not
a same-SHA retry or a performance result. One corrected experiment is dispatched.

### Second attempt: test command recompiled before its run deadline

Run `36270378757`, harness
`a165f0944eeb931ce5bd498fa4b6fb5bd7ce80da`, is **FAIL**, not a passing
UPnP result. The privileged UDP invocation again passed **39/39** in 23.10
seconds. The next `cargo test` invocation printed `Compiling easytier` and
exited **124** at its 120-second command limit, before any of the four selected
hole-punch tests started. The initial permission failure was not re-observed,
but its affected UPnP behaviors are still unverified in this experiment.
The retained output does not identify the fingerprint which caused Cargo to
recompile; do not claim a proven cache or source-mutation root cause.

Evidence artifact `10915304489`, 92,047 bytes, has ZIP SHA-256
`eef7ee52731ab995e1a4143028c91a64a5c4179e23a182f9c0ab4cad45c60560`.
All 13 members passed CRC and all 12 manifest digests matched. Core binary build
and performance phases again did not run. Both prior failures remain visible.

The narrow harness correction separates compilation from execution, following
formal Test's compile/archive/run model: one `cargo test --no-run` with machine
readable artifact identity, then both unchanged filters execute the exact same
checksummed test binary under `sudo -E`. Both execution deadlines are unchanged;
no test assertion, filter, adapter or production behavior changes. The test
binary is retained in the existing experiment artifact even on later failure,
so future harness diagnosis need not discard the expensive compilation result.

### Third attempt: contracts complete; missing metrics initially misattributed

Run `36271631477`, harness
`d06292a42ac3c1d0b7f020b4e7b8a535a95e6f17`, remains **FAIL**.
All **39 UDP tests** and **4 hole-punch tests**, including both UPnP namespace
cases, passed (22.76 s and 3.13 s). Optimized Core, CLI and the exact test binary
were produced. The first fixed-load off case completed both 64 MiB transfers,
UDP echo, byte-integrity and zero-loss ICMP checks. Both Core processes exited
0 without forced kill; both namespaces were empty on deletion and host routes
were unchanged. No on sample or A/B performance result was collected.

The harness then failed because there were **zero** shutdown metric files.
The first explanation attributed this to diagnostic `Receiver::drop` writes
not completing before `shutdown_background()`. That attribution was not proved
and is withdrawn: the fourth attempt established that the existing lab removes
the experiment's environment variables before spawning Core. This includes both
the activation option and the old metrics path. Non-waiting runtime shutdown
is still not a guarantee of diagnostic destructor writes, but it is not the
demonstrated cause of this sample. Do not change product shutdown to accommodate
the observation tool. Neither run proves a Core GRO regression or product leak.

Evidence artifact `10916103565` (183,728 bytes), ZIP SHA-256:
`4ec18e240a098ad011e11160b171982e915e360be4daa71461813a42a8820737`.
All 64 members passed CRC; all 63 manifest digests matched.
Binary artifact `10915569351` (210,751,920 bytes), published ZIP digest:
`9532c659b5c51e280bc88f5af7d9afe5f4cdfd3a8d2695471282c4133f550cb3`.
The large binary artifact stays on GitHub; its bytes will be verified there.
Recorded Core SHA-256:
`ae0d57a5c1c7d9568ff3158fcfd0c8b544a0a1cf236aab4b36706ca28d6b7777`;
Build ID `7e874215b5dcf724df2af1461d1814eb75eedd9b`.

Continuation reuses those immutable binaries without compiling. Activation is
verified independently through bounded `setsockopt`/`recvmsg` traces, with no
payload dump: off must have no GRO activation; on must have successful enabling
and GRO aggregates from both endpoint source addresses. IPv4/IPv6 with Stealth
are observed. Traced runs are explicitly excluded from performance statistics.
The subsequent untraced fixed/saturated/mixed matrix keeps the original lab's
traffic, integrity, ICMP and cleanup assertions. The old Drop-only diagnostic
assertion is replaced because its premise was false, not because packet or
lifecycle acceptance is relaxed. No receive-adapter or production change is
included. Earlier failures remain recorded and actual Core benefit is pending.

### Fourth attempt: the lab removes the experiment selector

Run `36273977755`, harness `00c191f7f099d31e4e0ac687cfe02dd909acb20c`,
reused artifact `10915569351` after outer and inner digest checks on the runner.
There was no Core compilation. The run is **FAIL**, not a completed off/on
comparison. Both IPv4/Stealth cases completed the existing traffic and cleanup
checks, but both recorded zero successful UDP_GRO enables and zero GRO receives.
The activation assertion stopped the run before IPv6 or untraced performance.

The source boundary explains this directly. `gro_lab.py` puts
`ET_ISSUE4_UDP_GRO` in its child environment, while `lab.py::run` constructs each
Core environment by excluding every key beginning with `ET_`. The two case
labels therefore both ran GRO-off. The earlier metrics path was also removed.
The external syscall observer correctly caught this; do not remove its assertion
or present equal off-path samples as a negative GRO performance result.

Evidence artifact `10916640738` is 248,136 bytes, with ZIP SHA-256
`fd3e49dbf9cea731f164fbe1e8b805e6806d507f1cc0f295f32bf9ffb4a988a8`.
All 362 ZIP members passed CRC and all 361 manifest digests matched. Each case
has 11 clean cleanup entries and unchanged host routes. Traced timings are not
performance evidence. The contract results from the separately executed test
binary remain valid; they did not use this lab's filtered Core environment.

The maintainer approved the correction. The lab now takes an explicit optional
`--udp-gro-mode=off/on` argument, passed by the GRO driver and applied after
environment isolation. Omission preserves existing lab behaviour; unrelated
`ET_` variables remain excluded. Every activation, integrity, ICMP and lifecycle
assertion is unchanged. The next run reuses the same immutable Core binary,
without recompiling the receiver or altering product code. Run `36273977755`
is the retained runtime failure for this wiring defect; the corrected live-Core
activation check must pass before any green or performance claim is made.

## Residual Core cost: existing profile, not another performance claim

While the harness correction awaits confirmation, the existing flat profiles
from run `36235812507` were analysed without another build or deployment.
Their candidate is **`6e90dbf102e5c93d56c531b87eea1858c4a8e61e`**, not the
later experimental receiver. Artifact `10904457128` has verified ZIP SHA-256
`535f0f80d919aaee7abd05bc4f471ee0860394e00c5f4595c713349bf17650af`.
Both endpoints were Core processes in two network namespaces joined by veth
on one GitHub runner: AMD EPYC 9V45, four logical CPUs, Linux 6.8.0-1064-azure.
The path was UDP/IPv4 underlay, IPv4 overlay, Stealth off, with no Leaf policy.
These are not WAN, original-host, or sustained-load acceptance results.

The candidate upload/download captures contain 1,032/1,053 CPU-clock samples
and report zero lost samples. All 500/507 printed rows were parsed. The reports
use `--no-children`, so these are self samples, not inclusive call-stack totals.
Each printed row is rounded to two decimals: their totals are 100.58% and
98.21%, respectively. Do not give aggregated percentages false precision or
use them as measured achievable savings.

| Observed self-sample location | Upload, approximate % | Download, approximate % |
| --- | ---: | ---: |
| All kernel locations | 45.8 | 46.5 |
| AES-GCM encode/decode update assembly only | 4.3 | 5.4 |
| SOCKS peer packet filter closure | 3.3 | 3.1 |
| Userspace `memcpy` | 2.8 | 2.5 |
| Bounded MPSC sender future | 1.8 | 2.4 |

The AES row excludes other crypto work; it is not total encryption cost. Kernel
samples include scheduling, locks and packet processing, not just UDP receive.
Flat samples cannot attribute all of those to one syscall or infer a GRO gain.

The SOCKS filter deserves a bounded follow-up, not deletion. The exact
`easytier/src/gateway/socks5.rs` blob is identical at the profiled `6e90dbf1`
and frozen `3166ab67`: `63c30e5f780e3d265fd307ecafd132b058ebb3ae`.
`try_process_packet_from_peer()` still loads the entry/enabled flags and checks
`entries.is_empty()` before passing inactive traffic through. Its existing
`socks5_mirrors_fragmented_udp_even_when_entry_count_is_stale_zero` regression
explicitly requires a nonempty table to be honoured when the counter is zero.
Removing that table check based on the counter alone would violate the current
contract. The samples also do not isolate how much of the closure's self time
comes from that check versus other inlined work.

This profile justified obtaining a valid actual-Core GRO comparison before a
separate inactive-filter or queue experiment. The following result supplies that
comparison, not permission for crypto/stack replacement, queue enlargement or
filter bypass. Original-host acceptance and the older saturation failure remain
open.

## Actual-Core result: benefit confirmed, pressure trade-off remains open

[Run 36279752505](https://github.com/lovitus/EasyTier/actions/runs/36279752505)
completed with **SUCCESS**, at harness commit
`10f4e9bbd6aa9af7c4275501fc938f498ec85a42`. Only the explicit experiment-option
handoff was repaired. The receive adapter and frozen production source were not
changed. The run reused binary artifact `10915569351`; it did not compile Core.
The 39 UDP and four hole-punch contract results belong to run `36271631477`,
not to a fictitious test re-execution in this reuse-only run.

### Exact identity and both endpoints

- Source base: `3166ab672d347cdcc5a6768bc77056cd8ec38323`, with the unchanged
  disposable GRO receiver introduced by harness `4e14d18a`.
- Core SHA-256: `ae0d57a5c1c7d9568ff3158fcfd0c8b544a0a1cf236aab4b36706ca28d6b7777`.
- Core Build ID: `7e874215b5dcf724df2af1461d1814eb75eedd9b`.
- Original binary artifact ZIP SHA-256:
  `9532c659b5c51e280bc88f5af7d9afe5f4cdfd3a8d2695471282c4133f550cb3`.
  The reuse job checked the archive and each executable on GitHub. The large
  archive was not downloaded to the maintainer machine.
- Endpoint A and endpoint B were Core processes in two separate Linux network
  namespaces on the **same GitHub-hosted runner**, joined by a veth pair. This
  is a real Core/TUN/UDP/crypto path, but is neither two physical hosts nor WAN.
- Runner: Intel Xeon Platinum 8370C, 2.80 GHz, four logical CPUs / two cores;
  Linux `6.8.0-1064-azure`, x86_64. AES-GCM remained enabled. No Leaf or Mihomo
  path was involved.
- Evidence artifact `10919035320`: 2,041,889 bytes; ZIP SHA-256
  `5fb93544ac74d49d114b81759702171b53fa36bff57d2146e2344e484bf99caa`.
  All 2,355 members passed CRC, and all 2,354 manifest hashes matched.

Four traced activation cases are excluded from performance statistics. The
untraced phases contain 24 fixed-load, six saturation and six mixed-load cases.
Each matched group uses three samples per arm, interleaved
`off/on/on/off/off/on`. Both arms execute identical Core bytes. These short
runner samples are not a confidence interval or a sustained capacity guarantee.

### Activation is now demonstrated, not inferred from a label

For each of IPv4 and IPv6, the on case records three successful UDP_GRO enables.
Aggregated receives larger than one frame occur from **both endpoint sources**:
1,523/1,379 for IPv4 and 1,469/1,507 for IPv6. Both off cases record zero enables
and zero GRO aggregates. Stealth is enabled in these activation cases.

The old failed on case in run `36273977755` and this corrected on case form the
runtime failing/passing evidence for the **lab environment wiring defect**.
They are not evidence of a bug in production's GRO-disabled reader. Tracing
results must not be used as throughput or CPU samples.

### Fixed offered rate: three-sample medians

The cap is 200 Mbit/s; delivered rates are approximately 193 Mbit/s in both
arms. CPU is the sum of both Core processes' user and system CPU seconds per
delivered GiB. It is not total runner CPU or a single endpoint's consumption.

| Outer / inner family | Stealth | Direction | Off CPU s/GiB | On CPU s/GiB | Change |
| --- | --- | --- | ---: | ---: | ---: |
| IPv4 / IPv4 | off | A to B | 14.72 | 13.44 | -8.70% |
| IPv4 / IPv4 | off | B to A | 14.72 | 13.28 | -9.78% |
| IPv4 / IPv4 | on | A to B | 16.64 | 15.68 | -5.77% |
| IPv4 / IPv4 | on | B to A | 16.32 | 15.68 | -3.92% |
| IPv6 / IPv6 | off | A to B | 14.56 | 13.60 | -6.59% |
| IPv6 / IPv6 | off | B to A | 14.72 | 13.44 | -8.70% |
| IPv6 / IPv6 | on | A to B | 16.32 | 15.36 | -5.88% |
| IPv6 / IPv6 | on | B to A | 16.64 | 15.36 | -7.69% |

All fixed-load cases have zero recorded TCP retransmissions. The shorter
32 MiB Stealth trials have coarse CPU-tick resolution and some overlapping
sample ranges; their small percentage differences should not be overinterpreted.
Plain cases transfer 64 MiB in each direction.

### Saturation and simultaneous opposite-direction load

These cases use UDP/IPv4 underlay, IPv6 overlay and Stealth off. Each flow
delivers 1 GiB. Mixed cases carry two simultaneous, opposite-direction flows.

| Load / primary direction | Off Mbit/s median [range] | On Mbit/s median [range] | Goodput change | Core CPU s/GiB off / on |
| --- | ---: | ---: | ---: | ---: |
| Single A to B | 2213.87 [2201.17, 2229.43] | 2546.06 [2519.82, 2582.76] | +15.01% | 11.02 / 9.95 |
| Single B to A | 2239.52 [2012.32, 2242.22] | 2494.20 [2425.86, 2523.69] | +11.37% | 10.93 / 10.00 |
| Mixed, primary A to B | 1198.17 [1193.89, 1203.00] | 1331.52 [1329.68, 1376.20] | +11.13% | 11.01 / 9.87 |
| Mixed, primary B to A | 1200.49 [1192.34, 1218.05] | 1330.38 [1318.13, 1339.01] | +10.82% | 11.01 / 9.89 |

The mixed Mbit/s columns show only the primary flow, **not aggregate throughput**.
Secondary-flow medians are respectively 1215.23 / 1334.47 and
1206.20 / 1342.60 Mbit/s, off / on. Mixed CPU denominators include both flows'
delivered bytes. Single-flow CPU/GiB improves 8.51-9.71%; mixed improves
10.17-10.35%. These are incremental comparisons within this exact run. Do not
add them to previous GSO or TUN-head percentages from other candidates/hosts.
Lower CPU per delivered byte does not promise lower peak CPU under saturation:
the faster arm also processes more traffic per second.

### Important adverse result: TCP recovery work increases

Kernel UDP/IP and link error/drop counters remain zero in the recorded windows,
but this is **not a loss-free path**. TCP retransmissions occur in both saturation
arms and increase with GRO. The table uses namespace counter deltas summed over
both endpoints, normalized by delivered bytes. `TCPFastRetrans` counts the same
retransmits here and is not added to `Tcp.RetransSegs`.

| Load / primary direction | Off retransmits/GiB median [range] | On retransmits/GiB median [range] | Retransmits / original data segments, off / on |
| --- | ---: | ---: | ---: |
| Single A to B | 273 [192, 382] | 464 [417, 487] | 0.03387% / 0.05470% |
| Single B to A | 253 [218, 258] | 540 [357, 666] | 0.02915% / 0.06250% |
| Mixed, primary A to B | 355.5 [336.5, 469] | 835 [766, 891.5] | 0.04642% / 0.09966% |
| Mixed, primary B to A | 333.5 [321.5, 415] | 771.5 [706, 816] | 0.04278% / 0.09170% |

The percentages use each group's sum of `Tcp.RetransSegs` divided by
`TcpExt.TCPOrigDataSent`, not an application UDP loss rate. The Linux
[SNMP counter documentation](https://docs.kernel.org/networking/snmp_counter.html)
distinguishes original data segments from ACK-inclusive outgoing segments and
GRO-sensitive incoming segment counts. Counter locations do not identify the
point of packet loss inside Core.

Recovery episodes and retransmitted segments must also be distinguished. Across
the three samples, `TCPSackRecovery` falls from 169 to 77 (single A to B),
160 to 84 (single B to A), 461 to 258 (mixed primary A to B), and 473 to 257
(mixed primary B to A). More segments are retransmitted in fewer recovery
episodes. This is consistent with larger loss/recovery bursts, but is **not
proof** that the Core ring is their cause. No DSACK or spurious-retransmission
counter increase was observed in these snapshots. Ten `TCPLostRetransmit`
increments occurred across all saturation/mixed samples; seven belong to one
GRO-on single-download case. These are retained, not waived by workflow success.

Saturation also has different achieved loads between arms. A matched-rate
comparison near the baseline capacity is needed before claiming a causal
same-load regression. The existing 200 Mbit/s points alone cannot answer that.

### Control progress, resources and cleanup

All 1,600 ICMP probes were answered, but the probe is only 20 echoes near the
start of each transfer, not continuous full-transfer control-latency coverage.
The 32 MiB fixed-load transfer can finish before its ping series. Do not turn
these samples into a sustained fairness or production tail-latency guarantee.

| Load / primary direction | Median of per-case ICMP p95, off / on, ms | Worst observed RTT, off / on, ms |
| --- | ---: | ---: |
| Single A to B | 1.150 / 1.210 | 1.440 / 1.720 |
| Single B to A | 0.899 / 1.040 | 1.480 / 1.270 |
| Mixed, primary A to B | 1.270 / 1.770 | 1.570 / 1.920 |
| Mixed, primary B to A | 1.360 / 1.740 | 1.610 / 1.950 |

Only 20 replies underlie each per-case percentile. Mixed-load latency is higher;
there is no basis for calling this a zero-latency-cost optimization.

The full run completed 92 bulk transfers / 38.5 GiB, 80 independent byte-integrity
checks, and 1,200 UDP echo datagrams. The UDP/integrity checks occur before bulk
load, not as saturation UDP delivery evidence. All 80 Core processes exited
cleanly, all 80 namespaces were removed, all 536 recorded cleanup checks passed,
and host routes were unchanged. No forced kill was required.

Observed per-process RSS ranges were 25.8-27.8 MiB at fixed load,
26.1-28.4 MiB at saturation, and 26.0-29.2 MiB at mixed load, across both arms.
Snapshots show 13-14 threads and 26-29 FDs; maximum Core log size was 5,689 bytes.
These short pre/post snapshots do not establish peak memory or long-term leak
freedom. The enabled adapter still adds one 64 KiB scratch buffer per receive
owner; there is no demonstrated memory reduction. This run has no measured idle
phase and makes no idle-power claim.

### Source reconciliation and next discriminator

At frozen `3166ab67`, `UdpConnection::handle_packet_from_remote()` in
`easytier/src/tunnel/udp.rs` uses `RingSink::try_send()` for lossy Data. The
receive ring has capacity 128; `try_send()` in `tunnel/ring.rs` rejects when
occupancy reaches capacity minus the existing four reserved entries. Non-lossy
traffic uses `force_send()` and still fails when the ring is full. The shared
listener must not await one peer's ring indefinitely and stall all other peers.

The exact locked Tokio 1.52.1 `Registration::async_io()` already consumes
cooperative budget; its poll-read path does too. The experimental receiver also
calls `consume_budget()` for every returned segment. A missing cooperative yield
has therefore **not** been established. Larger GRO bursts, scheduler placement,
ring pressure and downstream processing remain hypotheses, not demonstrated
causes of this run's retransmissions.

Next investigation is narrowly bounded: reuse these exact binaries for a
matched-rate high-load off/on comparison, record control progress throughout
the transfer, and distinguish kernel, receiver parsing and per-peer ring losses
before changing packet flow. Any diagnostic instrumentation must be separated
from performance samples. No larger ring, unbounded staging, timer-based batch
delay, packet-class priority or blanket yield is approved by these results.

The successful workflow closes the lab-option wiring failure and proves a real
Linux Core CPU/goodput benefit. It does **not** close the old missing IPv6 echo
in run `36260893872`, original-host validation, physical NIC/WAN behavior,
cross-platform fallback, long-duration memory, or saturated Stealth coverage.
Keep the adapter experimental and the production candidate unchanged while the
pressure trade-off is investigated. No merge, release or completed-performance
goal is implied.

## Next bounded run: matched higher-load pressure, no Core rebuild

The existing reuse lane now has an optional `udp_gro_pressure=true` scope,
used together with `udp_gro_reuse=true`. It repeats independent activation
observation, then runs 24 untraced cases instead of repeating the old full
matrix. Source, receiver, binary hashes, socket buffers, ring capacity, protocol
and all pre-existing traffic/cleanup assertions are unchanged.

Each case carries 256 MiB per flow, using the existing no-catch-up paced probe.
Total target rates are 500 and 1,000 Mbit/s. A single flow uses that cap; mixed
opposite-direction flows each use half. Each rate/topology has three off and
three on samples in `off/on/on/off/off/on` order. Both directions and both mixed
flow roles are reported separately. Actual delivered-rate medians must match
within 2% in each group before the run supports an equal-load CPU comparison.
The cap is not substituted for measured goodput. Python pacing remains part of
the existing load generator, not a new production timer or rate controller.

The optional control observer uses 120 timestamped echoes at the existing
100 ms spacing. Every reply is required. Its first/last replies must bracket
the complete measured transfer, including the secondary flow, with at least
20 replies inside that interval. Only in-load samples inform latency analysis;
pre/post-load samples are retained but not mixed into that analysis. A transfer
which outlasts observation fails coverage rather than obtaining a partial PASS.
Existing short-window callers and transfer sizes retain their defaults.

This run can establish whether the retransmission/latency trade-off persists
under matched higher load. It cannot by itself identify an internal ring drop
or clear the earlier saturation failure. Higher rates are not necessarily near
the capacity of a different hosted runner. Preserve kernel counters, raw errors,
resource identities and scoped cleanup even if rate matching or coverage fails.
No production receive-flow patch is included. The result is recorded below.

## Matched higher-load result: CPU benefit persists, not a loss fix

[Run 36281869121](https://github.com/lovitus/EasyTier/actions/runs/36281869121)
completed successfully at harness
`7473503e6287ecedf1ad39909a352a9eb4d8fd4a`. It reused binary artifact
`10915569351`; no Core compilation or contract-test rerun occurred. Core SHA-256
remains `ae0d57a5c1c7d9568ff3158fcfd0c8b544a0a1cf236aab4b36706ca28d6b7777`,
with base `3166ab672d347cdcc5a6768bc77056cd8ec38323` and the unchanged disposable
receive adapter. The earlier 39 UDP and four hole-punch contracts are earlier
evidence, not tests executed by this run.

Endpoint A and B were separate Core processes/network namespaces connected by
veth on one GitHub-hosted runner: Intel Xeon Platinum 8573C, four logical CPUs,
two cores, Linux 6.8.0-1064-azure. Pressure traffic used UDP/IPv4 underlay,
IPv6 overlay, AES-GCM and Stealth off. This is neither physical-host/WAN evidence
nor Leaf/Mihomo measurement. The previous saturation runner used an 8370C;
cross-run differences are not a controlled experiment on saturation alone.

The small evidence artifact `10919382367` is 1,687,005 bytes, SHA-256
`23ba50b4e6acd62c5418ed28d7e2b8a84fc64f1f562f221b4e9bdc176c35b70f`.
All 1,790 ZIP members passed CRC, and all 1,789 manifest hashes matched. Independent
activation traces again showed actual GRO receives from both IPv4/IPv6 peers;
off had no enables/aggregates. Activation traces are excluded from performance.

There were 24 untraced cases, with three interleaved off/on samples for each
total cap/topology. Each flow transferred 256 MiB. Mixed flows each received
half the configured total cap. All 12 direction/flow-role groups met the
two-percent actual-rate matching gate. CPU below is the sum of both Core
processes' user+system seconds per delivered GiB; mixed CPU includes both flows.

| Total cap Mbit/s | Load / primary direction | Actual off / on Mbit/s | Core CPU s/GiB off / on | CPU change |
|---|---|---:|---:|---:|
| 500 | Single A to B | 465.132 / 464.511 | 12.96 / 11.88 | -8.33% |
| 500 | Single B to A | 465.022 / 464.948 | 12.96 / 11.84 | -8.64% |
| 500 | Mixed, primary A to B | 239.973 / 239.958 | 13.04 / 12.04 | -7.67% |
| 500 | Mixed, primary B to A | 239.984 / 239.956 | 13.04 / 11.96 | -8.28% |
| 1000 | Single A to B | 872.721 / 872.840 | 12.52 / 11.60 | -7.35% |
| 1000 | Single B to A | 872.693 / 872.862 | 12.36 / 11.44 | -7.44% |
| 1000 | Mixed, primary A to B | 463.135 / 462.827 | 12.56 / 11.78 | -6.21% |
| 1000 | Mixed, primary B to A | 463.064 / 462.837 | 12.62 / 11.74 | -6.97% |

Mixed rates are primary-flow rates, not aggregates. The existing no-catch-up
pacer delivers less than its configured ceiling; this is not evidence of
achieving a literal 1 Gbit/s load. Do not add these savings to results from
different machines or parent candidates.

Single-flow cases recorded zero TCP retransmissions. Mixed cases recorded two
single-segment retransmissions off and four on in total; corresponding peers
reported DSACK-old-sent. No fast-retransmit/SACK-recovery clusters were recorded.
DSACK alone does not prove harmless tail probes or identify the cause. This
does not erase the substantially worse saturation counters in run 36279752505.
Kernel UDP/IP error/drop deltas were zero. Separately, all 2,352 link error/drop
deltas across 112 before/after snapshot pairs were zero; internal ring rejection
is not covered by those counters.

All 48 complete transfer-control windows delivered 120/120 timestamped echoes,
5,760 total, with 2,388 replies during actual load. Their first/last replies
bracketed complete primary and secondary transfers. One GRO-on mixed 1000-cap
sample reached 4.07 ms maximum latency, versus 1.01 ms maximum in the matching
off group. The opposite primary direction reached 1.17 ms on versus 0.40 ms off.
The per-case in-load p95 medians were broadly similar; the outliers are retained,
not rounded into a no-latency-regression claim.

Including activation: 80 bulk transfers/18.25 GiB, 56 independent integrity
checks, 840 UDP echoes and 5,920/5,920 ICMP replies completed. UDP echoes preceded
bulk traffic and are not saturated application-UDP evidence. All 56 Core exits,
56 namespace removals and 356 cleanup records passed; host routes were unchanged
and no forced kill was required. Maximum Core log size was 5,692 bytes.

Pressure RSS snapshots were 25.61-27.25 MiB off and 25.75-28.99 MiB on per Core,
with 13-14 threads and 25-29 FDs. No memory reduction or long-duration leak claim
is made. The adapter still owns 64 KiB per enabled reader. Idle power is untested.

Measurement caveat: some short single-flow host `/proc/stat` CPU deltas were
slightly below the summed per-process deltas. The helper arithmetic was inspected
and no arithmetic defect established, but the samples are not simultaneous and
the cause is not proven. Host totals remain diagnostic, not precise additive
machine-CPU acceptance. The table uses the separately identified Core processes.

## Next discriminator: bounded internal rejection observation

The same-load CPU benefit justifies further research, not a production merge.
The remaining question is whether the saturated receive path rejects packets at
the existing ring and whether a missing echo can be correlated to that stage.
At the frozen source, lossy data calls `try_send()` on a 128-entry ring with four
reserved entries. Broad UDP TRACE logging would print normal packets as well;
it is not an acceptable way to count rejection under load.

The next isolated build layers the existing bounded ICMP ciphertext correlation
over the unchanged GRO adapter, plus counters only after lossy/non-lossy ring
rejection. It emits only power-of-two bounds, at most 64 lines per rejection
class per x86_64 process. Final totals are intervals, not exact counts. No report
is interpreted as zero unless diagnostic stages and complete clean logs are
present. There is no packet retention, retry, new queue, scheduling change,
priority bypass or logging of keys/payloads.

Six interleaved off/on mixed-load cases reuse the original lab, 1 GiB per flow,
UDP4/inner IPv6, Stealth off, and full-transfer echo observation. Existing
integrity, loss, CPU identity, log-cap and cleanup assertions stay unchanged;
the first failure stops the run and is preserved. Both comparison arms contain
identical observation code. These samples are explicitly excluded from CPU or
throughput acceptance because instrumentation can perturb scheduling.

This narrow census does not measure every possible parser/decrypt drop or
establish that each TCP retransmission came from the ring. It is intended to
localize the next change, not justify an unmeasured ring enlargement or extra
yield. Original combined failure 36260893872, original-host/WAN acceptance and
production GRO acceptance remain OPEN.
