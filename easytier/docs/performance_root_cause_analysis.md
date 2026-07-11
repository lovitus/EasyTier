# EasyTier 性能根因分析

**日期**: 2026-07-10
**测试拓扑**: 192.168.1.37 ↔ 192.168.1.38 (10Gbps LAN, CentOS 7, 2核, kernel 3.10)
**EasyTier 版本**: v2.6.9 (debug build, musl)

---

## 1. 测试基准线

| 基准 | 测试方法 | 吞吐量 |
|------|----------|--------|
| B1: 物理线速 | iperf3 TCP 直连, 两台机器间 | **10.00 Gbps** |
| B2: 内核 VPN | WireGuard kernel (未测, 估算) | ~8-9 Gbps |

**B1 结论**: 两台测试机器的网络极限是 10 Gbps 线速。

---

## 2. EasyTier 分层性能

### 2.1 LAN TCP 吞吐量 (37↔38, 10G 物理线路)

| 模式 | 加密 | 混淆 | TCP 吞吐量 | 占物理线速 | vs Plain |
|------|------|------|-----------|-----------|----------|
| Plain | ❌ | ❌ | **0.75 Gbps** | 7.5% | baseline |
| Stealth | ❌ | ✅ | **0.79 Gbps** | 7.9% | +5% |
| Secure | AES-GCM | ❌ | **0.86 Gbps** | 8.6% | +15% |
| Stealth+Secure | AES-GCM | ✅ | **0.80 Gbps** | 8.0% | +7% |

### 2.2 逐秒吞吐量变化 (iperf3 10s 测试)

```
Plain:   695 771 745 778 724 730 791 739 808 752  avg=753 Mbps
Stealth: 699 798 840 809 807 776 821 747 790 797  avg=788 Mbps
Secure:  739 842 932 873 945 686 820 956 897 919  avg=861 Mbps
S+S:     740 754 787 835 832 841 810 854 784 768  avg=800 Mbps
```

TCP 拥塞窗口在 500-950 Mbps 之间波动，各模式波动模式一致。

### 2.3 CPU 利用率

测试中客户端 CPU < 3%, 服务端 CPU < 2%. CPU 远未饱和。

### 2.4 Plain UDP 丢包率

| 目标速率 | 实际发送 | 丢包率 |
|----------|---------|--------|
| 1 Gbps | 998 Mbps | **61.6%** |
| 100 Mbps | 100 Mbps | **94.3%** (netns 环境) |

---

## 3. 根因分析

### 3.1 四种模式吞吐量几乎相同 → 加密不是瓶颈

Plain (0.75) vs Stealth (0.79) vs Secure (0.86) vs Stealth+Secure (0.80) 的差异在 ±8% 范围内，属于统计噪声。如果加密是瓶颈，Secure 模式应该比 Plain 慢得多，但实际 Secure 反而最快。

现有优化文档 `stealth_secure_optimization_plan.md` 将 P1 优先级放在 HMAC-SHA256 → AES-GCM 替换上，但 **Plain 模式（零加密）也只有 0.75 Gbps**。即使把加密开销降到零，提升空间也非常有限。

### 3.2 CPU 远未饱和 → 瓶颈不是算力

0.8 Gbps 时 CPU 仅 2-3%。如果 CPU 是瓶颈，线性外推可以到 25+ Gbps。但实际卡在 0.8 Gbps，说明瓶颈是 **延迟/同步开销**（syscall + channel + framing），不是算力。

### 3.3 UDP 62% 丢包 → 确认为 per-packet 处理瓶颈

在 1 Gbps UDP 测试中，发送端以 998 Mbps 速率发包，接收端有 61.6% 丢包。按 TUN MTU 1360 字节计算：

```
1 Gbps / (1360 × 8) ≈ 92,000 pps  (到达速率)
0.8 Gbps / (1360 × 8) ≈ 73,500 pps  (实际处理能力)
丢包率 ≈ (92000 - 73500) / 92000 ≈ 20% ... 但实测 62%
```

实际丢包率高于理论值，说明在接近极限速率时 pipeline 中的队列/缓冲开始溢出，丢弃加速。

### 3.4 真正的瓶颈：per-packet 处理流水线

当前每个数据包的完整路径：

```
TUN read (syscall)           ← 1次 syscall
  → userspace 拷贝           ← 第1次 copy
  → mpsc channel send/recv   ← 调度开销
  → 路由查找
  → 加密 (可选)
  → framing (ZCPacket→Bytes) ← 第2次 copy
  → TCP socket write(syscall) ← 1次 syscall
─────────────────────────────────────
每包成本: 2次 syscall + 至少2次 copy + 调度
```

**~73,500 pps** 就是这个流水线在 CentOS 7 2核机器上的硬上限。要达到 10 Gbps 线速，需要处理 ~920,000 pps（12.5 倍提升）。

---

## 4. 现有优化方案的评估

`stealth_secure_optimization_plan.md` 的优化路线：

| 优先级 | 方案 | 评估 |
|--------|------|------|
| P1 | HMAC-SHA256 → AES-GCM | ❌ 打错靶子。Plain 模式无加密也只有 0.75 Gbps |
| P2 | 零拷贝解密 | ⚠️ 减少 copy 是对的，但只优化了加密路径的 copy |
| P3 | AtomicU64 替换 OsRng | ✅ 低成本改进，但提升微乎其微 |
| P4 | RwLock 读锁 | ✅ 同上 |
| P5 | 合并 replay 锁 | ✅ 同上 |

**问题**: P1-P5 都聚焦于加密层的实现细节，没有涉及根本的 per-packet 处理瓶颈。

---

## 5. 正确的优化方向

### 5.1 优先级排序

| 优先级 | 方向 | 预期提升 | 改动量 |
|--------|------|----------|--------|
| **P0** | **GSO/GRO 批处理** | **5-10x** | 中等 |
| **P1** | **减少拷贝次数** | **2-3x** | 较大 |
| **P2** | **TUN 设备参数调优** | 1.2-1.5x | 小 |
| P3 | 加密算法优化 | 1.0x（Plain 已经是零开销） | 中等 |
| P4 | 锁优化 | 1.05x | 小 |

### 5.2 P0: GSO/GRO 批处理（最关键）

**当前**: 每个包独立完成 TUN read → process → socket write
**目标**: 一次 syscall 读取/写入 N 个包（N=50-64），批量处理

```
现状:  read() → process 1 pkt → write() → read() → ...
批处理: read() → [pkt1, pkt2, ..., pkt50] → batch process → writev([pkt1..50])
```

syscall 次数从 2N 降到 2，吞吐量提升 ~N 倍。

在 10G 线路上：
- 当前: 73,500 pps → 0.8 Gbps
- GSO batch=50: 73,500 × 50 = 3,675,000 pps → **~40 Gbps**（理论，受 CPU 限制）

Linux TUN 设备的 GSO (Generic Segmentation Offload) 允许发送端一次写入最大 64KB 的大包，内核自动分段。接收端 GRO (Generic Receive Offload) 将小包合并。

### 5.3 P1: 减少拷贝次数

当前每包至少 2 次 memcpy。使用 `io_uring` 或 `sendfile` 风格的零拷贝可以减少到 0-1 次。

### 5.4 P2: TUN 设备参数

- `txqueuelen`: 默认 500，建议 5000+
- `/proc/sys/net/core/wmem_max`: 增大 socket 写缓冲
- `/proc/sys/net/core/netdev_max_backlog`: 增大 backlog

---

## 6. 优化验证记录

### 6.1 P1: `sink.feed()+flush()` 替代 `sink.send()` — ❌ 证伪

**假设**: `SinkExt::send()` 每包触发 `poll_flush`，改用 `feed()` + drain + 一次 `flush()` 减少开销。

**改动**: `virtual_nic.rs:do_forward_peers_to_nic` + `TunAsyncWrite::is_write_vectored()→false`，~20行。

**结果**: 0.64-0.66 Gbps vs 基线 0.75 Gbps (-12%)。**无提升。**

**根因**: Linux TUN `flush()` 是空操作（字符设备无缓冲）。改为 `feed()` 后仍是 N 次独立 `write()`，`try_recv()` drain + `yield_now()` 额外开销反而拖慢了吞吐。**`sink.send()` 逐包模式不是瓶颈。**

### 6.2 P2: channel 容量 32→256 / 128→256 — ❌ 证伪

**假设**: `mpsc.rs:61` 和 `peers/mod.rs:85` 的 channel 容量限制批聚合。

**改动**: 2 行。

**结果**: 0.71 Gbps vs 基线 0.75 Gbps (-5%)。**无提升。**

**根因**: Channel 容量不是瓶颈。`MpscTunnel::forward_one_round_no_timeout` 已有的 drain 机制不受容量显著影响。

### 6.3 四轮评审发现的硬性限制

| 发现 | 结论 |
|------|------|
| TUN `writev` 无法批处理 | 字符设备，writev 拼接多个 IP 包导致数据损坏 |
| TUN `recvmmsg` 无法使用 | 字符设备，仅 socket fd 支持 (ENOTSOCK) |
| `IFF_VNET_HDR` GSO 复杂度 | 需 TCP coalescing engine + virtio_net_hdr + TUNSETOFFLOAD，~200 行 |
| BiLock 并非明确瓶颈 | 不跨 `Pending` 持锁，需 profiling 确认 |

### 6.4 已验证的无效优化方向

| 方向 | 改动量 | 结果 | 结论 |
|------|--------|------|------|
| `feed()+flush()` | ~20行 | -12% | Sink API 调用模式不是瓶颈 |
| channel 容量增大 | 2行 | -5% | channel 大小不是瓶颈 |
| TUN `writev` | — | 未实施 | 会损坏数据，不能做 |
| TUN `recvmmsg` | — | 未实施 | ENOTSOCK，不能做 |

### 6.6 带符号 profiling (2026-07-11, GitHub workflow release + DWARF) ✅

**方法**: profiling-beta workflow 构建 (release + DWARF, not stripped), 37↔38 10G LAN, `--stealth-mode false`, iperf3 TCP 20s @ **1.00 Gbps**。

> **比较边界**：此前约 0.75--0.78 Gbps 的 `ec-rel` 是另一份 stripped
> 产物，提交身份无法从二进制确认，且日志显示其启用了 Stealth。它与本轮
> `ec-sym` 并非“同一提交只切换 Stealth”的受控 A/B，因此 28% 差值不能归因于
> Stealth。后续必须用同一 profiling 产物、相同参数和拓扑分别测试开/关 Stealth。

**perf flat profile (self, with symbols) — 节点 A (接收端)**:

| % | 符号 | 分类 |
|---|------|------|
| **6.85%** | `sccp` (syscall entry) | 内核 — sys_write→TUN |
| 5.32% | `_raw_spin_unlock_irqrestore` | 内核锁 |
| 3.46% | `copy_user_generic_unrolled` | 内核 — TUN 数据拷贝 |
| **3.43%** | `_aesni_ctr32_ghash_6x` | **shared-secret RingCipher AES-GCM（硬件加速）** |
| 2.11% | `system_call_after_swapgs` | 内核 — syscall 开销 |
| **1.76%** | `PeerManager::start_peer_recv::closure` | 对端接收路径 |
| **1.60%** | `mpsc::Sender::send::closure` | 通道发送 |
| **1.45%** | `FramedWriter::poll_flush` | TUN 写入刷新 |
| 1.38% | `memcpy` | 内存拷贝 |
| 1.28% | `nf_iterate` | netfilter/conntrack |
| **1.25%** | `quanta::get_now` | 时间戳获取 |
| **1.18%** | `Socks5Server::try_process_packet_from_peer` | Socks5 过滤 (即使未用) |
| **0.88%** | `TrafficCounters::clone` | 流量统计克隆 |
| **0.85%** | `malloc` | 堆分配 |

**perf flat profile (self, with symbols) — 节点 B (发送端)**:

| % | 符号 | 分类 |
|---|------|------|
| **6.98%** | `sccp` (syscall entry) | 内核 — sys_read←TUN |
| 3.59% | `copy_user_generic_unrolled` | 内核 — TUN 数据拷贝 |
| 3.57% | `vmxnet3_xmit_frame` | 虚拟网卡 |
| 3.26% | `eventfd_write` | tokio 事件通知 |
| **3.06%** | `_aesni_ctr32_ghash_6x` | **shared-secret RingCipher AES-GCM** |
| 2.25% | `system_call_after_swapgs` | 内核 — syscall 开销 |
| **1.52%** | `quanta::get_now` | 时间戳获取 |
| 1.45% | `memcpy` | 内存拷贝 |
| **1.39%** | `mpsc::Sender::send::closure` | 通道发送 |
| **0.92%** | `malloc` | 堆分配 |
| **0.89%** | `TrafficCounters::clone` | 流量统计克隆 |
| **0.80%** | `do_forward_nic_to_peers_task` | TUN→对端转发 |
| **0.72%** | `PeerManager::send_msg_by_ip` | IP 路由发送 |
| **0.64%** | `FramedWriter::poll_flush` | TUN 写入刷新 |

**调用图 (节点 A)**:
```
45.66%  do_forward_peers_to_nic  ← TUN 写入路径
  └── 38.83%  sccp → sys_write → vfs_write → tun_chr_aio_write (31.88%)
       └── 23.44%  netif_receive_skb  ← 内核网络栈
```

**调用图 (节点 B)**:
```
38.66%  do_forward_nic_to_peers_task  ← TUN 读取路径
  └── 23.62%  PeerManager::send_msg_by_ip
       └── 22.26%  send_msg_after_nic_pipeline
            └── 11.63%  send_msg_internal
```

**关键发现**:
1. **系统调用入口 `sccp` 是最大单一热点 (~7%)** — TUN write (A) 和 TUN read (B) 的 syscall 开销
2. **shared-secret AES-GCM ~3%** — `perf` 调用图落在 `RingCipher` 和
   `PeerManager::try_compress_and_encrypt`；这是现有共享密钥数据面加密，不是 Stealth
   outer AEAD，也不是 PeerSession secure 层。`--stealth-mode false` 不等于无加密
3. **`quanta::get_now` ~1.5%** — 每包时间戳，出人意料地昂贵
4. **`Socks5Server::try_process_packet_from_peer` ~1.2%** — 即使未用 Socks5，每包仍经过此过滤器
5. **`TrafficCounters::clone` ~0.9%** — 每包克隆统计，可优化
6. **`malloc` + memcpy ~2.3%** — 每包分配/拷贝
7. **本轮 DSO 采样约为内核 54-58%、`ec-sym` 42-45%** — 两侧都占有显著成本；
   旧报告中的 `4.14%` 只是一个未知用户态符号，并非全部用户态占比
8. **Stealth 的独立成本尚未由本轮数据证明** — 0.75/0.78 与 1.00 Gbps 来自
   不同构建和配置，只能作为后续同构 A/B 的线索

### 6.7 同构 TCP Stealth A/B（2026-07-11）

使用 profiling beta commit `784659e8` 的同一份 x86_64-musl 二进制，在相同两节点、
相同 network secret、TCP underlay、MTU 和 10 秒单流测试下，仅切换 TCP Stealth：

| 场景 | 第 1 轮 | 第 2 轮 | 第 3 轮 | 中位数 |
|------|---------:|---------:|---------:|-------:|
| `stealth_mode=false` | 1.13 Gbps | 1.05 Gbps | 1.15 Gbps | **1.13 Gbps** |
| `stealth_mode=true`, `stealth_protocols=tcp` | 1.05 Gbps | 1.09 Gbps | 1.14 Gbps | **1.09 Gbps** |

Stealth-on 中位数比 Stealth-off 低约 3.5%，小于三轮自身离散度。本轮可以确认：

- 当前快照的 TCP Stealth 能完成严格 listener 协商、PeerConn 建立、ping 和大流量传输。
- 没有复现早期 profiling beta 的 TCP Stealth 建连失败。
- 现有样本不能证明 TCP Stealth 有显著吞吐退化，也不能证明它完全零成本；需要更长时间、
  更多轮次和 CPU 归一化数据才能给出窄置信区间。
- 该结果不能与另一份 0.75--0.78 Gbps stripped 二进制直接比较并宣称“提升 50%”。

同次采集的接收端 perf 有约 93K 样本，可用于热点定位；发送端仅 107--120 个样本，
不足以形成可靠分布，相关百分比不进入性能结论。后续采样必须记录目标 PID、线程列表、
采样频率、实际样本数和进程 CPU，并在样本量不足时判定该轮无效。

### 6.8 优化计划当前状态

详见 `docs/failed_attempts/gso_gro_batching_plan.md`。

**代码已全部回退至 v2.6.9 基线。**

---

## 7. 建议

1. **不要在 tokio/Sink API 层继续优化** — P1/P2 已证伪，调用模式和 channel 容量不是瓶颈
2. **先做 profiling** — `strace -c` + `perf` 确定 syscall 和 CPU 热点，用数据驱动优化方向
3. **GSO/GRO 作为独立 spike** — 复杂度高（~200行 + TCP coalescing engine），不可与其他改动混合
4. **UDP/WG `recvmmsg/sendmmsg` 独立立项** — 需绕过 tokio 直接操作 raw socket fd
5. **建立 pps 性能指标** — 用 UDP 丢包率替代 Gbps 作为主要度量，排除 TCP 拥塞控制干扰

---

## 附录: 测试环境

| 机器 | OS | CPU | RAM | NIC | 角色 |
|------|-----|-----|-----|-----|------|
| 192.168.1.37 | CentOS 7 (3.10) | 2核 | 3.7GB | 10Gbps | EasyTier A |
| 192.168.1.38 | CentOS 7 (3.10) | 2核 | 3.7GB | 10Gbps | EasyTier B |
| 192.168.2.160 | CentOS 7 (3.10) | 8核 | 31GB | 10Gbps | 编译机 (docker) |

### 测试命令

```bash
# 物理基准
# 37: iperf3 -s; 38: iperf3 -c 192.168.1.37 -t 10

# EasyTier 测试 (脚本 easytier/easytier/docs/easy_bench.sh)
# 37: bash easy_bench.sh a [stealth] [secure]
# 38: bash easy_bench.sh b [stealth] [secure]
```

### 编译

- easytier-core debug build, musl target (x86_64-unknown-linux-musl)
- 编译于 192.168.2.160 docker (easytier-debug-builder)
- commit: releases/v2.6.9
