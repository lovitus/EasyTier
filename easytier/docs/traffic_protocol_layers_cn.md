# EasyTier 流量、代理协议与封装层级

本文帮助用户区分 EasyTier 状态页和配置中容易混淆的几个概念：overlay
传输、QUIC/KCP TCP Proxy、用户 SOCKS5 portal、Leaf managed mesh SOCKS、
smoltcp、HEV，以及 Leaf 的 Shadowsocks、Trojan、VMess、VLESS actor。

本文描述的是当前实现，不把“候选优先级”误写成多个协议依次封装。除非特别说明，
下面的 QUIC/KCP 都指 EasyTier 内层 Proxy；“外层 QUIC”才指 peer 之间的 overlay
连接。

## 1. 先记住五层模型

一条流量最多涉及五个层级：

```text
第 1 层：流量入口
         普通 TUN / 用户 SOCKS portal / Leaf policy

第 2 层：可选的内层流式代理
         Native IP / QUIC Proxy / KCP Proxy / smoltcp / Leaf 协议 actor

第 3 层：EasyTier mesh packet
         ZCPacket、路由、可选压缩/加密

第 4 层：当前 overlay transport（逐跳）
         QUIC / TCP / UDP / FakeTCP / WS / WSS / WG 等

第 5 层：物理网络
         Ethernet/Wi-Fi -> IP -> TCP/UDP/其他物理报文
```

最容易混淆的是第 2 层和第 4 层。状态页同时出现：

```text
Overlay: QUIC
TCP Proxy: QUIC
SOCKS: KCP
```

通常不是一条流量依次经过这三个协议，而是表示：

- 当前 peer 的外层 overlay 使用 QUIC；
- 某些普通 TUN TCP 流量使用内层 QUIC Proxy；
- 某些用户 SOCKS TCP CONNECT 使用内层 KCP；
- 这些内层 QUIC/KCP packet 最后都可能由外层 overlay QUIC 承载。

## 2. 三套选择机制互相独立

| 选择器 | 选择范围 | 何时选择 | 已有连接是否迁移 |
| --- | --- | --- | --- |
| Overlay transport | peer 下一跳的 QUIC、TCP、UDP、WS、WG 等 | peer 连接和路由变化时 | packet 会使用当前下一跳，外层可随 mesh 恢复而变化 |
| TCP Proxy selector | QUIC Proxy -> KCP Proxy -> Native | 普通 TCP SYN 或 mesh stream 建立时 | 不迁移内层 stream；新连接重新选择 |
| 用户 SOCKS connector | KCP 或 smoltcp；非 mesh 目标还可 kernel direct | 每次 SOCKS TCP CONNECT | 不迁移；新 CONNECT 重新判断 |

`transport_priority` 控制的是外层 direct-connect/PeerConn 选择，不直接指定 Leaf 或
用户 SOCKS 的内层协议。TCP Proxy selector 也不会因为外层已经是 QUIC 就自动禁用
内层 QUIC/KCP。

多跳路由中，第 4 层是逐跳的：A -> B 可以是外层 QUIC，B -> C 可以是外层 TCP。
内层 KCP/QUIC Proxy packet 在 B 被作为 EasyTier packet 转发，不要求每一跳使用相同
overlay transport。

## 3. 三种容易被称为“SOCKS”的功能

### 3.1 用户 SOCKS5 portal

这是给浏览器、curl 或其他设备手工配置的 EasyTier SOCKS5 server。

CLI：

```bash
easytier-core --socks5 1080 --enable-kcp-proxy true ...
```

GUI：在网络设置中打开“Socks5 服务器”，填写端口。

客户端示例：

```bash
curl --socks5-hostname 127.0.0.1:1080 http://10.144.144.2:8080/
```

`--socks5 1080` 当前监听 `0.0.0.0:1080`。如不希望局域网任意设备使用，应通过主机
防火墙限制来源。当前正式维护边界以 TCP CONNECT 为主，不要把这个入口等同于 Leaf
managed HEV 的 TCP/UDP 出口。

### 3.2 Leaf managed mesh SOCKS

这不是用户可连接的公开端口，而是 Leaf `via: mesh` 的私有 bridge。启用 policy 后：

```yaml
proxies:
  mesh-exit:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    via: mesh
    udp: true
```

省略 `port` 表示使用目标 peer 的 EasyTier managed HEV。用户无需打开 `--socks5`，也
不要手工启动 `easytier-hev-socks-egress`。EasyTier 在第一次请求时懒启动 HEV，
内部 bridge 使用临时端口和凭据。

如果写了显式端口：

```yaml
proxies:
  peer-gost:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    port: 1080
    via: mesh
```

则 `10.144.144.2:1080` 必须由用户在目标 peer 上自行提供，不再表示 managed HEV。

### 3.3 Leaf native SOCKS actor

```yaml
proxies:
  local-mihomo:
    type: socks5
    server: 127.0.0.1
    port: 7890
    via: native
```

它不会启动 SOCKS server，只让 Leaf 使用已经运行的 Mihomo、sing-box、gost 等服务。
这里的连接不经过 EasyTier mesh，除非该 actor 前面另有 `via: mesh` actor 组成 chain。

## 4. 普通 TUN 流量

### 4.1 普通 UDP、ICMP

它们不经过 TCP Proxy：

```text
原始 IP/UDP 或 ICMP
  -> EasyTier ZCPacket
  -> mesh 路由
  -> 当前外层 overlay transport
  -> 目标 peer
```

例如外层选中 QUIC：

```text
原始 IP/UDP
  -> ZCPacket
  -> 外层 QUIC reliable stream
  -> 物理 UDP/IP
```

### 4.2 普通 TCP，TCP Proxy 选择 Native

Native 表示不建立内层 QUIC/KCP Proxy stream，原始 TCP/IP packet 直接交给 mesh：

```text
应用 TCP
  -> 原始 IP/TCP packet
  -> ZCPacket
  -> 当前外层 overlay
  -> 目标 peer TUN/TCP 入站
```

若外层恰好是 QUIC，物理封装仍是 TCP-over-overlay-QUIC，但没有第二个内层 QUIC
Proxy。

### 4.3 普通 TCP，TCP Proxy 选择 QUIC

```text
应用 TCP
  -> 本机 TCP Proxy 终止源 TCP 段
  -> payload 写入内层 QUIC bidirectional stream
  -> 内层 QUIC packet
  -> EasyTier ZCPacket（QuicSrc/QuicDst）
  -> 当前外层 overlay
  -> 目标 peer 解开内层 QUIC
  -> 目标 peer 新建 TCP 到最终目标
```

如果外层也选择 QUIC，线上的主要层级为：

```text
分段 TCP -> 内层 QUIC Proxy -> ZCPacket -> 外层 QUIC -> 物理 UDP/IP
```

这是真正的 QUIC-over-QUIC：内外两层有独立的可靠性、拥塞控制和连接状态。

### 4.4 普通 TCP，TCP Proxy 选择 KCP

```text
应用 TCP
  -> 本机 TCP Proxy
  -> KCP stream/segment
  -> EasyTier ZCPacket（KcpSrc/KcpDst）
  -> 当前外层 overlay
  -> 目标 peer 解开 KCP
  -> 目标 peer 新建 TCP 到最终目标
```

如果外层是 QUIC，则是：

```text
分段 TCP -> KCP -> ZCPacket -> 外层 QUIC -> 物理 UDP/IP
```

## 5. 用户 SOCKS5 portal 的 TCP 路径

### 5.1 目标是远端 mesh 地址，并直接选择 KCP

当 source 开启 KCP Proxy、目标 peer 通告允许 KCP input 时：

```text
客户端 TCP
  -> 本机 SOCKS5 portal 解析 CONNECT
  -> KCP stream/segment
  -> ZCPacket
  -> 当前外层 overlay
  -> 目标 peer KCP Proxy
  -> 目标 peer 新建 TCP 到 SOCKS 目标
```

这条 direct KCP 路径绕过通用 TCP Proxy selector。KCP 已经被选中后若 connect 失败，
当前 SOCKS CONNECT 直接失败，不会在同一个请求里改试 QUIC 或 smoltcp。

从实现上说，这里不是先构造一个 IP/UDP packet 再送回 NIC pipeline。
`Socks5KcpConnector` 直接建立 `KcpStream`；KCP output 被包成
`PacketType::KcpSrc/KcpDst` 的 ZCPacket，然后调用 `send_msg_for_proxy`。此时已经
没有可供 TCP Proxy 识别的 IP/TCP SYN，因此不会被再捕获一次。

KCP 通常被理解为基于不可靠报文的可靠协议，但在这条 EasyTier 路径中，
KCP segment 的直接下层是 ZCPacket，不是独立的 kernel UDP socket。最后在物理网络
上是 UDP 还是 TCP，由第 4 层当前 overlay transport 决定。

### 5.2 不能直接使用 KCP，选择 smoltcp

```text
客户端 TCP
  -> 本机 SOCKS5 portal
  -> smoltcp 创建一条虚拟 IP/TCP 连接
  -> smoltcp TCP SYN 进入通用 NIC pipeline
```

这里还有一层容易忽略的自动选择：smoltcp 产生的 TCP packet 会经过通用 TCP Proxy
selector。因此最终可能是：

```text
SOCKS -> smoltcp TCP -> QUIC Proxy -> ZCPacket -> 外层 overlay
SOCKS -> smoltcp TCP -> KCP Proxy  -> ZCPacket -> 外层 overlay
SOCKS -> smoltcp TCP -> Native     -> ZCPacket -> 外层 overlay
```

对用户 SOCKS portal 的稳态配置而言，可用 KCP 通常已在前一步被 direct KCP
选中，因此 smoltcp 后常见的实际结果是 QUIC Proxy 或 Native。上表保留
KCP 是为了准确表达 NIC pipeline 的能力；它需要 capability/路由在两步之间
发生变化等非稳态条件。

所以“SOCKS 显示 smoltcp”只说明 SOCKS connector 没有直接建立 KCP stream，不保证其
产生的 TCP SYN 最终一定绕过通用 QUIC/KCP TCP Proxy。

实际调用链是：`SmolTcpConnector -> Net::tcp_connect -> stack_stream ->
send_msg_by_ip -> NIC pipeline`。`send_msg_by_ip` 先把 packet 标记为普通
`PacketType::Data`，再运行 NIC filters。统一 `DeferredProxySelector` 的优先级为
100，会在旧的单项 QUIC/KCP filter 之前处理这个 SYN。因此这不只是取决于
偶然的注册顺序，而是当前实现明确保留的行为。

用户 SOCKS portal 自身并不直接调用统一 selector：它创建
`Socks5AutoConnector` 时明确设置 `mesh_stream_selector: None`。只有在它选择
smoltcp 后，smoltcp 产生的普通 TCP packet 才可能被 NIC pipeline 中的 selector
间接捕获。

### 5.3 非 mesh 目标

用户 SOCKS portal 允许 kernel fallback。目标不是可路由的 mesh 虚拟地址，或目标是
loopback 时，可以从运行 SOCKS portal 的本机直接建立 kernel TCP：

```text
客户端 -> SOCKS portal -> 本机 kernel TCP -> 目标
```

这条路径不经过 EasyTier overlay。用户 SOCKS portal 因此不是严格的 mesh-only 出口。

## 6. Leaf managed mesh SOCKS

### 6.1 TCP

Leaf 不自行决定 QUIC/KCP，而是把每条 mesh stream 交给 mesh-owned selector：

```text
普通应用
  -> Leaf TUN/policy rule
  -> 本机私有 SOCKS bridge
  -> mesh-owned selector
       ├─ QUIC Proxy stream
       ├─ KCP Proxy stream
       └─ 无可用 accelerator 时使用 mesh-only smoltcp
  -> 目标 peer managed HEV 或显式 SOCKS 端口
  -> 最终目标
```

QUIC 和 KCP 是候选，不是依次封装。每条连接只返回其中一个 stream；两者都不可用才
进入 smoltcp。此路径禁止 kernel fallback，mesh 不可达时 fail-closed，不会从 Leaf
所在主机直接访问最终目标。

smoltcp 产生的 TCP packet 仍会经过通用 NIC pipeline；因此它是 mesh-only、
不使用 kernel fallback 的 packet 路径，但仍可能被当前通用 TCP Proxy selector
再次捕获，最后由当前 overlay 承载。

### 6.2 managed UDP

`udp: true` 的 portless managed HEV UDP 首先尝试内部 policy UoT v2：

```text
Leaf UDP
  -> 私有 SOCKS/UDP association
  -> policy UoT v2 framing
  -> mesh-selected TCP stream（QUIC Proxy、KCP Proxy 或 smoltcp）
  -> 目标 peer managed HEV
  -> native UDP 到最终目标
```

若目标不支持内部 UoT v2 或建立失败，才回落 legacy mesh datagram relay：

```text
Leaf UDP
  -> tokenized legacy datagram relay
  -> userspace smoltcp UDP data plane
  -> ZCPacket
  -> 当前外层 overlay
  -> 目标 peer managed HEV
  -> native UDP 到最终目标
```

这里的内部 policy UoT 是 EasyTier managed relay 的实现细节，不等同于用户配置的
Shadowsocks `udp: uot-v2`。

## 7. Leaf 协议 actor 在哪一层

Shadowsocks、Trojan、VMess、VLESS、TLS 和 WebSocket 都位于第 2 层。它们不会改变
EasyTier overlay 的选择。

Native 协议节点：

```text
应用 -> Leaf -> TLS/WS/协议 actor -> 本机物理网络 -> 协议服务器
```

协议经 mesh peer：

```text
应用
  -> Leaf
  -> mesh SOCKS actor（由 mesh selector 提供 stream）
  -> TLS/WS/协议 actor 在该 stream 上握手
  -> 协议服务器
```

例如：

```yaml
groups:
  vless-through-mesh:
    type: chain
    members: [mesh-hop, vless-wss]
```

大致层级为：

```text
VLESS payload
  -> WebSocket
  -> TLS
  -> mesh-selected stream（内层 QUIC/KCP/smoltcp）
  -> ZCPacket
  -> 当前外层 overlay
```

`vless-wss` 自身仍为 `via: native`；是否经过 mesh 由前面的 `mesh-hop` 决定。

## 8. 外层 overlay 的实际封装

第 2 层最终生成 ZCPacket 或原始 mesh packet，随后才由第 4 层承载：

| 外层 overlay | 物理侧主要形状 | 说明 |
| --- | --- | --- |
| QUIC | IP -> UDP -> QUIC reliable stream -> EasyTier frame | 当前 overlay QUIC 使用双向 QUIC stream，不是 QUIC DATAGRAM |
| TCP | IP -> TCP -> EasyTier frame | 一个可靠 TCP tunnel stream |
| UDP | IP -> UDP -> EasyTier datagram | 无 TCP/QUIC stream 可靠性 |
| WS | IP -> TCP -> WebSocket -> EasyTier frame | WebSocket tunnel |
| WSS | IP -> TCP -> TLS -> WebSocket -> EasyTier frame | 加密 WebSocket tunnel |
| WG | IP -> UDP -> WireGuard -> EasyTier traffic | WireGuard transport |
| FakeTCP | FakeTCP wire packet -> EasyTier traffic | 由 FakeTCP transport 负责伪装和收发 |

EasyTier 还可能按配置对 ZCPacket 压缩或加密；Secure mode 则在 PeerConn
层提供认证和加密。不要仅因状态页显示 QUIC 就认为已经具有 TLS 意义上的
保密性：当前 EasyTier overlay QUIC 的自定义 `CryptoKey` 验证 checksum，但不加密
QUIC header 或 payload。保密性由 EasyTier packet encryption、Secure mode，或 WSS/WG
等额外安全层提供。

因此“Secure + 外层 QUIC”和“内层 QUIC Proxy + 外层 QUIC”都是多层处理，
但只有后者必然包含两套 QUIC 可靠性与拥塞控制。

## 9. 故障、恢复与自动升级

### 9.1 外层 overlay

KCP、smoltcp 和内层 QUIC Proxy 都把后续 packet 重新交给 PeerManager。外层 route 或
PeerConn 从 TCP/UDP 恢复为 QUIC 后，已有内层 stream 的后续 packet 可以开始由新的
外层 QUIC 承载。

这不会改变内层身份：

```text
原来：KCP -> 外层 TCP
后来：KCP -> 外层 QUIC
```

内层仍然是 KCP。

### 9.2 mesh-owned QUIC/KCP selector

已有内层 KCP 或 smoltcp stream 不会在线迁移为内层 QUIC。新连接重新选择：

- QUIC 未 degraded：下一个新连接立即再次尝试 QUIC；
- QUIC 连续 3 次硬失败后 degraded；
- degraded 状态每 30 秒允许一个新连接执行半开探测；
- 连续 3 次成功后完全恢复 QUIC 首选；
- 有持续新连接时，线路恢复后通常约 60–90 秒完成全局恢复；
- 没有新连接就没有主动后台探测。

因此稳定后会出现一段过渡期：旧 KCP/smoltcp 连接继续存在，新连接逐渐改用 QUIC。

### 9.3 用户 SOCKS direct KCP

用户 SOCKS portal 没有上述健康恢复状态机。每个新 CONNECT 只重新检查 KCP endpoint
和目标 capability：

- 已有 smoltcp 连接不升级；
- 后续新连接看到 KCP 可用时直接使用 KCP；
- 已选 KCP 后 connect 失败，不在同一个 CONNECT 中回落 smoltcp。

## 10. 如何解读状态页

不要只凭同一秒出现的协议名称推断一条流量的完整路径。建议同时确认：

1. 流量入口：普通 TUN、用户 SOCKS portal，还是 Leaf policy；
2. 目标：本机、mesh 虚拟 IP、peer-local 服务，还是公网地址；
3. 内层：Native、QUIC Proxy、KCP、smoltcp 或 Leaf 协议 actor；
4. 外层：该目标下一跳当前实际使用的 overlay transport；
5. 是否多跳：每个 overlay hop 的协议可能不同。

常见组合：

| 状态/配置 | 实际含义 |
| --- | --- |
| Overlay QUIC + TCP Proxy QUIC | 普通 TCP 使用内层 QUIC Proxy，其 packet 再由外层 QUIC 承载 |
| Overlay QUIC + SOCKS KCP | SOCKS payload 使用 KCP，其 packet 再由外层 QUIC 承载 |
| Overlay QUIC + SOCKS smoltcp | SOCKS 使用虚拟 TCP；其 packet 进入 NIC pipeline 后再由当前代理/overlay 承载 |
| Leaf mesh SOCKS + QUIC | Leaf 的私有 mesh stream 由统一 selector 选中内层 QUIC，外层协议另行决定 |
| Leaf `via: native` | Leaf 直接访问协议/SOCKS服务器，不使用 EasyTier mesh actor |

只有客户端跨 mesh 访问另一节点的用户 SOCKS portal 时，TCP Proxy 和 SOCKS connector
才可能成为前后两个连续连接段：

```text
客户端 -> TCP Proxy -> SOCKS 节点 -> KCP/smoltcp -> 出口节点
```

它们仍是被 SOCKS server 分开的两条连接，不是同一个 QUIC packet 内再嵌套 KCP。

## 11. 选择建议

- 只想让浏览器访问 mesh：打开用户 SOCKS portal，客户端配置该端口。
- 想按 GeoSite/GeoIP/规则让整机流量选择出口 peer：启用 Leaf policy，使用 portless
  `socks5 + via: mesh`。
- 已有 Mihomo/sing-box/gost：使用 `socks5 + via: native`，或者把它放在 mesh actor
  后组成明确 chain。
- 排查性能时必须同时记录内层 proxy 和外层 overlay；只写“使用 QUIC”无法说明是哪一层。
- 外层已经是可靠 QUIC stream 时，内层 QUIC/KCP 仍然能工作，但会增加协议头、状态机
  和重复可靠性；是否更快应以同一拓扑的 Native/QUIC Proxy/KCP 对照测量为准。

## 12. 实现依据

- Overlay QUIC stream：[`src/tunnel/quic.rs`](../src/tunnel/quic.rs)
- TCP Proxy selector 和健康恢复：[`src/gateway/proxy_failover.rs`](../src/gateway/proxy_failover.rs)
- 用户 SOCKS KCP/smoltcp 选择和 smoltcp 回注 NIC：
  [`src/gateway/socks5.rs`](../src/gateway/socks5.rs)
- SOCKS/Leaf 私有数据面：[`src/gateway/socks5/dataplane.rs`](../src/gateway/socks5/dataplane.rs)
- NIC pipeline 与 `send_msg_by_ip/send_msg_for_proxy`：
  [`src/peers/peer_manager.rs`](../src/peers/peer_manager.rs)
- KCP packet 绕过 NIC pipeline 进入 PeerManager：
  [`src/gateway/kcp_proxy.rs`](../src/gateway/kcp_proxy.rs)
- QUIC Proxy packet 进入 PeerManager：[`src/gateway/quic_proxy.rs`](../src/gateway/quic_proxy.rs)
- Leaf mesh TCP bridge：[`src/policy_proxy/mesh_socks_bridge.rs`](../src/policy_proxy/mesh_socks_bridge.rs)
- Leaf managed UDP/UoT relay：[`src/policy_proxy/mesh_udp_relay.rs`](../src/policy_proxy/mesh_udp_relay.rs)
- Leaf 部署和配置：[`leaf_policy_proxy_cn.md`](leaf_policy_proxy_cn.md)
- SOCKS 性能边界：[`socks5_performance_investigation_cn.md`](socks5_performance_investigation_cn.md)
