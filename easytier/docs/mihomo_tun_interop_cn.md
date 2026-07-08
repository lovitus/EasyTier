# EasyTier 与 Mihomo TUN 共存的已知风险

本文记录 EasyTier 与 Mihomo/Clash/sing-box 这类系统 TUN 同时运行时可能出现的
CPU 异常、连接回流和 Proxy Failover 诊断污染。它是当前代码线的已知互操作边界，
不是已完成修复清单。

## 现象

典型现象如下：

- Mihomo 进程长期占用一个或多个 CPU core。
- EasyTier core 也持续保持较高 CPU，但 EasyTier 自身的虚拟网卡流量并不大。
- Mihomo 连接列表里出现大量进程名为 `easytier-gui` / `easytier-core` 的连接，
  目标端口集中在 EasyTier listener 端口，例如 `11010`、`11011`、`11012`、
  `11013`。
- EasyTier `failover` / GUI Proxy Failover 面板可能显示少量本机虚拟 IP 发起的
  QUIC/KCP/Native selector 状态，但通常不是 failover 表本身无界增长。
- 重启 EasyTier 或 Mihomo 后 CPU 会释放；重新进行 KCP/QUIC/direct-connect 压测后
  可能再次触发。

## 根因

当 Mihomo TUN 开启 `auto-route` 或等价全局路由捕获时，EasyTier 的 underlay 出站
连接可能先进入 Mihomo TUN，再由 Mihomo 决策转发。此时会出现两个独立问题：

1. EasyTier 采集本机接口地址时，可能把 Mihomo fake-IP/TUN 地址、EasyTier 自身虚拟
   网卡地址，或其他代理虚拟接口地址当成普通 `Interface IPv4/IPv6` 广告给 peer。
2. 远端的 `0.0.0.0` listener 会被 direct-connect 展开成这些接口地址，生成本不应尝试
   的 direct candidate。KCP/QUIC/WS/FakeTCP 等候选随后又被 Mihomo TUN 捕获，形成
   代理链污染、回流或高 CPU。

这类问题与 QUIC/KCP Proxy 的 `QUIC -> KCP -> Native` fallback 不是同一个层级。
Proxy Failover 表展示的是 TCP SYN selector 的短期状态；Mihomo TUN 捕获的是
EasyTier underlay socket，发生在更底层。

## 快速复现

下面步骤用于复现和定位，不需要修改 EasyTier 代码。

### 1. 启动 Mihomo TUN

启用以下等价配置：

```yaml
tun:
  enable: true
  auto-route: true
  stack: mixed
dns:
  fake-ip-range: 198.18.0.1/16
```

确保 EasyTier 进程没有被 TUN 层 bypass。

### 2. 启动 EasyTier

启动带虚拟网卡和多个 underlay listener 的节点，例如：

```bash
easytier-core \
  --network-name demo \
  --network-secret demo-secret \
  -i 10.44.0.3/16 \
  -l tcp://0.0.0.0:11010 \
  -l udp://0.0.0.0:11010 \
  -l ws://0.0.0.0:11011 \
  -l quic://0.0.0.0:11012 \
  -l faketcp://0.0.0.0:11013 \
  --enable-quic-proxy \
  --enable-kcp-proxy
```

再产生 KCP/QUIC/direct-connect 活动，例如连接多个 peer、访问远端虚拟 IP 上的 TCP
端口，或执行大量短连接测试。

### 3. 观察路由

macOS：

```bash
route -n get <peer-public-ip>
route -n get <peer-lan-candidate-ip>
netstat -rn -f inet
ifconfig
```

Linux：

```bash
ip route get <peer-public-ip>
ip route get <peer-lan-candidate-ip>
ip addr
```

Windows PowerShell：

```powershell
Get-NetRoute -AddressFamily IPv4
Get-NetAdapter
Test-NetConnection <peer-public-ip> -Port 11010
```

如果 EasyTier peer 的公网或 LAN candidate 路由指向 Mihomo TUN 设备，而不是物理网卡，
测试结果已经被代理 TUN 污染。

### 4. 观察 EasyTier 广告地址

```bash
easytier-cli -p 127.0.0.1:<rpc-port> node
```

风险信号包括：

- `Interface IPv4` 出现 `198.18.0.1` 或 `198.18.0.0/15` 内地址。
- `Interface IPv4` 出现本机 EasyTier 虚拟 IP，例如 `10.44.0.3`。
- `Interface IPv6` 出现代理 TUN 的 ULA 地址，并被用于 underlay 连接。

### 5. 观察 Mihomo 连接

如果启用了 Mihomo external controller：

```bash
curl -H "Authorization: Bearer <secret>" \
  http://127.0.0.1:9090/connections
```

重点查找：

- `metadata.process` 为 `easytier-gui` 或 `easytier-core`。
- `metadata.type` 为 `Tun`。
- `metadata.destinationPort` 为 EasyTier listener 端口。
- `metadata.sourceIP` 为 `198.18.x.x`、Mihomo TUN 地址或 EasyTier 虚拟 IP。

### 6. 观察 CPU 和接口计数

macOS：

```bash
ps axww -o pid,user,%cpu,rss,etime,comm,args | egrep -i 'easytier|mihomo|clash|sing-box'
netstat -ibn | egrep 'Name|utun'
```

Linux：

```bash
ps -eo pid,user,pcpu,rss,etime,comm,args | egrep -i 'easytier|mihomo|clash|sing-box'
ip -s link
```

如果 Mihomo TUN 包计数高速增长，而 EasyTier 虚拟 NIC 包计数增长很小，说明 CPU
主要消耗在代理 TUN/underlay 捕获侧。

## 临时规避

在代码修复前，建议按下面优先级处理：

1. 在 Mihomo/sing-box 的 TUN 层排除 EasyTier 进程或 EasyTier underlay 目标地址。
   仅写 `DIRECT` 规则不一定足够，因为数据包仍可能先进入 TUN；需要使用对应实现提供的
   route/process bypass、route-exclude 或等价能力。
2. 避免把 EasyTier SOCKS/Mihomo SOCKS 入口绑定到 EasyTier 虚拟 IP。优先绑定
   `127.0.0.1`，并避免代理链从 overlay 地址重新进入本机代理入口。
3. 压测 QUIC/KCP/Proxy Failover 时，先临时关闭系统 TUN 或确认 EasyTier underlay
   真实走物理网卡。
4. 触发高 CPU 后，重启 EasyTier core 和 Mihomo 可以释放当前循环状态；这只是恢复手段，
   不是根治。

## EasyTier 侧内置 Guard

跨 macOS、Windows、Linux “强制让某个进程完全绕过另一个系统 TUN”不是一个简单的
EasyTier 内部开关。Linux 可以使用 socket mark / policy route，macOS 和 Windows
需要不同的 socket/interface 绑定或 Network Extension/WFP 语义；这会变成平台级重构。

本 fork 先内置一层更小的 fail-safe guard，默认开启。它不是系统级 socket
protect，而是 underlay 候选地址净化 + 运行时局部熔断：

- `--underlay-candidate-guard` 默认是 `true`。
- 内置 fake-IP base set 固定包含
  `198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,192.19.0.0/24`，覆盖常见
  Mihomo/sing-box/Clash/V2Ray/Xray/Surge fake-IP 池。
- `--underlay-exclude-cidrs` 是用户附加列表；清空只关闭用户附加 CIDR，不关闭内置
  base set、EasyTier 运行态虚拟地址过滤或历史 EasyTier-managed IPv6 过滤。
- GUI 高级设置里同样提供“Underlay 候选地址净化”和一个完整 CIDR 列表输入框。
- guard 会过滤本机 IP 通告、direct candidate 展开、IPv6 hole-punch candidate、
  hole-punch RPC 前的 direct UDP 路由源地址验证、收到的 hole-punch RPC connector 地址、
  generic connector 目标/源地址，以及 connector bind-source 列表。
- generic connector 与 direct validation 会用临时 connected UDP socket 探测系统实际
  source IP；目标 IP 或 source IP 命中内置/用户/EasyTier 运行态 guard 时，直接拒绝
  该候选。
- 如果 source IP 未命中 CIDR，但反查到 `utun`/`tun`/`tap`/`wintun`/point-to-point
  这类可疑接口，v1 只记录 warning 和 bounded soft strike，不硬拒绝，也不触发熔断。
- 内部 breaker 固定最多 4096 个 key，按 `Endpoint(remote_addr, scheme, scope)` 或
  `Peer(peer_id, scheme, scope)` 计数。3 次 hard strike / 30 秒触发 300 秒 TTL，
  重复触发指数退避到 1800 秒。
- hard strike 只来自高置信信号：guard hard hit、已知目标 peer 的 handshake peer
  mismatch，以及现有 self-loop 检测。prepare timeout、ACL/Policy、目标拒绝和普通快速
  失败只记录日志，不触发熔断。
- Peer 与 Endpoint key 使用同一 lease 原子获取，TTL 错位时不会互相消费 half-open。
  preflight 取消只回滚自身 lease，第一次真实连接或打洞副作用前才 commit。
- TTL 到期后每组 key 只放行一个 half-open attempt；Direct、generic 和 TCP/UDP
  hole-punch 都必须等认证 PeerConn 收到首个 pong 后精确解除，单纯握手成功不会清理。
- 命中 guard 的公网 IPv4 UDP 直连候选会直接 fail-closed 跳过，不再退回 generic
  direct UDP fallback。
- 设置 `underlay_candidate_guard=false` 会旁路本次新增的净化钩子，仅保留旧的
  EasyTier-managed IPv6 过滤；同时不检查 breaker、不记录 hard/soft strike、不触发 TTL。

listener 仍然可以监听 `0.0.0.0`；guard 过滤的是 EasyTier 对外通告和主动拨打的
underlay candidate。它不改变 PeerManager、Proxy、Stealth、SOCKS、wire format，
也不改变 QUIC/KCP Proxy 固定 failover 顺序。真正“所有 generic underlay socket
都永远不进入系统 TUN”的硬保证，仍需要系统代理软件提供进程级或路由级 bypass，
或者未来实现平台分别适配的 socket protect/bind 层。
