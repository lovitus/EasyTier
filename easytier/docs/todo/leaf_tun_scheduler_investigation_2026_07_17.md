# Leaf TUN scheduler investigation (2026-07-17)

Status: diagnostic fix awaiting exact workflow artifact validation. The Android explicit actor and Linux portless actor remain separate release gates even though both use the same Leaf TUN backend.

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
