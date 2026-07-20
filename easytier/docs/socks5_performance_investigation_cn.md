# SOCKS5 性能调查与维护边界

用户使用方式、Leaf managed mesh SOCKS、QUIC/KCP TCP Proxy、smoltcp 与外层
overlay 的完整层级关系见
[`traffic_protocol_layers_cn.md`](traffic_protocol_layers_cn.md)。本文只保留 SOCKS5
性能归因和历史验证证据。

本文记录 2026-07-03 针对 EasyTier 内置 SOCKS5 的隔离测试。目的不是发布一组
通用性能指标，而是保留已经验证过的瓶颈归因，避免以后再次把目标端 `no_tun`
TCP 入站代理的性能问题误判成 SOCKS5 服务本身的问题。

测试对应提交为 `fab7fa5d`，当前构建版本为 `2.6.7`。混合版本测试使用上游官方
release `2.6.4-8428a89d`。

## 1. 当前 TCP 路径

内置 SOCKS5 的 TCP CONNECT 不是一条固定的数据路径：

1. 目标是本机虚拟 IP 时，地址会改写为 loopback，使用内核 TCP 直连本地服务。
2. 目标是远端虚拟 IP，且 source 开启 KCP Proxy、目标 capability 允许 KCP input
   时，`Socks5AutoConnector` 直接选择 KCP stream。
3. 不能使用 direct KCP 时，SOCKS5 使用用户态 smoltcp 发起普通 TCP SYN。该 SYN
   仍会经过 NIC pipeline，因此启用 QUIC Proxy 时可能被现有 proxy selector 接管。
4. 目标节点有 TUN 时，最终目标连接可走目标内核 TCP；目标节点 `no_tun` 时，连接
   需要经过目标端用户态 TCP 入站代理。

代码上的分界是：direct KCP 把 KCP output 标记为 `KcpSrc/KcpDst`，调用
`send_msg_for_proxy`，不再进 NIC pipeline；smoltcp 输出的仍是完整 IP/TCP packet，
调用 `send_msg_by_ip`，所以会先经过 NIC pipeline 和统一 proxy selector。

因此，SOCKS5 性能会同时受到 source connector、QUIC/KCP Proxy 和目标端是否
`no_tun` 的影响，不能只通过 SOCKS5 listener 的实现判断。

SOCKS5 direct KCP 当前仍发送 `proxy_prepare_version=0`，不等待 Proxy READY ACK，
运行时 KCP 故障也不会在同一个 SOCKS CONNECT 内再尝试 QUIC 或 smoltcp。这是本次
明确接受的行为边界，不计划为它增加额外状态机。

## 2. 测试环境

- 隔离的 Docker bridge，两个 Debian Bookworm 容器运行 release 优化构建。
- Docker 网络为本机 LAN 环境，不注入延迟、抖动或丢包。
- 两节点通过 EasyTier TCP listener 建立 underlay。
- 使用 Python HTTP server 提供 128 MiB 或 512 MiB 文件。
- 使用 `curl --socks5-hostname` 测量 SOCKS5；direct 场景直接访问目标虚拟 IP。
- 使用 `--disable-encryption true`，未显式启用压缩。
- 测试机和容器资源充足，结果用于路径间相对比较，不代表公网或低性能设备吞吐。

核心网络参数：

```text
A: 10.201.0.1/24, SOCKS5 :18080
B: 10.201.0.2/24, HTTP :19000
underlay: tcp://172.30.50.2:12001
```

## 3. 实测结果

`curl` 输出是十进制 B/s；表中的 MiB/s 为近似换算。

| Source / Target | 使用路径 | 实测吞吐 |
| --- | --- | --- |
| A `no_tun` / B 有 TUN | SOCKS 普通路径 | 149.1-152.7 MB/s，约 142-146 MiB/s |
| A 有 TUN / B `no_tun` | direct TCP | 14.87-14.89 MB/s，约 14.2 MiB/s |
| A 有 TUN / B `no_tun` | SOCKS 普通路径 | 14.7-17.3 MB/s，约 14-16.5 MiB/s |
| A `no_tun` / B `no_tun` | SOCKS 普通路径 | 14.65-14.82 MB/s，约 14 MiB/s |
| A `no_tun` / B `no_tun` | 仅 QUIC Proxy | 36.1-36.4 MB/s，约 34.5 MiB/s |
| A `no_tun` / B `no_tun` | KCP Proxy | 117.4-119.2 MB/s，约 112-114 MiB/s |
| `2.6.7 -> 2.6.4`，双端 `no_tun` | KCP Proxy | 116.1-118.1 MB/s |
| `2.6.4 -> 2.6.7`，双端 `no_tun` | KCP Proxy | 116.4-119.2 MB/s |

另外，source 开启 KCP、目标明确设置 `disable_kcp_input=true` 时，source 会根据
capability 自动退回普通路径，连接保持可用，吞吐回到约 14.2-14.3 MB/s。

## 4. 结论

### 已确认的瓶颈归因

- 当目标节点有 TUN 时，SOCKS5 普通路径可以达到与 direct TUN 相同量级的吞吐。
- 当目标节点 `no_tun` 时，direct TCP 和 SOCKS5 都降到约 14-17 MB/s。主要瓶颈位于
  目标端 `no_tun` TCP 入站代理，不是 SOCKS5 accept、认证或 source smoltcp connector。
- QUIC Proxy 可以改善普通 SYN 路径，但本次环境中仍明显慢于 SOCKS direct KCP。
- SOCKS direct KCP 不是可随意删除的历史分支。在双端 `no_tun` 场景中，它相对普通
  路径约有 8 倍吞吐提升。

### 维护决策

- 不为“统一架构”删除 SOCKS direct KCP 路径。
- 不把 SOCKS 会话全部迁移到一个全局 `JoinSet`；现有 net generation 和 port-forward
  取消域具有独立生命周期语义。
- 不为当前接受的 direct KCP prepare/fallback 边界增加额外状态机。
- 如果目标是改善 `no_tun` 吞吐，应先分析目标端 TCP proxy/capture 路径，而不是先
  重写 SOCKS5。
- 启用 QUIC/KCP Proxy 前后必须分别测试，因为它们会改变 SOCKS 发起流量的实际路径。

## 5. 资源与稳定性验证

- direct KCP：500 个短连接、并发 32，全部成功。
- 普通 smoltcp：并发 32 时出现 5 秒 connect timeout，说明该路径存在并发容量边界；
  降到并发 8 后，多轮每轮 500 个请求均无失败。
- 每轮结束后 EasyTier 的 FD 和线程数恢复到固定基线。
- source RSS 从约 19 MiB 上升到约 50 MiB、再到约 98 MiB，第三轮保持约 98 MiB，
  没有继续线性增长。该现象符合每个 smoltcp TCP socket 分配 128 KiB RX +
  128 KiB TX buffer 后，glibc allocator 保留高水位内存供后续复用。
- 100 个本机虚拟 IP SOCKS 请求、并发 16，全部成功；FD 和线程回到基线，没有观察
  到本机流量回环。
- UDP ASSOCIATE 当前返回 SOCKS reply `0x07`（Command not supported）。它不是
  可工作的半成品功能，不能仅打开 vendored library 的 UDP 开关就宣称支持。

这些测试没有证明所有长时间运行场景都不存在泄漏，但已经排除了“每个已完成 TCP
连接都会永久保留 FD、task 或 smoltcp socket buffer”的线性泄漏。

## 6. 后续排查顺序

以后遇到“SOCKS5 慢”时，按下面顺序隔离，不要直接改 connector：

1. 确认目标节点是否 `no_tun`。
2. 在同一 source/target 上比较 direct TCP 和 SOCKS TCP。
3. 分别关闭 QUIC/KCP Proxy，测普通路径。
4. 只开 QUIC Proxy，再只开 KCP Proxy，确认实际选中的路径。
5. 检查目标 capability 是否因 `disable_kcp_input`、`disable_quic_input` 或编译 feature
   而降级。
6. 用固定大小文件和相同并发重复测试，并同时记录 FD、线程、RSS 和失败数。
7. 只有当“目标有 TUN 时 SOCKS 明显慢于 direct”仍可复现，才优先调查 SOCKS source
   connector。

如需修改相关实现，至少重新运行以下回归矩阵：

- 当前版本双端普通路径、QUIC-only、KCP。
- 当前版本与一个官方旧 release 双向混合。
- 目标显式禁用 KCP input 的 capability fallback。
- 本机虚拟 IP、短连接并发、连接结束后的 FD/线程/RSS 回收。
