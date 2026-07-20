# Leaf policy 数据面性能根因调查

日期：2026-07-19

状态：根因已由同一精确制品的分层实测确认；优化实现尚未开始。

## 1. 结论

Linux 上当前 Leaf policy 的主要性能瓶颈不是 DIRECT、VLESS、TLS 或 WebSocket
actor，而是 EasyTier TUN 与 Leaf worker 之间的逐包数据面：

```text
kernel TUN
  -> EasyTier classifier / ZCPacket
  -> bounded mpsc
  -> Unix datagram（每个 IP 包一次）
  -> Leaf tun::AsyncDevice
  -> netstack-smoltcp
  -> actor
  -> Unix datagram（每个 IP 包一次）
  -> EasyTier ZCPacket
  -> kernel TUN
```

同一 Leaf worker、同一 VLESS 节点和同一 64 MiB HTTP 目标中，仅把运行配置从
EasyTier Unix-datagram bridge 改为 Leaf 自建 Linux TUN，VLESS 中位吞吐由完整
EasyTier 路径的约 277.4 Mbit/s 提升到约 540.0 Mbit/s，达到 sing-box SOCKS
对照 580.5 Mbit/s 的约 93%。DIRECT 从约 285 Mbit/s 提升到约 652.8 Mbit/s。

因此当前约一半吞吐损失发生在 EasyTier/Leaf 的逐包桥接、额外 TUN 往返、内核复制
和跨进程唤醒中。Leaf smoltcp 仍有进一步优化空间，但不是本轮约 2 倍差距的第一根因。

## 2. 精确候选与参考实现

- EasyTier commit：`0cf368072aad4882309e6f6d450e45f5f4e1a9ac`
- Linux profiling workflow：`29651991456`
- Leaf lockfile commit：`36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`
- Leaf：`leaf/src/proxy/tun/inbound.rs::{run,new_smoltcp}`
- EasyTier：
  - `easytier-policy/src/packet.rs::LeafPacketBridge`
  - `easytier/src/instance/virtual_nic.rs::do_forward_nic_to_peers_and_policy`
  - `easytier/src/instance/virtual_nic.rs::do_forward_peers_and_policy_to_nic`
  - `easytier-policy/src/leaf_process.rs::LeafProcessRuntime`
  - `third_party/netstack-smoltcp/src/{device,stack,tcp}.rs`
- sing-box：
  - `/Users/fanli/Documents/singbox-withfallback/option/tun.go::TunInboundOptions`
  - `/Users/fanli/Documents/singbox-withfallback/protocol/tun/inbound.go`
- Mihomo：
  - `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::{New,Listener.Close}`

Mihomo/sing-box 的参考语义是让 TUN stack 显式拥有设备生命周期。EasyTier 还必须保留
mesh 路由和单 VPN ownership，不能直接照搬；Linux policy TUN 则可以保持 Leaf 所有，
EasyTier 只管理路由和生命周期。

## 3. 分层数据

### 3.1 无探针吞吐

| 路径 | 中位吞吐 | 说明 |
|---|---:|---|
| EasyTier 完整 policy DIRECT | 约 285 Mbit/s | 既有同候选控制数据 |
| EasyTier 完整 CDN VLESS | 277.4 Mbit/s | 既有 3 x 64 MiB 精确制品数据 |
| Leaf auto-TUN DIRECT | 652.8 Mbit/s | 6 x 64 MiB，异常低值仍计入中位数 |
| Leaf auto-TUN CDN VLESS | 540.0 Mbit/s | 6 x 64 MiB |
| sing-box SOCKS CDN VLESS | 580.5 Mbit/s | 既有同窗口对照 |
| sing-box gVisor TUN CDN VLESS | 约 210.0 Mbit/s | 8 x 64 MiB；含一次约 92 Mbit/s 低值 |

Leaf auto-TUN DIRECT 六次：

`655.5, 657.5, 650.1, 367.0, 675.0, 634.4 Mbit/s`

Leaf auto-TUN VLESS 六次：

`537.2, 604.7, 543.0, 524.8, 504.9, 542.8 Mbit/s`

### 3.2 CPU 与调度

完整 EasyTier 路径的负载窗口中：

- EasyTier core 约 `51-52% CPU`；
- Leaf worker 约 `40% CPU`；
- 两个 core Tokio worker 与 Leaf worker 合计约 `2.3 万次非自愿切换/秒`；
- user CPU 只占约 10 个百分点，system CPU 约 81 个百分点。

这与“协议加密计算慢”不符，更符合 TUN、Unix datagram、epoll、逐包复制和跨进程唤醒。

Leaf auto-TUN 中：

- DIRECT 384 MiB 使用约 3.22 CPU 秒；
- VLESS 384 MiB 使用约 3.88 CPU 秒；
- VLESS 约为 1.94 CPU 秒/192 MiB，已经明显接近 sing-box，而不是完整路径的多倍开销。

### 3.3 syscall 数量

每个场景的 `strace -f -c` 都只附加指定进程，传输量为 3 x 64 MiB：

| 场景 | core syscalls | worker syscalls | 合计 |
|---|---:|---:|---:|
| EasyTier DIRECT | 895,949 | 379,369 | 1,275,318 |
| EasyTier CDN VLESS | 775,402 | 294,888 | 1,070,290 |
| sing-box SOCKS CDN VLESS | - | 152,781 | 152,781 |

完整 EasyTier 路径约为 sing-box SOCKS 的 7.0 至 8.3 倍 syscall。主要调用为
`write`、`recvfrom`、`read` 与 `epoll_pwait`。core perf 栈直接落在
`LinuxTunOffloadSink::poll_flush_inner`、`tun_chr_write_iter`、netfilter
与用户复制；Leaf worker 还存在 packet-buffer 分配/释放和 smoltcp relay 复制。

### 3.4 协议不是第一根因

- DIRECT 与 VLESS 的 EasyTier core CPU、syscall 数和吞吐上限相近。
- VLESS auto-TUN 已达到 sing-box SOCKS 对照约 93%。
- VLESS worker 中 AES-GCM、TLS、WebSocket 和 VLESS 确实占用 CPU，但远小于移除
  bridge 后释放的总预算。

所以不能通过修改 VLESS、扩大 copy buffer 或调整 KCP/QUIC 来解决这个瓶颈。

## 4. 被废弃或受限的证据

- 旧 `perf_4.19` 的逗号 PID 写法失败；重复 `-p` 又只采最后一个 PID。最终改为
  core-only/worker-only 采样，早期错误结果保留但不引用。
- `perf stat` 与高频 `perf record` 在该单 vCPU 虚拟机上会显著压低吞吐；它们只
  用于调用栈和计数，不用于无探针吞吐中位数。
- 手工创建 TUN 后继承到 FD 3 的实验在约 46 MiB 后停滞，设备参数/唤醒契约不等价，
  已废弃。最终因果数据使用 Leaf 自身 `auto` TUN 创建路径。
- sing-box system TUN 未形成性能证据：未启用 `auto_redirect` 时 0 字节超时；
  启用后该 Linux 4.19 主机因 nftables netlink 能力不足而明确拒绝启动并完成清理。
- sing-box gVisor TUN 是有效 TUN 对照，但不能代表 system/auto-redirect 上限。

## 5. 原始证据目录

验证机原始目录位于 NAS `/slab2`，仓库不保存代理域名、UUID 或密码：

- `.../lv1g2/resource-crosscheck-0cf36807-20260719/`
- `.../lv1g2/layer-profile-v2-0cf36807-20260719/`
- `.../lv1g2/core-profile-0cf36807-20260719/`
- `.../lv1g2/singbox-tun-profile-25a600db-20260719/`
- `.../lv1g2/singbox-system-plain-25a600db-20260719-without-auto-redirect-failed/`
- `.../lv1g2/leaf-direct-tun-profile-0cf36807-20260719/`（废弃隔离）
- `.../lv1g2/leaf-auto-tun-plain-0cf36807-20260719/`（有效因果证据）

有效目录保留候选 SHA、进程 stat/status、原始传输行、perf/strace 输出和清理结果。

## 6. 最小优化方向

### 6.1 Linux：Leaf worker 直接拥有 policy TUN

建议的 Linux fast path：

1. EasyTier mesh TUN 只承载 mesh/Magic DNS 路由，不再接收普通 policy 默认流量。
2. Leaf worker 使用自己的 TUN；EasyTier 只管理确定名称、地址、路由和生命周期。
3. policy routing table 的捕获边界指向 Leaf TUN；Leaf/EasyTier underlay socket 继续
   绑定物理 interface/mark，保持 fail-closed 和防回环。
4. mesh actor 仍通过 `MeshProxyBridgeSet` 与 EasyTier data plane 通信，不修改 mesh、
   KCP、QUIC、smoltcp fallback 或 endpoint selector。
5. 先确认 Leaf TUN ready，再切换 capture route；退出时 TUN、route 和 rule 一并清理。

该路径符合已实测的 540-653 Mbit/s 数据，而且比重写协议 actor、扩大缓冲区或加入
共享内存环更小、更解耦。

### 6.2 Android：不能直接复制双 TUN 方案

Android VpnService 只有一个系统管理的 TUN ownership。第一版应保留单 TUN 和 mesh
classifier，不应为了 Linux 性能引入第二 VPN。后续优化应放在同进程 packet adapter：

- 用有界、可背压的内存 packet channel 代替同进程 Unix datagram；或
- 给 Leaf/tun adapter 增加批量 packet API，减少每包 syscall 与复制。

Android 必须单独验证 DNS/network generation、stop/start、FD/任务回基线和锁屏耗电。

### 6.3 暂不采用

- 不调整 EasyTier mesh 数据面来补偿 policy 性能。
- 不扩大 TCP 窗口或固定占用大内存。
- 不用 KCP/QUIC 开关掩盖 policy TUN 瓶颈。
- 不先做共享内存 ring、io_uring 或跨平台自定义 TUN 框架。

## 7. 候选验收门槛

- policy disabled 与 2.9.10 数据面无可测回退；
- Magic DNS 与 mesh CIDR 始终归 EasyTier TUN；
- DIRECT、REJECT、FakeDNS IPv4/IPv6、native VLESS、mesh actor、chain、fallback；
- underlay 丢失/恢复、worker crash/restart、配置 reload、stop/start；
- TUN、route、rule、FD、线程和临时文件回基线；
- 同一 VPS 中 VLESS 中位吞吐不低于 500 Mbit/s，且不低于 Leaf auto-TUN 的 90%；
- core+worker CPU 秒相对当前完整路径至少下降 30%；
- Linux 快路径不改变 Android 单 TUN 路径，Android 由同候选制品完成功能回归。

在这些条件闭合前，不能把“Leaf actor 已接入”写成“policy 数据面已达到成熟代理核心性能”。
