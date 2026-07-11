# 底层传输协议 Stealth / DPI 抗性分析

本文档分析 EasyTier 各底层传输协议的 stealth 能力和 DPI（深度包检测）抗性，供协议选择和后续优化参考。

## 1. Stealth 架构概述

### 1.1 两阶段模型

源码位置：`easytier/src/tunnel/stealth.rs`

EasyTier 的 stealth 机制采用两阶段模型：

**Phase 1 — Pre-auth Gate（预认证门）**

- 基于 `network_secret` + 时间窗口的 HMAC-SHA256 令牌。
- 服务端只对持有正确 secret 的探测者响应，主动探测者得不到任何可区分的回复。
- 窗口默认 60 秒（`DEFAULT_GATE_WINDOW_SECS`），接收当前和上一个窗口以容忍时钟偏差。
- 令牌结构：16 bytes nonce + 16 bytes truncated HMAC tag = 32 bytes。

**Phase 2 — Outer Key（连接级密钥）**

- Noise 握手完成后，从握手哈希派生连接级密钥（`derive_outer_key`）。
- 用于后续数据包的 AEAD 加密，独立于时间窗口，不受时钟漂移影响。

### 1.2 Stealth 启用条件

`is_stealth_effectively_enabled()` 要求同时满足：

1. `stealth_mode = true`（默认开启）
2. `secure_mode = true`（或从 stealth_mode 派生）
3. `network_secret` 非空

### 1.3 Stealth 等级定义

源码位置：`easytier/src/common/stealth_registry.rs`

| 等级 | 常量 | 值 | 含义 |
| --- | --- | --- | --- |
| Silent | `STEALTH_LEVEL_SILENT` | 1 | 服务端不响应未认证探测，主动探测无效 |
| Authenticated | `STEALTH_LEVEL_AUTHENTICATED` | 2 | 需认证才建立连接，但流量模式可识别 |
| Camouflaged | `STEALTH_LEVEL_CAMOUFLAGED` | 4 | 流量伪装成正常 HTTPS，最难区分 |

## 2. 逐协议分析

### 2.1 QUIC — Stealth Level: Silent

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_SILENT` |
| 底层协议 | UDP |
| 默认端口 | 11012 (11010 + offset 2) |
| 拥塞控制 | BBR |
| Crypto | 自定义（SeaHash checksum，非 TLS 1.3） |

**Stealth 机制**：

- Stealth gate：服务端不响应未认证的 QUIC Initial 包，主动探测无效。
- `stealth_server_config()` 禁用 QUIC migration，防止地址关联攻击。
- 自定义 crypto 层：使用 `CryptoConfig` + `CryptoKey`（SeaHasher checksum）替代标准 TLS 1.3。

**DPI 抗性分析**：

| DPI 层级 | 能否识别 | 说明 |
| --- | --- | --- |
| L3/L4 (端口/协议) | 部分 | UDP 443 上的 QUIC 很常见，但 EasyTier 用非标准端口 |
| L7 包头检查 | **可识别** | QUIC Long Header 可识别，但 crypto frame 不是标准 TLS ClientHello（无 SNI、无证书、无 ALPN） |
| 流量模式分析 | 中等 | QUIC 流量模式与 Chrome HTTP/3 不同，高级 DPI 可区分 |
| 主动探测 | **无效** | Stealth gate 不响应未认证探测 |

**关键风险**：自定义 crypto（非 TLS 1.3）意味着没有标准 ClientHello。高级 DPI 可以通过 "QUIC 包头 + 非 TLS crypto frame" 模式识别为非标准 QUIC。

**结论**：中等 DPI 抗性。看起来像 QUIC（常见流量），但深入检查可区分。

### 2.2 FakeTCP — Stealth Level: Authenticated

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_AUTHENTICATED` |
| 底层协议 | TCP（模拟三次握手） |
| 默认端口 | 11013 (11010 + offset 3) |

**Stealth 机制**：

- 模拟 TCP 三次握手（SYN/SYN-ACK/ACK），在网络层看起来像正常 TCP 连接。
- 握手后载荷为 EasyTier 自定义协议（非 HTTP/TLS/SSH）。
- 需认证才建立连接。

**DPI 抗性分析**：

| DPI 层级 | 能否识别 | 说明 |
| --- | --- | --- |
| L3/L4 (端口/协议) | 不易 | 看起来像 TCP 连接 |
| L7 深度检查 | **可识别** | 握手后载荷不是任何已知应用层协议 |
| 流量模式分析 | 中等 | TCP 连接但没有 HTTP/TLS/SSH 特征 |
| 主动探测 | 需认证 | 无法直接探测 |

**关键风险**：能骗过只看 TCP 头的简单防火墙，但骗不过深度检查。中等 DPI 可能标记为 "未知 TCP 协议"。

**结论**：中等 DPI 抗性。过简单防火墙有效，过深度检查无效。

### 2.3 TCP — Stealth Level: Authenticated

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_AUTHENTICATED` |
| 底层协议 | TCP |
| 默认端口 | 11010 |

**DPI 抗性分析**：

- 纯 TCP 连接，载荷是 EasyTier 自定义协议。
- 无 HTTP/TLS/SSH 等已知协议特征，容易被 DPI 标记为 "可疑 TCP"。
- 在严格 DPI 环境下最容易被阻断。

**结论**：DPI 抗性较弱。

### 2.4 UDP — Stealth Level: Silent

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_SILENT` |
| 底层协议 | UDP |
| 默认端口 | 11010 |

**DPI 抗性分析**：

- 原始 UDP，载荷加密但包头就是 UDP。
- 非 DNS/NTP/QUIC 的 UDP 流量，在很多网络环境中直接被阻断。
- Stealth gate 提供主动探测防护。

**结论**：DPI 抗性最弱。很多防火墙默认阻断未知 UDP。

### 2.5 WebSocket (WS) — Stealth Level: Authenticated

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_AUTHENTICATED` |
| 底层协议 | TCP |
| 默认端口 | 80 |

**DPI 抗性分析**：

- HTTP Upgrade 握手 + WebSocket 帧。
- 看起来像 WebSocket 连接，但 URL path 和内容非标准。
- 能骗过只检查 "是 WebSocket" 的 DPI。

**结论**：中等偏上 DPI 抗性。

### 2.6 Secure WebSocket (WSS) — Stealth Level: Camouflaged

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_CAMOUFLAGED` |
| 底层协议 | TCP |
| 默认端口 | 443 |

**DPI 抗性分析**：

- 完整 TLS 握手 + WebSocket over HTTPS。
- 流量看起来像正常 HTTPS，最难区分。
- 可配合 CDN/反向代理使用，进一步隐藏。

**结论**：DPI 抗性最强。

### 2.7 WireGuard (WG) — Stealth Level: Silent

| 属性 | 值 |
| --- | --- |
| Stealth 等级 | `STEALTH_LEVEL_SILENT` |
| 底层协议 | UDP |
| 默认端口 | 11011 (11010 + offset 1) |

**DPI 抗性分析**：

- WireGuard 有非常特征性的握手包模式（1-2-3 消息握手）。
- DPI 可以精确识别 WireGuard 流量（即使加密）。
- Stealth gate 提供主动探测防护，但被动流量分析仍可识别。

**结论**：DPI 抗性弱（容易被识别为 WireGuard）。

## 3. 综合对比

| 协议 | Stealth 等级 | 主动探测防护 | 被动 DPI 抗性 | 跨境性能 | 推荐场景 |
| --- | --- | --- | --- | --- | --- |
| **QUIC** | Silent | ✓ 强 | 中等 | **最佳** (BBR) | 首选，跨境优先 |
| **WSS** | Camouflaged | ✓ | **最强** | 中等 (TCP) | 严格 DPI 环境首选 |
| **WS** | Authenticated | ✓ | 中等偏上 | 中等 (TCP) | 备选，过简单 DPI |
| **FakeTCP** | Authenticated | ✓ | 中等 | 中等 (TCP) | 过简单防火墙 |
| **WG** | Silent | ✓ | 弱 | 好 (UDP) | 信任网络，非 DPI 环境 |
| **TCP** | Authenticated | ✓ | 弱 | 差 (TCP) | 最后 fallback |
| **UDP** | Silent | ✓ | **最弱** | 好 (UDP) | 信任网络，非 DPI 环境 |

## 4. Fallback 链路 DPI 风险

当前推荐配置 `global:quic,faketcp,ws,wg,udp,tcp` 的 fallback 链路：

```
QUIC (中等 DPI 抗性)
  → FakeTCP (中等)
    → WS (中等偏上)
      → WG (弱)
        → UDP (最弱)
          → TCP (弱)
```

**主要风险**：当 QUIC 被阻断后，fallback 到 WG/UDP/TCP 时 DPI 抗性明显下降。在严格 DPI 环境下，这些 fallback 协议也可能被阻断，导致最终无法连接。

**建议**：

1. **在 transport priority 中加入 WSS**：`global:quic,wss,faketcp,ws,wg,udp,tcp`。WSS 作为 QUIC 后的第二选择，在严格 DPI 环境下提供最强伪装。
2. **考虑 QUIC TLS 伪装模式**：让 QUIC 使用真实证书 + SNI，使流量看起来像标准 HTTP/3。
3. **FakeTCP 流量伪装增强**：考虑在握手后模拟已知协议的流量模式（如 TLS ClientHello 开头）。

## 5. KCP 代理 vs 底层传输

KCP 在 EasyTier 中作为 **代理层协议** 而非底层传输协议实现：

| 层级 | 协议 | 用途 |
| --- | --- | --- |
| Tunnel (底层传输) | TCP, UDP, WG, QUIC, WS, WSS, FakeTCP | peer 间建立直连隧道 |
| Proxy (代理层) | TCP proxy, KCP proxy, QUIC proxy | 在已建立隧道上代理用户 TCP 流量 |

KCP proxy 复用已建立 tunnel 的传输能力，只负责可靠性层（激进重传 + 低延迟）。作为 proxy 层协议，它可以按需启用，不影响 peer 连接本身。

如果将 KCP 做成独立 tunnel 协议，它和 QUIC 的定位会高度重叠（都是 UDP 上的可靠传输 + 拥塞控制），而 QUIC 已经是更完整的方案（自带多路复用、stealth gate）。

## 6. 测试验证摘要

2026-07-09 多节点测试验证结果：

- **Transport priority**：`global:quic,faketcp,ws,wg,udp,tcp` 配置被正确执行，QUIC/FakeTCP 被优先选择。
- **Failover/fallback**：当 QUIC 被阻断时，正确 fallback 到 FakeTCP → WS → ... → TCP。
- **KCP proxy**：通过 SOCKS5 + KCP proxy 代理 TCP 流量功能正常。
- **QUIC proxy**：通过 SOCKS5 + QUIC proxy 功能正常。
- **Stealth mode**：默认开启（除非用户明确禁用），符合预期。
- **端口冲突**：QUIC 默认 11012，UDP 默认 11010，不会冲突（测试中手动配同端口才导致问题）。
