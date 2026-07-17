# Leaf TUN scheduler investigation (2026-07-17)

Status: the lost-waker correction is retained. The high-BDP large-window implementation and the portless startup prewarm are rejected pending controlled evidence. Android explicit actor and portless managed HEV remain separate release gates.

Superseding decision, 2026-07-17: the Android/KR receive-window observation remains useful diagnostic evidence, but the mobile-to-VPS path was too variable to authorize a permanent per-stream memory increase. EasyTier therefore keeps the ordinary 128 KiB smoltcp buffers and relies on the user's existing KCP/QUIC settings when acceleration is required. The earlier 32-stream and later four-stream large-window candidates are historical rejected experiments, not the current design.

## 2026-07-17 high-BDP explicit actor diagnosis

Exact `c7a8d8af` evidence separates the earlier scheduler loss from a second, independent throughput limit.

- Locked sources inspected: Leaf `2f62208187f7980d066e479bd70bb55613c066d2`; smoltcp `0a926767a68bc88d5512afefa7529c5ecdade4ea`. The similarly versioned crates.io smoltcp checkout was explicitly rejected as invalid audit evidence.
- EasyTier path: `mesh_socks_bridge.rs::relay_socks5` -> `data_plane_tcp_connect_mesh_only` -> `Socks5AutoConnector` -> `SmolTcpConnector` when the mesh-owned selector has no usable QUIC/KCP stream.
- Current allocation: `Socks5ServerNet` constructs the shared smoltcp net with `tcp_rx_size = tcp_tx_size = 128 KiB`. smoltcp chooses window scaling from that fixed buffer at connection setup and the locked revision has no runtime resize API.
- Android 4G/KR RTT was approximately 260-300 ms. A 128 KiB receive window has a 3.5-4.0 Mbps bandwidth-delay ceiling; KR measured about 3.7 Mbps and 57.6% receive-window-limited time. This is direct causal evidence.
- Mihomo parity boundary: `common/net/sing.go::Relay` uses buffered bidirectional copy and `CloseWrite` over kernel TCP sockets; it does not add a userspace TCP receive window. EasyTier cannot copy that exact architecture because the policy process UID cannot safely route a kernel connection back through its own mobile VPN. The documented intentional difference is a bounded larger smoltcp window for native mesh fallback.
- Leaf boundary: `leaf/src/app/dispatcher.rs` passes `LINK_BUFFER_SIZE * 1024` (default 2 KiB) to its asynchronous copy loop. That block size may be a later CPU tuning candidate, but it does not advertise the 128 KiB TCP window and is not included in this root-cause fix.
- Memory boundary: native fallback gets 2 MiB in each direction for at most 32 active streams. Maximum additional allocation is 128 MiB. Further streams use the existing 128 KiB buffers instead of failing. Accelerated KCP/QUIC and ordinary EasyTier SOCKS/TCP proxy paths are unchanged.
- Validation tooling boundary: Chrome CDP page navigation is rejected for throughput completion evidence after Android exposed multi-megabyte unread kernel queues. The packaged captured-UID instrumentation probe gains bounded raw HTTP download, byte count, body time and Mbps fields.

## Scope and configuration boundary

The reported Android configuration is an explicit user SOCKS actor:

```yaml
server:
  virtual-ip: 10.44.0.8
port: 24443
via: mesh
```

It does not use portless managed HEV. Portless results below are an independent configuration. Neither result permits changes to the normal EasyTier mesh data plane.

## Exact unmodified candidate

- EasyTier commit: `d307a4e460a230599f595e1f59b832453d20b888`
- Profiling workflow run: `29467798238`
- Target: `x86_64-unknown-linux-musl`
- Leaf commit: `2f62208187f7980d066e479bd70bb55613c066d2`
- Underlay between the two isolated peers: `tcp6`, 0% observed loss
- Candidate bundle SHA256 verification passed on both peers

## Controlled Linux evidence

All policy measurements used the same client, target, HTTP service, physical path, EasyTier candidate, and transport flags.

| Path | Result | Leaf CPU evidence |
| --- | --- | --- |
| Physical IPv6, same 16 MiB file | about 4.47-6.10 Gbit/s | Leaf bypassed |
| Native EasyTier mesh, same 16 MiB file | about 218-291 Mbit/s | Leaf bypassed |
| Portless managed HEV | about 2.68-3.14 Mbit/s | main thread usually 91-96% of one CPU |
| Explicit `via: mesh`, `port: 11080` | about 2.83 Mbit/s | main thread usually 93-98% of one CPU |
| `MATCH,DIRECT` | about 10.5 Mbit/s | main thread usually 94-98% of one CPU |

The explicit test uses the same actor semantics as the Android report but a controlled peer and port. It proves that the severe Linux failure is not restricted to portless resolution. It does not replace Android real-device validation against `10.44.0.8:24443`.

During the failure:

- HEV stayed effectively idle and the native mesh path remained fast.
- `strace` attached to the Leaf worker for three seconds observed only the tracing helper thread waiting in `futex`; the main Leaf thread made no system calls.
- Five `/proc/<pid>/task/<tid>/syscall` samples reported the main thread as `running` while the helper remained in `futex`.
- EasyTier logged `TUN writer queue is full`; the cumulative fail-closed drop count increased by powers of two through `131072`.
- After the transfer stopped, roughly 192 background TUN packets per second were sufficient to keep the Leaf main thread runnable and the drop count increasing. This is not a throughput load that should occupy one CPU.

These observations place the first common bottleneck in the EasyTier-to-Leaf TUN/netstack path. Mesh, KCP, managed HEV, and explicit SOCKS add separate overhead, but none explains the DIRECT control failure.

## Source-level diagnosis

EasyTier generates a Leaf TUN inbound with `tun2socks: "smoltcp"` in `easytier-policy/src/leaf_config.rs::compile_leaf_config`.

Linux chooses `leaf::RuntimeOption::SingleThread` when `available_parallelism()` is one in `easytier-policy/src/bin/easytier-leaf-worker.rs::run`. Android uses `available_parallelism().min(2)` in `easytier/src/instance/virtual_nic.rs` and passes it through `easytier-policy/src/inprocess.rs::InProcessLeafRuntime::start`.

The vendored `third_party/netstack-smoltcp/src/tcp.rs::TcpListenerRunner::create` places `handle_packet` and `handle_socket` as sibling futures in one `tokio::select!`. In `handle_socket`, the loop previously awaited only when `iface_ingress_tx_avail` was false. If that flag stayed true, the future could loop without any await, monopolize its runtime worker, and prevent its sibling from consuming more packets. `third_party/netstack-smoltcp/src/device.rs::VirtualDevice::receive` clears the flag only after particular empty/backpressure observations, so a persistent or stale true value is possible.

The diagnostic change adds `tokio::task::yield_now().await` only in the true branch. It does not change:

- queue capacities or fail-closed behavior;
- packet formats, MTU, routing, DNS, or rule semantics;
- mesh selection, KCP, QUIC, smoltcp fallback, or HEV ownership;
- explicit-port versus portless actor resolution.

## Mihomo reference and intentional difference

Reference files and functions inspected before the diagnostic edit:

- `/Users/fanli/Documents/mihomo-rev/constant/tun.go`: `StackTypeMapping`, `TUNStack`.
- `/Users/fanli/Documents/mihomo-rev/listener/parse.go`: TUN listener default is `TunGvisor`.
- `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go`: stack construction through `tun.NewStack(...)`, activation through `tunStack.Start()`, and ownership cleanup through `Listener.Close()`.

Externally observable Mihomo semantics relevant here are that TUN stack choice is explicit, stack startup/cleanup has a dedicated lifecycle, and its default hot path is delegated to sing-tun/gVisor rather than a custom non-cooperative smoltcp loop. EasyTier intentionally differs because the pinned Leaf API currently uses its smoltcp TUN backend and an fd bridge. This note does not claim Mihomo TUN-stack compatibility.

## Validation gates for the diagnostic change

The change is not accepted merely because it compiles. The exact workflow artifact must prove:

1. DIRECT no longer leaves the Leaf main thread in a persistent user-space loop or overflows the TUN writer queue under the controlled transfer.
2. Explicit mesh and portless managed HEV both improve without changing actor selection or transport ownership.
3. Native mesh throughput and disabled-policy behavior do not regress.
4. IPv4 and external IPv6 policy traffic both complete.
5. Stop/start, worker failure, peer loss/recovery, and RSS/FD/thread/task counts return to baseline.
6. The target Android app, without uninstalling the separate baseline app, passes direct SOCKS versus Leaf explicit `10.44.0.8:24443` A/B after wireless ADB is available.

If the yield removes starvation but leaves unacceptable throughput, the next comparison is Leaf lwIP versus smoltcp or a corrected event-driven smoltcp wake protocol. Increasing queue sizes is not an acceptable substitute because it only delays fail-closed loss.
## 2026-07-17 USB Android / cellular independent baseline (`089d7e0a`)

This evidence is from a new USB-connected Android 13 device on a cellular dual-stack
network. It is not the earlier Android device or LAN, and no earlier Android result is
reused as its baseline.

- Exact candidate: `089d7e0a61132f8cb59e02919f9f23d6a66e496d`.
- The candidate used virtual IPv4 `10.44.0.90`; the live peer table was checked first.
  `10.44.0.88` and `10.44.0.89` were occupied, while `.90` was absent before startup.
- The production mesh peer at `10.44.0.8` observed `.90` joining over IPv6-capable
  transports (`ws6`, `tcp6`, and `wg6` in the first stable sample).
- Captured application probe UID `10020` was inside the active Tauri VPN UID ranges;
  candidate owner UID `10019` was excluded.
- `MATCH,DIRECT` control: trusted TLS to `www.cloudflare.com:443` completed in 552 ms.
- Explicit actor under test retained an actual port and was never portless:
  `virtual-ip: 10.44.0.8`, `port: 24443`, `via: mesh`, `udp: true`.
- Explicit actor trusted-TLS results: Cloudflare 1286 ms and 1385 ms; GitHub 1862 ms.
  Every valid run reported TCP connected, TLS handshake true, connected true, and
  `probe_valid=true`.
- A bounded capture on `10.20.0.65` observed the same probe as SOCKS5 negotiation,
  CONNECT, and TLS data on `tun0` from `10.44.0.90` to `10.44.0.8:24443`. This rules
  out an accidental DIRECT result for the explicit-actor probe.
- One GitHub probe launch crashed only because two instrumentation invocations were
  mistakenly started concurrently against the same runner package. Its serialized
  rerun passed and the concurrent launch is not product-failure evidence.

Controlled throughput used a temporary file server on the peer to remove CDN and
public-egress variance. Android shell curl is diagnostic throughput evidence only;
it is not accepted as policy correctness evidence.

| Path | Object | Time | Throughput |
| --- | ---: | ---: | ---: |
| policy disabled, direct SOCKS `10.44.0.8:24443` | 64 MiB | 25.25 s | 21.3 Mbit/s |
| explicit Leaf actor | 64 MiB | 23.85 s | 22.5 Mbit/s |
| policy disabled, direct SOCKS | 16 MiB | 10.48 s | 12.8 Mbit/s |
| policy disabled, direct SOCKS repeat | 16 MiB | 18.44 s | 7.3 Mbit/s |
| explicit Leaf actor | 16 MiB | 5.64 s | 23.8 Mbit/s |
| explicit Leaf actor repeat | 16 MiB | 10.46 s | 12.8 Mbit/s |

The cellular/mesh path is highly variable, but this candidate did not show a general
explicit-actor slowdown relative to direct SOCKS on this device. This does not explain
or invalidate the earlier device-specific severe slowdown. The old device must be
retested separately if it becomes available.
