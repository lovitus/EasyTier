# Stealth / Secure 已知问题

本文记录当前 `releases/v2.6.9` 代码线在 Stealth 与显式 `secure_mode` 组合上的已知问题。
结论来自 2026-07-08 的远端验证、实机观察和当前代码路径审计。

## 1. 显式 secure + Stealth 吞吐明显下降

**状态：已复现，未定位根因。**

复现场景：

```text
--secure-mode true
--stealth-mode true
--stealth-protocols tcp
```

在两台远端 Linux 测试节点的同 LAN TCP underlay 测试中：

| 模式 | 512 MiB 下载速度 |
| --- | ---: |
| plain | `107.75 MB/s` |
| `stealth_mode=true`，无显式 secure，运行期派生 secure | `107.91 MB/s` |
| 显式 `secure_mode.enabled=true` | `107.83 MB/s` |
| 显式 secure + Stealth | `10.90 MB/s` |

RSS 没有持续增长，约 `15 MB`；CPU 接近单核满载。因此目前判断更像 CPU 加密/封装热路径
瓶颈，而不是内存泄漏。

**当前不应做的结论**

- 不能说 Stealth 本身一定慢；派生 secure + Stealth 的测试没有明显慢。
- 不能说显式 `secure_mode` 本身一定慢；显式 secure 单独开启也没有明显慢。
- 不能说这是内存泄漏；当前 RSS 证据不支持。

**后续建议**

- 对 `secure_mode=true + stealth_mode=true` 做 CPU profiling。
- 优先检查是否存在重复保护、重复 copy、record protector 与 `PeerSessionTunnelFilter`
  叠加导致的热路径成本。
- 优先检查显式 secure 才会打开的 relay/session 分支：
  `RelayPeerMap::new()` 和 `PeerManager::RpcTransport` 使用
  `GlobalCtx::is_explicit_secure_mode_enabled()`，运行期派生 secure 不会启用这些分支。
  因此显式 secure 可能额外触发 `RelayPeerMap::ensure_session()`、
  `encrypt_payload()` / `decrypt_if_needed()` 等路径。该差异已通过代码审计确认，但
  2026-07-08 的慢速吞吐样本在 peer 状态里仍显示 `cost=p2p`、`tunnel_proto=tcp`，
  所以它还不能单独解释这次 LAN direct 测试的全部性能下降。后续 profiling 应分别对比
  direct p2p、relay/foreign network 两种拓扑。
- 单独比较 `tcp`、`udp`、`quic` 和 `ws`，确认是否只在 TCP Stealth record 路径明显。
- 在修复前，不建议对高吞吐场景默认推荐显式 `secure_mode=true + stealth_mode=true`。

## 2. 派生 secure 的性能接近 plain，不代表没有加密

**状态：代码路径已确认。**

`stealth_mode=true`、`network_secret` 非空且无显式 `secure_mode` 时，`GlobalCtx` 会在运行期
派生 `SecureModeConfig`。该配置只在 tunnel 携带有效 Stealth `OuterSessionState` 时被
`PeerConn` 使用。

当前代码路径：

- `GlobalCtx::get_effective_secure_mode()` 生成运行期派生 secure 配置。
- `GlobalCtx::get_secure_mode_for_tunnel(stealth_protected=true)` 只对
  Stealth-protected tunnel 返回该配置。
- `PeerConn::new()` 根据该配置启用 `PeerSessionTunnelFilter`。
- Noise 握手完成后，`PeerSessionTunnelFilter` 对普通 PeerManager payload 调用
  `PeerSession::encrypt_payload()` / `decrypt_payload()`。

因此，派生 secure 性能接近 plain 的合理解释是：当前热路径在该测试条件下开销较低，
而不是“完全没有加密”。

但派生 secure 不是显式 `secure_mode` 的全局替代品：

- 不写入 TOML/RPC。
- 不发布 RoutePeerInfo `noise_static_pubkey`。
- 不启用全局 RelayPeerMap / PeerManager secure relay/session。
- 不进入 credential 身份模式。
- 不保护未携带 Stealth `OuterSessionState` 的 legacy/plain PeerConn。

## 3. TCP strict Stealth listener 对同 secret plain 客户端不够严格

**状态：已复现。**

复现场景：

- 服务端：`stealth_mode=true`，`stealth_protocols=tcp`
- 客户端：`stealth_mode=false`
- 两端使用相同 `network_secret`
- 客户端拨 `tcp://server:port`

结果：客户端仍能建立连接并传输数据，peer 显示 `tunnel_proto=tcp`。

这说明当前 TCP Stealth listener 没有严格拒绝“同 secret 但未启用 Stealth”的 plain
客户端。文档中“strict listener 不接受 legacy/plain”的强语义当前应限定到已验证的
UDP strict listener；TCP/FakeTCP/WS/QUIC/WG/WSS 的 strict anti-legacy 行为需要逐项
验证和修复。

**风险**

- 对随机陌生探测的隐藏能力仍可能存在，但对“知道 network_secret、但未启用 Stealth”的
  旧/混合客户端，TCP listener 当前不够严格。
- 混合部署时，用户可能误以为所有协议 listener 都已经具备 UDP 同等级 strict 行为。

**后续建议**

- 为每个 Stealth 协议补负向测试：服务端启用该协议 Stealth，客户端关闭 Stealth，同 secret
  拨入必须失败。
- 修复前，文档必须明确“UDP strict listener 已验证；非 UDP strict listener 仍有已知缺口”。
- 修复时不要放宽 UDP 的 anti-probe 行为，也不要把 phase-2 数据面降级为 gate/plain。
