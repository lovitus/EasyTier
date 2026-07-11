# QUIC 传输参数调优分析

本文档分析 EasyTier QUIC 传输层的当前参数配置，评估其激进度，并提供调优建议供后续修改或高阶用户参考。

## 1. 当前配置

源码位置：`easytier/src/tunnel/quic.rs` `transport_config()` 函数。

### 1.1 EasyTier 显式设置的参数

| 参数 | 设置值 | quinn 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_concurrent_bidi_streams` | 255 (`u8::MAX`) | 100 | 最大并发双向流 |
| `max_concurrent_uni_streams` | 0 | 100 | 禁用单向流（EasyTier 只用双向流） |
| `keep_alive_interval` | 5s | None | 保活间隔 |
| `initial_mtu` | 1200 | 1200 | 初始 MTU |
| `min_mtu` | 1200 | 1200 | 最小 MTU |
| `enable_segmentation_offload` | true | true | GSO 分段卸载 |
| `congestion_controller_factory` | `BbrConfig::default()` | `CubicConfig::default()` | BBR 拥塞控制 |

### 1.2 未设置（使用 quinn 默认值）的关键参数

| 参数 | quinn 默认值 | 设计依据 | 说明 |
| --- | --- | --- | --- |
| `stream_receive_window` | 1,250,000 bytes | 12.5 MB/s × 100ms | 单流接收窗口，按 100 Mbps / 100 ms RTT 设计 |
| `receive_window` | `VarInt::MAX` (≈18 EB) | 无上限 | 连接级接收窗口 |
| `send_window` | 10,000,000 bytes (10 MB) | 8 × stream_window | 发送窗口 |
| `initial_rtt` | 333 ms | RFC 9308 建议 | 初始 RTT 估计值 |
| `max_idle_timeout` | 30s | RFC 9308 §3.2 | 空闲超时 |
| `crypto_buffer_size` | 16 KB | — | crypto 层缓冲区 |
| `datagram_receive_buffer_size` | 1,250,000 bytes | 同 stream_window | datagram 接收缓冲 |
| `datagram_send_buffer_size` | 1 MB | — | datagram 发送缓冲 |
| `mtu_discovery_config` | 默认启用, upper_bound=1452 | RFC 8899 | MTU 探测配置 |
| `packet_threshold` | 3 | RFC 5681 | FACK 丢包检测阈值 |
| `time_threshold` | 9/8 = 1.125 | RFC 9308 | 时间域丢包检测阈值 |
| `persistent_congestion_threshold` | 3 | RFC 9308 | 持续拥塞阈值 |
| `allow_spin` | true | — | QUIC spin bit |

### 1.3 BBR 拥塞控制参数（`BbrConfig::default()`）

quinn-proto 0.11.12 的 BBR 实现基于 Google QUICHE 的 BBR sender。

| 参数 | 值 | 说明 |
| --- | --- | --- |
| `initial_window` | 200 × 1200 = 240,000 bytes (240 KB) | 初始拥塞窗口，远大于 CUBIC 的 ~14 KB |
| `K_DEFAULT_HIGH_GAIN` | 2.885 (= 2/ln2) | Startup 阶段 pacing & cwnd 增益 |
| `K_DERIVED_HIGH_CWNDGAIN` | 2.0 | Startup cwnd 增益 |
| `K_PACING_GAIN` | [1.25, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0] | ProbeBw 阶段 8 轮增益循环 |
| `K_STARTUP_GROWTH_TARGET` | 1.25 | Startup 退出带宽增长阈值 |
| `K_ROUND_TRIPS_WITHOUT_GROWTH_BEFORE_EXITING_STARTUP` | 3 | Startup 退出所需无增长 RTT 数 |
| `K_MAX_INITIAL_CONGESTION_WINDOW` | 200 packets | 初始拥塞窗口上限 |
| `PROBE_RTT_BASED_ON_BDP` | true | ProbeRtt 基于 BDP |
| `DRAIN_TO_TARGET` | true | Drain 到目标 |

BBR 状态机：Startup → Drain → ProbeBw (循环) → ProbeRtt (周期性)。

## 2. 传输模型：Stream 而非 Datagram

EasyTier QUIC tunnel 使用 **单个双向 stream**（`open_bi()` / `accept_bi()`）传输所有 tunnel 数据，上层是 `FramedReader` / `FramedWriter` 做长度分帧。没有使用 QUIC datagram。

这意味着所有数据都走 QUIC stream 的流控路径，受 `stream_receive_window` 和 `send_window` 约束。如果使用 datagram，则不受 stream flow control 限制（但会受 datagram buffer 大小和拥塞窗口约束）。

## 3. 评估：参数足够激进吗？

### 3.1 拥塞控制：BBR — 激进 ✓

BBR 是基于带宽延迟积（BDP）的拥塞控制，不依赖丢包信号。相比默认的 CUBIC：

- **不因随机丢包大幅降速**：CUBIC 遇到丢包会将窗口减半，BBR 只在 ProbeRtt 阶段短暂降速。
- **Startup 阶段非常激进**：gain=2.885，初始窗口 240 KB，会快速探测可用带宽。
- **ProbeBw 阶段周期性加速**：25% 的 pacing gain 轮次会主动探测更多带宽。

对于跨境高延迟链路（如 170 ms RTT），BBR 显著优于 CUBIC。

### 3.2 流控窗口：硬上限，不是斜率问题 ⚠️

**关键区别**：流控窗口不是影响"加速快慢"的参数，而是**吞吐量天花板**。

QUIC stream 流控机制：接收方通告 `max_stream_data`，发送方在途数据不能超过此限制。接收方消费数据后发 MAX_STREAM_DATA 帧增加额度，但新额度 ≈ `已确认偏移 + stream_receive_window`。因此任何时刻在途数据量被硬限制在 ~`stream_receive_window`。

**最大吞吐量 ≈ stream_receive_window / RTT**

quinn 默认值按 **100 Mbps / 100 ms RTT** 设计：

- `stream_receive_window` = 1.25 MB
- `send_window` = 10 MB

各场景下的吞吐量天花板：

| 链路 | RTT | BDP | 理论上限 | stream_window 天花板 | send_window 天花板 | 实际天花板 | 能否跑满 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 100 Mbps LAN | 1 ms | 12.5 KB | 100 Mbps | 10 Gbps | 80 Gbps | 100 Mbps | ✓ |
| 100 Mbps 跨境 | 170 ms | 2.1 MB | 100 Mbps | **59 Mbps** | 470 Mbps | **59 Mbps** | ❌ 59% |
| 500 Mbps 跨境 | 170 ms | 10.6 MB | 500 Mbps | **59 Mbps** | 470 Mbps | **59 Mbps** | ❌ 12% |
| 1 Gbps 跨境 | 170 ms | 21 MB | 1 Gbps | **59 Mbps** | **470 Mbps** | **59 Mbps** | ❌ 6% |
| 1 Gbps LAN | 1 ms | 125 KB | 1 Gbps | 10 Gbps | 80 Gbps | 1 Gbps | ✓ |

**结论**：对于跨境高延迟链路（RTT ≥ 100ms），`stream_receive_window` = 1.25 MB 会把吞吐量硬限制在 ~59 Mbps，无论 BBR 怎么探测带宽。这不是"曲线平缓"的问题，而是**天花板**。BBR 解决了"丢包不降速"的问题，但流控窗口的瓶颈是另一个维度。

### 3.3 MTU：可优化 ⚠️

`initial_mtu=1200, min_mtu=1200`，MTU discovery 默认启用（upper_bound=1452）。

- 初始阶段每个包有效载荷约 1170 bytes，效率 ~97.5%。
- MTU discovery 会逐渐探测到 1452，但需要时间。
- 可以设 `initial_mtu=1452` 来避免初始阶段的低效，但需确认网络路径支持。

### 3.4 初始 RTT：影响首包延迟和收敛速度 ⚠️

`initial_rtt=333ms` 是 RFC 9308 建议值。

BBR 用 `initial_rtt` 计算初始 pacing rate = `initial_window / initial_rtt`。

- **当前**：240 KB / 333ms = **720 KB/s ≈ 5.7 Mbps** 初始发送速率。
- **设为 50ms**：240 KB / 50ms = **4.8 MB/s ≈ 38 Mbps** 初始发送速率。

**LAN 场景**（RTT < 1ms）：333ms 导致初始 pacing 极慢，BBR 需要几个 RTT 才能收敛。首包延迟和初始吞吐都受影响。

**跨境场景**（RTT ~170ms）：333ms 偏高但差距不大，收敛较快。

### 3.5 发包延迟分析

BBR 的 pacing 机制会均匀间隔发包以避免突发，这本身引入少量延迟（每个包之间间隔 pacing_time）。这是 BBR 的设计权衡——用 pacing 换取低丢包和稳定吞吐。quinn 的 BBR 实现没有暴露关闭 pacing 的选项。

可调的延迟相关参数：

| 参数 | 当前值 | 影响 | 建议 |
| --- | --- | --- | --- |
| `initial_rtt` | 333 ms | 初始 pacing rate 偏低，首包延迟高 | 降至 50-100 ms |
| `initial_mtu` | 1200 | 小包更多系统调用，更高每包延迟 | 提高到 1452 |
| `keep_alive_interval` | 5s | 合理，不引入额外延迟 | 保持 |
| `enable_segmentation_offload` | true | GSO 批量发送，微秒级延迟但减少 syscall | 保持 |

## 4. 当前配置是偷懒还是有其它考虑？

从代码分析，开发者**有意选择了 BBR**（性能意识明确），设置了 streams/keepalive/MTU 等关键参数。但**流控窗口完全使用 quinn 默认值**，没有针对 EasyTier 的典型使用场景（跨境 VPN tunnel）调优。

quinn 文档明确写了默认值是 "tuned for a 100Mbps link with a 100ms round trip time"。对于一个 VPN 工具，尤其是经常用于跨境链路的场景，这个默认值明显偏低。

**判断**：不是偷懒，更像是**遗漏**。开发者关注了拥塞控制（BBR vs CUBIC）这个最显眼的性能参数，但没有深入到流控窗口这一层。BBR 解决了"丢包不降速"的问题，但流控窗口的瓶颈是另一维度的问题，两者独立。

## 5. Hysteria2 参数对比佐证

Hysteria2 是业界知名的基于 QUIC 的高性能代理工具，专为高延迟跨境链路优化。其 QUIC 参数配置可以作为业界最佳实践的参考。

### 5.1 Hysteria2 默认参数（源码验证）

源码位置：`apernet/hysteria core/server/config.go`

```go
const (
    defaultStreamReceiveWindow = 8388608 // 8MB
    defaultConnReceiveWindow   = defaultStreamReceiveWindow * 5 / 2 // 20MB
    defaultMaxIdleTimeout      = 30 * time.Second
    defaultMaxIncomingStreams  = 1024
)
```

Hysteria2 `quic.Config` 完整配置：

```go
quicConfig := &quic.Config{
    InitialStreamReceiveWindow:     8388608,  // 8 MB
    MaxStreamReceiveWindow:         8388608,  // 8 MB
    InitialConnectionReceiveWindow: 20971520, // 20 MB
    MaxConnectionReceiveWindow:     20971520, // 20 MB
    MaxIdleTimeout:                 30 * time.Second,
    MaxIncomingStreams:             1024,
    DisablePathMTUDiscovery:        false,    // 启用 MTU discovery
    EnableDatagrams:                true,     // 启用 datagram
    MaxDatagramFrameSize:           protocol.MaxDatagramFrameSize,
    DisablePathManager:             true,
}
```

### 5.2 参数对比

| 参数 | Hysteria2 默认 | EasyTier 当前 | 我的推荐 | 佐证 |
| --- | --- | --- | --- | --- |
| `stream_receive_window` | **8 MB** | 1.25 MB (quinn 默认) | **8 MB** | ✅ 完全一致 |
| `conn_receive_window` | **20 MB** (stream × 2.5) | `VarInt::MAX` (无上限) | — | EasyTier 已更激进 |
| `send_window` | 未单独设置 (quic-go 无此参数) | 10 MB (quinn 默认) | **64 MB** | Hysteria2 由 conn window 20MB 间接限制 |
| `max_idle_timeout` | 30s | 30s (quinn 默认) | 30s | ✅ 一致 |
| `max_incoming_streams` | 1024 | 255 | 255 | Hysteria2 更高，但 EasyTier 用单 stream |
| `keep_alive` | 默认启用 | 5s | 5s | ✅ 合理 |
| `path_MTU_discovery` | 启用 | 启用 | 启用 | ✅ 一致 |
| 拥塞控制 | BBR / Brutal | BBR | BBR | ✅ 一致 |
| `initial_rtt` | 未设置 (quic-go 默认 333ms) | 333ms (quinn 默认) | **50 ms** | 我的建议比 Hysteria2 更激进 |
| `initial_mtu` | 未设置 (默认 1200) | 1200 | **1452** | 我的建议比 Hysteria2 更激进 |

### 5.3 关键结论

1. **`stream_receive_window` = 8 MB 完全佐证**：Hysteria2 作为业界领先的 QUIC 代理工具，默认值就是 8 MB。EasyTier 当前使用 quinn 默认的 1.25 MB，仅为 Hysteria2 的 1/6.4。这是最关键的差距。

2. **stream:conn 窗口比例 2:5**：Hysteria2 文档建议保持 stream receive window 与 connection receive window 的比例接近 2/5，防止单个 stream 占满连接。EasyTier 的 `receive_window` = `VarInt::MAX`（无上限），不存在此问题。

3. **Hysteria2 使用 datagram + stream 双通道**：Hysteria2 启用了 `EnableDatagrams: true`，UDP 流量走 datagram 不受 stream flow control 限制，TCP 代理走 stream 受 8 MB 窗口限制。EasyTier 只用 stream，所有流量都受 stream window 限制，因此调大 stream window 更重要。

4. **Hysteria2 有 Brutal 拥塞控制**：除了 BBR，Hysteria2 还有自研的 Brutal 算法——固定速率，不因丢包降速，通过过量发送补偿丢包。但需要用户准确指定带宽上限，否则适得其反。EasyTier 的 BBR 是更安全的选择。

5. **`initial_rtt` 和 `initial_mtu` 我的建议更激进**：Hysteria2 没有特别调优这两个参数（使用 quic-go 默认值 333ms 和 1200）。我的建议（50ms 和 1452）比 Hysteria2 更激进，适合 EasyTier 的 LAN + 跨境混合场景。

6. **Hysteria2 文档原文**："We strongly recommend that you maintain a stream-to-connection receive window ratio close to 2/5" 和 "We do not recommend changing these values unless you fully understand what you are doing"——说明 8 MB / 20 MB 是经过充分测试的最佳值。

## 6. 推荐配置

### 6.1 推荐修改（代码 patch）

```rust
pub fn transport_config() -> Arc<TransportConfig> {
    let mut config = TransportConfig::default();

    config
        .max_concurrent_bidi_streams(u8::MAX.into())
        .max_concurrent_uni_streams(0u8.into())
        .keep_alive_interval(Some(Duration::from_secs(5)))
        .initial_mtu(1452)
        .min_mtu(1200)
        .enable_segmentation_offload(true)
        .congestion_controller_factory(Arc::new(BbrConfig::default()))
        // 流控窗口：支持 1 Gbps × 170ms 跨境链路
        .stream_receive_window(VarInt::from_u32(8_000_000))
        .send_window(64_000_000)
        // datagram 缓冲（虽然当前用 stream，保留以备未来使用）
        .datagram_receive_buffer_size(Some(8_000_000))
        .datagram_send_buffer_size(8_000_000)
        // 更激进的初始 RTT，加快收敛
        .initial_rtt(Duration::from_millis(50));

    Arc::new(config)
}
```

需要新增 import：

```rust
use quinn::VarInt;
```

### 6.2 参数调优说明

| 参数 | 当前值 | 建议值 | 理由 | 风险 |
| --- | --- | --- | --- | --- |
| `initial_mtu` | 1200 | **1452** | 避免初始阶段低效，减少包数量 ~17% | 部分网络路径 MTU < 1452 会丢包，但有 black hole detection fallback 到 min_mtu=1200 |
| `stream_receive_window` | 1.25 MB (默认) | **8 MB** | 跨境 170ms 天花板从 59 Mbps 提升到 376 Mbps | 内存消耗 +6.75 MB/连接 |
| `send_window` | 10 MB (默认) | **64 MB** | 跨境 170ms 天花板从 470 Mbps 提升到 3 Gbps | 内存消耗 +54 MB/连接 |
| `datagram_receive_buffer_size` | 1.25 MB (默认) | **8 MB** | 匹配 stream_receive_window，备未来 datagram 使用 | 内存消耗 +6.75 MB/连接 |
| `datagram_send_buffer_size` | 1 MB (默认) | **8 MB** | 匹配接收侧 | 内存消耗 +7 MB/连接 |
| `initial_rtt` | 333 ms (默认) | **50 ms** | 初始 pacing rate 从 5.7 Mbps 提升到 38 Mbps，加快收敛 | 跨境场景可能偏低，但 BBR 会在 1-2 个 RTT 内修正 |
| `min_mtu` | 1200 | **1200** | 保持不变，作为 black hole fallback | — |

### 6.3 推荐配置的吞吐量天花板

| 链路 | RTT | 当前天花板 | 推荐天花板 | 提升 |
| --- | --- | --- | --- | --- |
| 100 Mbps 跨境 | 170 ms | 59 Mbps | 376 Mbps | 6.4x（不再受限） |
| 500 Mbps 跨境 | 170 ms | 59 Mbps | 376 Mbps | 6.4x（仍受 stream_window 限制） |
| 1 Gbps 跨境 | 170 ms | 59 Mbps | 376 Mbps | 6.4x（需更大窗口才能跑满） |
| 100 Mbps LAN | 1 ms | 10 Gbps | 80 Gbps | 无影响 |

> 注：要支持 1 Gbps × 170ms = 21 MB BDP，`stream_receive_window` 需 ≥ 21 MB。8 MB 支持 ~376 Mbps。如需千兆跨境，可设 32 MB，但内存消耗更大。

### 6.4 内存消耗估算

每连接内存增量（相比当前）：

| 参数 | 当前 | 推荐 | 增量 |
| --- | --- | --- | --- |
| `stream_receive_window` | 1.25 MB | 8 MB | +6.75 MB |
| `send_window` | 10 MB | 64 MB | +54 MB |
| `datagram_receive_buffer_size` | 1.25 MB | 8 MB | +6.75 MB |
| `datagram_send_buffer_size` | 1 MB | 8 MB | +7 MB |
| **合计** | | | **~74.5 MB/连接** |

场景评估：

| 场景 | 连接数 | 内存增量 | 可接受？ |
| --- | --- | --- | --- |
| 个人使用（2-5 peer） | 2-5 | 150-373 MB | ✓ |
| 小型中继（10-20 peer） | 10-20 | 745 MB-1.5 GB | ⚠️ 需评估 |
| 大型中继（50+ peer） | 50+ | 3.7 GB+ | ❌ 需更保守配置 |

对于中继节点等大连接数场景，可以降低窗口大小（如 `stream_receive_window` = 4 MB, `send_window` = 32 MB），在内存和性能之间取平衡。

### 6.5 分级配置建议

| 场景 | stream_receive_window | send_window | initial_rtt | initial_mtu | 目标 |
| --- | --- | --- | --- | --- | --- |
| **默认（推荐）** | 8 MB | 64 MB | 50 ms | 1452 | 平衡性能与内存 |
| **高带宽跨境** | 32 MB | 128 MB | 50 ms | 1452 | 1 Gbps+ 跨境链路 |
| **中继节点** | 4 MB | 32 MB | 100 ms | 1452 | 大连接数，控制内存 |
| **LAN 直连** | 2 MB | 16 MB | 10 ms | 1452 | 低延迟 LAN |

### 6.6 BBR 自身调优（可选）

当前使用 `BbrConfig::default()`，只有一个可调参数 `initial_window`：

```rust
let mut bbr = BbrConfig::default();
bbr.initial_window(400 * 1200); // 480 KB，更激进的初始窗口
```

默认 200 packets (240 KB) 已经相当激进，一般不需要调整。

## 7. 总结

| 方面 | 当前状态 | 推荐修改 | 评级 |
| --- | --- | --- | --- |
| 拥塞控制 | BBR | 保持 | 激进 ✓ |
| 初始拥塞窗口 | 240 KB (200 packets) | 保持 | 激进 ✓ |
| 发送窗口 | 10 MB (quinn 默认) | → 64 MB | 当前中等，推荐激进 |
| 流接收窗口 | 1.25 MB (quinn 默认) | → 8 MB | 当前保守（跨境 59 Mbps 天花板），推荐提升 |
| MTU | 1200 → 1452 探测 | → 1452 初始 | 可优化 |
| 初始 RTT | 333 ms | → 50 ms | 偏保守，推荐降低 |
| 并发流 | 255 双向 / 0 单向 | 保持 | 合理 ✓ |
| 保活 | 5s | 保持 | 合理 ✓ |
| GSO | 开启 | 保持 | 合理 ✓ |

**核心结论**：

1. **BBR 拥塞控制本身足够激进**，不因丢包降速。
2. **流控窗口是硬天花板，不是斜率问题**。当前 `stream_receive_window` = 1.25 MB 把跨境吞吐限制在 ~59 Mbps，调大到 8 MB 可提升到 ~376 Mbps。
3. **`initial_rtt` = 333ms 导致初始 pacing rate 仅 5.7 Mbps**，降至 50ms 可提升到 38 Mbps，加快收敛。
4. **当前配置是遗漏而非偷懒**——开发者关注了 BBR 但未调优流控窗口。
5. **推荐配置增加 ~74.5 MB/连接内存**，对个人使用可接受，中继节点需权衡。
