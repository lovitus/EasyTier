# Stealth 实现与兼容性说明

本文记录当前分支已经实现的多传输 stealth、混合版本协商、回环避退、协议优先级和
QUIC/KCP proxy failover。它描述的是当前代码事实。

## 1. 范围与启用条件

当前支持 `udp`、`tcp`、`faketcp`、`quic`、`wg`、`ws` 和 `wss`。`stealth_mode=true`
且 `stealth_protocols` 留空时仍只保护 UDP，避免升级后隐式改变其他传输的 wire format。
显式配置示例：

```text
--stealth-protocols udp,tcp,faketcp,quic,wg,ws,wss
```

以下条件必须同时满足，节点才会宣告对应结构化 capability 并实际启用：

- `flags.stealth_mode = true`
- secure mode 已启用
- `network_secret` 存在且非空

命令行入口：

```text
--stealth-mode
--stealth-window-secs <seconds>
--stealth-protocols <comma-separated protocols>
--disable-legacy-udp-hole-punch
--transport-priority <single-line rules>
```

对应环境变量使用同名 `ET_` 前缀。窗口配置为 `0`
时使用 60 秒默认值。stealth 是 opt-in，默认关闭，因此升级本身不会改变既有网络
行为。

## 2. 协议流程

### 2.1 Phase 1：预认证 gate

新 UDP 连接的 `Syn`/`Sack` 不再携带固定 magic body，而是携带 32 字节 gate
token：

```text
nonce[16] || HMAC-SHA256(network_secret-derived-window-key,
                         "et-gate-token" || nonce || conn_id)[0..16]
```

gate key 由 `network_secret` 和时间窗口派生。接收端接受当前窗口和前一窗口，
用于容忍有限时钟偏差。`GateReplayGuard` 只保留这两个窗口，每个窗口最多记录
4096 个 nonce，状态大小有界。

未通过 gate 验证的 UDP `Syn`、STUN 探测和其他 datagram 会被静默丢弃，不发送
可区分响应。

### 2.2 Noise 握手期间

通过 gate 后，UDP、QUIC 和 WG datagram 使用当前 gate key 做外层 seal，隐藏 tunnel header
和 peer-manager header。外层格式为：

```text
random_nonce[12] || ciphertext || tag[16]
```

TCP/FakeTCP 使用认证 preface 和带长度的 record protector；WS/WSS 使用认证 HTTP
upgrade header、challenge-bound response ACK 和受保护的 binary frame。当前 seal 使用
HMAC-SHA256 派生的独立 stream key 和 MAC key，并采用
encrypt-then-MAC。内层 payload 仍由 secure mode 的会话保护。

### 2.3 Phase 2：连接级 outer key

Noise 完成后，双方从相同的 handshake hash 派生连接级 `outer_key`。initiator 的
Noise msg3 仍使用 gate key seal，发送该包后切换；responder 验证 msg3 后切换。

切换后普通数据只接受 `outer_key`，不会把 gate key 重新作为通用数据面解密 key，
避免 phase-2 数据面降级。

每条连接持有独立 `OuterSessionState`。listener/connector 上的状态只是模板，
不会在多条连接间共享 handshake key。

## 3. 新旧版本协商

### 3.1 固定 listener / direct connect

UDP v1 继续通过 `PeerFeatureFlag.stealth_supported` 兼容旧版本；其他协议使用
`stealth_capabilities` 的 protocol/wire-version/level。新节点已获知目标 feature flag
且目标明确不支持某协议 stealth 时，只对该次 outbound connector 降级为 plain。
本地固定 listener 的安全策略不会因此放宽。

这意味着：

- 新 stealth 节点主动连接旧节点：使用 plain UDP，兼容旧节点。
- 旧节点主动连接新 stealth 节点的固定 UDP listener：探测被静默丢弃。
- 两端均宣告支持：使用 stealth UDP。
- feature flag 尚未知时，不主动假定远端不支持，避免无依据降级。

旧节点主动连接新节点失败是固定 stealth listener 的预期 anti-probe 行为，不是双向
自动协商失败。混合部署仍可依靠新节点主动连接旧节点、其他 underlay 或 relay 建立
初始可达性。

这里要区分两类路径：

- `direct-connect`：已经通过路由拿到目标 `PeerFeatureFlag` 时，可以按目标能力对该次
  outbound UDP 尝试降级 plain。
- `generic/manual/bootstrap udp://`：能力未知时先尝试 1 秒 stealth，再以新 `conn_id`、
  独立状态和同一 bound socket 执行 plain fallback。QUIC/WG/WS/WSS 同样使用独立连接
  尝试，避免迟到响应污染 fallback。

因此，若某节点开启 `stealth_mode=true` 且使用固定 UDP listener：

- 新 stealth 节点主动拨旧/plain 节点的静态 `udp://` 可自动 fallback；旧节点主动拨
  新节点的 strict listener 仍会被静默丢弃，因此混合部署不保证双向互拨。
- 这类失败主要发生在 `manual connector`、启动参数 `-p udp://...`、以及尚未形成路由
  前的 bootstrap UDP 场景。
- 这不是 “整个集群的所有初始连接都必须启用 stealth”。TCP、WebSocket、QUIC 等其他
  underlay 仍可先建立初始可达性，随后再进入带能力信息的 direct-connect / hole-punch
  流程。
- 如果部署目标要求 fixed UDP 在新旧节点间继续静态互通，只能采用显式 plain listener、
  双端口，或 URL/配置层显式声明能力；不能让同一 stealth listener 接受 plain `Syn`，
  否则会直接破坏 anti-probe 取舍。

### 3.2 UDP hole punch

hole-punch RPC 增加两个 optional 字段：

- 请求：`use_stealth`
- 响应：`stealth_enabled`

服务端只有在请求明确为 `true` 且本地确实支持时才选择 stealth listener。旧客户端
不会发送该字段，服务端按 `false` 处理并分配短期 plain listener。客户端以响应中的
实际选择为准配置 UDP connector；旧服务端缺失响应字段时也按 plain 处理。

plain 和 stealth hole-punch listener 分池管理，单一模式的突发请求不会占满另一
模式的 listener 配额。

`disable_legacy_udp_hole_punch=false` 为默认值。启用后只拒绝缺失 `use_stealth` 的旧
RPC (`None`)；新客户端显式 `Some(false)` 仍可协商 plain。

### 3.3 四种典型组合

| 发起端 | 接收端 | 固定 UDP direct | UDP hole punch |
| --- | --- | --- | --- |
| 新 stealth | 新 stealth | stealth | 协商 stealth |
| 新 stealth | 旧版本 | 已知不支持时降级 plain | optional 字段缺失，plain |
| 旧版本 | 新 stealth | 新固定 listener 静默丢弃 | 新节点为旧 RPC 分配 plain listener |
| 旧版本 | 旧版本 | 原 plain 行为 | 原 plain 行为 |

随着节点升级完成，plain hole-punch fallback 只会在对端未启用 stealth、能力信息
尚未形成或显式协商 plain 时触发。

## 4. 回环避退

只有完成 underlay 后确认 `peer id conflict` 才视为高置信 self-loop signal。
普通超时、拒绝连接和网络错误不会触发回环拉黑。

- direct connect：按 `(dst_peer_id, listener_url)` 拉黑 300 秒。
- UDP hole punch：按目标 `PeerId` 拉黑 300 秒。
- TCP hole punch：按目标 `PeerId` 拉黑 300 秒；响应侧同时按具体远端地址避退。
- 新的 hole-punch 路径不再把单个目标的回环升级为全局 scheme 熔断。
- TTL 状态使用有界 `TimedMap`，不会按历史流量永久增长。

`GlobalCtx` 中仍保留旧的 scheme/scope suppression 结构作为兼容安全栏，但 generic
connector 不以它为 gate，新 hole-punch 路径也不再向它记录 self-loop。

## 5. QUIC/KCP Proxy 修复

QUIC proxy 和 KCP proxy 共用 kernel `TcpProxy`。旧实现把虚拟网段的
`first_address()` 当作 loopback 伪源地址，例如为 `10.144.144.1/24` 选择
`10.144.144.0`。Linux 将该地址视为网络地址，重写后的 TCP SYN 在进入本地 proxy
listener 前即被丢弃，表现为 ICMP 正常但 TCP 超时。

当前实现选择网段中的确定性可用单播 host，并处理 `/31`、`/32` 边界。该修复不改变
QUIC/KCP 封装协议，只修正它们共用的 TCP capture 路径。

Linux 三节点测试已分别验证：

- QUIC proxy 的 proxy CIDR、虚拟 IP、TCP 和 UDP 数据路径。
- KCP-only proxy 的 proxy CIDR、虚拟 IP、TCP 和 UDP 数据路径。

Deferred-SYN selector 在改写前冻结目标 peer、原始 SYN 和发送 context，按
`QUIC -> KCP -> Native` prepare。prepared stream 绑定 flow generation 和目标 peer；
route 变化、late result、健康降级和 native fallback 均有界处理，proxy 分发保持本地
自投递，不设置额外 wire `no_proxy`。

## 6. Direct-connect 协议优先级

`transport_priority` 使用统一单行格式，例如：

```text
global:tcp,udp;wan:quic,wss;lan:udp,faketcp;10.44.0.3:tcp,quic
```

首版只重排 direct-connect 候选，不改变 manual/bootstrap 显式 URL。地址展开后先分
LAN/WAN bucket；LAN 全部失败后才进入 WAN。同协议地址并发，协议组间隔 300ms。
listener 不会因远端偏好而动态启动。

## 7. 当前安全边界

- stealth listener 不会因为旧 peer 存在而接受未认证 probe。
- phase-2 普通数据不会回退到 gate key。
- connector 侧的兼容降级只影响该次 outbound UDP 尝试。
- hole-punch plain listener 是协商后按需创建，不会把固定 stealth listener 改为
  plain。
- stealth 依赖共享 `network_secret`，不适用于没有该 secret 的公共共享 listener。
- fixed stealth UDP listener 的隐蔽性优先级高于 static/manual UDP 的新旧混合互通。
- phase-2 普通数据不会在任何已实现传输上回退为 gate-key 数据面。
- 所有 replay/session/pending-flow 表均有固定容量或 TTL。

## 8. 固定边界与后续项

- 不按远端请求动态启用未配置 listener。
- manual/bootstrap 显式 URL 不受 `transport_priority` 重排。
- LAN bucket 完整失败后才进入 WAN，这是明确接受的局域网优先取舍。
- 旧的 scheme-global suppression 兼容结构暂时保留，但 direct/hole-punch 的新增回环
  避退使用目标级 TTL。
