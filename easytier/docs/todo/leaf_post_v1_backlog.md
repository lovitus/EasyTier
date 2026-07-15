# Leaf Post-v1 Backlog

These items must not silently expand the Leaf v1 release boundary. Promote an item to `leaf_v1_release_gates.md` only when the maintainer explicitly changes the v1 scope.

## Transports and proxy composition

- Optional UoT with explicit capability negotiation and a compatible remote endpoint.
- Optional KCP-based actors where their reliability/latency tradeoffs are justified; never present KCP as a cure for blocked UDP.
- Full UDP loss, reordering, rebinding, idle-timeout, and association-resource soak matrices.
- High-throughput UDP optimization and performance comparison against native mesh forwarding.
- More complex chain/fallback generation changes and concurrent health transitions.

## DNS and policy

- Split DNS and per-outbound resolver selection beyond the frozen v1 behavior.
- DNS cache replacement, leak, negative-cache, and network-generation matrices.
- Online GeoIP/GeoSite update, atomic replacement, rollback, and signature policy.
- Additional Mihomo/sing-box compatibility fields after explicit first-match, fallthrough, and unsupported-field tests.

## Platforms and lifecycle

- Full runtime validation on Windows, macOS, iOS, OHOS, FreeBSD, MIPS/MIPSel, and remaining 32-bit targets.
- Multi-instance policy proxy and multiple simultaneous TUN owners.
- Network namespace support beyond validation-only Linux isolation.
- Long-duration mobile background/foreground, sleep/wake, captive portal, DHCP, address-family, and network-handover soak.

## Product surface

- HTTP actor and additional inbound protocols.
- Advanced configuration editor and diagnostics without exposing internal Leaf/HEV implementation details.
- Online resource management UI.
- Detailed per-rule and per-outbound observability with bounded cardinality and storage.

## Performance

- Cross-platform CPU, latency, throughput, wakeup, allocation, RSS, FD, thread, and battery baselines against EasyTier 2.9.10.
- Profile-guided work only after exact-candidate correctness and lifecycle gates pass.
- Zero-copy or direct virtual-interface exit designs only if measurements show the temporary SOCKS boundary is materially limiting.
