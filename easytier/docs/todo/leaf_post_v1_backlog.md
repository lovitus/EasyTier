# Leaf Post-v1 Backlog

These items must not silently expand the Leaf v1 release boundary. Promote an item to `leaf_v1_release_gates.md` only when the maintainer explicitly changes the v1 scope.

## Transports and proxy composition

- Trojan、VMess、VLESS 已获准采用每协议一个窄 actor compiler，并通过现有 mesh SOCKS
  前置 actor 组合 native、TLS 和 WebSocket；Reality 等未实现 transport 继续留在首版后。
  当前实现边界和验收矩阵见
  [`leaf_trojan_vmess_vless_plugins_undecided.md`](leaf_trojan_vmess_vless_plugins_undecided.md)。
- Shadowsocks TCP/UDP plus explicit UoT v2 support, using the existing policy
  actor compiler and chain composition instead of a new proxy framework. The complete design and validation boundary are
  recorded in [`leaf_shadowsocks_uot_v2.md`](leaf_shadowsocks_uot_v2.md). UoT is
  selected explicitly and fails closed when the remote endpoint is incompatible;
  it is not probed and never silently falls back to native UDP.
- HEV's proprietary `FWD UDP` request uses SOCKS command `0x05` and HEV-specific UDP-in-TCP framing. Mihomo/sing UoT instead uses magic destinations and a different request/frame protocol; they are not configuration-compatible. Reusing HEV `FWD UDP` would require an explicit Leaf/EasyTier client adapter plus interoperability and fallback tests, not a hidden `uot: true` switch.
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

## Measured HEV worker policy (2026-07-16)

- Keep the v1 default at one HEV worker. Exact-candidate testing reached approximately `117 MB/s` for both one stream and eight concurrent streams with workers 1, 2, or 4; extra workers did not improve throughput.
- Worker growth increased idle ownership from `12 FDs / 2 threads` to `24 FDs / 5 threads`. A future target-specific override requires CPU saturation, latency, battery, or higher-concurrency evidence on that target rather than a CPU-count heuristic.
