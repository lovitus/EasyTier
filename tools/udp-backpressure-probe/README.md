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
