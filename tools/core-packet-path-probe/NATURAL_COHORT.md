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
