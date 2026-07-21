# quinn-udp 0.5.15 与 quic-brutal Overlay 验证 TODO

> 状态：首个 immutable artifact 与 Linux 早期 A/B 已完成；GUI/可选速率修订、Q 回归、Android 与资源验证进行中
>
> 修订日期：2026-07-21
>
> 范围：Linux、Android；Android 使用修订后的同 SHA artifact 补齐真机门槛

> 代码基线：维护者 fork 的 `fc03500806986acfd566060b91d6a33a07120ce3`；本候选不合并或追踪 upstream 的后续提交。

`v3.0.1` 到 `fc035008` 之间没有 EasyTier 产品 Rust、Cargo 或 native 代码变更；差异仅为 workflow、发布审计脚本、维护说明和发布文档。因此本候选以 `fc035008` 作为维护者 fork 的最新开发基线。

## 1. 决策与实施顺序

本工作只包含四个串行步骤：

1. **Q0：修复现有 Stealth QUIC GSO 最后短 segment。**
2. **Q：仅将 `quinn-udp` 从 `0.5.4` 升级到 `0.5.15`，证明现有 QUIC 和其他功能无回归。**
3. **H0：用锁定 Quinn 验证 Brutal-inspired controller 的 pacing 可行性。**
4. **H：增加 EasyTier 私有 `quic-brutal://` overlay，验证国内链路容量和资源效率。**

Q0/Q 未通过，不开始 H0。H0 未通过或早期性能没有稳定、明显提升，不实现 H。H 只实现一个独立 overlay transport，不替代或改变现有 QUIC/KCP/native。

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
- 普通 `quic://` 的 BBR config、默认 endpoint client config 和连接路径是不可改变的基线；Q/H 任一阶段出现功能、稳定性、资源或不可接受的性能回归都必须回退，不得以 Brutal 收益抵消普通 QUIC 回归。

## 4. Q0：Stealth GSO 最后短 segment

当前 `QuicStealthSocket::try_send` 使用：

```rust
let num_segments = contents.len() / seg_size;
```

UDP GSO 允许最后一个 segment 小于 `segment_size`。向下取整会遗漏该尾段，必须在依赖升级前建立明确基线。

- [x] 增加聚焦单元测试，逐个比对解密后的原始 datagram：
  - [x] `len == N * segment_size`
  - [x] `len == N * segment_size + 1`
  - [x] `len == N * segment_size + segment_size - 1`
- [x] 使用向上取整和 `min(contents.len())` 处理最后一段。
- [x] 断言加密 overhead 后的 `segment_size`、最后短段长度及接收 GRO stride 正确。
- [x] Q0 与 Q 作为同一批实现进行远端预检，不为 Q0 单独触发 GitHub workflow。
- [ ] Q 的正式性能回归使用不受 Q0 影响的普通 QUIC 对比现有 `0.5.4` optimized artifact。
- [ ] Stealth QUIC 只要求 Q0 聚焦测试、完整功能验证和整体性能 sanity，不要求构造 `0.5.4 + Q0` optimized artifact。

明确不会实现：

- 不增加跨 SHA、双 checkout、双 optimized artifact 的专用 Q workflow。
- 不要求 Stealth 的 Q0 修复和依赖升级在性能归因上成为唯一变量。

## 5. Q：quinn-udp 0.5.4 → 0.5.15

### 5.1 依赖边界

- [x] 从 `Cargo.lock` 记录 `quinn-udp 0.5.4` 的精确版本、校验和和依赖链。
- [x] 检查 `0.5.15` 的精确 crate 源码和以下差异：
  - [x] Linux/Android/其他平台 `cfg`。
  - [x] `UDP_SEGMENT`、`UDP_GRO`、GSO/GRO 最大 segment 数。
  - [x] `sendmsg`/`recvmsg`、ECN、源地址和 GRO stride。
  - [x] `EIO`、`EINVAL`、socket option 失败和旧内核回退。
  - [x] MSRV、libc/socket2 和 Android NDK 边界。
- [x] 只更新允许范围内的 `quinn-udp` lockfile，保持 `quinn 0.11.8`、`quinn-proto 0.11.12` 和当前 QUIC 参数不变。
- [x] 如果不能独立升级，停止 Q，另开依赖栈升级计划，不顺手扩大范围；本次可独立升级，无需扩大范围。

### 5.2 正确的降级语义

`quinn-udp 0.5.15` 在 GSO `sendmsg` 返回 `EIO/EINVAL` 时，将后续 `max_gso_segments` 降为 1；它不保证把当前 GSO batch 立即拆成多个 datagram 重发。

- [x] 审计锁定的 `quinn-udp 0.5.15` 源码，确认 `EIO/EINVAL` 后后续 transmit 上限降为单 segment。
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
| `lv1g2` | 新内核性能端 | 实际 GSO/GRO、IPv4/IPv6、吞吐与资源 |
| `lv1g3` | 新内核性能端 | 实际 GSO/GRO、IPv4/IPv6、吞吐与资源 |

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
- [ ] 在至少一条可重复的内网或受控云链路上与普通 QUIC 做早期 A/B；中位吞吐提升不足 20%、提升不能重复，或 CPU/Gbit/丢包代价不可接受时，立即停止 H。
- [ ] 内网到云路径如受跨境干扰而无法得到稳定配对样本，只记录为环境受扰，不据此强行继续；改为分别验证内网侧功能/资源和云侧受控性能。
- [ ] 目标速率、实际 goodput、pacing burst、丢包、CPU/Gbit 和 GSO batch 均可接受且达到早期收益门槛时才进入 H。

H0 明确不会实现：

- 不声称精确复刻 sing-quic Brutal。
- 不使用 mesh handshake 后的动态 controller 切换或共享原子状态间接修改 controller。
- 不为 H0 fork/patch/升级 Quinn；当前公开接口不可行时直接停止 H，另开独立依赖计划。
- 不为 H0 单独触发 profiling-beta workflow。
- 不因已经投入开发时间而降低收益门槛；效果有限时不得声称 H 有普遍性能优势或把它加入默认 listener/priority。若实现已保持隔离且维护者明确要求，可保留为显式 opt-in transport，继续以普通 QUIC 零回归作为硬门槛。

## 7. H：quic-brutal Overlay

### 7.1 最小实现

- [x] 新增独立的 `quic-brutal://` scheme、`IpScheme` 和 `TunnelInfo.tunnel_type`，接入现有 listener/connector factory、协议发布和 transport priority 解析；不引入 overlay 专用选择或降级逻辑。
- [x] 复用现有 QUIC endpoint、Tunnel framing、mesh 鉴权、连接管理和错误传播路径，不复制一套 QUIC transport 实现。
- [x] 只新增已经通过 H0 的 Brutal-inspired controller；每端在建连前独立配置本端静态 `tx_bps`。
- [x] `tx_bps` 单位为 bit/s，显式值允许范围为 `1_000_000..=100_000_000_000`；使用 checked arithmetic 转换，0、`auto`、畸形、重复、未知或越界值均在启动/建连前明确报错，不静默 clamp。省略时该发送方向复用普通 BBR，不创建 Brutal controller，也不声称已自动估速。
- [x] 完成端口索引、IPv4/IPv6 companion、socket bind/mark、生命周期和 endpoint 清理，行为遵循其他现有 IP transport 的统一约定。
- [x] 复用现有 endpoint pool；客户端必须使用 `Endpoint::connect_with` 传入专用 Brutal client config，普通 QUIC 继续使用 BBR 默认 config。
- [x] 增加普通 QUIC config 快照测试，以及同一 endpoint 上 BBR/Brutal 连接配置不串用的测试；不为此建立第二套 endpoint pool。
- [ ] 复用现有 mesh 鉴权；错误 secret、错误握手和畸形 overlay frame 必须被拒绝且资源有界。
- [ ] listener、connector 或握手失败时返回现有调用方可识别的普通 transport 错误，并完整释放本次连接资源；后续选择由现有 connector 和用户配置决定。
- [ ] 增加聚焦测试，证明未配置 `quic-brutal` 时普通 QUIC 行为不变、失败连接不污染共享 endpoint、关闭后无 Brutal 专属资源残留。
- [ ] `tx_bps` 是本地发送参数：listener 与 initial peer 分别只控制其所在节点的发送方向；listener 发布为 direct candidate 时必须剥离该 query，不能把远端发送速率复制成本地速率。
- [ ] GUI 选择 `quic-brutal` 时使用现有 `+3` 预配置端口 `11013`；带宽字段可留空并明确显示“使用 BBR”，不得用 100 Mbps 等统一静态默认值限制 10 Gbps 节点。

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

Android 设备已重新接入；H 已按“隔离良好的显式 opt-in experimental transport”保留。只使用包含 GUI、可选速率和 direct-candidate 参数隔离修订的同 SHA workflow artifact 开始真机验证：

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

## 10. 2026-07-21 pre-build candidate manifest

### 10.1 Intended build snapshot

- 基线：`fc03500806986acfd566060b91d6a33a07120ce3`。
- 依赖：只把 lockfile 中 `quinn-udp 0.5.4`（checksum `8bffec3605b73c6f1754535084a85229fa8a30f86014e6c81aeec4abb68b0285`）更新为 `0.5.15`（checksum `35a133f956daabe89a61a685c2649f13d82d5aa4bd5d12d1277e1072a21c0694`）；其直接 lockfile 依赖改为既有的 `cfg_aliases`、`socket2 0.6.3` 和 `windows-sys 0.59.0`，没有更新 `quinn 0.11.8` 或 `quinn-proto 0.11.12`。
- Q0：`QuicStealthSocket::seal_gso_segments` 正确处理最后短 segment 和零 segment size。
- H：新增独立 `quic_brutal.rs` controller；`quic.rs` 只抽取共享 transport 参数，标准 client/server config 仍显式使用 BBR，Brutal client 使用同一 endpoint 的 `connect_with`。
- 入口：独立 `quic-brutal://` scheme、listener/connector factory、CLI 显式 URL、协议发布和现有 transport priority 解析；不增加协议专用调度、后台 task、timer、socket pool 或自动 listener。
- 速率语义：显式 `tx_bps` 只控制本地发送方向；省略时该方向使用 BBR。listener 发布为 direct candidate 时剥离本地 `tx_bps`，避免非对称链路把远端速率复制成本地速率；无法携带本地参数的 SRV 自动发现仍不发布 Brutal。
- GUI：选择 `quic-brutal` 使用与后端 offset 一致的 `11013`；`tx_bps` 可留空并显示 BBR fallback，不使用会限制 10 Gbps 网络的统一静态默认值。
- 隔离修正：QUIC/Brutal 的双栈冲突按物理 UDP port 而不是 scheme 判断。

### 10.2 Locked source audit

- 精确检查本机 crates.io source 中 `quinn-udp-0.5.4` 与 `quinn-udp-0.5.15`。`0.5.15/src/unix.rs` 在 Linux/Android 上尝试 `UDP_GRO`，GSO 同时检查内核 `>= 4.18` 和临时 socket 的 `UDP_SEGMENT` option，成功时上限为 64 segments，失败时为 1。
- `sendmsg` 的 `EIO/EINVAL` 会把共享 `max_gso_segments` 降为 1；`EINVAL` 还会切换 cmsg fallback 并重试。`EIO` 不承诺当前 batch 立即拆包重发，可靠性仍由 QUIC 恢复。
- Android 使用数值 `UDP_SEGMENT=103`、`UDP_GRO=104` 并进入相同能力检查；Linux 3.10 因版本门槛直接使用单 segment。最终 Android 能力仍必须以 workflow artifact 真机证据确认。
- 精确检查 `quinn-proto-0.11.12/src/congestion.rs` 与 `connection/pacing.rs`：公开 controller 只能返回 window，Quinn pacer 以 `1.25 * window / RTT` refill。当前 controller 返回一个经 ACK rate 补偿的 BDP，并由 in-flight window 约束长期发送量；这只是可测的近似实现，是否保留由有效 A/B 决定。

### 10.3 `.160` pre-build evidence

- 完整工作快照经 Rust 1.95、edition 2024 `rustfmt` 后同步到 builder；所有 Cargo 调用前均确认无其他 cargo/rustc，使用全核、`--locked`、timeout、独立日志和反向代理。
- `cargo test --locked --no-run --package easytier --lib --no-default-features --features quic,stealth-aead`：PASS。
- 上述精确测试二进制的 `tunnel::quic::tests::`：23/23 PASS，串行覆盖普通 QUIC、Stealth QUIC、IPv4/IPv6、bind 校验、endpoint retry/cleanup、Q0 GSO 尾段、显式 Brutal、省略速率的 BBR fallback、两端发送模式独立，以及同一客户端上下文的 `QUIC -> Brutal -> QUIC` 配置隔离。
- 所有名称包含 `quic_brutal` 的入口/controller/direct-candidate 测试：15/15 PASS；确认 direct candidate 不携带 listener 的本地 `tx_bps`。
- QUIC listener port index、CLI listener parser、transport priority 聚焦测试分别 2/2、1/1、2/2 PASS。
- `cargo test --locked --no-run --package easytier --lib` 和其 `tunnel::quic::tests::`：PASS，后者 23/23。
- `cargo check --locked --package easytier --lib --no-default-features`：PASS，证明生产库在不编译 QUIC 时没有候选 `cfg` 泄漏。
- GUI 聚焦 Vitest：3/3 PASS；精确覆盖 `11013`、留空不添加 query、IPv6 URL 解析/编辑和切回普通 QUIC 时清除 `tx_bps`。完整 frontend-lib Vitest：87/87 PASS。`frontend-lib` build、frontend build、`tauri-plugin-vpnservice` build 和 `easytier-gui` build 均 PASS。
- `cargo test --no-run --no-default-features` 仍会命中基线 `peer_conn.rs::quic_secure_mode_bench` 未加 QUIC feature gate 的 ignored benchmark import；该文件相对 `fc035008` 无差异，不把它归因于候选，也不在本候选顺手修改。
- 现有 `quic_stealth_three_node_carries_phase2_tcp` 与 `tests::three_node::quic_proxy` 曾在候选和精确 `fc035008` 基线二进制上以相同方式失败；它们保留为基线缺口，不作为候选 PASS，也不触发候选回退。其余候选判断只使用可重复的有效证据。

### 10.4 Dispatch and validation plan

- Required workflows：一次 `profiling-beta` Linux optimized/symbolized x86_64-musl bundle和同 SHA Android candidate；push 前再次确认自动触发状态，禁止重复 dispatch。
- `.37/.38`：验证 exact musl artifact、普通/Stealth QUIC、Brutal、IPv4/IPv6、旧内核单 segment、listener/连接恢复、FD/task/RSS/CPU 和清理回落；不从 builder GNU binary 或旧内核主机推断 GSO 收益。
- `lv1g2/lv1g3`：验证真实 GSO/GRO、双向 IPv4/IPv6、普通 QUIC 回归、Brutal A/B、受控损伤和资源稳定性。两台主机 glibc 不一致，因此 pre-build GNU debug binary 不作为双端性能候选。
- 国内到云按 `.37/.38` 四条出向路径做配对 A/B；如跨境路径明显受扰，只把它标为环境受扰，分别保留内网功能/资源证据和云端受控性能证据，不把噪声误判为协议回归或收益。
- Android：设备当前 USB ADB 在线；使用同 SHA workflow artifact 保留数据升级安装，验证普通 QUIC 不回归、Brutal、GSO capability/fallback、Wi-Fi/蜂窝/断网恢复、VPN lifecycle 和资源回落。
- Workflow 等待期间：完成 candidate diff、Cargo.lock、platform `cfg`、workflow pin 和生成文件复核；预清理验证主机、分配显式端口、准备 exact artifact 校验和失败注入矩阵，不修改正在构建的 snapshot。

## 11. 首个 immutable candidate 的早期证据

首个 artifact 只用于验证显式静态 `tx_bps` 实现；后续 GUI/可选速率修订必须形成新的 build-affecting candidate，不能把下列结果冒充为修订后 artifact 的证据。

- SHA：`0e38a79da501a3468fc6a904fb6e190665884817`。
- Linux profiling workflow：run `29799908659`，SUCCESS；Android candidate workflow：run `29799908653`，SUCCESS。两个 `headSha` 均为上述 SHA，Linux/Android 外层与内层校验和、`BUILD_INFO`、target 和 run ID 均匹配。
- `.37/.38` 普通 QUIC 双向 1 GiB 冒烟约为 648/669 Mbps；Brutal 为 814/783 Mbps。随后 3 GiB、30 秒级样本中，普通 QUIC 为 721/697 Mbps，Brutal 为 817/800 Mbps，即约 +13%/+15%。连接明确报告 `tunnel_type=quic-brutal`，没有被其他协议接管。
- 两条国内到云路径的普通 QUIC RTT 约为 170/183 ms。相同 128 MiB 的 A-B-A 中，Brutal 相对前后普通 QUIC 均值：上传约 +26%/+25%，下载约 -3%/+3.5%。这证明某些出向上传可获益，但不证明双向或普遍容量提升。
- 早期运行中旧内核节点 task 均稳定为 9，云节点 task 均稳定为 6；FD 分别约 21-22，日志无 panic/fatal/error。每轮均按精确测试 PID 清理并确认进程/TUN 释放；该短时证据不替代后续资源恢复和长稳态门槛。
- 早期容量没有满足“LAN 与跨境双向均稳定显著提升”的 accept 门槛。维护者决定因实现隔离而保留为显式 opt-in transport，因此当前决策为 `H=experimental`，不得把上述结果写成普遍性能声明或改变现有默认协议顺序。
