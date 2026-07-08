# EasyTier 2.6.8 远端功能与性能验证报告

本文记录 2026-07-08 对当前 `releases/v2.6.8` 代码线的远端验证结果。验证目标是把
GUI/CLI 默认 Stealth、派生 secure、显式 secure、KCP/SOCKS 和 Mihomo TUN 共存风险
放在同一组对照数据里，避免只凭单次现象判断。

## 1. 验证环境

- 代码线：`releases/v2.6.8`
- 被测提交：`514b8260 fix: harden underlay loopback guard [skip ci]`
- 二进制：`easytier-core 2.6.8`
- 服务端：远端 Linux 测试节点 A
- 客户端：远端 Linux 测试节点 B
- 链路：同 LAN，物理 HTTP baseline 约 `117.07 MB/s`
- 测试约束：不在维护者本机编译；临时 EasyTier 网络名均使用 `codex_perf_*` /
  `stealth-*`，测试结束后清理临时进程和监听端口。

测试节点 A 上存在一组早前 namespace/bridge 实验接口，例如 `br_a`、`veth_ns_*`、
`veth_net_*`。它们不是本次 EasyTier 性能测试进程残留，验证过程中没有自动删除。

## 2. 吞吐对照

测试方式：在测试节点 A 上启动 HTTP 文件服务，测试节点 B 通过 EasyTier TUN 地址下载文件。
除特别说明外，underlay 使用 `tcp://`。

| 模式 | 256 MiB x3 平均 | 512 MiB 单次 | 结论 |
| --- | ---: | ---: | --- |
| 物理直连 HTTP baseline | N/A | `117.07 MB/s` | LAN 对照上限。 |
| EasyTier plain | `106.71 MB/s` | `107.75 MB/s` | 接近物理链路，约为 baseline 的 91% 左右。 |
| `stealth_mode=true`，无显式 `secure_mode`，运行期派生 secure | `107.22-108.32 MB/s` | `107.91 MB/s` | 与 plain 基本一致，没有观察到明显吞吐损耗。 |
| 显式 `secure_mode.enabled=true` | `107.17-108.25 MB/s` | `107.83 MB/s` | 与 plain 基本一致，没有观察到明显吞吐损耗。 |
| 显式 `secure_mode.enabled=true` + `stealth_mode=true` | `10.80-10.95 MB/s` | `10.90 MB/s` | 明显性能缺陷，约只有 plain 的 10%。 |

补充 CPU/RSS 监控：

| 模式 | 服务端 CPU | 客户端 CPU | RSS |
| --- | ---: | ---: | ---: |
| plain | max 约 `79%` | max 约 `81.6%` | 约 `15 MB` |
| `stealth_mode=true` 派生 secure | max/avg `79.5% / 48.6%` | max/avg `72.2% / 50.8%` | 约 `15 MB` |
| 显式 `secure_mode.enabled=true` | max/avg `73.6% / 44.8%` | max/avg `71.0% / 48.5%` | 约 `15 MB` |
| 显式 secure + Stealth | max 约 `97-103%` | max 约 `97-103%` | 约 `15 MB` |

`secure + stealth` 的 RSS 没有持续增长，现象更像单核 CPU 加密/封装瓶颈，不像内存泄漏。
具体根因尚未定位，已记录为 known bug：
[Stealth/Secure 已知问题](known_bugs/stealth_secure_known_bugs.md)。

额外代码审计发现一个值得优先 profiling 的差异：`RelayPeerMap` 和
`PeerManager::RpcTransport` 使用 `is_explicit_secure_mode_enabled()`，只会在显式
`secure_mode.enabled=true` 时打开 secure relay/session 相关分支；运行期派生 secure
不会启用这些分支。这可以解释某些 relay/foreign network 拓扑下显式 secure 比派生
secure 更重。但本次慢速样本的 peer 状态仍显示 `cost=p2p`、`tunnel_proto=tcp`，
因此该分支目前只能作为 profiling 假设，不能直接认定为本次 LAN direct 吞吐下降的根因。
后续应补 direct p2p 与 relay/foreign network 两组对照。

## 3. 派生 secure 是否真的加密

这次性能数据本身不能证明“派生 secure 已加密”，因为吞吐接近 plain 也可能来自“没有进入
secure 数据面”。因此额外做了代码路径审计，结论如下：

- `GlobalCtx::get_effective_secure_mode()` 在 `stealth_mode=true`、`network_secret`
  非空且没有显式 `secure_mode` 时，会运行期生成 `SecureModeConfig`。
- `GlobalCtx::get_secure_mode_for_tunnel(stealth_protected=true)` 只在 tunnel 带有有效
  Stealth `OuterSessionState` 时返回该派生配置。
- `PeerConn::new()` 用这个配置启用 `PeerSessionTunnelFilter`。
- Noise 握手完成后，`PeerSessionTunnelFilter` 会对非握手、非 ping/pong 的
  PeerManager payload 调用 `PeerSession::encrypt_payload()` / `decrypt_payload()`。

因此，当前结论是：**派生 secure 不是完全没加密；它会保护 Stealth-protected PeerConn
的 payload。** 但它不是显式 `secure_mode` 的全局替代品：

- 不发布 RoutePeerInfo `noise_static_pubkey`。
- 不启用全局 RelayPeerMap / PeerManager secure relay/session 语义。
- 不参与 credential 身份模式。
- 只作用于携带 Stealth `OuterSessionState` 的 PeerConn。

这解释了为什么派生 secure 可以同时满足“有 Stealth-protected PeerConn payload 保护”和
“不等价于完整显式 secure mode”。

## 4. Stealth 协议连通性

两端均启用：

```text
--stealth-mode true
--stealth-protocols udp,tcp,faketcp,quic,wg,ws,wss
```

逐个协议作为唯一 peer URL 验证：

| 协议 | 结果 | 观察到的 tunnel proto |
| --- | --- | --- |
| `tcp` | 通过 | `tcp` |
| `udp` | 通过 | `udp` |
| `ws` | 通过 | `ws` |
| `quic` | 通过 | `quic6` |
| `faketcp` | 通过 | `faketcp_linux_bpf` |
| `wg` | 通过 | `wg` |
| `wss` | 未测 | 需要 TLS/cert 配置，不属于本次基础 smoke 范围。 |

这只证明两端均开启 Stealth 时基础连通，不等价于严格 anti-probe 覆盖全部协议。

## 5. Strict listener 边界

负向测试：

- 服务端：`stealth_mode=true`，`stealth_protocols=tcp`
- 客户端：`stealth_mode=false`
- 两端共享相同 `network_secret`
- peer URL：`tcp://<server>:<port>`

结果：客户端仍可连接并完成小文件传输，peer 显示 `tunnel_proto=tcp`。

这说明当前代码线的 **TCP Stealth listener 没有严格拒绝“同 secret 但未启用 Stealth”的
plain 客户端**。因此文档中“strict listener 不接受 legacy/plain”的强结论必须限定到
已验证的 UDP strict listener；TCP 等非 UDP 传输当前只能按“支持 Stealth 连接，但
listener strict anti-legacy 语义不完整”处理。

该问题已记录为 known bug：
[Stealth/Secure 已知问题](known_bugs/stealth_secure_known_bugs.md)。

## 6. 本机混合 mesh、KCP 和 SOCKS

本机运行最终版本时，观察到：

- 混合 peer 版本包括 `2.6.4`、`2.6.6`、`2.6.8`。
- 多个远端虚拟 IP ICMP 无丢包、无 DUP。
- 远端 SOCKS 服务可访问外网。
- 临时 KCP-only 节点经 KCP Proxy 完成小文件传输，Proxy 表显示 `Kcp`。
- KCP-only 小规模结果约 `3.9 MB/s`，测试后临时 peer 和 Proxy entry 可清理。

这些结果说明当前 KCP/SOCKS 基础可用，但不能证明高并发或 Mihomo TUN 共存下不存在
所有回环风险。相关边界继续以
[KCP SOCKS 生命周期缺陷与 Mihomo TUN 回环放大](known_bugs/kcp_bugs_and_mihomo_loopback.md)
为准。

## 7. Mihomo / 系统 TUN 共存

本机 Mihomo 观察结果：

- EasyTier 进程仍可能被 Mihomo TUN 捕获。
- 规则命中 `DIRECT` 后，2 分钟 idle 采样没有复现持续 CPU 跑满。
- EasyTier 侧 underlay guard 可以减少污染候选和回环放大，但不能替代 Mihomo/sing-box
  侧的 process bypass / route exclude。

因此，和 Mihomo/sing-box/Clash/NekoBox/Throne 等系统 TUN 工具共存时，仍必须在对方
配置里排除 EasyTier/Tailscale 进程，并把 EasyTier/Tailscale 虚拟网段设为 DIRECT。
完整规则见 [Mihomo TUN 互操作说明](mihomo_tun_interop_cn.md)。

## 8. 当前结论

- `stealth_mode=true` 派生 secure 的 TCP underlay 性能正常，约等于 plain。
- 显式 `secure_mode.enabled=true` 单独开启时性能正常，约等于 plain。
- 显式 `secure_mode.enabled=true + stealth_mode=true` 存在严重吞吐缺陷，需要后续 profiling。
- 派生 secure 已经通过代码路径确认会加密 Stealth-protected PeerConn payload，但不是
  全局 secure identity/relay/session。
- TCP strict listener 对同 secret plain 客户端不够严格，需要后续修复或明确降级文档语义。
- EasyTier 无法单方面彻底规避所有系统 TUN 回流；Mihomo/sing-box 侧排除规则仍是必要条件。
