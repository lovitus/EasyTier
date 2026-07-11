# GSO/GRO 批处理优化方案（证伪）

**状态**: ❌ 已证伪
**日期**: 2026-07-10
**结论**: 0.8 Gbps 瓶颈不在 tokio/Sink API 调用层或 channel 容量。P1 (feed/flush) 和 P2 (channel 增大) 均无提升，代码已回退。

---


## virtual_nic.rs 设计审查

`virtual_nic.rs` 核心架构（TUN 创建 → stream/sink 分离 → 两个 tokio task 驱动读写）合理。经四轮评审确认的可优化点：

### `sink.send()` 逐包 poll_flush 开销

`SinkExt::send()` = `poll_ready + start_send + poll_flush`。每次 flush 触发 Future 轮询、BiLock 获取/释放、tokio readiness 检查。Linux TUN 的 `flush()` 是空操作（字符设备无缓冲），但开销在于异步调度层面。**可试验 `feed() + drain ≤64 包 + flush()` 减少开销。**

注意：TUN 不支持 writev（会拼接多个 IP 包导致数据损坏），每包仍是独立 `write()`。收益来自减少 Future/锁开销，不是减少 write syscall。

### BiLock 读写共享 — 需先测量

BiLock 不在 `Pending` 期间持锁，只保护短暂 poll/syscall。**不能认定 50% 损失。** 需临时 instrumentation 记录 `poll_lock()` Pending 次数和等待时长；`perf` 只辅助确认 CPU 热点，之后再决定是否做 fd 分离 spike。

### TUN 读端无法批处理

TUN 是字符设备，`recvmmsg` 只适用于 socket fd（会 `ENOTSOCK`）。内核每次 `read()` 只返回一个包，用户态无法批处理 TUN 读 syscall。

### 现状

| 观察 | 状态 |
|------|------|
| `sink.send()` 逐包开销 | 🟢 P1 可试验（限 64 包/批, 不承诺倍数） |
| BiLock 串行化影响 | 🟡 先 profiling, 确认热点再 spike |
| TUN 读端无批处理 | 🔴 字符设备限制, 无法在用户态解决 |
| IFF_VNET_HDR/GSO | 🔴 独立立项, 需 TCP coalescing engine |

---

## 研究结论

三个研究代理完整追踪了数据包从 TUN 到 socket 的全路径。四轮评审验证的关键发现：

### 已确认的优化点

| 发现 | 状态 |
|------|------|
| `sink.send()` 逐包 poll_flush 开销 | 🟢 P1 试验: `feed()+flush()` 限64包/批 |
| `MpscTunnel` 已有 `feed()+drain+flush` 调度模式 | 🟢 可复用调度思路；TUN仍须单独禁用vectored write并验证包边界 |
| channel 容量可能限制突发聚合 | 🟢 P2 独立 A/B，先测 occupancy/丢包/延迟 |
| BiLock 可能造成读写竞争 | 🟡 需 profiling 确认 |

### 已确认不可行

| 原设想 | 原因 |
|--------|------|
| TUN `writev` 批处理 | 字符设备, writev 拼接多个 IP 包会导致数据损坏 |
| TUN `recvmmsg` 批处理 | 字符设备, recvmmsg 只适用于 socket fd (ENOTSOCK) |
| RingSink `sendmmsg` | RingSink 是内存 buffer, 不是 socket |

### P1 可做的和不能做的

P1 (`feed+flush`) 把 N 次 `send()`（每次 poll_ready + start_send + poll_flush）改为 N 次 `feed()` + 1 次 `flush()`。TUN 每次仍是独立 `write()`（字符设备限制，不可批处理 syscall）。**收益来自减少 Future 轮询、BiLock 获取/释放、tokio readiness 检查的开销，不是减少 write 次数。**

---

## 方案设计

**核心原则**: 零 trait 变更，零 API break，只改调用模式。

### 执行状态总览

| 阶段 | 状态 | 理由 |
|------|------|------|
| **P1: feed/flush** | 🟢 **可试验** | TUN flush() 是空操作, 收益来自减少 Future/锁开销, 待测 |
| **P2: channel capacity** | 🟢 **压测变量** | 两个 channel 分别做 32/128/256 A/B, 数据决定最终值 |
| P0a: BiLock消除 | 🟡 **先测量再 spike** | 先记录异步锁竞争；确认热点后才处理 fd 分离 |
| P0b: VNET_HDR + GSO/GRO | 🔴 **独立 spike** | 不拆分合入；需完整大包、coalescing、MTU和回退语义 |
| P3: epoll drain | 🔴 **搁置** | TUN字符设备无法批处理syscall, 边际收益 |
| P4: recvmmsg 隧道读 | 🔴 **独立 spike** | 复用 Tokio readiness，保持单一 reader和datagram边界 |
| P5: sendmmsg 隧道写 | 🔴 **独立 spike** | 复用 Tokio readiness，处理部分成功、背压和防重复 |
| Phase -1 | 🟡 **仅在对应 spike 证明需要后进行** | 外部 crate 改动独立 PR，不作为前置默认改动 |

### Phase -1: tun-easytier 扩展候选 🟡 仅在 spike 证明需要后进行

**问题**: EasyTier fork 的 `tun-easytier` crate 缺少两个 API：
1. `PlatformConfig` 不支持 `raw_flags()` — TUN flags 在 `Device::new()` 硬编码，无法传 `IFF_VNET_HDR`
2. Linux 上 `Configuration::raw_fd()` 返回 `Error::NotImplemented`（仅 iOS/macOS 支持）— 无法从 `dup()` fd 构造 `Device`

**文件**: `rust-tun` fork (EasyTier 控制)
**改动量**: ~30 行

**方案**:
```rust
// 1. PlatformConfig 加 setter（pub，供外部 crate 调用）
impl PlatformConfig {
    pub fn raw_flags(&mut self, flags: c_short) -> &mut Self {
        self.raw_flags = Some(flags);
        self
    }
}

// 2. Linux Device::new() 使用 raw_flags（若提供）
let flags = config.platform_config.raw_flags.unwrap_or(
    device_type | if packet_information { 0 } else { iff_no_pi }
);

// 3. Linux raw_fd 路径: 去掉 NotImplemented，直接包装 fd
if let Some(raw_fd) = config.raw_fd {
    return Ok(Device { tun: Tun::from_fd(raw_fd)? });
}
```

**替代方案（不修改 crate）**: P0a 用 `dup()` + `tokio::io::unix::AsyncFd<OwnedFd>` 包装 dup fd，手写只接受单包 `write()` 的 TUN writer，明确不暴露 vectored write。P0b spike 可手动创建带 `IFF_VNET_HDR` 的 TUN，但仍须完整实现 header、大包输入、回退和清理，不作为 P0a 的附带改动。

**决策门槛**: P0a profiling 或 P0b spike 明确证明现有 API 无法安全实现时，才修改 fork crate。crate 改动必须独立提交，明确 raw fd 所有权、关闭语义、packet-information、MTU 和错误回滚；不得只修改本机 Cargo checkout。

### Phase 1: 减少 Peer→TUN 的异步 flush 开销 🟢 单变量试验，达标后合入

**文件**: `easytier/src/instance/virtual_nic.rs`
**函数**: `do_forward_peers_to_nic` (line 1124)
**改动量**: ~20 行

**关键修正**: TUN 是消息边界设备（一次 `write()` = 一个 IP 包）。`writev()` 会把多个 iovec 拼接成一个包，导致数据损坏。**`FramedWriter` 的 vectored I/O 对 TCP 字节流正确，对 TUN 不适用。**

**方案**: `feed()` + bounded drain + 一次异步 `flush()`，同时禁用 vectored I/O。Linux TUN 的底层 `flush()` 本身不产生 syscall；优化对象是 Future/poll/锁/readiness 开销，每个包仍然是独立 `write()`：

```rust
// Step 1: TunAsyncWrite 标记不支持 vectored I/O
// virtual_nic.rs line 238: is_write_vectored() → false
// 原因: TUN 设备 writev() 会拼接多个包, 导致数据损坏

// Step 2: do_forward_peers_to_nic 中:
// Before (current):
while let Ok(packet) = recv_packet_from_chan(&mut channel).await {
    sink.send(packet).await?;  // poll_ready + start_send + poll_flush (每次flush)
}

// After (corrected, with batch limit and explicit error handling):
while let Ok(first) = recv_packet_from_chan(&mut channel).await {
    if let Err(ret) = sink.feed(first).await {
        tracing::error!(?ret, "do_forward_tunnel_to_nic sink feed error");
        continue; // 保持现有写失败后记录并继续的行为
    }
    let mut count = 1;
    while count < 64 {                           // ← 硬上限, 防止长期不 yield
        match channel.try_recv() {
            Ok(pkt) => {
                if let Err(ret) = sink.feed(pkt).await {
                    tracing::error!(?ret, "do_forward_tunnel_to_nic sink feed error");
                    break;
                }
                count += 1;
            }
            Err(_) => break,
        }
    }
    if let Err(ret) = sink.flush().await {        // N个独立write(), 限N≤64
        tracing::error!(?ret, "do_forward_tunnel_to_nic sink flush error");
    }
    if count == 64 {
        tokio::task::yield_now().await;           // 明确保证高负载下的任务公平性
    }
}
```

**效果**: Linux TUN 的 `flush()` 实际是空操作（字符设备无缓冲）。改成 `feed()` 后仍是 N 次独立 `write()`，差异仅在于减少 Future 轮询、BiLock 获取/释放、tokio readiness 检查的次数。**收益待测，可能是小幅提升而非数量级变化。**

**限流与公平性**: drain 循环上限 64 包；满批后显式 `yield_now()`，不依赖 Tokio cooperative budget 的隐含行为。低流量单包立即 flush，不额外等待定时器。

**错误语义**: 不使用 `?` 隐式结束 NIC writer task。首版保持现有“记录错误并继续”的行为，并测试 converter error、write error 和 flush error 下不重复发送、不越过包边界。若实测持续错误会形成日志风暴，再单独讨论现有错误策略，不夹带在性能改动中。

**预期**: 待压测，不做倍数承诺

### Phase 2: channel 容量独立 A/B 🟢 不与 P1 绑定提交

**文件**: `easytier/src/tunnel/mpsc.rs` line 61 + `easytier/src/peers/mod.rs` line 85
**改动量**: 2 行

```rust
// 当前值（保持不变，实验分支分别调整）:
// mpsc.rs:61:  channel(32)
// peers/mod.rs:85: channel(128)

// 实验时分别测试 32/128/256, A/B 对比决定最终值
```

**注意**: 增大 channel 不会自动减少 syscall，且可能增加突发排队延迟。保持生产默认值不变，分别建立 32/128/256 实验变量。只有吞吐或丢包明确改善、低负载 RTT 与 p99 延迟不回归时才采用新值；否则维持原值。

### Phase 3: TUN 读端 epoll drain 🔴 搁置 — TUN 字符设备无法批处理 syscall，边际收益

**重大修正**: TUN 是字符设备（`/dev/net/tun`），不是 socket。`recvmmsg()` 仅适用于 socket fd，对 TUN fd 会返回 `ENOTSOCK`。**TUN 读端无法批处理 syscall** — 内核 TUN driver 每次 `read()` 只返回一个包，这是字符设备的固有语义。

**候选方向**: 若 profiling 显示 task 调度而非 `read()` syscall 是热点，再为具体 `TunStream` 增加有界的 nonblocking drain API。不能在 `Pin<Box<dyn ZCPacketStream>>` 上假设存在同步 `try_next()`，也不能用 noop waker 轮询造成丢失唤醒。该项必须保持每包一次 read、最多固定批次并显式保证公平性。

**效果边界**: 不减少 syscall 数，只可能减少 task 调度。没有 profiling 证据时不实现。

**预期**: 待测，不承诺倍数

### Phase 0a: 消除 TUN 读写互斥 BiLock 🟡 先 spike 验证收益

**候选问题**: `virtual_nic.rs` 每次 TUN 读/写都先 `poll_lock()`（第 77/212/218/224/234 行）。两个 task 的短暂 poll/syscall 临界区互斥，但锁不跨 `Pending` 持有。它是否构成实际瓶颈必须测量，不能仅由结构推断。

**文件**: `easytier/src/instance/virtual_nic.rs` lines 626, 703 (两处创建点)
**改动量**: ~50 行（含 `dup()` + `from_fd()` + 回退 + `#[cfg]` 分支 × 2）

**候选方案**: profiling 达到门槛后，比较 `Device::split()` 与 `dup + OwnedFd + AsyncFd` 两个 spike。若选择后者，再决定是否需要 Phase -1 的 Linux `raw_fd` 支持。以下仅为候选草图，不是直接实施代码：

```rust
// Current:
let dev = AsyncDevice::new(raw_dev)?;
let (a, b) = BiLock::new(dev);

// After (需 Phase -1: Linux raw_fd 支持):
#[cfg(target_os = "linux")]
{
    let raw_fd = dev.as_raw_fd();
    let dup_fd = unsafe { libc::dup(raw_fd) };
    if dup_fd >= 0 {
        let read_dev = AsyncDevice::new(dev)?;
        let write_dev = AsyncDevice::new(
            tun::create(&Configuration::default().raw_fd(dup_fd))?
        )?;
        // read_dev → TunStream, write_dev → TunAsyncWrite, NO BiLock
    } else {
        // dup() 失败(EMFILE): 退回 BiLock
        let shared = AsyncDevice::new(dev)?;
        let (a, b) = BiLock::new(shared);
    }
}
#[cfg(not(target_os = "linux"))]
{
    // fallback: current BiLock path unchanged
}
```

**若不想改 crate**: 用 `AsyncFd<OwnedFd>` 包装 dup_fd，手写 `AsyncWrite`（只暴露 `poll_write`，不暴露 `poll_write_vectored`；`is_write_vectored() → false`），~80 行绕过 tun crate 写路径。

**预期**: 待 instrumentation 与 perf 联合验证。BiLock 不在 `Pending` 期间持锁，只保护短暂 poll/syscall 临界区。**不能认定 50% 损失或承诺 2x 提升。** 实施时可考虑 `Device::split()` 或 `dup + OwnedFd + AsyncFd` 两条路径。

**`dup()` 失败回退**:
```rust
let raw_fd = dev.as_raw_fd();
let dup_fd = unsafe { libc::dup(raw_fd) };
if dup_fd >= 0 {
    // 成功: 读写各用独立 fd
} else {
    // 失败(EMFILE等): 退回 BiLock, 零功能影响
    tracing::warn!("dup TUN fd failed (errno={}), falling back to BiLock", errno);
    let shared = AsyncDevice::new(dev);
    let (a, b) = BiLock::new(shared);
    // ...
}
```
必须处理并记录 `dup()` 的全部错误，至少包括 `EBADF`、`EMFILE` 和被封装库返回的构造/注册错误；任一步失败都关闭已创建 fd 并回退现有 BiLock，禁止 double-close 或 fd 泄漏。

### Phase 0b: IFF_VNET_HDR + GSO/GRO 🔴 单一独立 spike

**问题**: TUN 创建时没有 `IFF_VNET_HDR` flag，内核无法做 GSO segmentation offload。TUN 字符设备不支持 writev 批处理（会拼接包），GSO 是理论上唯一正确的 TUN 批处理路径。但复杂度极高（需 TCP coalescing engine），独立 spike，不在 P1/P2 阶段排期。

**文件**: `easytier/src/instance/virtual_nic.rs` `create_tun()` (line 506)
**改动量**: 整体 spike ~200+ 行（VNET_HDR flag + TUNSETOFFLOAD + 大包读取 + virtio header 解析 + TCP coalescing engine + 非 GSO 回退 + MTU + 内核版本探测 + Linux 3.10 验证）

**0b.1 + 0b.2 合并为一个独立 spike**:

不能拆开。启用 `IFF_VNET_HDR/TUNSETOFFLOAD` 会：
- 改变双向 TUN 帧格式
- 内核可能向读端返回大于当前 2500 字节缓冲的 GSO packet
- 影响 overlay MTU、分片、Stealth/Secure packet filter

spike 需覆盖：大包读取、virtio header 解析、非 GSO 回退、同流 TCP 合并（需同一 flow、连续 sequence、兼容 TCP options、重构 header/checksum）、MTU 处理、Linux 3.10 验证。

**0b.1 不能先单独合入。**不拆分，不做倍数承诺，不在 P1/P2 阶段排期。

### 本轮 P1/P2 不做的事

- **不改 Tunnel trait** — 太侵入，破坏所有 tunnel 实现
- **不加 timer-based flush** — 引入延迟/复杂度，opportunistic 批处理已足够
- **不改 QUIC 隧道** — 本轮与 NIC P1/P2 解耦；QUIC 是否已实际启用平台 GSO/GRO 另行测量，不在本计划中假定

### Phase 4: UDP/WG 隧道读端 recvmmsg 🔴 独立 spike

**问题**: UDP 和 WireGuard 隧道的读端是逐包 `recv_from().await`，没有任何批处理。这两个传输协议在数据平面上比 TCP 使用更广（UDP 支持 NAT 穿透，WG 支持内核态加密）。

**文件**: `easytier/src/tunnel/udp.rs`（`UdpTunnelListenerData::do_forward_task` line 745）
          `easytier/src/tunnel/wireguard.rs`（`handle_udp_incoming` line ~658）
**实现边界**: Unix Tokio `UdpSocket` 可取得 raw fd。难点是复用现有 Tokio readiness，并确保同一 socket 只有一个 reader；不得为同一 fd重复注册独立 reactor，也不得绕过 readiness 后阻塞 worker。候选实现应使用现有 socket 的 `readable()/try_io`（或等价受支持接口）包裹 nonblocking `recvmmsg`。

**方案**: 在 UDP listener 的读循环中用 `recvmmsg` 替代逐包 `recv_from`。模式与 veth (`linux_veth.rs:1478`) 已有实现相同：

```rust
// UDP tunnel — current (udp.rs do_forward_task):
while let Ok((n, addr)) = socket.recv_buf_from(&mut buf).await { ... }

// UDP tunnel — batched:
let mut msgs = [Default::default(); 32];
if let Ok(n) = recvmmsg(sock_fd, &mut msgs, MSG_DONTWAIT) {
    for i in 0..n {
        // parse, stealth-decrypt, forward each datagram
    }
}
```

**适用**: Linux only (`#[cfg(target_os = "linux")]`)，其他平台回退逐包路径

**预期**: 待 spike 验证，不做倍数承诺

### Phase 5: UDP/WG 隧道写端 sendmmsg 🔴 独立 spike

**修正**: `RingSink` 是内存 ring buffer，不是 socket。`sendmmsg` 应在真正写 UDP socket 的层。

**文件**: `easytier/src/tunnel/udp.rs`（`do_forward_one_packet_to_conn` 及等价写路径）
          `easytier/src/tunnel/wireguard.rs`（WG socket 写路径）
**实现边界**: 与 P4 相同，复用现有 socket readiness，保持每个 datagram 的地址和消息边界；背压、部分批次成功和错误重试不得重复发送。

---

## 覆盖矩阵

P1/P2/P0a 位于 NIC 边界，与底层 tunnel 协议无关；不能按 TCP/UDP/QUIC 列误解为修改了对应协议实现。

| Phase | TUN | veth | no-tun | 实际方向 |
|-------|-----|------|--------|----------|
| P1: bounded feed/flush | ✅ 试验目标；保持一包一 write | ✅ 行为兼容；当前仍逐帧 `sendto`，不承诺收益 | — 不经过虚拟 NIC writer | Peer→NIC |
| P2: channel A/B | ✅ | ✅ | 仅相关 channel 实际被使用时 | 两个 channel 分别测试 |
| P3: bounded read drain | 🟡 独立 spike | — veth已有批量接收 | — | NIC→Peer |
| P0a: BiLock removal | 🟡 profiling 后 spike | — | — | TUN wrapper |
| P0b: VNET_HDR GSO/GRO | 🔴 独立 spike | — | — | Linux TUN 双向 |
| P4/P5: recvmmsg/sendmmsg | 与 NIC backend 无关 | 与 NIC backend 无关 | 与 NIC backend 无关 | UDP/WG socket 边界 |

---

## 影响分析

| 维度 | 评估 |
|------|------|
| **TUN/veth/no-tun** | P1 主要优化 TUN；veth只验证行为兼容，no-tun不声明收益 |
| **Stealth/Secure** | ✅ TunnelFilterChain 在 Sink trait 之上，批处理在其下层，零影响 |
| **跨平台** | Phase 1/2 全平台；Phase 3-5 + P0a/b Linux only (`#[cfg]` 隔离 + 静默 fallback) |
| **延迟** | 见下方延迟分析 |
| **内存** | Phase 2: +32KB×2; Phase 3: +80KB; Phase 4/5: +32KB each |
| **回滚** | 全部是调用模式变化 + cfg 隔离，git revert 即可 |

### 延迟分析

**Phase 1 (feed/flush)**: `try_recv()` 非阻塞 drain，空 channel 立即退出。第一个包到达后最多聚合当前已经排队的 63 个包，不等待新包或定时器；满 64 包后显式 yield。不得预设 `<1μs` 或 `<64μs`，以低负载 RTT、p99 和 task poll 时间实测为准。

**交互流量影响**: SSH/游戏等低速率流量不会累积批次（channel 空 → 1 包即 flush）。批量效果只在高速率下自然发生 — 这正是批处理的理想特性：低负载零延迟，高负载自动聚合。

**Phase 0a (BiLock 消除)**: 只有 profiling 证明存在显著竞争后才试验。延迟和吞吐均以 A/B 为准，不预判一定改善。

**Phase 0b (GSO/GRO)**: 独立 spike 必须定义立即发送阈值和最大聚合时间，并证明低速交互流量不被等待。不得在实现前假定固定 64KB、44 包或 500μs。

## 执行顺序与依赖

```
🟢 第一阶段（不改生产默认）:
建立 perf/strace/吞吐/延迟/CPU 基线 → P1 单变量实验 → P2 两个 channel 分别 A/B

🟡 数据达标后:
只合入通过正确性和性能门槛的变量 → 根据锁竞争数据决定是否做 P0a spike

🔴 搁置, 不排期:
Phase 3 (epoll drain) — TUN字符设备无法批处理syscall
Phase 4/5 (UDP/WG mmsg) — 独立socket readiness spike
Phase 0b (VNET_HDR + GSO/GRO) — 完整独立 spike
```

## 性能推算

### 当前观测

实测 73,500 pps / 0.8 Gbps。尚未完成 perf/strace 和任务级 instrumentation，因此不能把 syscall、BiLock 或 task 切换中的任何一项认定为主瓶颈。

### P1/P2 实测结果 (2026-07-10)

**37↔38 (10G LAN)**:

| 版本 | 吞吐量 | vs 基线 |
|------|--------|---------|
| 基线 | **0.75 Gbps** | — |
| P1 (feed/flush) | 0.64-0.66 Gbps | -12% |
| P2 (channel 256) | 0.71 Gbps | -5% |

**P1 和 P2 均无提升。** 证实了评审判断：`sink.send()` 模式和 channel 容量都不是 0.8 Gbps 瓶颈。代码已全部回退。

### 已完成/剩余

| 项目 | 状态 | 说明 |
|------|------|------|
| P1 feed/flush | ❌ 已证伪 | 无提升, 已回退 |
| P2 channel A/B | ⬜ 可选 | 2行改动, 独立测试; 但P1证伪降低了优先级 |
| profiling 基线 | ⬜ 未做 | perf/strace 仍未跑 |
| P0a BiLock | ⬜ 先要 profiling | 没有数据不能决定 |
| P0b GSO | ⬜ 独立立项 | 复杂度高 |
| P4/P5 UDP/WG | ⬜ 独立立项 | 需绕过 tokio |

## 遗漏检查（修订后）

### ✅ P1+P2 覆盖
- Peer→TUN flush 开销减少 (P1: `feed()+flush()`, 禁用 writev for TUN)
- 两个 channel 容量分别进行A/B (P2: 默认值先保持不变)
- TCP 隧道已有 MpscTunnel + FramedWriter writev (无需改动)

### 🔴 搁置 — 有硬性技术问题
| 项目 | 原因 |
|------|------|
| TUN recvmmsg 读端批处理 | TUN 是字符设备, 不支持 recvmmsg (ENOTSOCK) |
| writev 到 TUN | 会拼接多个 IP 包导致数据损坏 |
| RingSink sendmmsg | RingSink 是内存 buffer 不是 socket, 抽象层错误 |
| GSO/GRO (P0b) | 需完整 virtio header、大包输入、coalescing、MTU与回退语义 |
| UDP/WG recvmmsg/sendmmsg | 需安全接入现有 Tokio readiness 并避免并发 reader/writer |

## Phase 0a/b 风险分析

### P0a: BiLock 消除 — 先 profiling，确认热点再 spike

**现状**: BiLock 不在 `Pending` 期间持锁，只保护短暂 poll/syscall 临界区。**不能认定为主要瓶颈或承诺倍数。**

**执行顺序**:
1. 临时 instrumentation 分别记录读/写 `poll_lock()` 返回 Pending 的次数、等待时长和每包占比；`perf` 只用于确认 CPU 热点，不能替代异步等待统计
2. 若竞争包占比或等待时间不足以解释瓶颈 → 不继续
3. 若确认热点 → 比较 `Device::split()` 与 `dup + AsyncFd<OwnedFd>` spike
4. 双向 iperf3 对比；吞吐收益 <10%，或低负载 RTT/p99、丢包、CPU 任一回归，则不合入

**spike 注意事项**: `dup/raw_fd` 需验证 fd 所有权、关闭语义、packet-information、MTU 和 Tokio reactor 注册，不能定义为"零风险"。

### P0b: IFF_VNET_HDR — TUN GSO/GRO 硬件前提

**机制**: 在 TUN 设备创建时加 `IFF_VNET_HDR` flag，每个包前多 10 字节 `virtio_net_hdr`。需 `TUNSETOFFLOAD` ioctl 启用 offload。

**风险更正**:

| 风险 | 说明 |
|------|------|
| **VNET_HDR 不能单独合入** | 启用后改变双向 TUN 帧格式; `TUNSETOFFLOAD` 可能让读端收到大于当前 2500 字节缓冲的 GSO packet, 必须和读缓冲+大包处理一起 spike |
| **GSO 不是拼 payload** | GSO super-packet 必须是同一 TCP flow、连续 sequence、兼容 TCP options、无重传/乱序, 并重构 IP/TCP header 和 checksum metadata. 本质上是小型 GRO/coalescing engine |

**结论**: VNET_HDR、GSO输入输出和coalescing合并为一个独立 spike，不混入 P1/P2。

### 功能影响总结

| 阶段 | Stealth | Secure | 压缩 | 加密 | ACL | 路由 |
|------|---------|--------|------|------|-----|------|
| P0a BiLock | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| P0b GSO spike | ⚠️ 可能影响包尺寸→overlay MTU→分片→filter | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| P1 feed/flush | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| P2 channel | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| P3-P5 recvmmsg | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

P1/P2 改动在 Sink/Stream 调用层，不接触业务逻辑。P0a/P0b 涉及 TUN 创建参数和 fd 管理，需独立 spike 验证。

## 合入门槛与回归矩阵

每个变量独立提交、独立 A/B、可单独回退。P1 与 P2 不允许在同一性能样本中同时变化。

### 正确性门槛

- TUN 每次底层 write 恰好对应一个完整 IPv4/IPv6 packet；严禁 vectored TUN write。
- 单测用记录型 fake writer 验证64个输入严格产生64次有序单包 write，`poll_write_vectored`调用次数必须为0；1包、63包、64包及跨批65包均覆盖。
- TCP、UDP、ICMP、IPv4/IPv6、MTU边界、分片、广播和组播结果与基线一致。
- Stealth、显式/派生 Secure、压缩、ACL、subnet、exit-node、Magic DNS、public IPv6行为不变。
- TUN、veth、no-tun分别回归；P1在veth/no-tun不宣称性能收益，但不得改变功能。
- 写错误、channel关闭、实例停止和设备重建时无重复包、任务泄漏、FD泄漏或日志风暴。

### 性能门槛

- 同机同拓扑至少重复5轮，报告中位数和离散度，不以单次峰值决策。
- 同时测试单向、双向、单流、多流、小包pps及低速交互流量。
- 记录吞吐、CPU、上下文切换、syscall、channel occupancy/drop、RTT和p99。
- P1只有在吞吐或CPU出现稳定可复现改善，且低负载RTT/p99、丢包和控制面心跳不回归时才合入。
- P2分别测试两个channel；只修改被数据证明是瓶颈的那个容量，另一处保持原值。

### 构建与实机边界

- 不在维护者本机编译。使用 `root@192.168.2.160` 的远端 builder，并遵守项目 `AGENTS.md` 的全核、timeout和cargo pre-flight要求。
- 手工验证只使用debug构建；老Linux主机使用非release musl产物。
- P1/P2验证通过前不修改fork crate、不启用VNET_HDR、不触发正式release workflow。
