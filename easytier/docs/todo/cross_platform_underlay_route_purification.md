# 跨平台 Underlay 路由净化

## 状态与优先级

- 状态：**待实现**。
- 优先级：后于
  [`p2p_connection_storm_normalization.md`](p2p_connection_storm_normalization.md)。
- 主要已确认缺口：macOS 和 Windows 桌面在另一个 TUN/policy VPN 共存时，EasyTier 部分
  underlay socket 未显式绑定物理出口。
- 实现必须全局考虑 Linux、Android、iOS、OHOS、FreeBSD 和 `bind_device=false`，但不能
  无证据改变这些平台的现有生产语义。
- 当前阶段只记录问题和候选边界；不授权提交、push、构建或工作流。

## “路由净化”的准确含义

本 TODO 中的路由净化不是动态改写 Mihomo 配置，也不只是向系统路由表添加 endpoint
host route。目标是：

> EasyTier 自己创建的 underlay socket 在首次网络 I/O 前携带明确、可验证、与当前网络
> generation 一致的物理出口身份，不被 EasyTier、Mihomo、Wintun 或其他 policy TUN
> 再次捕获。

Mihomo中的 `PROCESS-NAME,easytier-gui,DIRECT` 发生在包进入 Mihomo TUN 之后，只能选择
Mihomo的 direct outbound。它不能阻止 TUN 捕获，也不能消除 Mihomo为每个UDP五元组创建
的替代 socket 和会话状态。

动态维护 `route-exclude-address` 同样不适合作为主方案：

- peer endpoint 动态变化且包含 IPv4/IPv6、STUN映射和多协议端口；
- endpoint 生命周期和系统路由生命周期难以原子同步；
- 多实例和多个 VPN owner 容易互相覆盖；
- 路由排除无法替代 socket source/interface 的明确所有权。

## 当前实机证据

macOS测试机只读观测：

- Mihomo controller 记录约 202 条来自 `easytier-gui` 的 UDP `DIRECT` 会话；
- 约 85 条指向同一远端 endpoint，与 hard-symmetric 84-socket pool 吻合；
- Mihomo看到的 source address 为其 TUN gateway，证明包先进入了 Mihomo TUN；
- `nettop` 显示 wildcard UDP socket 走 Mihomo utun，而明确绑定物理 IPv4 的 socket 走物理
  Wi-Fi接口；
- EasyTier root core 进程本身约有 508 个 UDP socket，Mihomo又为捕获的五元组维护第二份
  socket/session状态。

该证据证明 macOS 缺口真实存在，但不证明每个平台、每种 socket 都有相同故障。

## 已实现基础

v3.0.6 的 interface enumeration remediation 已提供本候选所需的主要基础：

- `UnderlayInterfaceSnapshot`：地址、接口 name/index、generation 和 unmapped fallback；
- 5 秒按需 TTL、singleflight 和 invalidation epoch；
- `UnderlayPreflightGuard::underlay_snapshot()`：一次 attempt 使用同一个快照；
- `ResolvedBindAddr` 和 crate-private `bind_resolved()`；
- macOS/iOS resolved index 可直接用于 `IP_BOUND_IF` / `IPV6_BOUND_IF`；
- stale local bind error 可失效generation，由下一次完整connector attempt重验；
- Windows普通transport继续使用原生name-to-index，不声称通用snapshot index已经验证；
- Linux现有 `SO_MARK` / `SO_BINDTODEVICE` 行为。

本 TODO 必须复用这些能力，禁止新建第二套快照缓存、Darwin index刷新周期或公共
`BindDev::Resolved`。

## Mihomo参考语义

实现前已核对本地 Mihomo source：

- `/Users/fanli/Documents/mihomo-rev/component/dialer/bind_darwin.go`
  - `bindControl`
  - `bindIfaceToDialer`
  - `bindIfaceToListenConfig`
  - 对 global-unicast socket 在首次I/O前设置 `IP_BOUND_IF` 或 `IPV6_BOUND_IF`。
- `/Users/fanli/Documents/mihomo-rev/component/dialer/bind_windows.go`
  - `bind4`
  - `bind6`
  - `bindControl`
  - `bindIfaceToDialer`
  - `bindIfaceToListenConfig`
  - 使用 `IP_UNICAST_IF` / `IPV6_UNICAST_IF`，并处理 wildcard `udp6` 到 IPv4 destination
    及接口IPv6不可用的兼容情况。
- `/Users/fanli/Documents/mihomo-rev/component/dialer/dialer.go`
  - `DialContext`
  - `ListenPacket`
  - 在实际dial/listen前通过显式interface或`DefaultInterfaceFinder`确定出口。
- `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go`
  - `cDialerInterfaceFinder::DefaultInterfaceName`
  - `cDialerInterfaceFinder::FindInterfaceName`
  - `DefaultInterfaceMonitor`随平台网络变化更新物理默认接口，并避免把policy TUN选为
    underlay。

EasyTier不要求复制Mihomo的Go dialer结构，但应保持可观察语义：出口绑定发生在首次I/O
之前，使用当前物理接口，并在网络变化后失效旧身份。

## 平台边界

| 平台 | 当前/候选机制 | 本 TODO 边界 |
|---|---|---|
| Linux | `SO_MARK`，必要时 `SO_BINDTODEVICE` | 保持现有mark和权限失败语义，防止跨平台重构造成回归 |
| Android | VPN owner保护/平台网络，部分原生接口枚举被禁用 | 不恢复pnet热路径，不新增SELinux packet-socket探测 |
| macOS桌面 | `IP_BOUND_IF` / `IPV6_BOUND_IF` | 主要实现和实机验收平台 |
| iOS/macOS NE | 平台VPN owner和现有移动端门禁 | 不把桌面枚举机制错误搬入extension |
| Windows | `IP_UNICAST_IF` / `IPV6_UNICAST_IF` | 主要实现平台；name-to-index必须原生解析或先由原生测试证明snapshot index等价 |
| FreeBSD/其他BSD | 当前逻辑 | 先保持编译和行为，不在无实机证据时声明完成 |
| OHOS/Fuchsia | 现有平台路径 | 不改变现有cfg边界 |

`bind_device=false` 在所有平台必须保留系统默认路由行为，不能暗中强制接口绑定。

## 第一批实现范围

连接风暴整改完成并取得独立证据后，本候选先覆盖 UDP hole-punch 中绕过普通transport
resolved-bind的生产socket。当前至少包括：

1. `UdpSocketArray::start()` 的 84/25 批量socket；
2. Cone客户端本地socket；
3. Sym-to-cone direct尝试socket；
4. `send_symmetric_hole_punch_packet()` 的发送socket；
5. 会发送打洞应答的hole-punch listener/socket。

测试中的loopback或固定端口裸bind不机械替换。

后续必须再审计所有绕过普通connector的 underlay socket，包括但不限于：

- manual/bootstrap connector；
- STUN和UPnP辅助socket；
- FakeTCP/raw socket；
- WebSocket特殊默认bind；
- QUIC/WireGuard endpoint；
- TCP hole punch；
- listener reply path。

只有经过审计和验证的路径才能标为净化完成，不能由UDP hole-punch修复外推“全局所有
socket均已绕行”。

## 最小实现方向

### 1. 从同一次preflight消费resolved target

对于已知remote endpoint的attempt：

1. 先完成现有 `prepare_hole_punch_attempt()`；
2. 从其 `UnderlayPreflightGuard::underlay_snapshot()` 选择对应地址族的
   `ResolvedBindAddr`；
3. 保留现有地址顺序和first-match语义；
4. unmapped fallback只阻断对应地址族；
5. 在RPC/preflight失败时不提前创建84个socket。

### 2. 一个pool解析一次

增加一个很薄的 crate-private hole-punch bind helper，内部复用 `bind_resolved()`：

```text
resolved target once
  -> create N sockets with the same address/interface identity
  -> apply platform option before first I/O
```

不得让84个socket分别调用 `NetworkInterface::show()`、`if_nametoindex()` 或Windows adapter
枚举。

macOS直接消费快照中已验证的name/index。Windows第一批继续保留原生name-to-index语义：
在pool准备阶段原生解析一次，然后让本pool复用；除非Windows原生自动测试先证明通用
snapshot index与 `IP_UNICAST_IF/IPV6_UNICAST_IF` 所需index完全等价，否则不直接消费。

### 3. 保持错误和generation语义

- 只有明确的本地socket bind/interface option失败才标记私有`LocalBindError`；
- DNS、握手、超时和普通connect错误不能伪装成stale interface；
- stale generation由下一次正常connector/hole-punch attempt完整重验；
- 一个真实transport attempt内不因stale bind自动重复握手；
- 网络事件发生在收集或发布期间时，旧快照不得覆盖新epoch；
- partial bind成功时保留成功socket，不因其他地址族或其他source失败而销毁。

## 与连接风暴整改的关系

两个候选必须分开验证：

1. **连接风暴规范化候选**
   - 仍允许Mihomo捕获必要underlay；
   - 用Mihomo session数辅助证明空闲campaign和shared pool是否减少；
   - 不把Mihomo仍能看到一次合法campaign误判为调度整改失败。
2. **路由净化候选**
   - 人为触发一次合法UDP hole-punch campaign；
   - EasyTier自身campaign和datagram计数应存在；
   - Mihomo中不应再出现对应84/25路EasyTier UDP五元组；
   - 物理接口必须观察到实际发送，hole-punch成功率和恢复时间不得下降。

这样可以区分“没有发包”和“发包但正确绕过TUN”，避免两个修复互相掩盖。

## 自动化测试范围

- [ ] 同一个preflight snapshot生成hole-punch resolved target，不发生第二次接口扫描。
- [ ] 84/25 socket pool复用同一个target和generation。
- [ ] macOS IPv4使用 `IP_BOUND_IF`，IPv6使用 `IPV6_BOUND_IF`。
- [ ] Windows IPv4使用 `IP_UNICAST_IF`，IPv6使用 `IPV6_UNICAST_IF`。
- [ ] Windows pool只执行一次原生name-to-index解析。
- [ ] Windows wildcard UDP6到IPv4 destination保持Mihomo参考兼容行为。
- [ ] Linux `socket_mark`和现有device binding无回归。
- [ ] `bind_device=false`不请求snapshot target且保持wildcard bind。
- [ ] IPv4/IPv6 unmapped fallback只阻断对应地址族。
- [ ] 重复地址保持first-match。
- [ ] preflight/RPC失败不创建批量socket。
- [ ] network invalidation后不复用旧generation pool。
- [ ] local bind stale error与DNS/connect/handshake error严格隔离。
- [ ] partial success保留成功socket。
- [ ] cancellation、owner drop和刷新期间invalidation不泄漏socket/task。
- [ ] FreeBSD和其他未启用平台保持现有编译门禁。

平台socket option应优先使用真实native测试；纯fixture测试只能证明选择和调用次数，不能
证明内核实际路由。

## 实机验收

### macOS

- 同一Mihomo TUN配置下，触发可重复的hard-symmetric campaign；
- EasyTier聚合计数证明84-socket和datagram实际执行；
- Mihomo controller中对应EasyTier UDP会话接近零；
- `nettop`/系统socket信息证明出口是预期物理接口而非Mihomo utun；
- IPv4、IPv6、Wi-Fi切换、接口generation失效和恢复通过；
- UDP P2P成功率、首连时间和失败回退无显著回归。

### Windows

- 在Mihomo/Wintun TUN启用时运行同等campaign；
- 对应EasyTier UDP不进入Wintun/Mihomo会话；
- 原生接口index解析次数为每pool一次；
- IPv4、IPv6、禁用接口IPv6、网络切换和恢复通过；
- 普通TCP/UDP/WS/WG transport既有bind路径无回归。

### Linux与BSD边界

- Linux现有 `SO_MARK` focused tests和真实policy-route路径继续通过；
- FreeBSD先要求编译及现有功能无回归，不声称已实现等价接口净化；
- 不用Linux成功外推macOS或Windows native socket option正确。

## 非目标

- 不动态修改Mihomo配置或维护per-endpoint系统排除路由。
- 不改变`transport_priority`、`lazy_p2p`或P2P task eligibility；这些属于前序TODO。
- 不以减少84/25、三连发或200ms掩盖路由问题。
- 不新增公共 `BindDev` variant、大型mock trait或第二套interface cache。
- 不声称一个UDP路径修复即代表所有underlay socket均已完成净化。
- 文档变更本身不触发profiling-beta或发布工作流。

## 相关记录

- [`macos_interface_enumeration_cpu_remediation.md`](macos_interface_enumeration_cpu_remediation.md)
  定义snapshot、generation、resolved bind和平台兼容边界。
- [`../known_bugs/kcp_bugs_and_mihomo_loopback.md`](../known_bugs/kcp_bugs_and_mihomo_loopback.md)
  记录generic connector进入policy TUN的既有防护。
- [`p2p_connection_storm_normalization.md`](p2p_connection_storm_normalization.md)
  是本候选的前置整改。
