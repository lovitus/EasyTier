# SOCKS5 Performance Investigation and Maintenance Boundaries

This note records an isolated EasyTier built-in SOCKS5 investigation performed
on 2026-07-03. Its purpose is not to publish universal benchmark numbers. It
preserves the verified bottleneck attribution so future work does not mistake
the destination-side `no_tun` TCP ingress proxy for a SOCKS5 server bottleneck.

The current build used commit `fab7fa5d` and reported version `2.6.7`. Mixed
deployment tests used the official upstream release `2.6.4-8428a89d`.

## 1. Current TCP Paths

SOCKS5 TCP CONNECT does not always use the same data path:

1. A local virtual-IP destination is rewritten to loopback and connected through
   the kernel TCP stack.
2. For a remote virtual IP, `Socks5AutoConnector` selects a direct KCP stream
   when the source enables KCP Proxy and the destination capability permits KCP
   input.
3. Without direct KCP, SOCKS5 originates a normal TCP SYN through user-space
   smoltcp. That SYN still crosses the NIC pipeline, so the existing proxy
   selector may capture it when QUIC Proxy is enabled.
4. A destination with TUN can terminate the destination connection through its
   kernel TCP stack. A `no_tun` destination requires the user-space TCP ingress
   proxy.

SOCKS5 performance therefore depends on the source connector, QUIC/KCP Proxy,
and whether the destination uses `no_tun`. It cannot be inferred from the
SOCKS5 listener implementation alone.

Direct SOCKS KCP currently sends `proxy_prepare_version=0`, does not wait for a
Proxy READY ACK, and does not retry QUIC or smoltcp inside the same SOCKS CONNECT
after a runtime KCP failure. This investigation explicitly accepts that behavior
and does not plan an additional state machine for it.

## 2. Test Environment

- Isolated Docker bridge with two Debian Bookworm containers running optimized
  release builds.
- Local LAN Docker network without injected latency, jitter, or packet loss.
- The nodes established an EasyTier TCP underlay.
- A Python HTTP server served 128 MiB or 512 MiB files.
- `curl --socks5-hostname` measured SOCKS5; direct cases accessed the virtual IP.
- Tests used `--disable-encryption true` and did not explicitly enable
  compression.
- The host had sufficient CPU and memory. Results compare paths in this setup;
  they are not public-Internet or low-end-device performance promises.

Core addressing:

```text
A: 10.201.0.1/24, SOCKS5 :18080
B: 10.201.0.2/24, HTTP :19000
underlay: tcp://172.30.50.2:12001
```

## 3. Results

`curl` reports decimal B/s. MiB/s values below are approximate conversions.

| Source / Destination | Path | Throughput |
| --- | --- | --- |
| A `no_tun` / B with TUN | Plain SOCKS path | 149.1-152.7 MB/s, about 142-146 MiB/s |
| A with TUN / B `no_tun` | Direct TCP | 14.87-14.89 MB/s, about 14.2 MiB/s |
| A with TUN / B `no_tun` | Plain SOCKS path | 14.7-17.3 MB/s, about 14-16.5 MiB/s |
| A `no_tun` / B `no_tun` | Plain SOCKS path | 14.65-14.82 MB/s, about 14 MiB/s |
| A `no_tun` / B `no_tun` | QUIC Proxy only | 36.1-36.4 MB/s, about 34.5 MiB/s |
| A `no_tun` / B `no_tun` | KCP Proxy | 117.4-119.2 MB/s, about 112-114 MiB/s |
| `2.6.7 -> 2.6.4`, both `no_tun` | KCP Proxy | 116.1-118.1 MB/s |
| `2.6.4 -> 2.6.7`, both `no_tun` | KCP Proxy | 116.4-119.2 MB/s |

When the source enabled KCP and the destination explicitly set
`disable_kcp_input=true`, capability negotiation selected the plain path. The
connection remained usable and throughput returned to about 14.2-14.3 MB/s.

## 4. Conclusions

### Verified bottleneck attribution

- With TUN on the destination, the plain SOCKS path reached the same throughput
  class as direct TUN.
- With `no_tun` on the destination, both direct TCP and SOCKS dropped to about
  14-17 MB/s. The primary bottleneck is the destination-side `no_tun` TCP ingress
  proxy, not SOCKS accept/authentication or the source smoltcp connector.
- QUIC Proxy improved the normal-SYN path but remained materially slower than
  direct SOCKS KCP in this environment.
- Direct SOCKS KCP is not a disposable legacy branch. It improved throughput by
  roughly eight times over the plain path when both nodes used `no_tun`.

### Maintenance decisions

- Do not remove direct SOCKS KCP merely to unify the architecture.
- Do not move every SOCKS session into one global `JoinSet`; the existing net
  generation and port-forward cancellation scopes have distinct ownership.
- Do not add another state machine for the currently accepted direct-KCP
  prepare/fallback boundary.
- To improve `no_tun` throughput, investigate the destination TCP proxy/capture
  path before rewriting SOCKS5.
- Benchmark with QUIC/KCP Proxy both disabled and enabled because they change
  the effective SOCKS traffic path.

## 5. Resource and Stability Checks

- Direct KCP completed 500 short connections at concurrency 32 without failure.
- The plain smoltcp path hit 5-second connection timeouts at concurrency 32,
  establishing a concurrency-capacity boundary. Repeated batches of 500 at
  concurrency 8 completed without failure.
- EasyTier FD and thread counts returned to fixed baselines after each batch.
- Source RSS rose from about 19 MiB to 50 MiB and then 98 MiB, but remained near
  98 MiB for the third batch. This matches reusable allocator high-water memory
  after allocating 128 KiB RX plus 128 KiB TX buffers per smoltcp TCP socket.
- One hundred concurrent requests to the local virtual IP completed without
  failure, and FD/thread counts returned to baseline. No local traffic loop was
  observed.
- UDP ASSOCIATE currently returns SOCKS reply `0x07` (Command not supported).
  It is not a partially working supported feature, and enabling only the
  vendored library switch would not be sufficient.

These checks do not prove that every long-running workload is leak-free. They
do rule out a linear leak where every completed TCP connection permanently
retains an FD, task, or smoltcp socket buffer.

## 6. Future Diagnostic Order

For a future "SOCKS5 is slow" report, isolate the path before changing the
connector:

1. Determine whether the destination uses `no_tun`.
2. Compare direct TCP and SOCKS TCP with the same source and destination.
3. Disable both QUIC and KCP Proxy and measure the plain path.
4. Enable QUIC-only, then KCP, and verify the effective selected path.
5. Check whether `disable_kcp_input`, `disable_quic_input`, or compiled features
   changed the destination capability.
6. Repeat a fixed-size transfer at fixed concurrency while recording failures,
   FD count, thread count, and RSS.
7. Investigate the SOCKS source connector first only if SOCKS remains
   materially slower than direct traffic when the destination has TUN.

Any related implementation change should rerun at least this regression matrix:

- Current-to-current plain, QUIC-only, and KCP paths.
- Current version in both directions with an official older release.
- Capability fallback with KCP input explicitly disabled.
- Local virtual-IP access, concurrent short connections, and post-connection
  FD/thread/RSS recovery.
