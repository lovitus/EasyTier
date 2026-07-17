# Leaf v1 过度设计、性能回退与解耦审计

日期：2026-07-16

精确候选：`5d71abed66a1ad1957834a40bc67b0d0092a95af`

本报告只审计已经冻结的精确候选和对应实机制品，不把后续本地文档注释视为新的运行候选。审计硬边界为：

- Leaf 只负责规则、DNS、outbound 和其自身连接级 chain/fallback；
- policy adapter 只负责配置编译、loopback/mesh bridge、跨 peer 准备和生命周期；
- mesh 自己负责 route、overlay、QUIC/KCP/smoltcp、能力协商和 transport health。

## 结论

没有发现阻塞 Leaf v1 的架构级过度设计或 disabled-mode 性能回退。当前新增 mesh 接口均可归类为必要的通用 data-plane primitive 或隔离在 `policy_proxy` 下的 adapter，没有 Leaf 配置、规则、DNS、group 或 runtime 类型进入 mesh route/overlay/transport selector。

未来 Windows 和 macOS 不需要修改 EasyTier overlay、KCP、QUIC 或 peer route 协议即可接入，但不能据此宣称当前已经支持：

- 普通 macOS 进程模式可复用 Unix Leaf process/packet host；
- macOS Network Extension 需要自己的 packet/routing host；
- Windows 需要替换当前 Unix datagram/raw-FD packet bridge，并增加 Windows routing/loop-prevention 与子进程 ownership；
- 这些都是 host adapter 工作，不是 policy 接管 mesh 或重写协议。

保留一个 P3 技术债：`DeferredProxySelector::connect_mesh_stream()` 与 SYN deferred path 尚未共享同一个内部 selector primitive。并发 route change 时，direct-stream path 可能先给旧 peer/transport 记一次失败再观察 route 变化。成功 stream 返回前会重新校验 route，health 又按 peer/transport 隔离，因此不会把流量送错 peer、不会降级新 peer，也不会 DIRECT 泄漏。除非实测出现显著恢复延迟，否则不应在 v1 候选上重构两套状态机。

## 需求与证据

| 门槛 | 证据 | 结论 |
| --- | --- | --- |
| disabled-mode CPU | feature/no-Leaf 同 SHA，72/73 秒负载 source ticks `3410/3403`、target `5222/5212` | 无可测回退 |
| disabled-mode 吞吐 | 3x10 秒 bracket，feature 约 `1592 Mbit/s`，no-Leaf 约 `1587 Mbit/s` | 差异在噪声内 |
| disabled-mode 延迟 | 100 ping，feature `0.303 ms`，no-Leaf `0.309 ms` | 无回退 |
| disabled-mode RSS/FD/线程 | idle RSS约 16 MiB，FD均 `26`，线程稳定 `9`，瞬态 `10` | 无持续增长 |
| explicit/portless 一致性 | 两者均为同一 peer、`10.255.0.2:11080`、`kind=Kcp`；重复吞吐差约 2.4% | 同一 mesh selector/data plane |
| transport ownership | QUIC-only、KCP-only、QUIC优先、native smoltcp均由精确制品观察 | mesh决定 QUIC/KCP/native |
| fallback + UDP叠加 | healthy chain、停止 peer-local SOCKS 后 mesh-direct fallback、同时 UDP `300/300` | 连接级 fallback正确 |
| 网络切换 + UDP | 400 datagram中停机窗口 23 次超时，恢复后同一应用 socket继续成功 377 次；物理泄漏计数 `0/0` | fail-closed且可恢复 |
| UDP资源回收 | 10并发 association 活跃 FD source `57/36`、target `88/48`；回收后 `37/16`、`38/18` | 无任务/FD/线程泄漏 |
| Android Wi-Fi/DNS | 同一设备侧 detached script完成 disable/enable；PID和 VPN不变，underlay key更新，DNS恢复，mesh和 TLS恢复 | 平台恢复通过 |
| Android资源回收 | 隔离 cycle FD `347 -> 335 -> 323`，回到冷启动 `323`；线程 `68 -> 67`，RSS低于基线 | bounded延迟回收，不是泄漏 |

## 所有权审计

### Leaf

`easytier-policy` 不依赖 EasyTier crate。Leaf依赖仅在 Unix target和显式 `leaf-runtime` feature下启用。它负责：

- 严格 policy schema、first-match规则、GeoX资源和完整性检查；
- 将 native/mesh SOCKS actor和 chain/fallback编译为 Leaf outbound；
- direct/proxy DNS集合和 `domainResolve: false`；
- TUN packet FD对应的 Leaf runtime启动与关闭。

`MeshServerResolver` 是反向依赖接口：policy compiler只请求“这个 mesh actor对应的本地 SOCKS endpoint”，不知道 peer manager、route、KCP、QUIC或smoltcp。编译结果只包含带临时凭据的 `127.0.0.1:ephemeral`，不会把 mesh virtual IP直接交给 Leaf。

### policy adapter

`MeshProxyBridgeSet` 位于 `easytier/src/policy_proxy/mesh_socks_bridge.rs`。它承担 Leaf SOCKS outbound与 EasyTier mesh data plane之间不可避免的协议适配：

- 每个 mesh actor只创建一个loopback listener和临时随机凭据；
- actor generation变化会 cancel旧 session并替换 remote snapshot；
- TCP调用 `data_plane_tcp_connect_mesh_only()`；
- UDP通过有容量、per-peer上限、token、idle timeout和 cancellation ownership的 relay/UoT association；
- shutdown先 disable全部 remote，再关闭 Leaf runtime，最后由 Drop取消 listener/session。

这里实现窄 SOCKS ingress而不是修改 Leaf自定义 outbound，是有意解耦：固定 Leaf版本可独立升级/回退，EasyTier不接管 Leaf chain/fallback状态机。

### mesh

`data_plane_tcp_connect_mesh_only()` 是通用 mesh data-plane primitive。调用者只提供 mesh destination；内部 selector依据用户 `--enable-quic-proxy`、`--enable-kcp-proxy`、目标 capability和 health选择 QUIC、KCP或native smoltcp。policy adapter不能指定 transport。

`DeferredProxySelector::connect_mesh_stream()` 不引用 Leaf或policy类型。它复用已有 prepare ACK、capability、health和route revalidation，并在没有 accelerator时返回 `None`，由mesh原生数据面fallback。

`PolicyUdpRelayRpc` 是隔离的跨 peer adapter surface，不改变 generic RPC transport。它只负责：

- portless built-in HEV最终endpoint准备；
- UDP/UoT association open/close；
- caller peer、目标virtual IP、凭据和容量校验；
- 返回 data-plane endpoint/token，而不是要求某个 overlay transport。

service名称同时包含TCP prepare是命名上的P3技术债，但为保持wire compatibility不应在v1重命名。

## 被否决的过度设计

以下方案不应加入v1：

- 由policy根据actor自行创建、注册或选择KCP/QUIC endpoint；
- 为三个保留smoltcp ingress端口与HEV kernel端口计算动态交集；
- SOCKS UDP payload丢失后猜测链路失败并自动迁移到fallback成员；
- 修改正常mesh封装、route或overlay来识别“policy流量”；
- Windows、Android、Linux分别维护不同SOCKS协议状态机；
- 为了消除P3 route/health顺序差异，在精确候选验证期间重写selector状态机。

## 性能边界

同条件已观察：

- DIRECT约 `941 Mbit/s`；
- policy KCP历史约 `478 Mbit/s`，本轮精确制品约 `75-91 MB/s`；
- policy QUIC约 `75 MB/s`；
- native smoltcp约 `6.95 MB/s`，即约 `55.6 Mbit/s`。

因此保留mesh拥有的QUIC/KCP fast path是必要性能设计。应移除的是policy对transport的所有权，不是mesh accelerator本身。disabled-mode不启动Leaf或HEV业务runtime，A/B结果也没有显示常驻性能或资源代价。

## 平台矩阵

| 平台 | 当前host | 未来所需工作 | 是否影响mesh协议 |
| --- | --- | --- | --- |
| Linux | Unix packet bridge + Leaf/HEV process | v1已验证 | 否 |
| Android | VpnService TUN + in-process Leaf/HEV | v1已验证；实际第三方SOCKS UDP仍由服务端负责 | 否 |
| macOS普通进程 | Unix process/packet结构可复用 | 实机路由、DNS、lifecycle验证 | 否 |
| macOS Network Extension | 尚无policy host | NE packet flow、route/DNS ownership | 否 |
| Windows | 尚无policy host | packet endpoint抽象、Wintun/route、loop prevention、Job Object | 否 |
| OHOS/iOS等mobile | 当前显式不可用 | 各平台VPN packet host和in-process runtime接线 | 否 |

当前具体类型 `LeafPacketBridge` 仍在 Unix-gated `virtual_nic` 中，未来实现Windows/NE时应提取一个最小 packet endpoint/bridge trait。这是局部host重构，不需要把platform cfg带入policy schema、Leaf config或mesh selector。

## Android实际SOCKS UDP边界

当前用户配置的显式actor为 `10.44.0.8:24443` 且声明 `udp: true`。通过VPN捕获的Jelly UID `10142`分别向两个公开STUN endpoint发起WebRTC ICE，只获得 `10.44.0.88` 和 `fd00::1` host candidate，没有server-reflexive candidate。

准确结论仅为：该实际SOCKS路径没有形成可观察UDP roundtrip。它不证明EasyTier托管HEV UDP错误，也不应触发自动fallback。`udp: true`是用户能力声明，不是主动探测；用户此前已经确认这种情况下UDP失败即可，必要时应选择实际支持UDP的mesh peer出口。

## 发布判断

本报告覆盖的五项门槛均已取得直接证据。没有发现需要推翻Leaf/mesh边界或重新构建候选的阻塞项。v1仍只声明Linux和Android；Windows/macOS结论是“架构不绑定、host尚未实现/验证”，不能写成平台支持声明。

详细逐次证据保留在：

- `docs/todo/leaf_validation_journal.md`
- `docs/todo/leaf_parallel_workboard.md`
- `docs/leaf_v1_function_performance_report_cn.md`

