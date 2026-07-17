# Netstack runner event-driven fix and validation

Status: implementation and exact-candidate Linux/Android validation completed on
2026-07-18. This document closes the confirmed smoltcp runner busy-loop defect.
It does not claim that every remaining Android UID CPU/FD fluctuation is caused by
or fixed by netstack-smoltcp.

## Scope and compatibility boundary

- Candidate: `6dd2fe7e84ffd68fdb69d61088861c4c79ca7659`.
- Parent comparator: `48ae8825f627cde741bb1ff464718ac92fbafec6`.
- Locked Leaf source: `4af133266367bc6ef1d369b4b519a0a56da48760`.
- Reference lifecycle: Mihomo
  `listener/sing_tun/server.go::{New, Listener.Close}` owns and explicitly closes
  its stack. Locked Leaf `leaf/src/proxy/tun/inbound.rs::new_smoltcp` instead
  detaches the runner without retaining a join handle. This candidate keeps that
  Leaf ownership boundary and uses output-receiver closure as the runner exit
  protocol.
- Changed implementation files are limited to
  `third_party/netstack-smoltcp/src/device.rs` and
  `third_party/netstack-smoltcp/src/tcp.rs`. The preflight filter and workboard
  were updated separately.
- No Leaf API, route selection, HEV, mesh data plane, QUIC/KCP selection, packet
  format, MTU, TCP window, FakeDNS, UDP NAT, or first-match policy behavior was
  changed.

## Implementation

- Removed the separate ingress `AtomicBool`; the bounded channel and `Notify`
  are the only ingress state.
- Replaced the unbounded internal ingress with a bounded channel whose capacity
  is the existing `tcp_rx.max_capacity()` value. `handle_packet().await` now
  propagates backpressure instead of permitting unbounded growth.
- `VirtualDevice::receive()` reserves output capacity before consuming ingress,
  so output `Full` cannot drop or reorder the input packet.
- Output `Full` is sticky for the current `iface.poll()` and waits for a real
  sender permit before retrying. Output closure wakes the runner and returns
  `BrokenPipe`.
- `poll_delay=None` waits for ingress notification or close; zero delay consumes
  Tokio cooperative budget; a positive delay waits for notification, close, or
  timer. No fixed sleep or polling interval was introduced.

## Build and focused tests

- Local machine: formatting only; no repository compilation.
- Mandatory remote preflight: `scripts/leaf-remote-preflight.sh` on
  `192.168.2.160`, including the full locked no-run target and the exact focused
  Leaf/HEV/netstack tests.
- Added/passed tests:
  - `full_output_preserves_ingress_until_capacity_returns`
  - `bounded_ingress_backpressures_and_preserves_order`
  - `runner_exits_when_output_receiver_is_dropped`
  - `immediate_poll_path_keeps_runtime_cooperative`
  - existing `full_ingress_channel_wakes_waiting_stack_sender`
- Candidate inspection found no `Cargo.lock`, generated protocol, platform cfg,
  workflow pin, or dependency change.
- Linux workflow `29597762791` and Android workflow `29597762786` both passed.
- Linux artifact metadata: exact candidate SHA, run `29597762791`,
  `x86_64-unknown-linux-musl`, Rust 1.95.0, HEV pin
  `97e74f1068bd924e740032382cdc94ca83741ae6`; release SHA256 verification passed.
- Android artifact metadata and all packaged SHA256 entries passed; application
  data was preserved byte-for-byte across `adb install -r` before first start.

## Android evidence

Device package: `com.kkrainbow.easytier.policycandidate`. Only this candidate
package was upgraded. The exact workflow probe UID was inside the active VPN UID
ranges and was removed after validation.

Functional evidence before and after the lifecycle matrix:

- `www.baidu.com:443`, expected `GEOSITE,CN -> DIRECT`: valid TLS handshake,
  about 200-243 ms.
- `www.wikipedia.org:443`, expected overseas/MATCH -> explicit mesh SOCKS actor:
  valid TLS handshake, about 1.7-1.9 seconds.
- Both probes reported `probe_valid=true`, `probe_tcp_connected=true`, and
  `probe_tls_handshake=true`; TCP connect alone was not used as acceptance.

Busy-loop evidence:

- Confirmed parent symptom: one idle Leaf netstack runner consumed about 0.997
  CPU core; 95.11% of samples were in
  `netstack_smoltcp::tcp::TcpListenerRunner`, with zero UID network traffic.
- Candidate 60-second post-lifecycle sample: 5656.785 ms task-clock over 60.062
  seconds, or 0.0942 CPU core for the complete application UID.
- Candidate post-lifecycle symbol sample: two active `handle_socket` tasks were
  0.83% and 0.30% of sampled cycles; runner-create/main-loop symbols were about
  0.07%. No runner or old TID dominated a core after ten restarts.
- The full UID still exceeds the original 5% single-core target. Earlier exact
  profiling attributed the remaining dominant work to EasyTier direct connector,
  underlay, and QUIC activity, not the former netstack runner loop. This is a
  separate power follow-up and must not be reported as fixed by this candidate.

Lifecycle/resource evidence:

- Ten semantic UI stop/start cycles completed. Stop reduced the process from
  roughly 68-73 tasks to 62-63 tasks each time; the VPN and DIRECT/mesh-SOCKS TLS
  paths recovered after the matrix.
- RSS stayed in the observed 336-349 MiB band and did not grow monotonically.
- Active-process FD counts varied from 353 to 527 and later settled near 470,
  versus an initial active snapshot of 400. Peer/QUIC activity makes these
  instantaneous active counts noisy; this is not evidence of monotonic leakage,
  but it is also not a strict proof that Android FD count returns to one exact
  active baseline.
- With logging set to `off`, no regular file appeared under the application
  `files/` directory during the post-cycle sampling window.
- Wireless ADB temporarily reconnected during some VPN starts; Android system
  VPN state, not CDP response delivery, was used as the start/stop oracle.

## Linux evidence

Topology used an isolated source namespace on `192.168.1.37` and an iperf/SOCKS
target on `192.168.1.38`, with identical listeners, policy YAML, ports, namespace,
and target for parent and candidate.

Functional paths, candidate:

| Path | Result |
| --- | ---: |
| DIRECT TCP | 380.2 Mbps initial; 384.4 Mbps final |
| Portless managed HEV TCP | 405.5 Mbps initial; 425.8 Mbps final |
| Chain TCP | 399.1 Mbps initial; 415.9 Mbps final |
| Dead-first fallback TCP | 394.9 Mbps initial; 415.9 Mbps final |
| UDP mesh actor | 49.0 Mbps; 0.23% initial loss and 0% final loss |

Immediate same-window A/B was required because the earlier parent measurement
was taken under a materially different shared-network condition:

| Path | Candidate median, 3 x 8 s | Parent recheck median, 3 x 8 s | Delta |
| --- | ---: | ---: | ---: |
| DIRECT | 413.3 Mbps | 410.8 Mbps | +0.6% |
| Portless HEV | 422.0 Mbps | 428.1 Mbps | -1.4% |

Both paths pass the 95% no-regression gate. The older 563.0 Mbps parent DIRECT
number is retained as historical evidence but is not a valid same-window
comparator for this candidate.

Linux lifecycle/resource evidence used `/proc/PID/exe` basename identity rather
than cmdline substring matching:

- Ten candidate stop/start cycles: every old core/Leaf PID disappeared.
- Core after start: 11-12 tasks, 29-32 FD, about 18 MiB RSS.
- Leaf after start: 4 tasks, 11 FD, about 5.65 MiB RSS.
- No monotonic task, FD, or RSS growth.
- A fixed two-second ping succeeded in 4/10 cycles; the other cycles needed more
  route convergence time. After the final start and settle, mesh ping was 3/3.
- Final 60-second idle interval: core 0.1333% CPU and Leaf 0.0833% CPU. Core
  remained 9 tasks/29 FD and RSS decreased from 17,796 to 17,484 KiB; Leaf
  remained 4 tasks/11 FD/5,652 KiB.

## Cleanup and release judgment

- Linux test cores, Leaf workers, iperf servers, namespace, TUN devices, and the
  exact temporary NAT/FORWARD rules were removed; residual checks were empty.
- Android probe packages, CDP forward, and simpleperf temporary data were
  removed. The candidate application and its VPN were intentionally left running.
- Judgment: the event-driven runner fix is functionally compatible and shows no
  same-window Linux throughput regression. It removes the confirmed full-core
  runner defect without modifying Leaf/HEV/mesh semantics.
- Residual boundary: do not claim that overall Android battery use or exact active
  FD return-to-baseline is fully closed. Track those independently; they are not a
  reason to restore the known 0.997-core netstack busy loop.
