# Leaf Shadowsocks 与 UoT v2 最小接入计划

> 状态：post-v1 实现与验证进行中。
>
> 修订原则：不建设新的通用代理框架，不改变现有 policy、group、chain、fallback、DNS、HEV 或 EasyTier mesh 数据面。Shadowsocks 作为一个编译期协议适配模块接入现有 Leaf outbound 配置生成边界。

## 1. 目标

- 支持 Shadowsocks TCP。
- 支持 Shadowsocks 标准 UDP。
- 支持 SagerNet UDP-over-TCP v2，配置名为 `uot-v2`。
- Shadowsocks 使用 native endpoint；需要经 mesh 时，与现有 `via: mesh`
  SOCKS actor组成 chain。
- Shadowsocks 可以作为现有 chain、fallback 和规则出口组的成员。
- 给以后接入 Leaf 已支持的 Trojan、VMess、VLESS 等协议保留一个简单的编译期模块入口。

“未来兼容”只表示新协议复用现有的 rule、group、chain、fallback、DNS 和 `via` 架构。它不表示所有协议共用 Shadowsocks UoT v2，也不提前抽象各协议的 UDP wire format。

## 2. 明确不做

- 不引入动态插件、ABI、注册中心或 trait object。
- 不引入通用 transport contract、UDP transform 类型系统或新的 actor IR。
- 不修改现有 rule first-match、group、chain、fallback 或健康检查语义。
- 不修改 EasyTier mesh underlay、overlay、QUIC、KCP、smoltcp 或路由选择。
- 不增加新的 association manager、逐包 timer、线程池或通用生命周期层。
- 不引入 policy schema v2；只做旧 `udp: true/false` 的输入兼容。
- 不提前添加 Trojan、VMess、VLESS 的配置字段。
- 不承诺 UoT 比标准 UDP 性能更高，也不做 native UDP 与 UoT 的自动回退。

## 3. 参考实现与兼容边界

### 3.1 锁定 Leaf

实现审查以原 `Cargo.lock` 锁定提交为基线；UoT 实现冻结在后续 fork 提交：

```text
https://github.com/lovitus/leaf.git
base:      4af133266367bc6ef1d369b4b519a0a56da48760
candidate: 742ad65c441f9d60279916b82628b810efbd48fb
```

实现前核对以下文件：

- `leaf/src/proxy/shadowsocks/outbound/stream.rs`：Shadowsocks TCP stream actor。
- `leaf/src/proxy/shadowsocks/outbound/datagram.rs`：Shadowsocks 标准 UDP datagram actor。
- `leaf/src/proxy/chain/outbound/datagram.rs`：chain 按后缀的 reliable/unreliable 要求选择 stream 或 datagram transport。
- `leaf/src/proxy/failover/datagram.rs`：fallback 成员分别建立自己的 datagram outbound。

这些现有行为已经能够承担 transport 选择，不在 EasyTier policy 层复制一套 transport 状态机。

### 3.2 Mihomo 与 SagerNet UoT v2

实现前核对：

- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/shadowsocks.go` 中 Shadowsocks `StreamConn`、`ListenPacket` 和 `SupportUOT` 的外部语义。
- SagerNet sing `common/uot/protocol.go` 中 v2 request 和 magic destination。
- SagerNet sing `common/uot/conn.go` 中 unconnected packet framing。
- SagerNet sing `common/uot/lazy.go` 中首包延迟写入 request 的行为。

UoT v2 兼容语义固定为：

- magic destination：`sp.v2.udp-over-tcp.arpa:0`；
- unconnected request：显式写入 `0x00`，随后写初始目标的标准 SOCKS address；
- packet framing：每包使用 sing UoT 地址族 `0/1/2` 写目标或来源，再写 `u16`
  大端长度和 payload；
- 不依赖 Rust `bool` 的内存表示；
- 只实现 v2，不静默降级到 v1 或标准 UDP。

## 4. 配置

示例：

```yaml
proxies:
  ss-kr:
    type: shadowsocks
    server: 203.0.113.10
    port: 8388
    cipher: aes-256-gcm
    password: example-password
    via: native
    udp: uot-v2
```

首期支持锁定 Leaf 已有的 AEAD cipher：

- `aes-128-gcm`
- `aes-256-gcm`
- `chacha20-poly1305`
- `chacha20-ietf-poly1305`

`udp` 输入兼容：

```yaml
udp: false       # off
udp: true        # native，兼容旧配置
udp: off
udp: native
udp: uot-v2
```

规范化输出使用 `off`、`native` 或 `uot-v2`。旧版本不能读取新协议配置属于正常的功能版本边界，不为此增加 schema 迁移系统。

字段校验保持简单：

- Shadowsocks 必须提供 `server`、`port`、`cipher` 和 `password`。
- 不支持的 cipher 在配置阶段报错。
- `uot-v2` 首期只允许用于 Shadowsocks。
- `udp: off` 继续沿用当前 UDP 规则过滤行为。
- 不为尚未实现的协议预留或静默接受字段。

## 5. 最小代码结构

不移动或重写现有 SOCKS 编译代码。只新增一个 Shadowsocks 配置编译模块，由现有 Leaf 配置编译入口调用：

```text
easytier-policy/src/
├── leaf_config.rs
└── shadowsocks.rs
```

入口保持静态分派：

```rust
match proxy.kind {
    ProxyKind::Socks5 => compile_existing_socks5(proxy, context),
    ProxyKind::Shadowsocks => shadowsocks::compile(proxy, context),
    ProxyKind::Http => compile_existing_http(proxy, context),
}
```

不定义公共插件 trait。以后确实接入新协议时，再增加 `trojan.rs`、`vmess.rs` 或 `vless.rs`，并在同一个 `match` 中增加分支。

## 6. Actor 生成边界

Shadowsocks 适配模块只负责把一个逻辑 proxy 编译为 Leaf 已有
Shadowsocks actor。UoT v2 是该 actor 的 datagram mode，不新增通用协议层：

| 配置 | Leaf 内部路径 |
|---|---|
| native + TCP | Shadowsocks stream |
| native + standard UDP | Shadowsocks datagram |
| native + UoT v2 | Shadowsocks reliable datagram handler |
| mesh + TCP | chain `[mesh SOCKS, Shadowsocks]` |
| mesh + standard UDP | chain `[mesh SOCKS, Shadowsocks native UDP]` |
| mesh + UoT v2 | chain `[mesh SOCKS stream, Shadowsocks UoT v2]` |

约束：

- Shadowsocks 节点只接受 `via: native` 和地址字符串。
- 经 mesh 时，用户显式定义现有 mesh SOCKS actor，并在 chain 中把它放在
  Shadowsocks 前面；不增加 `via-peer` 或第二套 server 字段。
- group、rule 和用户配置继续只引用现有逻辑 actor 名称；不生成隐藏 tag。
- UoT 只存在于 Leaf 适配层，不进入 EasyTier mesh 数据面。

## 7. UDP 能力判断

继续使用当前 `actor_supports_udp` 布尔能力和现有递归规则：

- `udp: off`：false；
- SOCKS5 `udp: native`：true；
- Shadowsocks `udp: native`：true；
- Shadowsocks `udp: uot-v2`：true；
- chain：所有成员支持 UDP 时为 true；
- fallback：至少一个成员支持 UDP 时为 true。

不新增 `UdpPath` 或 transport contract。具体走 stream 还是 datagram 由锁定 Leaf 的 actor 和 chain/failover 逻辑决定。

## 8. UoT actor 范围

锁定 Leaf 没有 SagerNet UoT v2，因此在现有 Shadowsocks outbound datagram
handler 中增加一个窄模式。该模式只负责：

- 声明自己需要 reliable stream；
- 写入 UoT v2 unconnected request；
- 按包编解码目标/来源地址和长度前缀；
- 将关闭、错误和 cancellation 交还 Leaf 现有生命周期。

该 actor不得：

- 管理 EasyTier mesh transport；
- 实现新的 DNS、路由或 fallback；
- 自动重试另一种 UDP mode；
- 建立无界队列；
- 增加独立常驻 runtime。

## 9. 失败语义

- 配置错误在启动前失败，并指出 proxy 名称和字段。
- 标准 UDP 服务端不可达时直接失败，不自动切换 UoT。
- UoT 服务端不支持 v2 时直接失败，不自动切换标准 UDP。
- chain 中任一必需成员不支持 UDP 时，沿用当前 chain 不支持 UDP 的结果。
- fallback 继续沿用 Leaf 当前的成员选择和建立阶段回退，不承诺 payload 发送后的协议切换。
- policy proxy 失败不得影响 EasyTier mesh 基础连接。

## 10. 测试范围

### 10.1 配置与编译测试

- 旧 `udp: true/false` 与新字符串形式解析一致。
- Shadowsocks 必填字段和 cipher 校验。
- `uot-v2` 用于不支持的协议时明确报错。
- native、mesh、standard UDP、UoT 的 actor 顺序快照。
- 用户 group/rule 只引用逻辑 tag。
- 现有 SOCKS、HTTP、rule、chain 和 fallback 配置结果不变。

### 10.2 UoT 精确测试

- v2 magic destination byte vector。
- unconnected request 首字节固定为 `0x00`。
- request 使用标准 SOCKS 地址族，packet 使用 sing UoT `0/1/2` 地址族。
- IPv4、IPv6、域名目标和来源地址编码。
- 多个 packet 的长度、顺序和边界。
- EOF、半关闭、取消和错误传播。
- 服务端仅支持标准 UDP 或仅支持 UoT 时均保持明确 fail-closed。

### 10.3 互操作与实机验证

- Mihomo Shadowsocks TCP。
- Mihomo Shadowsocks 标准 UDP。
- Mihomo UoT v2。
- sing-box UoT v2。
- `via: native` 与 `via: mesh`。
- Shadowsocks 参与 chain 和 fallback。
- Linux、Android 的停止、重启、资源回基线。
- 标准 UDP 与 UoT 的吞吐、CPU、RSS、延迟和丢包对照。

服务端配置必须固定并随验证记录保存，避免误把标准 UDP、其他 UDP-over-stream framing 或未启用 UoT 的服务端当成 UoT v2 证据。

## 11. 实施顺序

1. 扩展最小配置字段和兼容解析，不改变现有协议行为。
2. 在锁定 Leaf fork 的 Shadowsocks datagram handler 中实现窄 UoT v2 模式及 byte-vector 测试。
3. 增加 `shadowsocks.rs`，调用 Leaf 已有 Shadowsocks actor。
4. 接入现有 `via`、chain、fallback 和 UDP 能力判断。
5. 在 `192.168.2.160` 完成 `--locked` no-run 和 focused tests。
6. 批量形成一个候选，由同一 Linux/Android artifact 验证全部 native、mesh、TCP、UDP、UoT、chain、fallback 和资源场景。

## 12. 复杂度预算与停止条件

首期实现应保持以下边界：

- EasyTier policy 侧只新增一种 proxy kind、一个 cipher 字段、一个 UDP mode 和一个编译模块。
- Leaf fork 侧只扩展 Shadowsocks datagram handler及其配置入口。
- 不修改 mesh、HEV、DNS、规则引擎、group/fallback 实现。

如果实现过程中必须修改 mesh transport、重写 fallback、引入新的通用 transport 类型系统或增加第二套生命周期管理，则说明边界判断错误，应暂停并重新评估，不继续扩大实现。

## 13. 未来协议接入

Trojan、VMess、VLESS 后续分别使用 Leaf 原生协议能力：

- 新增对应配置字段；
- 新增一个小型编译模块；
- 复用现有 `via`、group、chain、fallback 和 rule；
- UDP 使用该协议原生 framing，不默认套用 Shadowsocks UoT v2。

只有真实协议接入暴露出重复逻辑后，才提取共享帮助函数。禁止为了尚未实现的协议提前建立新的公共抽象。
