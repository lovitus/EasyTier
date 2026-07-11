# OpenWrt Overlay/Underlay Validation and QUIC IPv6 Listener Audit

[English](openwrt_overlay_underlay_quic_ipv6_2026_07_11.md) | [中文](openwrt_overlay_underlay_quic_ipv6_2026_07_11_cn.md)

Date: 2026-07-11

## Scope

This report compares an OpenWrt x86_64 router's EasyTier path to two remote
nodes with the corresponding public Internet paths. It also audits why the
default QUIC listener is IPv4-only and what would be required to enable IPv6 by
default safely.

Public host names and addresses are intentionally omitted. The two targets are
identified only by their EasyTier virtual addresses:

- Target A: `10.44.0.8`
- Target B: `10.44.0.12`

The router and both targets were already running EasyTier. No EasyTier process
was restarted or reconfigured. Temporary iperf3 listeners used private ports and
were removed after the test. One temporary IPv6 UDP firewall rule on Target A
was also removed after the test.

## Topology and State

The router ran Linux 5.4/OpenWrt and EasyTier `2.6.9-7080ecea`. Both targets also
reported `2.6.9-7080ecea` during the test.

Before load:

| Target | Route | Peer latency | Peer loss | Live transports |
| --- | --- | ---: | ---: | --- |
| A | DIRECT, path length 1 | about `144 ms` | `28%` | `udp6,ws6,tcp6` |
| B | DIRECT, path length 1 | about `180 ms` | `0%` | `tcp6,wg6,ws6,quic,ws` |

After all TCP and UDP tests, both routes were still DIRECT with path length 1.
Neither target fell back to relay. The router load average remained low.

## RTT

Each row used ten ICMP requests. These are short samples and should not be read
as long-term SLA measurements.

| Target | Path | Received | RTT min/avg/max |
| --- | --- | ---: | --- |
| A | EasyTier `10.44.0.8` | 5/10 | `143.3/143.6/144.1 ms` |
| A | Public IPv4 | 0/10 | ICMP blocked |
| A | One advertised public IPv6 | 9/10 | `207.3/208.4/211.2 ms` |
| B | EasyTier `10.44.0.12` | 10/10 | `184.5/241.7/571.6 ms` |
| B | Public IPv4 | 8/10 | `176.9/177.6/178.0 ms` |
| B | Public IPv6 | 8/10 | `185.1/185.7/186.1 ms` |

Target A's overlay RTT was lower than the single public IPv6 address selected
for the baseline. The peer advertises multiple IPv6 addresses and transports,
so this does not prove that encapsulation reduces RTT; it shows that the tested
public address was not equivalent to EasyTier's selected underlay path.

For Target B, normal overlay samples were approximately `185 ms`, close to the
public IPv6 path and about `8 ms` above public IPv4. Two overlay outliers raised
the ten-packet average substantially.

## TCP Throughput

Each result is an 8-second single-stream iperf3 test. `up` means router to
target; `down` means target to router. Two trials were run for the primary
comparison.

| Target | Path | Direction | Trial 1 | Trial 2 | Mean |
| --- | --- | --- | ---: | ---: | ---: |
| A | EasyTier | up | `39.9 Mbit/s` | `36.1 Mbit/s` | `38.0 Mbit/s` |
| A | EasyTier | down | `63.4 Mbit/s` | `59.3 Mbit/s` | `61.4 Mbit/s` |
| A | Public IPv6 | up | `45.3 Mbit/s` | `48.3 Mbit/s` | `46.8 Mbit/s` |
| A | Public IPv6 | down | `1.05 Mbit/s` | `9.81 Mbit/s` | `5.43 Mbit/s` |
| B | EasyTier | up | `35.9 Mbit/s` | `38.8 Mbit/s` | `37.4 Mbit/s` |
| B | EasyTier | down | `57.2 Mbit/s` | `59.8 Mbit/s` | `58.5 Mbit/s` |
| B | Public IPv4 | up | `48.1 Mbit/s` | `48.1 Mbit/s` | `48.1 Mbit/s` |
| B | Public IPv4 | down | `0.107 Mbit/s` | `0.272 Mbit/s` | `0.190 Mbit/s` |
| B | Public IPv6 | up | `4.95 Mbit/s` | not repeated | - |
| B | Public IPv6 | down | `0.146 Mbit/s` | not repeated | - |

The overlay upload cost relative to the best repeatable raw path was about 19%
for Target A and 22% for Target B. This includes encryption, encapsulation,
userspace forwarding, and any difference between the selected underlay and the
single public address used for comparison.

The raw public reverse TCP paths were severely impaired and highly variable.
EasyTier's reverse throughput was much higher and repeatable. This is not proof
that EasyTier generally accelerates TCP; the overlay can choose another
transport/address and its UDP/QUIC underlay can avoid a poor raw TCP path.

## UDP Throughput

UDP used 1,200-byte datagrams to avoid unnecessary overlay fragmentation. Tests
offered fixed `100 Mbit/s` and `50 Mbit/s` loads. The receiver rate and loss are
the meaningful values.

| Target | Path | Offered | Direction | Receiver rate | Loss |
| --- | --- | ---: | --- | ---: | ---: |
| A | EasyTier | `100 Mbit/s` | up | `39.0 Mbit/s` | `62%` |
| A | EasyTier | `100 Mbit/s` | down | `68.1 Mbit/s` | `32%` |
| A | EasyTier | `50 Mbit/s` | up | `44.5 Mbit/s` | `11%` |
| A | EasyTier | `50 Mbit/s` | down | `38.2 Mbit/s` | `24%` |
| B | EasyTier | `100 Mbit/s` | down | `23.5 Mbit/s` | `76%` |
| B | EasyTier | `50 Mbit/s` | down | `23.3 Mbit/s` | `53%` |
| B | Public IPv4 | `100 Mbit/s` | up | `55.7 Mbit/s` | `57%` |
| B | Public IPv4 | `100 Mbit/s` | down | `49.1 Mbit/s` | `57%` |
| B | Public IPv4 | `50 Mbit/s` | down | `26.3 Mbit/s` | `47%` |

Some raw and Target B forward UDP tests failed because the iperf control
connection completed but no UDP payload reached the receiver. Target A's public
IPv6 UDP remained unusable even after temporarily allowing the test port. Target
B's public forward UDP also intermittently delivered zero payload. Server logs
showed the router's CGNAT public source, so the failure is consistent with an
asymmetric firewall/NAT/public-path restriction rather than an EasyTier listener
failure.

Consequently, this test cannot provide a symmetric raw-UDP capacity baseline.
It does show that the overlay carried bidirectional application UDP to Target A
and reverse UDP from Target B while the comparable raw paths were asymmetric.
At `50-100 Mbit/s`, loss is high enough that these paths should not be described
as having a loss-free UDP capacity at the offered rates.

## Performance Conclusions

1. Both targets remained direct P2P throughout sustained TCP and UDP load.
2. Overlay TCP upload was approximately 19-22% below the best repeatable raw
   upload baseline.
3. Overlay TCP download was stable near `58-61 Mbit/s`, while the tested raw TCP
   reverse paths were badly impaired.
4. UDP is strongly asymmetric and lossy at `50-100 Mbit/s`; the public-path NAT
   and firewall behavior prevents a complete symmetric raw-UDP comparison.
5. No result here demonstrates a relay fallback, resource leak, or EasyTier
   process instability.

## QUIC IPv6 Listener Audit

### Current behavior

The CLI's generated all-protocol listener set uses
`quic://0.0.0.0:11012`. An equivalent GUI/manual listener has the same behavior. In
`instance/listeners.rs`, automatic IPv6 listener expansion explicitly excludes
QUIC based on this comment:

```text
quic enables dual-stack by default, may conflict with v4 listener
```

That statement does not describe the effective default behavior:

- An IPv4 UDP socket bound to `0.0.0.0` cannot receive IPv6 traffic.
- `QuicEndpointManager::server()` enables dual stack only when the configured
  address is IPv6 unspecified (`[::]`) and its `both` pool is enabled.
- Therefore the default `0.0.0.0` QUIC listener is IPv4-only. Runtime socket
  inspection and peer transport lists confirm this.

### History

The exclusion and comment were added by commit `40b5fe9a` in the 2025
`support quic proxy (#993)` change. At that point the QUIC listener still bound the configured address
directly and had no default-IPv4-to-dual-stack conversion. The comment was an
incorrect assumption even when introduced.

The current dual-stack endpoint pool was added later by commit `8311b117` in the
2026 QUIC endpoint manager refactor. It supports an explicitly configured `[::]` listener but the
old listener-manager exclusion remained unchanged. This is a stale integration
bug, not a documented Quinn limitation that requires QUIC to remain IPv4-only.

### Why simply removing the exclusion is unsafe

Removing `l.scheme() != "quic"` would create both `0.0.0.0:port` and
`[::]:port`. The current QUIC IPv6 endpoint initially requests dual-stack mode,
so it overlaps the existing IPv4 socket.

There are several concrete risks:

1. `tunnel/common.rs::setup_socket2_ext()` currently logs and suppresses IPv6
   bind errors. A conflicting QUIC IPv6 socket can therefore appear to start
   while remaining unbound or using port zero, instead of cleanly falling back.
2. If an OS permits overlapping sockets, IPv4 packets may be delivered to the
   wrong Quinn endpoint. QUIC connection IDs are endpoint-local, so this can
   cause intermittent handshake or established-connection loss.
3. A dual-stack socket reports IPv4 peers as IPv4-mapped IPv6 on Unix, while
   Windows commonly reports native IPv4. Breaker keys, Stealth session keys,
   logs, tunnel labels, and address guards must normalize this difference.
4. Replacing the IPv4 listener with a single `[::]` listener can remove QUIC
   completely on IPv4-only systems unless IPv6 bind failure has an explicit
   IPv4 fallback.
5. Enabling IPv6 expands the externally reachable UDP surface. Strict Stealth
   limits unauthenticated response behavior, but non-Stealth configurations
   still require correct host firewall policy.

Quinn itself supports IPv4-mapped IPv6 UDP and includes cross-platform handling;
the primary risks are EasyTier's listener orchestration, bind-error handling,
address normalization, and fallback behavior.

### Lowest-risk implementation direction

Do not silently rewrite every default QUIC listener to `[::]`, and do not rely
on an intentional bind conflict to select a fallback.

The lowest-risk design is:

1. Give `QuicTunnelListener` an internal bind mode: `V4Only`, `V6Only`, or
   `DualStack`. This is local state, not a public config or wire field.
2. Preserve the existing IPv4 listener. When `enable_ipv6=true` and the user
   configured an unspecified IPv4 QUIC listener, add one explicit `V6Only`
   listener on the same port. `IPV6_V6ONLY=true` makes coexistence deterministic.
3. Make QUIC bind errors authoritative. A requested nonzero port must either be
   bound exactly or return an error; it must never become a silent port-zero
   listener. Keep this correction local to QUIC unless the generic bind contract
   is separately reviewed.
4. Keep explicit `[::]` configuration as the existing `DualStack` behavior. The
   default companion path does not use IPv4-mapped addresses, so mapped-address
   normalization can remain a separate hardening task for explicit dual-stack
   listeners rather than expanding this fix.
5. Do not mutate the process-global endpoint-pool mode because one listener had
   a local collision.
6. Advertise IPv6 QUIC candidates only after the V6 listener has successfully
   bound and entered the running-listener set.

Required tests include Linux, macOS, and Windows IPv4/IPv6 same-port listeners;
IPv4-only hosts; explicit dual stack; occupied-port failure; multiple instances;
Stealth correct/wrong secret; socket mark/netns behavior; mapped-address
normalization; listener advertisement; and repeated IPv4/IPv6 QUIC reconnect.

## Final Assessment

Default QUIC being IPv4-only is an implementation integration bug. There is no
evidence that IPv6 QUIC itself is inherently unreliable. However, changing the
default by deleting one condition would be unsafe. A small QUIC-local bind-mode
extension plus strict bind verification and mapped-address normalization can add
default IPv6 coverage without changing wire behavior, configuration syntax,
Proxy ordering, or non-QUIC listeners.
