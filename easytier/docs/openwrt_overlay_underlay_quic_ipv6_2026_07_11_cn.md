# OpenWrt Overlay/Underlay 性能验证与 QUIC IPv6 Listener 审计

[English](openwrt_overlay_underlay_quic_ipv6_2026_07_11.md) | [中文](openwrt_overlay_underlay_quic_ipv6_2026_07_11_cn.md)

日期：2026-07-11

## 范围

本报告比较一台 OpenWrt x86_64 路由器经 EasyTier 访问两个远端节点时的性能，以及对应
公网路径的性能；同时审计默认 QUIC listener 为什么只有 IPv4，以及安全地默认启用 IPv6
需要满足哪些条件。

报告有意省略公网主机名和公网地址。两个目标仅使用 EasyTier 虚拟地址标识：

- 目标 A：`10.44.0.8`
- 目标 B：`10.44.0.12`

路由器和两个目标在测试前均已运行 EasyTier。本次没有重启或修改任何 EasyTier 实例。
临时 iperf3 listener 使用独立测试端口，测试后已经停止；目标 A 上临时添加的 IPv6 UDP
防火墙规则也已删除。

## 拓扑与状态

路由器运行 Linux 5.4/OpenWrt 和 EasyTier `2.6.9-7080ecea`。测试时两个目标也报告
`2.6.9-7080ecea`。

负载测试前：

| 目标 | 路由 | Peer 延迟 | Peer 丢包 | 存活传输 |
| --- | --- | ---: | ---: | --- |
| A | DIRECT，路径长度 1 | 约 `144 ms` | `28%` | `udp6,ws6,tcp6` |
| B | DIRECT，路径长度 1 | 约 `180 ms` | `0%` | `tcp6,wg6,ws6,quic,ws` |

完成全部 TCP/UDP 测试后，两条路由仍为 DIRECT、路径长度仍为 1；没有目标回落到 relay，
路由器 load average 仍然较低。

## RTT

每行使用 10 个 ICMP 请求。样本时间较短，不能视为长期 SLA 数据。

| 目标 | 路径 | 收到 | RTT min/avg/max |
| --- | --- | ---: | --- |
| A | EasyTier `10.44.0.8` | 5/10 | `143.3/143.6/144.1 ms` |
| A | 公网 IPv4 | 0/10 | ICMP 被阻断 |
| A | 一个已通告的公网 IPv6 | 9/10 | `207.3/208.4/211.2 ms` |
| B | EasyTier `10.44.0.12` | 10/10 | `184.5/241.7/571.6 ms` |
| B | 公网 IPv4 | 8/10 | `176.9/177.6/178.0 ms` |
| B | 公网 IPv6 | 8/10 | `185.1/185.7/186.1 ms` |

目标 A 的 overlay RTT 低于本次选取的单个公网 IPv6 地址。该 peer 通告了多个 IPv6 地址
和传输协议，因此不能据此声称“封装降低了 RTT”；它只能证明本次公网基线地址与 EasyTier
实际选择的 underlay 路径并不等价。

目标 B 的常规 overlay 样本约为 `185 ms`，接近公网 IPv6，比公网 IPv4 约高 `8 ms`。
两个 overlay 异常值显著拉高了 10 包平均值。

## TCP 吞吐

每项为 8 秒、单流 iperf3。`上行`表示路由器发送到目标，`下行`表示目标发送到路由器。
主要对照均测试两次。

| 目标 | 路径 | 方向 | 第一次 | 第二次 | 平均 |
| --- | --- | --- | ---: | ---: | ---: |
| A | EasyTier | 上行 | `39.9 Mbit/s` | `36.1 Mbit/s` | `38.0 Mbit/s` |
| A | EasyTier | 下行 | `63.4 Mbit/s` | `59.3 Mbit/s` | `61.4 Mbit/s` |
| A | 公网 IPv6 | 上行 | `45.3 Mbit/s` | `48.3 Mbit/s` | `46.8 Mbit/s` |
| A | 公网 IPv6 | 下行 | `1.05 Mbit/s` | `9.81 Mbit/s` | `5.43 Mbit/s` |
| B | EasyTier | 上行 | `35.9 Mbit/s` | `38.8 Mbit/s` | `37.4 Mbit/s` |
| B | EasyTier | 下行 | `57.2 Mbit/s` | `59.8 Mbit/s` | `58.5 Mbit/s` |
| B | 公网 IPv4 | 上行 | `48.1 Mbit/s` | `48.1 Mbit/s` | `48.1 Mbit/s` |
| B | 公网 IPv4 | 下行 | `0.107 Mbit/s` | `0.272 Mbit/s` | `0.190 Mbit/s` |
| B | 公网 IPv6 | 上行 | `4.95 Mbit/s` | 未复测 | - |
| B | 公网 IPv6 | 下行 | `0.146 Mbit/s` | 未复测 | - |

与最佳可重复裸公网路径相比，目标 A 的 overlay 上行代价约 19%，目标 B 约 22%。该差异
同时包含加密、封装、用户态转发，以及 EasyTier 实际 underlay 与单个公网基线地址不完全
相同造成的影响。

裸公网反向 TCP 严重受损且波动很大；EasyTier 下行明显更高且可重复。这不能证明 EasyTier
普遍“加速 TCP”，因为 overlay 可以选择不同地址和传输，UDP/QUIC underlay 也可能绕开
质量很差的裸 TCP 路径。

## UDP 吞吐

UDP 使用 1,200 字节 datagram，避免不必要的 overlay 分片。测试固定发送 `100 Mbit/s` 和
`50 Mbit/s`，接收速率及丢包率才是有效指标。

| 目标 | 路径 | 发送负载 | 方向 | 接收速率 | 丢包 |
| --- | --- | ---: | --- | ---: | ---: |
| A | EasyTier | `100 Mbit/s` | 上行 | `39.0 Mbit/s` | `62%` |
| A | EasyTier | `100 Mbit/s` | 下行 | `68.1 Mbit/s` | `32%` |
| A | EasyTier | `50 Mbit/s` | 上行 | `44.5 Mbit/s` | `11%` |
| A | EasyTier | `50 Mbit/s` | 下行 | `38.2 Mbit/s` | `24%` |
| B | EasyTier | `100 Mbit/s` | 下行 | `23.5 Mbit/s` | `76%` |
| B | EasyTier | `50 Mbit/s` | 下行 | `23.3 Mbit/s` | `53%` |
| B | 公网 IPv4 | `100 Mbit/s` | 上行 | `55.7 Mbit/s` | `57%` |
| B | 公网 IPv4 | `100 Mbit/s` | 下行 | `49.1 Mbit/s` | `57%` |
| B | 公网 IPv4 | `50 Mbit/s` | 下行 | `26.3 Mbit/s` | `47%` |

部分裸公网 UDP 和目标 B 上行 UDP 测试中，iperf 控制连接成功，但接收端没有收到 UDP
payload。目标 A 即使临时放行测试端口，公网 IPv6 UDP 仍不可用；目标 B 的公网正向 UDP
也间歇性为零。服务端日志记录到了路由器的 CGNAT 公网源地址，因此更符合非对称
防火墙/NAT/公网路径限制，而不是 EasyTier listener 故障。

所以，本次无法得到对称的裸 UDP 容量基线。可以确认的是：overlay 能向目标 A 双向承载
应用 UDP，也能从目标 B 反向承载 UDP，而对应裸公网路径具有明显非对称性。在
`50-100 Mbit/s` 负载下丢包已经很高，不能把这些结果描述为对应发送速率下的无损容量。

## 性能结论

1. 两个目标在持续 TCP/UDP 负载期间始终保持 DIRECT P2P。
2. Overlay TCP 上行比最佳可重复裸公网基线低约 19-22%。
3. Overlay TCP 下行稳定在约 `58-61 Mbit/s`，而测试到的裸公网反向 TCP 严重受损。
4. UDP 在 `50-100 Mbit/s` 时高度非对称且丢包明显；公网 NAT/防火墙行为使完整对称
   裸 UDP 对比无法完成。
5. 本次没有发现 relay 回落、资源泄漏或 EasyTier 进程不稳定。

## QUIC IPv6 Listener 审计

### 当前行为

CLI 自动生成的全协议 listener 中包含 `quic://0.0.0.0:11012`；GUI/手工配置同样地址时
行为一致。`instance/listeners.rs` 明确排除了 QUIC 的自动 IPv6 listener，理由是：

```text
quic enables dual-stack by default, may conflict with v4 listener
```

这条注释与当前默认实际行为不一致：

- 绑定 `0.0.0.0` 的 IPv4 UDP socket 不能接收 IPv6。
- `QuicEndpointManager::server()` 只有在地址为 IPv6 unspecified（`[::]`）且 `both` pool
  可用时才启用 dual stack。
- 因此默认 `0.0.0.0` QUIC listener 只有 IPv4。运行时 socket 和 peer transport 列表均
  验证了这一点。

### 历史

排除条件和注释由 `40b5fe9a` 在 2025 年 `support quic proxy (#993)` 中加入。当时 QUIC
listener 仍直接绑定配置地址，也没有把默认 IPv4 地址转换为双栈；该注释从引入时就是错误
假设。

当前 dual-stack endpoint pool 由 `8311b117` 在 2026 年 QUIC endpoint manager 重构时
加入。它支持显式 `[::]` listener，但旧的 listener-manager 排除条件没有同步修改。这是
遗留的集成 bug，不是“QUIC IPv6 天生异常”的 Quinn 限制。

### 为什么不能直接删除排除条件

直接删除 `l.scheme() != "quic"` 会同时创建 `0.0.0.0:port` 和 `[::]:port`。当前 IPv6
QUIC endpoint 首先尝试 dual-stack，因此会与已有 IPv4 socket 重叠。

真实风险包括：

1. `tunnel/common.rs::setup_socket2_ext()` 会记录但吞掉 IPv6 bind 错误。冲突的 QUIC
   IPv6 socket 可能表面启动成功，实际未绑定或处于 port zero，而不是可靠 fallback。
2. 如果系统允许重叠 socket，IPv4 QUIC 包可能被送到错误的 Quinn endpoint。QUIC
   connection ID 属于 endpoint，本问题可能造成间歇握手失败或已建连接丢失。
3. Unix 双栈 socket 通常把 IPv4 peer 表示为 IPv4-mapped IPv6，Windows 常返回原生 IPv4；
   breaker、Stealth session、日志、tunnel label 和地址 guard 可能出现跨平台 key 不一致。
4. 如果直接用一个 `[::]` listener 替换 IPv4 listener，在 IPv4-only 系统上缺少明确
   fallback 时可能让 QUIC 完全不可用。
5. 默认开放 IPv6 会扩大可访问 UDP 面。Strict Stealth 会限制未认证响应，但 plain 配置
   仍需正确的主机防火墙策略。

Quinn 本身支持 IPv4-mapped IPv6 UDP，并包含跨平台处理。主要风险位于 EasyTier 的
listener 编排、bind 错误语义、地址归一化和 fallback，而不是 QUIC 协议本身。

### 最小、最安全且解耦的实现

不要把所有默认 QUIC listener 静默改成 `[::]`，也不要故意制造 bind 冲突来选择 fallback。

最低风险方案是：

1. 给 `QuicTunnelListener` 增加私有 bind mode：`V4Only`、`V6Only`、`DualStack`。不增加
   公共配置、protobuf 或 wire 字段。
2. 完整保留现有 IPv4 listener。当 `enable_ipv6=true` 且用户配置 unspecified IPv4 QUIC
   listener 时，在相同端口增加一个明确的 `V6Only` companion listener，强制
   `IPV6_V6ONLY=true`，从机制上避免端口重叠。
3. QUIC bind 必须严格校验。请求非零端口时，要么精确绑定成功，要么返回错误，禁止静默
   变成 port-zero listener。该修复先限制在 QUIC 内，不扩大到其他协议。
4. 显式 `[::]` 保持当前 `DualStack` 语义。默认 companion 不产生 IPv4-mapped 地址，
   所以 mapped-address 归一化可以作为显式双栈 listener 的独立加固任务，不扩大本次改动。
5. 单个 listener 的局部冲突不得修改进程全局 endpoint pool 模式。
6. 只有 V6 listener 确认绑定并进入 running-listener 集合后，才通告 IPv6 QUIC candidate。
7. 如果配置里已经存在相同端口的显式 IPv6 QUIC listener，不得再自动添加 companion。

该方案只修改 QUIC listener/endpoint 和 listener-manager 的 QUIC 特殊分支，不修改通用
bind、PeerManager、Direct 排序、Stealth 协议、Proxy 或其他 listener。

测试必须覆盖 Linux/macOS/Windows 同端口 IPv4+IPv6、IPv4-only、显式 dual stack、端口
占用、多个实例、Stealth 正确/错误 secret、socket mark/netns、running listener 通告，
以及 IPv4/IPv6 QUIC 反复重连。

## 最终判断

默认 QUIC 只有 IPv4 是集成 bug，目前没有证据表明 QUIC IPv6 本身不可靠。但直接删除
一个条件并不安全。增加一个 QUIC 私有 bind mode、V6Only companion 和严格 bind 校验，
即可在不改变 wire、配置语法、Proxy 顺序和其他协议的前提下补齐默认 IPv6。
