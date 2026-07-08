# Stealth 实现与兼容性说明

本文记录当前分支已经实现的多传输 stealth、混合版本协商、回环避退、协议优先级和
QUIC/KCP proxy failover。它描述的是当前代码事实。

如果你先想区分“这个 fork 相比上游修了什么、加了什么、参数怎么配”，请先看
[fork 差异与配置说明](fork_differences_cn.md)；本文只展开 stealth / proxy /
rollout 相关的实现与兼容细节。

2026-07-08 的远端验证报告见
[performance_validation_2026_07_08.md](performance_validation_2026_07_08.md)。
当前 Stealth/Secure 已知问题见
[known_bugs/stealth_secure_known_bugs.md](known_bugs/stealth_secure_known_bugs.md)。

## 1. 范围与启用条件

当前默认对 `udp`、`tcp`、`faketcp`、`quic`、`wg`、`ws` 和 `wss` 开启 Stealth。
显式把 `stealth_protocols` 设为空时才退回只保护 UDP 的兼容模式。配置示例：

```text
--stealth-protocols udp,tcp,faketcp,quic,wg,ws,wss
```

以下条件必须同时满足，节点才会宣告对应结构化 capability 并实际启用：

- `flags.stealth_mode = true`
- `network_secret` 存在且非空

未显式配置 `secure_mode` 时，认证握手配置只在运行期派生，不写入 TOML 或 RPC。
显式 `secure_mode` 仍表示高级 credential/Noise 配置；旧配置中存在 `[secure_mode]`
但未显式设置 `stealth_mode=true` 时会保持旧行为，Stealth 关闭。显式
`stealth_mode=true` 加 `secure_mode.enabled=false` 会被拒绝为配置冲突。
运行期派生的 Stealth 密钥只用于 Stealth-protected PeerConn 握手，不会作为
RoutePeerInfo 的 `noise_static_pubkey` 发布，也不会启用全局 RelayPeerMap/PeerManager
secure relay/session 语义。

对普通用户和 GUI 用户，推荐只理解 `network_secret + stealth_mode`：

| 配置方式 | 生效范围 | 不会改变的范围 |
| --- | --- | --- |
| GUI/新默认：`network_secret` + Stealth | 对配置的底层传输启用 Stealth 外层握手，并保护 Stealth-protected PeerConn 的内层 payload。 | 不发布 RoutePeerInfo `noise_static_pubkey`，不启用全局 RelayPeerMap/PeerManager secure relay/session，不进入 credential 身份模式。 |
| 显式 `secure_mode.enabled=true` | 启用完整显式 Noise 身份：RoutePeerInfo 公钥发布、secure relay/session、credential 兼容身份。 | 不是默认 Stealth 的必要条件；只在需要显式身份/credential/secure relay 语义时使用。 |

更直白地说：

- GUI 的 `network_secret + Stealth` 负责隐藏连接入口、防陌生探测、防止 DPI 直接识别
  EasyTier 握手。它作用在 `udp`、`tcp`、`faketcp`、`quic`、`wg`、`ws`、`wss`
  这些底层传输的握手入口。
- 打洞只负责找到路径；路径找到后，真正建立对应底层传输时仍可按 capability 和配置走
  Stealth。混合旧版本时，新节点可能为了兼容明确降级到 plain。
- 显式 `secure_mode.enabled=true` 负责节点身份：发布 Noise 公钥、允许其他节点 pin
  公钥、启用 secure relay/session，并支持 credential 临时节点不持有 `network_secret`
  也能被网络识别和撤销。
- 因此，普通隐藏协议特征和 anti-probe 不需要显式 `secure_mode`；需要 credential、
  共享节点身份、节点公钥 pinning 或 secure relay/session 时，才需要显式开启。
- 当前 GUI 只编辑 Stealth 偏好，不编辑显式 `secure_mode`。后续 GUI “全局安全身份”
  入口计划见 [todo/gui_global_secure_identity.md](todo/gui_global_secure_identity.md)。

命令行入口：

```text
--stealth-mode
--stealth-window-secs <seconds>
--stealth-protocols <comma-separated protocols>
--disable-legacy-udp-hole-punch
--transport-priority <single-line rules>
```

对应环境变量使用同名 `ET_` 前缀。窗口配置为 `0`
时使用 60 秒默认值。显式 `stealth_mode=false` 可恢复原有非 Stealth 行为。

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

注意：上面这条严格 anti-probe 语义当前只对 UDP gate 路径有明确验证。2026-07-08
实测发现 TCP Stealth listener 在同 secret、客户端关闭 Stealth 的混合场景下仍可连接。
非 UDP listener 的 strict anti-legacy 行为需要按协议继续修复和补测。

### 2.2 Noise 握手期间

通过 gate 后，UDP、QUIC 和 WG datagram 使用当前 gate key 做外层 seal，隐藏 tunnel header
和 peer-manager header。外层格式为：

```text
random_nonce[12] || ciphertext || tag[16]
```

TCP/FakeTCP 使用认证 preface 和带长度的 record protector；WS/WSS 使用认证 HTTP
upgrade header、challenge-bound response ACK 和受保护的 binary frame。当前 seal 使用
HMAC-SHA256 派生的独立 stream key 和 MAC key，并采用 encrypt-then-MAC。Stealth
保护的 PeerConn 会使用 effective secure mode；没有显式 `secure_mode` 时使用运行期
派生的会话配置保护该连接的内层 payload。

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

旧节点主动连接新节点固定 UDP listener 失败是 UDP stealth listener 的预期 anti-probe 行为，不是双向
自动协商失败。混合部署仍可依靠新节点主动连接旧节点、其他 underlay 或 relay 建立
初始可达性。

这里要区分两类路径：

- `direct-connect`：已经通过路由拿到目标 `PeerFeatureFlag` 时，可以按目标能力对该次
  outbound UDP 尝试降级 plain。
- `generic/manual/bootstrap udp://`：能力未知时先尝试 1 秒 stealth，再以新 `conn_id`、
  独立状态和同一 bound socket 执行 plain fallback。QUIC/WG/WS/WSS 同样使用独立连接
  尝试，避免迟到响应污染 fallback。
- manual connector 在有效 UDP stealth 开启时为整次连接保留 6 秒预算，覆盖 1 秒
  stealth attempt、最多 3 秒 plain attempt、地址处理和 PeerConn 握手；普通 plain UDP
  仍保持原 2 秒外层预算，避免无条件放慢失败检测。

因此，若某节点开启 `stealth_mode=true` 且使用固定 UDP listener：

- 新 stealth 节点主动拨旧/plain 节点的静态 `udp://` 可自动 fallback；旧节点主动拨
  新节点的 strict UDP listener 仍会被静默丢弃，因此混合部署不保证双向互拨。
- 这类失败主要发生在 `manual connector`、启动参数 `-p udp://...`、以及尚未形成路由
  前的 bootstrap UDP 场景。
- 这不是 “整个集群的所有初始连接都必须启用 stealth”。TCP、WebSocket、QUIC 等其他
  underlay 仍可先建立初始可达性，随后再进入带能力信息的 direct-connect / hole-punch
  流程。
- 如果部署目标要求 fixed UDP 在新旧节点间继续静态互通，只能采用显式 plain listener、
  双端口，或 URL/配置层显式声明能力；不能让同一 UDP stealth listener 接受 plain `Syn`，
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

这是一层“目标级避退 / backoff 强化”，不是“已经彻底消除所有回环流量”的承诺；当前
仍可能观察到残余回环或回环数据面问题，需要继续单独修复。

## 5. QUIC/KCP Proxy 修复

QUIC proxy 和 KCP proxy 共用 kernel `TcpProxy`。旧实现把虚拟网段的
`first_address()` 当作 loopback 伪源地址，例如为 `10.144.144.1/24` 选择
`10.144.144.0`。Linux 将该地址视为网络地址，重写后的 TCP SYN 在进入本地 proxy
listener 前即被丢弃，表现为 ICMP 正常但 TCP 超时。

当前实现选择网段中的确定性可用单播 host，并处理 `/31`、`/32` 边界。该修复不改变
QUIC/KCP 封装协议，只修正它们共用的 TCP capture 路径。

KCP 的连接生命周期使用仓库内 `third_party/kcp-sys`。关闭时先排空发送队列；收到
peer FIN 后先排空 KCP 已接收但尚未交给 stream 的数据，再返回 EOF。FIN 使用
`LastAck` 状态重传并通过 FIN+ACK 确认。若输出链路消失或对端始终不响应，5 秒后发送
RST 并同时删除 connection/state map；live state 最多 4096 项。该机制替代 KCP stream
关闭阶段的固定 sleep，保证正常关闭不丢尾部数据，异常关闭也不会永久保留坏连接。

本地 TCP proxy 从 pending SYN 转为 active connection 时，先发布 active map，再删除
pending map，避免首个 ACK/Data 在两个表之间的空窗期绕过本地 proxy。

Linux 三节点测试已分别验证：

- QUIC proxy 的 proxy CIDR、虚拟 IP、TCP 和 UDP 数据路径。
- KCP-only proxy 的 proxy CIDR、虚拟 IP、TCP 和 UDP 数据路径。
- QUIC-only、KCP-only 和 `QUIC -> KCP` 两候选在目标拒绝时均回退 Native，且
  `DESTINATION_FAILED` 不降低 transport health。
- QUIC/KCP prepare 被远端 ACL 拒绝时均回退 Native，且 `POLICY_DENIED` 不降低
  transport health。
- KCP 双节点连续 3000 次短 TCP 连接无数据错误、RST 或 forced close，结束后两端
  proxy 表为空；首个 FIN 丢失、接收 channel 堵塞和输出链路消失由确定性测试覆盖。
- 新 source 到旧 target 连续 500 次短连接通过。旧 source 仍保留旧版本自身的 TCP
  capture 故障，新 target 无法在远端修复该本地行为。

Deferred-SYN selector 在改写前冻结目标 peer、原始 SYN 和发送 context，按
`QUIC -> KCP -> Native` prepare。prepared stream 绑定 flow generation 和目标 peer；
route 变化、late result、健康降级和 native fallback 均有界处理，proxy 分发保持本地
自投递，不设置额外 wire `no_proxy`。

`failover` 状态表展示的是 selector 对 TCP SYN 的短期决策缓存，不是已建立
QUIC/KCP 封装连接的清单。因此 Pending、最终选择 Native 的流以及 prepare
失败过的流都可能显示；Pending 最长保留 5 秒，已决策项保留 30 秒后清理。
表中的 `src` / `dst` 是原始 TCP socket endpoint，`dst_peer_id` 才是路由解析的
目标 peer。

封装准备成功后，selector 会故意把 PeerManager header 标记为
`from_peer_id == to_peer_id == 本机`，用于把已准备 stream 交给本机 QUIC/KCP source
proxy。该包通过 pipeline 之后的发送入口交接，不会再次进入 selector，所以这种
“本机到本机”是内部交接，不是 underlay 回环。原始 TCP 目标精确等于本机虚拟 IP
时则会在进入清理、建表和 prepare 前跳过 failover selector。旧实现会让本机 proxy
目标连接再次被 TUN 捕获并递归进入 selector；与其他 TUN/proxy 软件共存时，可能
快速产生大量 self-target 连接和状态。该旁路不删除 prepared stream 的内部交接标记，
也不影响远端 QUIC/KCP Proxy。`failover` RPC 和 GUI 最多展示最近 256 条状态，避免
短连接风暴把诊断界面本身放大成内存问题。

新版本 peer 通过 `proxy_prepare_ack_version` 宣告就绪确认能力。双方支持 v1 时，
远端 proxy 在 ACL 通过后返回 `ACCEPTED`，目标 TCP 建立成功后返回 `READY`；源端只有
收到 `READY` 才提交 prepared stream。旧 peer 仍使用原有 fire-and-forget 流程，新远端
不会向未请求 ACK 的旧源端写入额外字节。QUIC/KCP Proxy 的选择顺序固定为
`QUIC -> KCP -> Native`，不受 `transport_priority` 控制。

仅收到 `ACCEPTED` 后超时记为 ambiguous soft strike；严格连续两次才折算一次 transport
failure，任一明确的 transport/业务结果都会中断该 soft-strike 序列。远端明确返回策略
拒绝、目标连接失败或业务超时不会降低 transport 健康度。全部候选失败进入 Native 时，
状态接口保留每个候选的具体失败原因，而不是只报告通用失败。

## 6. Direct-connect 协议优先级

`transport_priority` 使用统一单行格式，例如：

```text
global:tcp,udp;wan:quic,wss;lan:udp,faketcp;10.44.0.3:tcp,quic
```

常见全局偏好可以写成：

```text
global:quic,faketcp,ws,wg,udp,tcp
```

规则必须带 `scope:` 前缀；像 `quic,faketcp,ws,wg,udp,tcp` 这样的裸列表会被配置校验拒绝，
并报 `failed to parse transport_priority`。

首版只重排 direct-connect 候选，不改变 manual/bootstrap 显式 URL。地址展开后先分
LAN/WAN bucket；LAN 全部失败后才进入 WAN。同协议地址并发，协议组间隔 300ms。
listener 不会因远端偏好而动态启动。

数据面选择不是无条件按协议名覆盖最低延迟连接。`transport_priority` 为空时保持原有
最低 RTT 行为；非空时，Peer 会先找出最低 RTT 连接，再只保留
`candidate_rtt <= min_rtt * 125%` 的合格连接，最后在合格集合内按
`path_class -> protocol_rank -> RTT -> conn_id` 选择。也就是说，偏好协议恢复后只有
进入 125% RTT 合格线才会自动回切；否则继续使用更低延迟的低优先级热备连接。

exact virtual IP 规则针对 peer 级 direct tunnel。双栈 peer 的 IPv4 和 IPv6 地址同时
配置 exact 规则时，确定性采用 IPv4 规则；一条 direct tunnel 会同时承载该 peer 的
IPv4/IPv6 流量。

## 7. 当前安全边界

- UDP stealth listener 不会因为旧 peer 存在而接受未认证 probe。TCP 等非 UDP
  listener 当前存在 strict anti-legacy 已知缺口，见
  [known_bugs/stealth_secure_known_bugs.md](known_bugs/stealth_secure_known_bugs.md)。
- phase-2 普通数据不会回退到 gate key。
- connector 侧的兼容降级只影响该次 outbound transport 尝试，不放宽本地 listener。
- hole-punch plain listener 是协商后按需创建，不会把固定 stealth listener 改为
  plain。
- stealth 依赖共享 `network_secret`，不适用于没有该 secret 的公共共享 listener。
- fixed stealth UDP listener 的隐蔽性优先级高于 static/manual UDP 的新旧混合互通。
- phase-2 普通数据不会在任何已实现传输上回退为 gate-key 数据面。
- 所有 replay/session/pending-flow 表均有固定容量或 TTL。

## 8. 固定边界与后续项

- 不按远端请求动态启用未配置 listener。
- manual/bootstrap 显式 URL 不受 `transport_priority` 重排。
- `stealth_window_secs` 是网络级参数；`0` 等价于 60 秒，同一网络的 stealth 节点必须
  使用相同有效值。
- `disable_quic_input`/`disable_kcp_input` 只控制对应 Proxy 入站能力，不关闭底层
  `quic://` listener。
- LAN bucket 完整失败后才进入 WAN，这是明确接受的局域网优先取舍。
- 旧的 scheme-global suppression 兼容结构暂时保留，但 direct/hole-punch 的新增回环
  避退使用目标级 TTL。
