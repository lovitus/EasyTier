# Natural TUN cohort: kernel mechanism experiment

Scope: issue #4, pure mesh performance research. This is **not EasyTier** and
does not implement its routing, encryption, peer security, failover, or policy.
It tests whether preserving actual TUN-produced groups has enough kernel-path
value to justify a later guarded Core experiment. No shipping path is changed.

The source uses the same locked tun-rs 2.8.7 GSO splitter as Core. A real TCP
workload enters a native Linux TUN; each recv_multiple result is a cohort.
There is no synthetic 32-packet queue, coalescing timer, or cross-read batching.
The modes are single send_to, sendmmsg of the existing cohort, and UDP_SEGMENT
of compatible same-sized datagrams with an optional short last datagram.
All modes keep the same single-datagram UDP receive and TUN write path.
Single-item cohorts call the same send_to function in all modes.

The probe intentionally recycles its splitter buffers, omits Core processing,
and uses a single synchronous I/O owner. Its absolute throughput and CPU are
NOT Core performance, and gains may shrink or vanish after Core integration.
UDP_GRO is not enabled; neither this tool nor current Core may treat an
aggregated UDP buffer as one ordinary datagram without parsing ancillary data.

Safety: opt-in environment variable plus non-root-network-namespace check;
no default external route, production identity or user configuration. Native
I/O has bounded backpressure and signal-based stop. GSO submissions are at
most 64 datagrams / 60,000 bytes, sendmmsg retries only its unsent suffix, and
unsupported kernel behavior is a failed experiment, not silently a PASS.

The small dedicated workflow builds only this tool and the existing fixed-byte
Rust probe, never Core. Tests cover grouping bounds and actual kernel UDP
datagram bytes/order, including a short tail. The namespace lab uses three
interleaved repetitions of each mode, both directions, separate payload-digest
and half-close checks, throughput/CPU measurements, ICMP progress, actual
cohort histograms, clean exits and root-route/namespace cleanup evidence.
Raw failures stay in the artifact. Sparse/fixed-rate, crypto, relay, multi-peer,
cross-host and other-platform acceptance are still required before adoption.

References:

- https://github.com/lovitus/EasyTier/issues/4#issuecomment-5692616218
- https://www.wireguard.com/papers/wireguard-netdev22.pdf
- https://man7.org/linux/man-pages/man7/udp.7.html

No result is claimed before execution. A useful mechanism result would require
a repeatable material CPU/GiB gain without throughput or ICMP regression. A
positive standalone result only opens the Core experiment; it cannot authorize
production changes. The old synthetic GRO scratch and writer-drain failures
remain valid and are not superseded by this probe.

## First execution, 2026-09-16

Run 35061859589 failed before tool compilation because the workflow omitted
the repository-required mold linker. Run 35062151397 fixed that preparation
error: all three unit tests passed, including real kernel datagram boundaries.
The first single-send baseline then failed its download ICMP check: 20 sent,
19 received, maximum RTT 23.783 ms. Both 512 MiB TCP transfers and their
separate 1,048,595-byte digest/half-close checks completed, but this is NOT a
performance PASS. No mmsg/GSO arm ran. Cleanup was successful.

The two baseline endpoints observed 567,947 and 578,193 transmitted datagrams.
Their natural cohort histograms include 8,125 and 8,092 reads yielding 49
segments; natural multi-segment input is real, not a prefilled synthetic queue.
The original evidence did not collect kernel drop counters, so it cannot
attribute missing datagrams to socket overflow rather than another boundary.

Artifact 10432588706 has ZIP SHA256
`8e6103a26f4ee77b16890f8b115f8b8c5caeea90423f8279ec998d5a657b9168`;
all 23 manifest entries and both binary hashes were checked independently.
Its GNU binary requires GLIBC_2.39 and cannot run on the intended GLIBC_2.35
lab host. The next artifact is therefore static musl, consistently for all
arms. Raw transfer snapshots are now retained even when a later ICMP check
fails; exit-time namespace SNMP/socket/link counters add diagnostic evidence.
Neither this observation nor static packaging relaxes the original assertions.
Do not compare absolute GNU/musl throughput as a batching effect.

## Paired UDP_GRO arm, pending validation

The next experiment adds `gso-gro`; the original single/mmsg/gso modes retain
single-datagram receive. Only the new arm enables UDP_GRO and uses recvmsg.
Linux v6.8 `include/linux/udp.h::udp_cmsg_recv` emits the GRO segment size as
a native-endian int, not the u16 used for UDP_SEGMENT sends. The parser rejects
payload/control truncation, invalid cmsg bounds, duplicate GRO values and
invalid segment sizes. Each recovered IPv4 datagram is validated separately.

TUN output remains one ordinary write per datagram. The existing receive
buffer is reused: once one datagram is written, its final header-sized bytes
may become the next zero virtio prefix. No payload-sized copy, extra receive
allocation, TCP GRO scratch, socket-buffer increase or coalescing timer is
introduced. New tests cover metadata errors, prefix reuse and actual kernel
GRO with a short tail plus a plain datagram. Runtime counters must demonstrate
actual GRO activation rather than merely a successful setsockopt.

An owned receive aggregate is finished before polling another descriptor;
the receive loop's 64-packet soft budget may therefore be exceeded by one
bounded aggregate. This scheduling difference belongs to the experiment and
must be evaluated with control latency; it is not an unchanged Core contract.
The new arm starts first, while previous arms retain their relative order.
All existing failure assertions remain fatal and old failed runs remain FAIL.
Sparse-load and whole-Core acceptance are still not established.

## Receive-only decision and route evidence

The `gro` arm leaves every outgoing datagram on send_to and enables only UDP
receive aggregation. It tests whether a bounded receive-only Core experiment
could be useful without extending the mesh sending API. Single datagrams from
legacy senders remain valid whether or not the kernel coalesces them. Lack of
observed GRO is recorded as absence of mechanism activation, not disguised as
a batching gain. The lab asserts that this arm makes no mmsg/GSO submissions.
The unit test checks actual received bytes/source/order from a legacy sender.
Underlay offload features are captured without modifying them.

The paired-offload artifact b53fbe63 passed six unit tests, but unpaced CI and
lab runs failed ICMP gates. A separate paced diagnostic completed 24 transfers,
digest/half-close and ICMP checks, then failed root-route equality. The original
before/after root-route dumps were not retained, so that failure is unresolved
and must not be waived. A subsequent passive 60-second route observation had
no events and identical IPv4/IPv6 snapshots; it cannot prove what changed in
the earlier run. Production Core process/binary remained unchanged.

The lab now preserves both complete original route responses while keeping
the exact same equality assertion. No sorting, expiry normalization, ignored
route, automatic restore, or retroactive PASS is introduced. Private-host
snapshots stay in private evidence, never in public issue text or Git files.
The receive-only arm runs first; older arms retain their relative order and
every existing failure assertion remains fatal. No production source changes.
