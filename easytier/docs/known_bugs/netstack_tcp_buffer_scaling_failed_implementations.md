# [IMPLEMENTATION FAILED] netstack TCP 缓冲缩放与千连接内存优化

**状态：已确认的规模限制；2026-07-18 的所有缓冲优化实现均失败并已回退。**

本文记录一次已经完成实现、远端预检、优化产物构建和压力验证，但最终未能满足语义、
活性或吞吐门槛的 netstack 缓冲优化。这里的失败结论必须保留，避免以后重新引入已经被
实测否决的固定窗口或队列结构。

事件驱动 runner 和 output-channel 公平性修复不属于本失败项。它们已经通过 Linux、
Android 验证并保留。失败的是试图同时减少每连接 TCP/Stream 缓冲、降低千连接虚拟内存
以及保持高吞吐的后续实现。

## 当前保留行为

`third_party/netstack-smoltcp/src/tcp.rs` 当前继续使用原始常量：

```rust
const DEFAULT_TCP_SEND_BUFFER_SIZE: u32 = 0x3FFF * 20;
const DEFAULT_TCP_RECV_BUFFER_SIZE: u32 = 0x3FFF * 20;
```

`0x3fff * 20` 是 `327,660` bytes，约 `320 KiB`。每条连接在建立时分配：

| 层 | 方向 | 容量 |
| --- | --- | ---: |
| smoltcp socket | RX | 327,660 bytes |
| smoltcp socket | TX | 327,660 bytes |
| Leaf Stream staging | recv | 327,660 bytes |
| Leaf Stream staging | send | 327,660 bytes |
| 合计 | 每连接 | 1,310,640 bytes（约 1.25 MiB） |

这个数值是容量和虚拟地址上界，不等于空闲连接的实际常驻内存。验证基线在 1000 个
已建立空闲连接时为 `1,310,816 KiB VmSize / 32,300 KiB RSS`。因此“1000 连接必然消耗
1.25 GiB 物理内存”的原始判断不成立。

当前实现仍有两个已知规模边界：

1. 四份固定 ring 造成较大的虚拟地址预留，并可能在大量活跃连接实际触页后增加 RSS。
2. runner 和 smoltcp `SocketSet` 的集中轮询仍有 O(n) 成分；1000 个空闲连接同时承载
   一条 TCP 流时，验证吞吐从空载约 `868.0 Mbit/s` 降到约 `444.1 Mbit/s`。

这两个边界尚未修复。不得把已经成功的 runner 空闲/关闭修复描述成千连接内存或 CPU
扫描问题的解决方案。

## 失败实现与回退记录

所有下列 Rust 候选均先通过 `192.168.2.160` 的 locked no-run 和对应 focused tests，
然后才使用 GitHub 优化产物验证。单元测试通过不代表性能、活性或内存目标已经成立。

| 候选 | 实现 | 优化产物/压力结果 | 失败原因 | 回退 |
| --- | --- | --- | --- | --- |
| `caf226e1` | smoltcp RX/TX 和 Leaf send/recv 四份 ring 全部固定为 32 KiB，同时保留 output 公平性修复 | Linux `29621525512`，Android `29621525499`；空载中位吞吐 `871.9 -> 402.6 Mbit/s` | `-53.8%` 吞吐回退；发送窗口证据从 `335280` 降到 `43560` bytes；1000 空闲 RSS 反而约增加 18 MiB | `4f9395a9`，随后公平性逻辑单独以 `8201a4a8` 恢复 |
| `0c8894e2` | Leaf 两个固定 ring 改为按需 `VecDeque<Bytes>`，smoltcp 保持原容量 | Linux `29623951590`，Android `29623951560`；1000 空闲降至 `673,940 KiB VmSize / 26,080 KiB RSS` | 空载中位吞吐降至 `518.3 Mbit/s`，相对基线 `-40.3%` | `e26c9815` |
| `6c92be28` | 动态 chunk queue 恢复约 320 KiB 每方向上限 | Linux `29624958371`，Android `29624958377`；中位吞吐 `536.9 Mbit/s` | 仍回退 `38.1%`，证明问题不只是容量，而包括 chunk 分配、释放和 permit 热路径成本 | `728a8679` |
| `271351f0` | Leaf ring 首次活动时整块 lazy allocation，使用有限全局整环预算 | Linux `29626363000`，Android `29626363004`；单流约 `929.6 Mbit/s`，1000 空闲 RSS `24,252 KiB` | 128 并发正向发送超过预算槽位后不能完成，39.69 秒时中止；基线同场景 8.00 秒完成 | `062628ad` |
| `5e6d455b` | 每方向无条件 32 KiB 保底，预算允许时扩容到约 320 KiB；smoltcp RX/TX 固定 128 KiB | Linux `29628076254`，Android `29628076268`；128 正反向均完成 | 解决预算活性但空载中位吞吐仅 `658.5 Mbit/s`，同窗口基线 `918.0 Mbit/s`，回退 `28.3%` | `8d4dc0f8` |
| `78278eb3` | 保留上述 progress-safe Stream 结构，将 smoltcp RX/TX 改为固定 256 KiB | `.160` locked no-run 和 focused tests 通过；未推送、未生成 GitHub 候选 | 在优化产物派发前被设计审计否决：固定 256 KiB 仍是不可证明的高 BDP 上限，继续试 512 KiB/1 MiB 只是移动阈值 | 本地 `225b0a22`；与远端净源码差异为零 |

失败候选的详细逐轮证据保留在
[`leaf_parallel_workboard.md`](../todo/leaf_parallel_workboard.md)。

## 已被证伪的假设

### “本机应用到 TUN 的 RTT 很小，因此 32 KiB 一定足够”

错误。实际路径还包括 Tokio 调度、Leaf Stream staging、smoltcp poll、TUN/bridge 队列和
并发生产者。32 KiB 版本在零重传条件下仍回退 53.8%，表明有效 flight/window 和调度
批量共同限制吞吐，不能只按一段概念上的本机 RTT 推导。

### “把 VmSize 除以连接数就是实际内存成本”

错误。原始大 `vec![0; size]` 在 Linux 验证中大部分是 lazy-backed。32 KiB 版本显著降低
VmSize，却因为 allocator size class/触页行为增加约 18 MiB RSS。后续验收必须同时记录
VmSize、RSS/PSS、峰值触页和内存压力下的实际回收，不能只用理论容量表。

### “按需 `VecDeque<Bytes>` 与固定 ring 等价，只会节省内存”

错误。chunk 分配/释放、引用计数、队列操作和预算 permit 进入了热路径；即使恢复原最大
容量，优化产物仍回退约 38%。

### “全局整环预算只产生正常背压”

错误。如果一个连接必须等待另一个连接释放完整 staging ring 才能首次获得发送空间，
活动流数超过预算槽位时会产生跨连接进度依赖。128 并发测试已经复现不能完成的活性失败。

### “寻找一个更大的固定值即可兼顾所有链路”

错误。TCP 所需窗口由带宽时延积决定。10 Gbit/s 时，理论 BDP 大约为：

| RTT | BDP |
| ---: | ---: |
| 1 ms | 1.25 MB |
| 10 ms | 12.5 MB |
| 50 ms | 62.5 MB |

统一的 128/256/512 KiB 都可能成为单流吞吐上限。即便某个值通过当前局域网，也不能据此
宣称支持 10 Gbit/s 或高 RTT 网关。

## smoltcp API 边界

EasyTier 锁定的 smoltcp revision 是
`0a926767a68bc88d5512afefa7529c5ecdade4ea`。其
`src/socket/tcp.rs::{SocketBuffer,Socket::new}` 使用固定 `RingBuffer`，并在 socket 构造时
根据初始 RX capacity 计算 SYN 阶段的 receive-window scale。当前 API 只有容量查询，没有
在 established socket 上安全替换、增长或收缩底层 storage 的接口。

因此，仅把 Leaf Stream staging 改成动态队列不能解除 smoltcp 的窗口上限。真正的动态
窗口不能通过在外层增加一个全局 semaphore 冒充；它必须同时处理底层存储、窗口协商、
重组、已发送未确认数据和 advertised-window 更新。

## 禁止直接复用的方案

在出现新的架构证据和完整测试前，不得重新提交以下实现：

- 将四份 buffer 统一固定为 16/32/64/128/256/512 KiB。
- 在热路径使用逐 chunk `VecDeque<Bytes>` 加全局 permit，并只用单元测试证明性能。
- 使用有限“完整 ring 数量”作为预算，让新连接等待旧连接释放整个 ring。
- 只优化 Leaf staging，却宣称已经解决 smoltcp 高 BDP 窗口。
- 只比较 VmSize，或只在 1/100/1000 空闲连接下测试而不测试并发活跃流。
- 因为内存理论值直接切换到 lwIP；该替换没有被本轮证据验证。

## 后续可接受的实现方向

### 方向 A：真正的 smoltcp TCP autotuning

这是跨层功能，不是两常量级修复。至少需要：

1. SYN 时按可配置最大窗口能力协商 window scale，而不是由初始物理 allocation 决定。
2. RX/TX 使用可增长、可收缩、最好按页或 segment 懒分配的 storage；增长时不能复制或
   重排已接收、待发送和未确认字节。
3. advertised receive window 必须跟随实际可用容量，并正确处理 zero-window probe、窗口
   更新、SACK/reassembly、FIN 和半关闭。
4. 每连接具有无预算依赖的最小进度保证；额外容量由全局字节预算管理，而不是按完整 ring
   数量管理。
5. 预算归还和唤醒必须避免惊群、丢失唤醒、连接饥饿和一个慢连接长期占用全部内存。
6. 移动端、桌面和网关的最大预算必须可配置；默认值只能是资源保护边界，不能被描述成
   通用带宽窗口。

在动 vendored smoltcp 前，必须重新核对 `Cargo.lock` 的锁定 URL/SHA，并针对该 SHA 设计
补丁。这个方向应单独立项，不能与 runner 生命周期修复合并。

### 方向 B：Linux 网关使用 system TCP stack

如果目标是 Linux 网关长期承载数千个同时活跃的高吞吐连接，内核 TCP stack 更适合连接
查找、定时器、拥塞控制、窗口 autotuning、多核和内存压力回收。Android 等无法使用同一
转发结构的平台可继续保留用户态 stack。

该方向会引入平台分叉、策略路由和生命周期边界，成本明显高于本轮方案，但比继续调整
固定 smoltcp 常量更符合高端网关目标。实现前必须先确定 Linux system-stack 的透明转发
语义以及与 Mihomo/sing-box 的兼容边界。

### 方向 C：暂不优化缓冲，先解决 O(n) 扫描并补内存压力证据

当前 1000 空闲连接 RSS 仍低，可以先保持原 buffer 语义，增加 activity queue/dirty-set
测量，确定二次 `sockets.iter()` 和 smoltcp 自身 poll 各占多少 CPU。只有在真实内存压力
下观察到 RSS/PSS、OOM 或触页峰值问题后，才重新启动 buffer storage 项目。

## 新候选的强制验收矩阵

任何后续缓冲/autotuning 候选至少必须同时通过：

- 1/100/1000/2000 空闲连接：建立成功率、p50/p99、VmSize、RSS/PSS、FD、tasks、空闲 CPU。
- 1/32/128/1000 活跃流：正向和反向完成性、公平性、吞吐、重传、CPU、内存预算饱和。
- 单流同窗口 A/B：中位吞吐不得低于已验证 comparator 的 95%。
- 10 Gbit/s 或等效受控链路，至少覆盖 1/10/50 ms RTT 的 BDP 矩阵；不能只在本机 namespace
  的低 RTT 下验收。
- 预算槽位少于活动流数时，每条流必须取得有界进展；不得依赖另一连接完整关闭或释放整环。
- 关闭风暴、half-close、RST、收到数据后 EOF、发送数据后 FIN、取消、runtime shutdown。
- Android 内存压力、网络切换、反复 Leaf stop/start、旧 TID/FD/task/RSS 回收。
- TCP、UDP、FakeDNS、HEV chain/fallback、QUIC/KCP 和 policy first-match 行为不变。

这些门槛必须使用 `.160` locked no-run/focused tests 作为 pre-push gate，并使用同一精确 SHA
的 Linux/Android 优化产物。任何一个语义、活性、内存或 95% 吞吐门槛失败，都必须 literal
revert，不能通过继续增大固定常量绕过。

## 最终安全状态

- 接受的 netstack runner/fairness snapshot：`8201a4a8270a173949e8fa0cf994ac7328aa46b2`。
- 最终远端安全 snapshot：`6c377afee8303d10f0f5e37f6ab165c97838f156`；其 `device.rs` 和
  `tcp.rs` 与 `8201a4a8` 字节一致。
- 安全回退 Linux workflow `29628775508`：成功。
- 安全回退 Android workflow `29628775467`：成功。
- 本轮没有接受任何 TCP buffer size、动态 Stream queue、全局预算或回收逻辑。

本文件是实现失败记录，不是待合并设计。后续实现不得把这里的任何 rejected candidate
描述为已验证方案。
