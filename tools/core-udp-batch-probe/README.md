# Core UDP batch probe

Status: EXPERIMENTAL TOOL ONLY

This Linux-only standalone crate contains bounded mechanism probes for:

- the current EasyTier-style Tokio `UdpSocket::recv_from()` loop;
- a readiness-driven `AsyncFd<std::net::UdpSocket>` loop using
  `recvmmsg(MSG_DONTWAIT)`;
- conditional `sendmmsg` batching with exact order and partial-send checks;
- the cached traffic-metric update path, including deep handle cloning and
  per-counter timestamp updates.

It does not modify or link EasyTier. It exists to reject or support a possible
Linux UDP underlay receive-batching optimization before production code is
changed.

## Model

- Real IPv4 UDP sockets on loopback.
- Tokio current-thread runtime.
- 1,200-byte datagrams.
- Controlled bursts of 1, 2, 4, 8, 16, and 32 packets.
- A fixed sender address.
- Every packet carries and verifies a monotonically increasing sequence and
  the encoded burst size.
- The receiver verifies payload length, order, source IP, and source port.
- A 4 MiB requested receive buffer prevents the benchmark itself from
  manufacturing loss under the bounded 32-packet bursts.
- The sender completes each burst before the timed receiver drain, modelling
  packets already queued when Tokio reports readiness.

The output records actual receive syscall count, packets per syscall, timed
receive nanoseconds per packet, receiver-thread CPU nanoseconds per packet, and
end-to-end packets per second.

## Boundaries

- This is a best-case queued-burst model. It must include burst 1 and 2 so a
  sparse-traffic regression cannot be hidden by burst 32.
- Debug/manual results are mechanism evidence, not deployable Core throughput
  evidence.
- A result is invalid if any payload, order, or source check fails.
- This tool does not justify `sendmmsg`, queueing, a fixed coalescing delay, or
  a cross-platform API change.
- Production work is allowed only if repeated runs show a material
  receiver-CPU reduction for realistic burst sizes without making burst 1
  materially worse.

Run only on the remote builder or validation Linux hosts. Do not compile this
repository on the maintainer's Mac.

```bash
cargo run --locked -- 5000
cargo run --locked --bin metrics_hotpath
```
