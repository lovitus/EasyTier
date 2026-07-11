# Linux TUN GSO 根因修复计划

## 状态

- profiling commit: `784659e8`
- 记录日期: 2026-07-11
- 结论: 待独立 spike 验证，不直接进入生产路径

## Profiling 证据

测试使用同一份 GitHub profiling beta x86_64-musl 二进制、Linux 3.10 两节点、
TCP underlay、TCP Stealth、单流 iperf3。`perf record -a -F 499 -g` 覆盖全部线程，
两端各约 29.9K 样本且没有 lost sample。

| 指标 | 接收端 A | 发送端 B |
|------|---------:|---------:|
| perf 下吞吐 | 1.02 Gbps | 1.02 Gbps |
| EasyTier 用户态 DSO | 21.1% 系统总 CPU 样本 | 22.2% 系统总 CPU 样本 |
| Tokio worker（含其内核执行） | 45.4% 系统总 CPU 样本 | 43.3% 系统总 CPU 样本 |
| idle (`swapper`) | 45.2% | 54.4% |
| 两条主要 Tokio worker | 各约 52--71% CPU | 各约 52--71% CPU |
| AES-GCM | 3.58% | 3.00% |
| syscall entry | 2.83% | 2.74% |

AES-GCM 包含 shared-secret `RingCipher` 与 Stealth outer AEAD。两层合计只有约
3--3.6%，不是数量级吞吐差距的根因。`BiLock::poll_lock`、SOCKS fast gate、统计
clone 和时间戳单项均不足 1%。

在另一轮 15 秒、约 0.99 Gbps 的同构测试中，按 EasyTier 全线程统计：

| syscall | A | B |
|---------|--:|--:|
| `read` | 413,501 | 1,196,155 |
| `write` | 1,288,504 | 520,773 |
| `writev` | 98,083 | 310,471 |
| `recvfrom` | 444,741 | 29,686 |
| context switches | 81,301 | 135,634 |

计数量级与 MTU 分片后的 L3 packet 数一致。外层 TCP 已通过 `writev` 和 stream read
自然合并部分 packet；最大计数仍来自 TUN 的逐包 read/write。当前数据面以约
8--10 万 packets/s 处理 1 Gbps，每个 packet 都要单独跨越 TUN、异步任务、路由、
统计、加密和 tunnel framing。

因此根因是 **MTU 粒度逐包处理的固定成本**，不是某个加密算法、channel 容量、
`feed/flush` 调用方式或单一锁。

## 最小根治边界

新增 Linux-only 私有模块：

```text
instance/linux_tun_offload.rs
```

模块只适配现有 `ZCPacketStream + ZCPacketSink` 与 Linux TUN fd：

```text
Linux TUN + virtio_net_hdr
        ↕
LinuxTunOffloadAdapter
        ↕ normal MTU-sized ZCPacket
现有 VirtualNic / PeerManager / TunnelFilterChain
```

固定不修改：

- PeerManager 路由、ACL、压缩和 shared-secret encryption 语义。
- Stealth、Secure、Proxy、SOCKS、KCP/QUIC failover 和协议优先级。
- overlay wire format、protobuf、RPC、GUI 和 mixed-version 互通。
- veth、no-tun、Windows、macOS、移动端路径。

## Spike 实现

### 1. 创建与能力探测

- 仅 Linux TUN 尝试 `IFF_VNET_HDR`。
- 设置并回读 `TUNSETOFFLOAD` 的 `TUN_F_CSUM`、`TUN_F_TSO4`、`TUN_F_TSO6`。
- 在接口 UP 和地址/路由配置前完成探测。
- 任一步不支持或验证失败时，关闭该 fd并用当前参数重新创建 legacy TUN。
- resolved backend 一旦 ready 不在运行期切换，避免丢包和路由状态漂移。

对 `tun-easytier` 的修改限制为 Linux flag/offload/raw-fd 所需的最小 API，并作为独立
commit；所有 coalescing/segmentation 逻辑留在 EasyTier 私有模块。

### 2. TUN 读取方向

- 固定最大 64 KiB frame buffer，先解析 `virtio_net_hdr`。
- 普通 IPv4/IPv6 frame 去掉 header 后按现有 `ZCPacket` 输出。
- GSO frame 在 adapter 内按 `gso_type/gso_size/hdr_len` 安全分段为普通 MTU-sized
  `ZCPacket`，再交给现有 PeerManager。
- 分段结果使用固定容量 `VecDeque`/small storage；设置严格的 segment 数、总长度和
  header 深度上限。
- malformed、未知 GSO、checksum 状态不一致时丢弃并计数，不允许巨帧进入现有数据面。

该方向把多个内核 TCP segment 合并为一次 TUN read；PeerManager 仍逐 packet 工作，
因此不改变 ACL、路由、计数或 wire 语义。

### 3. TUN 写入方向

- `do_forward_peers_to_nic()` 只做一个小改动：从现有 channel opportunistic drain
  最多 64 个已经排队的 packet，用 `feed()` 交给 offload sink 后执行一次 `flush()`。
- 不等待新 packet，不增加 timer；低流量时第一个 packet立即写出。
- offload sink 只合并同一 TCP flow、连续 sequence、相同 IP/TCP header 形态且 options
  兼容的 packet。
- SYN、FIN、RST、fragment、IP options、不兼容 TCP options、乱序、重传、非 TCP、
  multicast/broadcast 均直接按普通 frame 写入。
- 合并成功时构造一个 `virtio_net_hdr` GSO frame，由 TUN 内核分段；不成功时每个 packet
  前加零 GSO header并维持当前逐包 write。
- 最大 GSO frame 64 KiB、最大 64 segments；所有长度计算 checked，checksum 明确按
  `NEEDS_CSUM/csum_start/csum_offset` 契约生成。

之前单独测试的 `feed()+flush()` 没有收益，因为仍然执行 N 次 TUN write。本方案只有在
offload sink 真正把 N 个 packet 合成一个 GSO write 时才启用 bounded drain，不重复旧实验。

### 4. 状态与生命周期

- coalescer 只持有当前 opportunistic batch，不维护跨 await、跨 timer 或长期 per-flow 状态。
- 正常内存上限固定为 `64 * MTU + 64 KiB scratch`，实例退出立即释放。
- write error、partial write、接口 down、MTU 变化时丢弃当前 batch并返回现有错误路径，
  不重发已经可能进入内核的 frame。
- DHCP/TUN 重建重新探测能力，不继承旧 fd 的 offload 状态。

## 不作为根治方案的项目

- 调整 channel 32/128/256：已经证伪，不减少 packet 或 syscall 数。
- 单独 `feed()+flush()`：已经证伪，不减少 TUN write 数。
- TUN `writev`：多个 IP packet 会被当成一个损坏 frame。
- TUN `recvmmsg`：字符设备返回 `ENOTSOCK`。
- 优化 AES/HMAC：当前 AEAD 只占约 3%，收益上限太低。
- 删除 SOCKS filter、统计或时间戳：可作为后续小优化，总收益预计只有几个百分点。
- 直接引入 packet batch wire format：会扩大 mixed-version、安全和维护边界；只有 GSO
  spike 仍不足时才另立协议级项目。

## 后续可选扩展

GSO spike 达标后，可独立增加 Linux TUN multiqueue，以提升多流 aggregate throughput。
multiqueue 必须按 flow hash 固定队列保证有序；它不会改善单流上限，不能替代 GSO。

UDP/WG 的 `recvmmsg/sendmmsg` 是 tunnel socket 边界的独立优化，只改善对应 datagram
underlay，不解决 TCP underlay 和 TUN 的共同逐包瓶颈。

## 验收门槛

### 正确性

- IPv4/IPv6 TCP、UDP、ICMP、fragment、MTU 边界、TCP options、TFO、重传、乱序。
- subnet、exit-node、Magic DNS、public IPv6、Proxy、SOCKS、KCP/QUIC 和 Stealth/Secure。
- packet capture 对比 legacy/offload，应用 payload 与 TCP sequence 完全一致。
- 不支持 offload、ioctl 失败、malformed GSO、接口重建时可靠回退 legacy TUN。
- Linux 3.10 和现代 Linux 均实测；其他平台 build matrix 不出现代码路径变化。

### 性能

- 同一 GitHub profiling artifact、相同拓扑至少五轮。
- 单流 TCP Stealth、Plain、UDP/QUIC underlay分别记录 throughput、pps、CPU/Gbit、
  syscall/Gbit、context switches/Gbit、RTT p50/p99 和 retransmits。
- 合入门槛：TCP 单流中位吞吐至少提升 20%，或同吞吐 EasyTier CPU/Gbit 至少下降 25%；
  syscall/Gbit 必须显著下降。
- 低负载 ping/SSH p99 不得回归 1 ms 以上；不允许以延迟换吞吐。
- 若达不到门槛，删除 spike，不把复杂度留在生产代码。

## 提交顺序

1. `tun-easytier`: 最小 Linux VNET_HDR/offload API。
2. `linux_tun_offload`: parser、segmenter、coalescer 纯单元测试，不接生产。
3. `VirtualNic`: runtime probe、legacy fallback和 bounded opportunistic drain。
4. GitHub profiling beta + 两节点实机 A/B。
5. 达标后再评审是否默认启用；未达标整体 revert。
