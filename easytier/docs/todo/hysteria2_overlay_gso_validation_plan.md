# quinn-udp 0.5.15 与 quic-brutal Overlay 验证 TODO

> 状态：待实施、待验证
>
> 修订日期：2026-07-20
>
> 范围：Linux、Android；Android 设备恢复后补齐真机门槛

## 1. 决策与实施顺序

本工作只包含四个串行步骤：

1. **Q0：修复现有 Stealth QUIC GSO 最后短 segment。**
2. **Q：仅将 `quinn-udp` 从 `0.5.4` 升级到 `0.5.15`，证明现有 QUIC 和其他功能无回归。**
3. **H0：用锁定 Quinn 验证 Brutal-inspired controller 的 pacing 可行性。**
4. **H：增加 EasyTier 私有 `quic-brutal://` overlay，验证国内链路容量和资源效率。**

Q0/Q 未通过，不开始 H0。H0 未通过，不实现 H。H 只实现一个独立 overlay transport，不替代或改变现有 QUIC/KCP/native。

## 2. 协议边界

### 2.1 要实现的内容

`quic-brutal://` 是 EasyTier 节点之间的私有 mesh overlay：

- 复用现有 Quinn endpoint、EasyTier `Tunnel` framing、mesh 鉴权和节点管理。
- 新增 Hy2/Brutal 思路的拥塞控制；v1 在建连前分别配置每一端的静态发送速率。
- 复用 Quinn/quinn-udp 的 GSO/GRO、旧内核回退、IPv4/IPv6、socket bind/mark 和生命周期管理。
- 不改变现有 `quic://` 的配置、wire format、拥塞控制或优先级。
- 作为独立 scheme 正常接入现有 listener、connector、协议发布和 transport priority 机制；协议选择及排序继续由现有机制和用户配置决定。

### 2.2 明确不会实现

以下评审建议不符合 EasyTier mesh-only 目标，本候选明确不实现：

- 不实现标准 Hysteria2 HTTP/3 鉴权或 masquerade。
- 不实现标准 Hysteria2 TCPRequest/TCPResponse、UDP Session、QUIC Datagram fragmentation、TTL 或第三方代理语义。
- 不兼容官方 Hysteria2、Mihomo、sing-box 等第三方 Hy2 客户端/服务器。
- 不实现 Salamander、Gecko 或其他会增加 UDP wrapper/GSO 复杂度的混淆。
- 不使用官方定义的 `hysteria2://` 或 `hy2://` URI，避免暗示第三方线协议兼容；使用 `quic-brutal://`。
- 不新增一套代理节点、proxychain、路由或 mesh secret 到 Hy2 auth 的映射。
- 不改变 TUN MTU，不在应用层强行拼接或分片 overlay datagram。
- 不在 v1 实现握手后的带宽交换、运行时 controller 切换或 `0/auto` 自动带宽语义。
- 不为该 overlay 新增专用 failover、热备连接、健康评分、circuit breaker 或协议调度逻辑；连接成功或失败均按现有 transport 统一语义处理。

标准 Hysteria2 完整语义见官方协议规范：<https://v2.hysteria.network/docs/developers/Protocol/>。本地参考版本为 Mihomo HEAD `0a87b94845ef908c15f8495871e4cd8e33116328`、`sing-quic 68e10a6afdc3`：

- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/hysteria2.go::NewHysteria2`
- `/Users/fanli/Documents/mihomo-rev/listener/sing_hysteria2/server.go::New`

这些参考用于确认“标准 Hy2 范围不适合本候选”，不作为第三方互通目标。

## 3. 当前基线

- `easytier/Cargo.toml` 当前使用 `quinn 0.11.8`、`quinn-proto 0.11.12`；`Cargo.lock` 锁定 `quinn-udp 0.5.4`。
- `easytier/src/tunnel/quic.rs::transport_config` 当前配置为 BBR、8 MiB stream window、初始 MTU 1200、启用 segmentation offload。
- `QuicStealthSocket` 负责逐 GSO segment 加密及 GRO stride 解密。
- `quinn-udp 0.5.4` 在 Linux 支持 GSO/GRO，在 Android 不进入相同实现分支。
- `quinn-udp 0.5.15` 增加 Android GSO/GRO 条件分支和内核能力检查；这只是升级动机，不是正确性证据。
- `docs/known_bugs/stealth_secure_known_bugs.md` 和 `docs/release_notes/v2.6.10.md` 中的现有 QUIC 数据只作趋势参考；正式 A/B 必须记录精确 SHA、artifact、配置和路径。

## 4. Q0：Stealth GSO 最后短 segment

当前 `QuicStealthSocket::try_send` 使用：

```rust
let num_segments = contents.len() / seg_size;
```

UDP GSO 允许最后一个 segment 小于 `segment_size`。向下取整会遗漏该尾段，必须在依赖升级前建立明确基线。

- [ ] 增加聚焦单元测试，逐个比对解密后的原始 datagram：
  - [ ] `len == N * segment_size`
  - [ ] `len == N * segment_size + 1`
  - [ ] `len == N * segment_size + segment_size - 1`
- [ ] 使用向上取整和 `min(contents.len())` 处理最后一段。
- [ ] 断言加密 overhead 后的 `segment_size`、最后短段长度及接收 GRO stride 正确。
- [ ] Q0 与 Q 作为同一批实现进行远端预检，不为 Q0 单独触发 GitHub workflow。
- [ ] Q 的正式性能回归使用不受 Q0 影响的普通 QUIC 对比现有 `0.5.4` optimized artifact。
- [ ] Stealth QUIC 只要求 Q0 聚焦测试、完整功能验证和整体性能 sanity，不要求构造 `0.5.4 + Q0` optimized artifact。

明确不会实现：

- 不增加跨 SHA、双 checkout、双 optimized artifact 的专用 Q workflow。
- 不要求 Stealth 的 Q0 修复和依赖升级在性能归因上成为唯一变量。

## 5. Q：quinn-udp 0.5.4 → 0.5.15

### 5.1 依赖边界

- [ ] 从 `Cargo.lock` 记录 `quinn-udp 0.5.4` 的精确版本、校验和和依赖链。
- [ ] 检查 `0.5.15` 的精确 crate 源码和以下差异：
  - [ ] Linux/Android/其他平台 `cfg`。
  - [ ] `UDP_SEGMENT`、`UDP_GRO`、GSO/GRO 最大 segment 数。
  - [ ] `sendmsg`/`recvmsg`、ECN、源地址和 GRO stride。
  - [ ] `EIO`、`EINVAL`、socket option 失败和旧内核回退。
  - [ ] MSRV、libc/socket2 和 Android NDK 边界。
- [ ] 只更新允许范围内的 `quinn-udp` lockfile，保持 `quinn 0.11.8`、`quinn-proto 0.11.12` 和当前 QUIC 参数不变。
- [ ] 如果不能独立升级，停止 Q，另开依赖栈升级计划，不顺手扩大范围。

### 5.2 正确的降级语义

`quinn-udp 0.5.15` 在 GSO `sendmsg` 返回 `EIO/EINVAL` 时，将后续 `max_gso_segments` 降为 1；它不保证把当前 GSO batch 立即拆成多个 datagram 重发。

- [ ] 审计锁定的 `quinn-udp 0.5.15` 源码，确认 `EIO/EINVAL` 后后续 transmit 上限降为单 segment。
- [ ] 允许当前 UDP batch 失败，由 QUIC 丢包恢复和重传；验证连接保持可用、可靠应用数据最终完整。
- [ ] 在 `.37/.38` 验证真实无 GSO 环境使用单 segment，且无持续重试或日志风暴。
- [ ] 若测试环境自然出现 `EIO/EINVAL`，保留运行证据；不把人工稳定注入该内核错误作为候选门槛。

明确不会实现：

- 不 fork/patch `quinn-udp` 实现“当前 batch 立即拆包重发”。
- 不把上游没有承诺的即时重发语义加入候选 Q。
- 不为注入 `EIO/EINVAL` 修改上游 crate、系统调用层或生产 socket wrapper。

### 5.3 GSO/GRO 观测

保持纯上游依赖，不增加生产数据面观测逻辑：

- [ ] 测试中记录 `max_transmit_segments()`/GRO stride 及降级前后变化。
- [ ] Linux 使用短时 `strace`、eBPF 或等价外部手段确认 `UDP_SEGMENT`、segment 数和 `sendmsg`。
- [ ] 性能 A/B 时关闭 tracing，只记录吞吐、CPU/Gbit、系统调用/Gbit、RSS、FD 和 task。
- [ ] Android 真机恢复后使用独立测试证据确认能力，不因内核版本或配置开关直接推断 GSO 已生效。

明确不会实现：

- 不 fork `quinn-udp` 暴露内部错误计数器。
- 不增加生产逐包计数、逐包日志、定时轮询或新的常驻 task。
- 不要求 EasyTier 精确报告每次底层 `sendmsg` 的失败原因。

### 5.4 现有功能回归

在 `192.168.2.160` 按 Remote Cargo Golden Pattern 执行最小 `--locked` no-run 和精确测试；不得在维护者本机编译。

- [ ] 运行以下现有测试：
  - `quic_bind_mode_requires_matching_address_family`
  - `strict_server_bind_validation_rejects_wrong_family_and_port`
  - `quic_pingpong`
  - `quic_stealth_pingpong`
  - `quic_unknown_capability_falls_back_to_plain`
  - `quic_stealth_listener_rejects_plain_and_wrong_secret`
  - `quic_stealth_session_transitions_from_gate_to_outer_key`
  - `quic_stealth_socket_does_not_replace_live_phase2_session`
  - `ipv6_pingpong`
  - `ipv6_domain_pingpong`
  - `listener_drop_removes_persistent_endpoint`
  - `connect_removes_stopped_endpoints_and_retries`
  - `invalid_peer_addr`
  - `quic_stealth_three_node_carries_phase2_tcp`
  - `quic_proxy`
- [ ] 运行 listener、端口索引、双栈 companion、transport priority、协议发布和网络恢复测试。
- [ ] 新增 Q0 尾段、GRO stride 和 Android `cfg` 测试；降级语义采用锁定源码审计加真实旧内核证据。
- [ ] 验证普通/Stealth QUIC、IPv4/IPv6、QUIC Proxy、listener 生命周期和混合版本互通。

### 5.5 Q 验收

| 主机 | 角色 | 必须收集的证据 |
|---|---|---|
| `192.168.2.160` | 旧内核构建机 | `--locked` 编译、focused tests；不作 WAN 结论 |
| `192.168.1.37` | 旧内核运行时 | 无 GSO 功能、资源、稳定性 |
| `192.168.1.38` | 旧内核运行时 | 无 GSO 功能、资源、稳定性 |
| `lv1g2.lovis.us` | 新内核性能端 | 实际 GSO/GRO、IPv4/IPv6、吞吐与资源 |
| `lv1g3.lovis.us` | 新内核性能端 | 实际 GSO/GRO、IPv4/IPv6、吞吐与资源 |

- [ ] 所有现有和新增回归测试通过。
- [ ] 普通/Stealth QUIC 和混合版本互通语义不变。
- [ ] `.37/.38` 自动使用单 segment，无崩溃、连接/重试风暴或资源增长。
- [ ] `lv1g2/lv1g3` 取得实际 UDP segmentation/GRO 证据，而不只是配置开启。
- [ ] 普通 QUIC 相对现有 `0.5.4` optimized artifact，吞吐中位数不低于 95%，CPU/Gbit 不恶化超过 10%。
- [ ] Stealth QUIC 完成 Q0 正确性、端到端功能和整体吞吐/资源 sanity；发现超过 10% 的异常下降必须诊断，但不要求依赖版本的纯净性能归因。
- [ ] 空闲 CPU、RSS、FD、task 无持续增长，连接关闭和网络恢复后回到基线。
- [ ] Android 编译边界通过；真机 GSO 和耗电证据标记 pending。
- [ ] 任一功能异常、数据边界错误、首包异常或降级循环都回退 Q，不进入 H。

## 6. H0：锁定 Quinn 的 Brutal-inspired 可行性

当前 `quinn-proto 0.11.12` 的公开 `Controller` 只能返回 congestion window；内部 `Pacer` 根据 `window / RTT × 1.25` 推导速率，不能像 sing-quic Brutal 一样独立控制 pacing rate 和约 `2 × BDP / ack_rate` 的 cwnd。

参考位置：

- `quinn-proto-0.11.12/src/congestion.rs::Controller`
- `quinn-proto-0.11.12/src/connection/pacing.rs::Pacer`
- `sing-quic/hysteria2/congestion/brutal.go::BrutalSender`

H0 只做最小可行性验证：

- [ ] controller 在建连前由专用 `TransportConfig` 创建，使用静态 `tx_bps`，不依赖 mesh handshake 后切换 controller。
- [ ] 写明 `target_rate`、RTT、ACK rate、返回 window 和 Quinn 推导 pacing rate 的数学合同及偏差。
- [ ] 用单元测试验证不同 RTT、丢包率、MTU 和极端配置不会溢出、返回零 window 或无界 window。
- [ ] 在 `.160` 完成最小 `--locked` no-run 和 focused tests。
- [ ] 可用 debug musl 原型在 `lv1g2 ↔ lv1g3` 做诊断性速率/GSO 验证；该结果只判断明显可行或不可行，不代替 H 的 profiling artifact 性能结论。
- [ ] 目标速率、实际 goodput、pacing burst、丢包、CPU/Gbit 和 GSO batch 均可接受时才进入 H。

H0 明确不会实现：

- 不声称精确复刻 sing-quic Brutal。
- 不使用 mesh handshake 后的动态 controller 切换或共享原子状态间接修改 controller。
- 不为 H0 fork/patch/升级 Quinn；当前公开接口不可行时直接停止 H，另开独立依赖计划。
- 不为 H0 单独触发 profiling-beta workflow。

## 7. H：quic-brutal Overlay

### 7.1 最小实现

- [ ] 新增独立的 `quic-brutal://` scheme、`IpScheme` 和 `TunnelInfo.tunnel_type`，接入现有 listener/connector factory、协议发布和 transport priority 解析；不引入 overlay 专用选择或降级逻辑。
- [ ] 复用现有 QUIC endpoint、Tunnel framing、mesh 鉴权、连接管理和错误传播路径，不复制一套 QUIC transport 实现。
- [ ] 只新增已经通过 H0 的 Brutal-inspired controller；每端在建连前独立配置本端静态 `tx_bps`。
- [ ] `tx_bps` 单位为 bit/s，允许范围为 `1_000_000..=100_000_000_000`；使用 checked arithmetic 转换，缺失、0、`auto`、畸形或越界值均在启动/建连前明确报错，不静默 clamp。
- [ ] 完成端口索引、IPv4/IPv6 companion、socket bind/mark、生命周期和 endpoint 清理，行为遵循其他现有 IP transport 的统一约定。
- [ ] 复用现有 endpoint pool；客户端必须使用 `Endpoint::connect_with` 传入专用 Brutal client config，普通 QUIC 继续使用 BBR 默认 config。
- [ ] 增加同一 endpoint 上 BBR/Brutal 连接配置不串用的测试；不为此建立第二套 endpoint pool。
- [ ] 复用现有 mesh 鉴权；错误 secret、错误握手和畸形 overlay frame 必须被拒绝且资源有界。
- [ ] listener、connector 或握手失败时返回现有调用方可识别的普通 transport 错误，并完整释放本次连接资源；后续选择由现有 connector 和用户配置决定。
- [ ] 增加聚焦测试，证明未配置 `quic-brutal` 时普通 QUIC 行为不变、失败连接不污染共享 endpoint、关闭后无 Brutal 专属资源残留。

实现边界：本候选只负责让 `quic-brutal` 成为安全、准确、高效且可独立启用的 overlay transport。用户是否配置该协议、放在什么优先级以及后续候选协议是什么，不属于本候选的协议实现范围。

不会实现新的“Hy2 framing vs QUIC framing”对照，因为 H 不引入标准 Hy2 framing。需要隔离的变量只有：

1. 当前 `quic://` + BBR。
2. `quic-brutal://` + Brutal。
3. 新内核上 `quic-brutal` GSO on/off。

### 7.2 GSO 与旧内核

- [ ] `lv1g2 ↔ lv1g3` 双向证明真实 GSO/GRO，GSO on 时发送系统调用/Gbit 必须明显低于 GSO off。
- [ ] `lv1g* → .37/.38` 验证新内核发送 GSO、旧内核正常接收。
- [ ] `.37/.38 → lv1g*` 不要求 GSO 收益；发送端是旧内核，重点验证 Brutal 容量、CPU 和稳定性。
- [ ] `.37/.38` 自动退化为单 segment，不改变 MTU、不应用层分片、无失败循环。
- [ ] 若新内核无法稳定启用真实 GSO，H 失败，不以“仍能通信”替代 GSO 门槛。

### 7.3 国内容量

四条出向路径全部必测：

- [ ] `.37 → lv1g2`
- [ ] `.37 → lv1g3`
- [ ] `.38 → lv1g2`
- [ ] `.38 → lv1g3`

测试方法保持工程化而不过度统计：

- [ ] 同一 SHA、相同配置和业务 payload，对比当前 QUIC 与 quic-brutal。
- [ ] 使用 1、4、16 条 TCP 流；每轮预热 10 秒、采样至少 30 秒。
- [ ] 每条路径做 5 个配对样本，按 AB/BA 交替顺序，记录每对提升比例。
- [ ] 记录中位数、最小值、最大值，不用 5 个样本计算 p10/p90。
- [ ] 容量通过条件：中位数提升至少 20%，且至少 4/5 个配对样本为正。
- [ ] 同 offered load 下 p99 RTT 不超过当前 QUIC 的 2 倍，持续丢包率不高于当前 QUIC 1 个百分点以上。
- [ ] 国内出向的提升归因于拥塞控制，不归因于发送端 GSO。

明确不会实现：

- 不要求 10 个 paired blocks、统计显著性检验或置信区间下界门槛。
- 不从 5 个独立轮次报告 p10/p90。
- 不用单次峰值吞吐代替可持续容量。

### 7.4 资源与稳定性

- [ ] 未配置 H 时不创建额外 socket、task、timer、线程、连接池或周期唤醒。
- [ ] 启用但空闲 30 分钟，无空转循环，RSS/FD/task 不持续增长。
- [ ] 活跃时用相同 delivered throughput 比较 CPU/Gbit、系统调用/Gbit、RSS、FD、task 和 p99 event-loop latency。
- [ ] 运行 1/4/16 条吞吐流和 1000 条应用 TCP 连接经同一 overlay。
- [ ] 同吞吐下 CPU/Gbit 和稳态 RSS 不得出现超过 5% 测量噪声的持续恶化。
- [ ] 连接关闭、listener 重启和网络恢复后，FD/task/RSS 回落，不随轮次单调增长。
- [ ] 完成至少 2 小时负载和 24 小时空闲验证。

### 7.5 简化的受控损伤测试

`lv1g2 ↔ lv1g3` 只做代表性场景，不展开完整排列组合：

- [ ] 无 netem 的新内核上限基线。
- [ ] RTT 100 ms、丢包 1%。
- [ ] RTT 180 ms、丢包 3%。
- [ ] 每个场景比较 QUIC/BBR、quic-brutal/GSO on、quic-brutal/GSO off。

临时 artifact、日志和测试数据存放在 `/slab2`。测试前检查 namespace CIDR、显式端口、残留进程和 TUN，避免影响生产实例。

## 8. Android 后续门槛

Android 设备不可用时可以完成 Linux 决策，但 H 只能标记为 Linux experimental：

- [ ] 使用同 SHA workflow artifact 保留数据升级安装。
- [ ] 验证 Android 内核 GSO/GRO 实际能力及不支持时的单 segment 回退。
- [ ] 覆盖 Wi-Fi/蜂窝切换、断网恢复、VPN stop/start、前后台和 IPv4/IPv6。
- [ ] 比较 QUIC/BBR 与 quic-brutal 的吞吐、CPU time、RSS、FD、task、wakeup 和耗电代理指标。
- [ ] 真机门槛完成前仅记录 Android 支持状态，不在本候选中调整用户协议排序策略。

## 9. Candidate manifest 与最终决策

每个候选 push 前更新 validation journal；只有同一候选还包含 Leaf/policy 代码时，才同步更新 `docs/todo/leaf_parallel_workboard.md`。记录：

- 精确 SHA/snapshot、包含的函数和测试。
- `.160` `--locked` no-run 与 focused test 结果。
- Cargo.lock、平台 `cfg`、workflow pins、生成文件和完整 diff 审计。
- 自动 Linux/Android workflow、exact artifact SHA。
- `.37/.38` 旧内核、`lv1g2/lv1g3` 新内核和 Android pending 证据。
- 构建等待期间的 diff 复核、主机清理、测试脚本和基线准备任务。

最终结果分别记录，允许 Q 已接受而 H 被拒绝：

- `Q = accept | revert`
- `H = not-started | accept | experimental | reject`
- `Android = pending | pass | fail`

其中 `H=accept` 要求四条国内路径均通过容量门槛、新内核真实 GSO 生效、旧内核和资源门槛通过；`H=experimental` 只表示验证状态，不在本候选中附带额外协议调度语义。

文档变更保持本地，不单独触发 GitHub workflow。构建候选失败时使用 `git revert`，不得用 destructive reset 隐藏失败快照。
