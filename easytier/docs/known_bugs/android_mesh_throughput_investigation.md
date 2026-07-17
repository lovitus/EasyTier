# Android 本地 Mesh 吞吐异常（原因未定）

**状态：已复现，调查中；目前没有足够证据确定根因。**

本文记录 2026-07-17 在 Android 实机上观察到的一组 mesh TCP 吞吐异常。该问题最初在
启用 Leaf policy proxy 后暴露，但关闭 policy proxy 后仍可复现，因此当前不能把 Leaf
本身认定为根因。

## 已确认现象

测试目标 `10.44.0.8` 是一台 IPv6-only VPS 的 EasyTier 虚拟 IPv4。远端临时 HTTP 服务
绑定在该虚拟地址，避免把公网 DNS、目标站点或 SOCKS 服务端纳入受控 HTTP 对照。

| 场景 | 结果 | 证据边界 |
| --- | --- | --- |
| Android 本地运行候选 `2.6.10-5d71abed~` | mesh HTTP 传输在 30 秒内未完成 | Android 本地 EasyTier 数据面参与 |
| Android 本地运行基线 `2.6.10-8637af4f~`，policy/KCP/QUIC proxy 均关闭 | 同一 mesh HTTP 传输在 30 秒内未完成 | 表明该现象不是候选 SHA 或 Leaf worker 单独造成 |
| 停止 Android 本地 EasyTier，经 `192.168.111.1` 网关 EasyTier 转发 | 4 MiB 在 1.82 秒完成，约 18.4 Mbit/s | 同一 Android、同一远端临时 HTTP 服务 |
| Android 本地 EasyTier 固定远端稳定 IPv6，分别建立 WS、TCP、UDP 连接 | 受控下载仍未在超时内完成 | Android 侧连接已收窄；远端回程连接选择尚未完整采集 |
| 远端临时关闭 TUN TSO/GSO/GRO/tx-checksum | 单次复测没有恢复 | 设置随后恢复；该样本不足以排除所有 offload 交互 |

维护者另有浏览器直接使用 `10.44.0.8:24443` SOCKS 可超过 60 Mbit/s 的历史/人工观察。
该数据与上述临时 HTTP 对照的测试方法不同，应作为重要线索保留，但不能直接替代同条件
基准。

附加观察：

- Android mesh ICMP 在固定单一 WS 路径上使用 56、512、1000、1200、1250 字节 payload
  均未丢包；1200 字节以上 RTT 和抖动明显增加。
- 远端 TUN 抓包显示下载方向 TCP ACK 前进很慢并伴随重复传输，但该抓包只能证明远端
  内层 TCP 的可见行为，不能单独判断数据丢在 underlay、PeerConn、队列还是 Android
  VpnService TUN 写入之后。
- Android 运行时曾观察到 QUIC 连接 loss 上升并关闭，随后切换 WS；这说明连接健康可能
  参与现象，但固定单连接样本尚未给出足够证据确定它是根因。
- 关闭 policy proxy 后 Android VPN DNS 仍指向 policy fake DNS 的现象另行处理；它会影响
  域名 peer 的诊断，但不能解释使用 IP 字面量的 mesh HTTP 吞吐异常。

## 当前不能下的结论

- 不能认定 Leaf、policy TUN 合并 writer 或规则分类是根因；policy-off 基线也复现。
- 不能认定 IPv6-only、稳定域名对应地址或远端 SOCKS 服务是根因；受控 HTTP 不经过
  SOCKS，但仍需补同条件公网 IPv6 基准。
- 不能认定某一种 TCP、UDP、QUIC 或 WS transport 是根因；双向实际承载连接和逐连接
  收发计数尚未形成完整证据链。
- 不能认定 Linux TUN GSO/offload 是根因，也不能凭一次运行时关闭 offload 就永久排除
  发送端批处理、分段或接收端兼容问题。
- 不能仅凭远端 TUN 抓包断言 Android 收到了超 MTU 包；还缺少 Android PeerConn 收包与
  TUN writer 之间的对应计数。

## 后续验证要求

1. 固定单一双向连接，在同一次下载中同步记录远端 TUN 包、两端逐连接 `rx/tx packets`
   和 Android 内层 TCP ACK。先判断数据是否已经到达 Android PeerConn。
2. 在 `192.168.111.1` 网关、Android 和同一远端之间使用相同目标、文件大小、MTU、协议
   和时间窗口做 A/B，记录网关与 Android 的实际双向 transport。
3. 使用双栈境外验证节点互测公网 IPv4/IPv6 TCP、UDP，并分别经 EasyTier 与物理网络做
   基准，避免把远端线路质量误判为 Android TUN 问题。
4. 只有在确认丢包发生在 Android PeerConn 之后，才增加最窄的 TUN writer 阶段计数；
   不应先修改正常 mesh selector、underlay 或远端生产网络。
5. 验证结束必须恢复 Android 原配置、远端 TUN offload 和所有临时服务。

## 当前清理状态

- Android 候选已恢复原始 policy、KCP proxy、QUIC proxy、peer、P2P 和 MTU 配置。
- Android 基线应用已停止，未卸载或覆盖。
- 远端临时 HTTP 服务和抓包文件已删除。
- 远端 TUN TSO/GSO/GRO/tx-checksum 已恢复为测试前状态。
- 本条只记录问题，不包含推测性数据面修复。

## Independent dual-stack path baseline (2026-07-17)

A direct `iperf3` baseline between the dual-stack overseas validation hosts produced:

| Path | Direction | Receiver throughput |
| --- | --- | ---: |
| IPv4 | lv1g2 -> lv1g3 | 4,151 Mbit/s |
| IPv4 | lv1g3 -> lv1g2 | 8,361 Mbit/s |
| IPv6 | lv1g2 -> lv1g3 | 8,288 Mbit/s |
| IPv6 | lv1g3 -> lv1g2 | 7,352 Mbit/s |

The IPv4 forward sample had transient stalls and all samples had some retransmissions, so these numbers are only a coarse physical-path baseline. They show that neither address family is generally constrained to the Android symptom's near-unusable rate between these hosts. They do not identify the Android fault layer and do not explain why gateway forwarding works while the phone's local EasyTier path does not.

## Linux dual-stack and IPv6-only peer reproduction (2026-07-17)

Candidate `d307a4e460a230599f595e1f59b832453d20b888` was exercised on Linux with a
controlled dual-stack mesh and against the existing IPv6-only peer. No code was
changed for this validation. The maintainer's failing Android configuration
uses an explicit `via: mesh` SOCKS actor with a configured port. The separate
portless managed-HEV result below is not a reproduction of that configuration
and must not be used to classify the Android symptom as platform-independent.

### Native EasyTier dual-stack control

An isolated candidate mesh between lv1g2 and lv1g3 used explicit listener ports
and disabled automatic P2P switching while each connector was selected. The
observed direct tunnel protocols were `tcp6`, `udp6`, `quic6`, and the `tcp4`
control. Both an IPv4 overlay and an IPv6 overlay were configured.

- IPv4 and IPv6 overlay ICMP completed with 0% loss on every tested transport.
- `tcp6` carried IPv4 and IPv6 overlay TCP at approximately 421-660 Mbit/s.
- `tcp4` carried the same overlay families at approximately 418-600 Mbit/s.
- `udp6` and `quic6` were functional for both overlay families, but high-load
  samples had larger transient throughput variation than `tcp6`; the samples
  are not sufficient to declare a capacity regression.
- One initial `tcp6` IPv6 reverse sample fell to 154 Mbit/s after a multi-second
  stall, but two interleaved repeats completed at 660 and 614 Mbit/s. It remains
  a transient observation, not a reproduced IPv6-specific fault.

This evidence does not show a general EasyTier IPv6-underlay or IPv6-overlay
failure.

### Gateway and the existing IPv6-only peer

The production gateway at `192.168.111.1` was used directly; the maintainer's
local workstation was not part of this topology. The gateway reported the
IPv6-only peer `10.44.0.8` as a one-hop direct peer using `ws6,tcp6`, with 0%
EasyTier-observed loss. Three ICMP requests completed at approximately 189 ms.

The peer's existing SOCKS service at `10.44.0.8:24443` downloaded a controlled
16 MiB object from an IPv4 target at approximately 34.0 Mbit/s and from an IPv6
target at approximately 35.7 Mbit/s. Both transfers completed. The gateway's
direct IPv4 control was approximately 48.2 Mbit/s. Its native route could not
reach the selected lv1g3 IPv6 prefix, while the SOCKS peer could, so that direct
IPv6 failure is a destination-prefix reachability difference and not evidence
of an EasyTier IPv6 failure.

A separate candidate instance on lv1g2 joined the existing mesh as
`10.44.0.240`. It formed a direct `quic6` tunnel to `10.44.0.8` before policy was
enabled. The same SOCKS service completed controlled 16 MiB IPv4 and IPv6
transfers at approximately 24.8 and 35.0 Mbit/s. An external IPv6 request also
completed and reported the peer's IPv6 egress address.

### Leaf portless managed-HEV result

On the isolated candidate mesh, a minimal policy sent all TCP and UDP to one
portless `via: mesh` actor targeting the other candidate peer. The underlay was
confirmed as `tcp6`; mesh IPv4 and IPv6 ICMP continued to work with 0% loss.

The policy runtime initially failed closed because the target route had not yet
arrived, then recovered about 3.25 seconds later after route and DNS state became
available. After recovery:

- A controlled IPv4 16 MiB transfer entered the policy TUN, received only
  11,387,703 bytes in 60 seconds, averaged about 1.52 Mbit/s, and timed out.
- An external IPv6 target outside the hosts' directly connected public `/48`
  entered the policy TUN but timed out during TCP connect after 15 seconds.
- The Leaf worker consumed 98-100% of the host's single CPU for eight
  consecutive one-second samples.
- Core and HEV CPU remained low.
- Logs show that the managed path successfully selected KCP and that the target
  peer accepted the KCP connection to its managed HEV endpoint. The result was
  not a silent fallback to the native smoltcp path.

The lv1g2 and lv1g3 public IPv6 addresses share a directly connected `/48`.
Requests to the other host's public IPv6 address therefore followed that more
specific physical route instead of the policy `::/1` and `8000::/1` routes. That
sample was correctly discarded as a Leaf IPv6 test; the external IPv6 target
above was used instead.

This is a separate Linux portless managed-HEV defect with a superficially
similar throughput symptom. It does not reproduce the maintainer's explicit
port configuration and therefore does not weaken or confirm an Android-TUN
explanation for the reported failure. It also does not yet prove whether the
CPU loop is inside Leaf packet handling, the EasyTier-to-Leaf bridge contract,
or a particular interaction with the portless managed-HEV path.

### Explicit `via: mesh` user SOCKS result

A second minimal policy used the real peer as an explicit actor:

```yaml
proxies:
  kr-explicit:
    type: socks5
    server:
      virtual-ip: 10.44.0.8
    port: 24443
    via: mesh
    udp: true

rules:
  - NETWORK,udp,kr-explicit
  - MATCH,kr-explicit
```

This path did not reproduce the managed-HEV worker spin:

- An external IPv6 request succeeded through Leaf and reported the same IPv6
  egress as the native SOCKS control.
- Leaf worker CPU averaged 0.13% during the short IPv6 check.
- In the same running instance with the same `quic` tunnel before and after both
  measurements, the native SOCKS path downloaded 16 MiB at approximately
  27.8 Mbit/s, while Leaf explicit `via: mesh` downloaded it at approximately
  15.5 Mbit/s.
- Leaf worker CPU averaged 1.9% during an earlier explicit-actor IPv4 transfer.

The same-transport result is an observed approximately 44% throughput reduction
for this single explicit-actor sample. It is not evidence that every workload,
transport, or platform has the same overhead, but it is too large to support a
claim of no obvious path-specific performance regression.

### Current conclusion and next diagnostic

The evidence now separates four facts:

1. Native EasyTier IPv6 underlay and dual-stack overlay are functional in the
   tested topology.
2. Native mesh access to the IPv6-only peer's existing SOCKS server is
   functional for IPv4 and IPv6 targets.
3. Leaf `via: mesh` is not uniformly broken: the explicit user-SOCKS actor works,
   while the tested portless managed-HEV actor reproduced severe throughput,
   IPv6-connect, and Leaf-worker CPU symptoms.
4. The maintainer's Android failure uses the explicit user-SOCKS form. The Linux
   explicit-actor sample showed a measurable slowdown but did not reproduce the
   near-unusable Android behavior, so the original Android fault remains open.

Do not yet label the Android root cause as IPv6, KCP, GSO, TUN batching, the
EasyTier mesh, or the independently discovered portless worker spin. The next
diagnostic for the reported Android failure must use the maintainer's exact
explicit-port policy and compare Android direct SOCKS with Android Leaf under
the same peer, transport, target, and payload. The portless managed-HEV worker
spin should be tracked and profiled separately with bounded counters for TUN
reads and writes, bridge queue progress, poll wakeups, and per-flow bytes.

All temporary EasyTier instances, Leaf/HEV workers, TUN devices, policy routes,
HTTP/iperf services, files, and listener ports used for this round were removed
from lv1g2 and lv1g3. The production gateway process and configuration were not
modified.
