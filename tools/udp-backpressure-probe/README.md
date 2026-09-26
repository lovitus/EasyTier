# UDP kernel backpressure fixture

This small Linux-only experiment establishes a way to produce real UDP send
`EAGAIN`, wait for kernel `POLLOUT`, then recover without losing the accepted
datagram sequence. It does not build Core, use Tokio, enable GSO, or establish
production fallback/cancellation correctness. Its result only qualifies a
fixture for subsequent tests of the existing Core experiment.

The Rust sender is dependency-free. It binds its socket explicitly, requests
a 4096-byte send buffer, and sends at most 256 1200-byte packets before requiring
EAGAIN. This excludes unbound-socket port exhaustion as the intended trigger.
Recovery has a five-second deadline and a bounded number of writable retries.
The receiver checks sequence, complete payload bytes, and the final count.

The Python wrapper creates a veth pair directly inside two temporary namespaces,
sets static neighbor entries, and applies TBF only to the disposable sender
interface. There are no external/default routes, host-interface changes or
production identities. Both IPv4 and IPv6 must pass; missing EAGAIN, packet loss,
route differences or incomplete cleanup fail the experiment.

Why not netem delay: Linux v6.8 `sch_netem.c` calls `skb_orphan_partial` for delayed
traffic. `sock.c` documents that this releases socket memory accounting to allow
further sends. TBF queues are the proposed alternative, not an assumed guarantee:
the fixture must demonstrate the actual syscall failure and readiness recovery.

- https://github.com/torvalds/linux/blob/v6.8/net/sched/sch_netem.c#L476
- https://github.com/torvalds/linux/blob/v6.8/net/core/sock.c#L2538
- https://github.com/torvalds/linux/blob/v6.8/net/core/sock.c#L2753
- https://github.com/torvalds/linux/blob/v6.8/net/sched/sch_tbf.c#L240

Run only through the dedicated lightweight CI or in an explicitly authorized
disposable Linux environment. No runtime result is claimed before execution.
Do not relax the EAGAIN requirement if a kernel/device combination does not
reproduce the mechanism; retain that failure as an inconclusive fixture result.

## Actual experiment adapter lane

The separate `tokio` crate pins bytes 1.9.0, libc 0.2.186, socket2 0.5.10 and
Tokio 1.52.1, matching the Core experiment base rather than the older standalone
tool's independently updated lock. Its build script extracts `group_end`,
`send_group` and `send_frames` verbatim from `mesh-udp-flush/adapter.rs.in`.
Only the frame/stat containers and test orchestration are substituted; Core
framing, encryption, MPSC, routing and lifecycle are not exercised in this lane.

With `lab.py --adapter`, each IP family runs recovery, pending-future cancellation
and two destinations sharing the same socket. Tokio readiness is primed before
raw sends fill the kernel queue, so the extracted code must itself encounter
real EAGAIN. Cancellation retains the immutable batch, requires no packet to
have been submitted by that operation, then retries it and checks exact received
sequence. A current-thread heartbeat must progress while the writers wait.
Every case is bounded and requires real GSO calls and full payload verification.

These checks do not prove fair scheduling bounds, owner-drop/restart semantics,
key rotation, PMTU handling or runtime capability fallback. The original
single-datagram mechanism lane remains an independent control. CI builds only
the two small tools, never Core, and archives the exact extracted Rust source.

Three additional real-kernel characterization cases constrain any later fallback
design without implementing it. With the isolated link MTU at 1280 and explicit
PMTU discovery, 1400-byte datagrams must produce GSO EINVAL and ordinary-send
EMSGSIZE for both IPv4 and IPv6. A smaller GSO group must still work on the same
socket. The IPv4 checksum control disables checksums on only the fixture socket:
GSO must fail with EINVAL, ordinary datagrams must arrive, and restoring checksums
must allow GSO again. Receivers verify all delivered bytes and sequence; oversized
or unexpectedly delivered rejected datagrams fail validation.

The nine-case adapter lane therefore distinguishes an offload-specific rejection
from a path-size error. It does not claim that EINVAL universally means unsupported
GSO, or that the fixture has covered EIO and every kernel/driver error origin.
No production socket option or default is changed by these experiments.
