# 性能瓶颈排除清单与后续定位 TODO

**状态**: active
**更新日期**: 2026-07-10
**目标**: 记录已经证伪或技术上不可行的优化方向，避免后续再次围绕同一假设投入；在不改变功能、协议语义和安全边界的前提下，定位当前约 0.8 Gbps 平台的真实瓶颈。

## 结论边界

- 当前性能问题仍未找到已被 profiling 证实的单一根因。
- 不再把 TUN `flush`、channel 容量或 `BiLock` 直接描述为主要瓶颈。
- 不再根据低 CPU 快照直接推导 syscall、锁、copy 或调度中的任一项是根因。
- `Plain`、Stealth、派生 Secure 和显式 Secure 吞吐接近，只能排除“加密算法是唯一主瓶颈”，不能证明所有加密、分帧和会话路径都没有额外成本。
- 任何新优化必须先有对应热点或等待证据，单变量实现和 A/B 验证，不能再根据代码结构估算倍数。

## A. 已实测证伪，不再重复实施

### A1. Peer→TUN `feed() + bounded drain + flush()`

- 假设：减少逐包 `SinkExt::send()` 带来的 Future、锁和 readiness 开销。
- 实测：约 0.64-0.66 Gbps，低于约 0.75 Gbps 基线，下降约 12%。
- 结论：TUN `flush()` 是空操作，每包仍需独立 `write()`；额外 drain/yield 没有收益。
- 处理：保持原逐包 `sink.send()` 行为，不再围绕 flush 调用模式优化。

### A2. channel 容量扩大

- 假设：将 32/128 容量扩大到 256，可改善聚合和高吞吐排队。
- 实测：约 0.71 Gbps，低于约 0.75 Gbps 基线，下降约 5%。
- 结论：现有容量不是已观测瓶颈；扩大容量可能增加排队延迟，并未提高吞吐。
- 处理：保持原容量。只有以后采集到 channel occupancy、Full/drop 和排队时延证据时才重新评估。

### A3. `BiLock`/fd 分离作为主要根因

- 假设：TUN 读写共享 `BiLock` 导致明显串行化。
- 最新验证结论：未观察到能解释当前数量级性能差距的改善，不能成立为主要根因。
- 代码事实：`BiLock` 不跨底层 `Pending` 长期持锁，只保护短暂 poll/syscall 临界区。
- 处理：不修改 `tun-easytier` raw-fd API，不引入 `dup + AsyncFd` 生产改动。只有新的锁 Pending 次数、等待时长和双向 A/B 数据证明收益时才允许重开。

### A4. 仅靠调大 TUN/socket 队列解决吞吐

- `txqueuelen`、socket buffer 和 backlog 只能缓解有证据的突发排队或 drop，不能减少每包处理成本。
- 当前没有证据表明单纯调大队列可以解决主要吞吐平台。
- 处理：不得作为默认“性能修复”；仅在明确观测到对应队列 drop 后做单变量验证。

## B. 技术上不可行或会破坏数据，不得实施

### B1. 对 TUN 使用 `writev()` 写多个 IP 包

- Linux TUN 是消息边界字符设备，一次 write 表示一个 L3 packet。
- `writev()` 会把多个 iovec 拼成一个 packet，不会提交多个独立 packet。
- 后果是包边界损坏，因此 TUN writer 必须保持 `is_write_vectored() == false` 或确保每次 vectored write 只描述一个 packet。

### B2. 对 TUN fd 使用 `recvmmsg()`

- `recvmmsg()` 只适用于 socket；TUN fd 会返回 `ENOTSOCK`。
- TUN 每次 `read()` 只返回一个 packet，不能通过 socket mmsg API 批量读取。

### B3. 在 `RingSink` 上实现 `sendmmsg()`

- `RingSink` 是内存 ring，不是 UDP/WG socket writer。
- mmsg 只能放在实际 socket 边界，并处理 readiness、部分成功、背压和防重复。

### B4. 直接拼接普通 TCP 包生成 GSO super-packet

- 普通 IP/TCP packet 不能简单拼 payload。
- 合法 coalescing 需要同一 flow、连续 sequence、兼容 options、无乱序/重传，并重建 IP/TCP header、checksum 和 virtio metadata。
- 未完成完整 GRO/coalescing 设计前禁止进入生产路径。

## C. 尚未证伪，但不得继续当作当前根因

### C1. `IFF_VNET_HDR` + GSO/GRO

- **2026-07-11 perf 数据**: 内核 ~50% + 用户态 ~50%，两者都是瓶颈。GSO 可降低 TUN write 开销（内核侧），但需同时解析用户态热点才能判断总体收益。
- 这是独立架构项目，不是小型 TUN 参数优化。
- 启用会改变双向 TUN frame，并可能产生超过当前 buffer/overlay MTU 的 packet。
- 只有 profiling 证明 packet-rate/TUN 边界是主要限制，并完成大包输入、分段、MTU、Stealth/Secure filter 和 Linux 3.10 回退设计后才能重新立项。

#### C1a. GSO 接入分析与复杂度评估（修订: 撤回过度简化的注入点假设）

**先前分析中的三个错误**:

1. **注入点错误**: `TunZCPacketToBytes` 是 `FramedWriter` 的内部 converter，raw bytes 不会暴露给 `do_forward_peers_to_nic`。且 VNET_HDR 模式下非 TCP 包也必须携带零 GSO header。正确边界是 Linux 专用的 `TunGsoSink` adapter（写端）和 `TunGsoStream` adapter（读端），而不是声称 FramedWriter/converter/TunAsyncWrite 全不改。

2. **Phase 0 不可行**: `IFF_VNET_HDR` 不能单独启用。启用后每个 TUN 读写 frame 都带 virtio header，现有 TunStream 会把 header 当 IP 头读取，现有 writer 不添加 header → 立即破坏全部流量。必须从第一步同时实现双向 header 编解码。

3. **TcpCoalescer 复杂度被严重低估**:
   - `&[u8] → Vec<u8>` 无法表达缓存无输出、flush 旧流+处理新包等 0..N 种输出
   - 500μs 超时需要独立 async 唤醒，否则流量停止时最后一批永久滞留
   - FIN/PSH/RST 不能简单清除；ACK/window/timestamp/SACK/ECN/IP ID 必须兼容
   - `flags=0` 与 checksum metadata 的组合需按内核 virtio 语义验证
   - 这不是约 200 行纯函数，而是需要设计成有界、可取消、可 flush 的异步 sink 状态机

**修正后的注入边界**:

```
TUN 写端（出向）:
  do_forward_peers_to_nic
    → [NEW] TunGsoSink (Linux 专用 adapter)
        内部: TcpCoalescer 状态机 + FramedWriter
        非 Linux/feature-gate 关闭: 完全等价于当前 FramedWriter 路径
    → TUN fd

TUN 读端（入向）:
  TUN fd
    → [NEW] TunGsoStream (Linux 专用 adapter)
        解析 virtio_net_hdr, 处理可能的 GSO 大包
        非 Linux/feature-gate 关闭: 完全等价于当前 TunStream 路径
    → do_forward_nic_to_peers_task
```

**不改动的模块**（确认仍成立）: PeerManager、路由、加密、Stealth/Secure、TunnelFilterChain。

**必须改动的模块**（修正后）: `virtual_nic.rs` 的 TUN 创建 + 读写 adapter；新增 `tunnel/gso.rs`（TcpCoalescer 状态机 + TunGsoSink/TunGsoStream adapter）。

**复杂度重估**: 整体 spike ~400-600 行（含双向 header 编解码、TcpCoalescer 异步状态机、超时唤醒、TCP 语义兼容、feature-gate、Linux 3.10 回退），不是约 200 行纯函数。

### C2. UDP/WG `recvmmsg()/sendmmsg()`

- socket mmsg 技术上可行，但只优化 UDP/WG socket 边界，不解释 TCP、Plain 等路径共同存在的吞吐平台。
- 只有 protocol-specific profile 显示 UDP/WG socket syscall 是热点时才实施。
- 必须复用 Tokio readiness，保持单 reader/writer、datagram 顺序和错误重试防重复。

### C3. Stealth/Secure 微优化

- 当前模式间吞吐接近，HMAC/AEAD、nonce、replay lock 等局部微优化不能解释 Plain 也慢的问题。
- 已完成且有独立安全价值的零拷贝/竞态修复可以保留，但不得把继续微调加密算法作为总吞吐问题的首要方案。
- 显式 Secure 的 relay/session 路径仍需在对应拓扑单独 profiling，不能由直连结果外推。

## D. 下一轮必须完成的定位任务

### D0. 带符号 profiling ✅ 已完成 (2026-07-11)

**方法**: GitHub workflow release + DWARF (332MB, not stripped), `perf record -g`,
`--stealth-mode false`, 1.00 Gbps。该模式仍启用 shared-secret `RingCipher` 数据面加密，
不能称为“零加密 Plain”。此前 0.75--0.78 Gbps 数据来自另一份 stripped 构建，不能
与本轮构成受控 Stealth A/B。

**用户态 self 热点 (带符号)**:

| % | 热点 | 可优化? |
|---|------|---------|
| 3.4% | `_aesni_ctr32_ghash_6x` (shared-secret RingCipher AES-GCM) | ⚠️ 已硬件加速；不是 Stealth/PeerSession 热点 |
| 1.6% | `mpsc::Sender::send` | 🟡 通道发送, P2 已证伪 |
| 1.5% | `quanta::get_now` | 🟢 时间戳, 可能可缓存/粗粒度 |
| 1.5% | `FramedWriter::poll_flush` | 🟡 P1 已证伪 |
| 1.2% | `Socks5Server::try_process_packet_from_peer` | 🟢 未用 Socks5 时仍每包检查 |
| 0.9% | `TrafficCounters::clone` | 🟢 每包克隆, 可优化 |
| 0.9% | `malloc` | 🟡 每包分配 |

**内核侧约 54-58%（DSO 汇总）**:
| % | 热点 |
|---|------|
| 7% | `sccp` (syscall entry) |
| 3.5% | `copy_user_generic_unrolled` (TUN copy) |
| 5.3% | `_raw_spin_unlock_irqrestore` (kernel locks) |
| 1.3% | `nf_iterate` (netfilter) |

**结论**:
- 用户态热点分散（无单一 >5% 项），是"千刀万剐"模式
- 内核 syscall 入口 (`sccp` 7%) 是单一最大项
- 0.75/0.78→1.00 Gbps 的差值混入了构建身份和配置差异，不能归因于 Stealth
- 可快速优化的低风险项: `quanta::get_now` 粗粒度化, Socks5 跳过未启用路径, `TrafficCounters::clone` 优化

### D1. 建立可信基线

- [ ] 使用同一提交、同一配置、同一拓扑分别测 Plain、Stealth、派生 Secure、显式 Secure。
- [x] 使用 commit `784659e8` 对 TCP Stealth off/on 完成同构三轮 A/B：中位数
  1.13/1.09 Gbps，差异落在样本波动内；仍需补派生 Secure、显式 Secure 和其他协议。
- [ ] 覆盖 TCP、UDP、QUIC、FakeTCP，以及单流、多流、小包 pps、单向和双向。
- [ ] 每个场景至少 5 轮，记录中位数、离散度、重传、丢包和 p99 RTT。
- [ ] debug 产物只用于正确性；性能结论使用 GitHub workflow 生成的优化产物。不得在远端手工使用 `--release`。
- [ ] 记录准确的 per-thread CPU，而不是只采集进程瞬时总 CPU。

### D2. CPU 与 off-CPU profiling

- [ ] 对优化产物采集 `perf record`/flamegraph，列出前 20 个符号及占比。
- [ ] perf 结果必须同时记录 PID、线程数、进程 CPU、采样频率和总样本数；发送端仅
  107--120 个样本的历史采集判定无效，不得据此解释热点。
- [ ] 分线程记录 Tokio worker、TUN reader/writer、tunnel socket task 的 CPU 和上下文切换。
- [ ] 采集 off-CPU/等待信息，区分 socket/TUN readiness、channel wait、timer 和锁等待。
- [ ] 若符号被 strip，单独通过测试 workflow 生成带符号的优化 profiling artifact，不改变正式发布 profile。

### D3. 分层定位

- [ ] 物理网络 iperf 基线。
- [ ] 本机/同机 TUN 或 veth 注入基线，隔离虚拟 NIC 边界。
- [ ] tunnel socket/framing 基准，隔离 PeerManager 和路由。
- [ ] PeerManager/ring/mpsc 基准，隔离真实 socket 和 TUN。
- [ ] 完整 overlay 基准，对照前三层确定吞吐下降发生在哪一段。
- [ ] 统计每层 packet size、packet count、复制字节数和队列 occupancy。

### D4. 决策规则

- [ ] 只有热点占比、等待时间或 drop 证据能解释主要差距时才提出代码方案。
- [ ] 每个优化单独提交、单独 A/B、可独立回退。
- [ ] 吞吐或 CPU 必须稳定改善，同时功能、低负载 RTT、p99、丢包、控制面心跳和资源回收不得回归。
- [ ] 无收益或负收益的实验移入 `docs/failed_attempts/`，记录提交、命令、原始结果和回退状态。

## 关联文档

- `docs/failed_attempts/gso_gro_batching_plan.md`: 原批处理计划和失败实验细节。
- `docs/performance_root_cause_analysis.md`: 现有基准与 P1/P2 验证数据；其中尚未经过 profiler 证明的“根因”表述应视为历史假设，不作为当前结论。
- `docs/stealth_zero_copy_validation_report.md`: Stealth/Secure 功能与协议性能对照。
- `docs/stealth_secure_profiling_2026_07_09.md`: 安全模式 profiling 记录。

## 固定边界

- 不为了吞吐改变 wire、Stealth strict listener、Secure identity、Proxy failover、SOCKS KCP-only、路由或 ACL 语义。
- 不在维护者本机编译；编译和测试遵循仓库 `AGENTS.md` 的远程 builder 规则。
- 未经真实设备验证，不触发正式 release workflow。
