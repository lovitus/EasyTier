# GSO supply and UDP GRO receive: bounded mechanism research

Status: standalone mechanism completed; useful paced CPU result, not Core acceptance.
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

Pending gates: compiler/tests, actual Core GRO occupancy, fixed-load CPU/RSS,
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

### Third attempt: contracts complete; Drop-only observation invalid

Run `36271631477`, harness
`d06292a42ac3c1d0b7f020b4e7b8a535a95e6f17`, remains **FAIL**.
All **39 UDP tests** and **4 hole-punch tests**, including both UPnP namespace
cases, passed (22.76 s and 3.13 s). Optimized Core, CLI and the exact test binary
were produced. The first fixed-load off case completed both 64 MiB transfers,
UDP echo, byte-integrity and zero-loss ICMP checks. Both Core processes exited
0 without forced kill; both namespaces were empty on deletion and host routes
were unchanged. No on sample or A/B performance result was collected.

The harness then failed because there were **zero** shutdown metric files.
That assertion incorrectly treated a diagnostic `Receiver::drop` write as
mandatory during process termination. The existing launcher ends its instance
runtime with `shutdown_background()` (`easytier/src/launcher.rs`), so diagnostic
destructor completion before process exit is not an established contract. This
is neither evidence of a GRO throughput failure nor proof of a product leak.
Do not change product shutdown to accommodate the observation tool.

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
