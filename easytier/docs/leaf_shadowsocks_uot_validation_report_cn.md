# Leaf Shadowsocks 与 UoT v2 功能/性能验证报告

> 状态：候选实现与验证进行中。未完成的项目不能作为发布证据。

## 候选身份

- EasyTier SHA：`1eb6f191cb049b56afd8c399adf0c37c92ecfa86`。
- Leaf SHA：`742ad65c441f9d60279916b82628b810efbd48fb`；EasyTier `Cargo.lock`
  精确 pin 已由 `.160 --locked` 预检验证。
- Gust：`lovitus/gust` `v3.2.9-porty8`，release commit `0f15013`。
- 平台：Linux、Android。
- Linux 主性能路径：两台 10Gbps 双栈公网验证节点之间分别运行 IPv4、IPv6和
  双栈矩阵；内网机器用于大陆 IPv4 对照。`10.20.0.65` 不作为大陆 IPv4 客户端基线。

## 预检证据

- `.160` 标准 `scripts/leaf-remote-preflight.sh` 已对精确 Leaf pin 完成 `--locked`
  no-run；最终增量编译 31.69 秒，新增四项 policy 测试和既有 Leaf/HEV/netstack
  focused suite 全部通过。
- Leaf UoT 测试二进制实际运行 5 项并通过：JSON/protobuf 映射、unconnected request、
  IPv4/IPv6/域名 packet 地址、lazy 首包、帧边界和短缓冲对齐。独立 Leaf 全量 lib
  test 的既有 Tokio-macros/GeoSite lite-protobuf 门槛未伪装为候选失败或成功。
- 两台双栈节点上的 Gust release 文件已校验；`gost` SHA-256 为
  `46ebef5815c6918f1c6e6102cc22a1af5398e92eee4070c05bddd62825c21647`。

## 功能矩阵

| 场景 | Linux | Android | 证据 |
|---|---|---|---|
| Shadowsocks TCP native | pending | pending | pending |
| Shadowsocks UDP native | pending | pending | pending |
| Shadowsocks UoT v2 native | pending | pending | pending |
| mesh SOCKS -> Shadowsocks TCP | pending | pending | pending |
| mesh SOCKS -> Shadowsocks UoT v2 | pending | pending | pending |
| chain/fallback first-match | pending | pending | pending |
| incompatible UoT fail-closed | pending | pending | pending |
| stop/start and resource baseline | pending | pending | pending |

## 性能矩阵

同一候选、同一服务器、同一 cipher、同一 payload 和相同测试时长比较：

| 路径 | TCP throughput | UDP throughput/loss | CPU | RSS | 状态 |
|---|---:|---:|---:|---:|---|
| SS native | pending | pending | pending | pending | pending |
| SS UoT v2 | n/a | pending | pending | pending | pending |
| mesh -> SS native | pending | pending | pending | pending | pending |
| mesh -> SS UoT v2 | n/a | pending | pending | pending | pending |

UoT 的目标是受限网络中的 UDP 可用性，不预设其吞吐优于标准 UDP。报告必须记录
RTT、丢包、并发、payload、cipher、Gust 命令和原始结果位置。

## 发布判断

pending。只有精确 artifact 的 Linux/Android 功能、资源和性能矩阵全部完成后更新。

## 2026-07-18 精确候选最终验证补充

### 候选与权威服务端

- EasyTier 候选：`1eb6f191cb049b56afd8c399adf0c37c92ecfa86`。
- Leaf 候选：`742ad65c441f9d60279916b82628b810efbd48fb`。
- Linux workflow：`29634958915`，x86_64-musl 优化制品，`SHA256SUMS.txt`、`BUILD_INFO.txt`、提交 SHA、target 和符号均已核对。
- Android workflow：`29634958928`，APK SHA-256 为 `2db8feb4464f5cbe99b5781dac0d8ebc7bb70935982e674d9b3951dd7b0295e7`。
- 权威 Shadowsocks 服务端改为官方 `sing-box v1.13.14` x86_64-musl，release SHA-256 为 `d5b46de6498427bccfeb87dbafcde4dbefdfe35680020d07d286ad915f0bfb34`。
- sing-box 在 `lv1g3` 的 `/slab2/easytier-validation/uot-1eb6f191` 下运行，单一 Shadowsocks inbound 同时提供 TCP、原生 UDP 和 UoT v2。
- Gust v3.2.9-porty8 仅保留为兼容夹具：`sings` 的原始 UoT 测试正常；`ssu` 在公网、内网和同机 loopback 都报 `use of closed network connection`，因此判定为该版本/配置的 SSU 服务端夹具故障，不归因于 Leaf、EasyTier、GFW 或防火墙。

### 已验证功能矩阵

| 场景 | 结果 | 关键证据 |
|---|---:|---|
| 标准 SS TCP | 通过 | Cloudflare HTTPS；3 次 5 秒 TCP 中位数约 715.452 Mbps |
| SS 原生 UDP，IPv4 目标 | 通过 | 100/100 echo；100 Mbps 时 0% loss |
| SS 原生 UDP，IPv6 目标 | 通过 | 100/100 echo |
| SS UoT v2，IPv4 目标 | 通过 | 100/100 echo；200 Mbps 时 0% loss |
| SS UoT v2，IPv6 目标 | 通过 | 100/100 echo；100 Mbps 时 0% loss |
| IPv6 Shadowsocks 服务端 -> IPv4 目标 | 通过 | TCP receiver 735 Mbps；UoT UDP 97.7 Mbps、0% loss |
| IPv6 Shadowsocks 服务端 -> IPv6 目标 | 通过 | TCP receiver 841 Mbps；UoT UDP 96.2 Mbps、0% loss |
| dead-first fallback -> 可用 UoT | 通过 | 首次 HTTPS 74 ms；TCP receiver 788 Mbps；UDP 19.5 Mbps、0% loss |
| mesh SOCKS -> peer-local SS -> UoT chain | 通过 | HTTPS、TCP、IPv4/IPv6 UDP 均成功 |
| IPv6-only EasyTier underlay 上的 chain | 通过 | 日志仅出现 IPv6 TCP/QUIC；TCP receiver 154 Mbps；UDP 48.8 Mbps、0% loss |
| 自动双栈 underlay | 通过 | 初始 IPv4 TCP 后自动建立 IPv6 QUIC；未关闭 KCP/QUIC |
| 5 次 stop/start | 通过 | 每轮 TUN 消失、旧 core/Leaf PID 为 0、无残留进程 |
| 60 秒空闲 | 通过 | core 约 0.20% 单核，Leaf 约 0.067% 单核；FD/线程不增长 |

Android 精确 APK 已完成保留数据升级，安装包 SHA 与 workflow 一致，标准 SS 的 captured-UID TLS 探针成功（TCP 与 TLS handshake 均成功）。Android 设备随后按维护者要求撤离，因此没有把 Chrome QUIC 或 TCP 探针误报成 Android UoT 实包证据；Android UoT 实包仍标为“未在本轮设备上证明”，不是失败。后续验证按要求仅使用 `.160`、`.37`、`.38`、`lv1g2`、`lv1g3`。

### 性能结果与边界

物理链路基线：

| 路径 | TCP receiver |
|---|---:|
| lv1g2 -> lv1g3 IPv4 | 8283.805 Mbps |
| lv1g2 -> lv1g3 IPv6 | 7762.8 Mbps |
| 原始 Gust SS TCP | 中位数 2513.983 Mbps |
| 原始 Gust sings TCP | 中位数 2552.455 Mbps |

EasyTier/Leaf：

| 模式 | 结果 |
|---|---:|
| 标准 SS TCP | 中位数约 715.452 Mbps |
| IPv6 SS 服务端、IPv4 目标 TCP | receiver 735 Mbps |
| IPv6 SS 服务端、IPv6 目标 TCP | receiver 841 Mbps |
| dead-first fallback TCP | receiver 788 Mbps |
| mesh -> SS -> UoT chain TCP | 3 次 5 秒中位数约 181.958 Mbps |
| IPv6-only underlay chain TCP | receiver 154 Mbps |

UDP 对照：

| offered rate | native SS | UoT v2 |
|---:|---:|---:|
| 100 Mbps | 95.791 Mbps / 0% | 94.962 Mbps / 0% |
| 200 Mbps | 191.346 Mbps / 0.1686% | 191.418 Mbps / 0% |
| 250 Mbps | 未单测 | 239.067 Mbps / 0.1727% |
| 300 Mbps | 284.566 Mbps / 2.2778% | 286.399 Mbps / 0.2373% |
| 500 Mbps | 468.007 Mbps / 62.0512% | 474.963 Mbps / 60.7876% |

结论：

- UoT 在 200-300 Mbps 区间相对原生 SS UDP 明显降低丢包，符合引入目的。
- 两种模式在 500 Mbps 左右都出现拥塞崩塌；不能把当前实现宣传为高吞吐线速 UDP。
- chain 的额外 mesh SOCKS/overlay 路径把 TCP 中位数从约 715 Mbps 降到约 182 Mbps，并把 echo RTT 从约 0.4 ms 提高到约 31 ms。这是明确的性能成本，当前没有通过修改 mesh、KCP 或 QUIC 掩盖问题。
- KCP/QUIC 始终开启；policy/Leaf 只消费 mesh 提供的现有 SOCKS actor，不拥有或重写 mesh transport。

### 资源与生命周期

- 5 次启动后 core RSS 为 26808-31468 KiB，Leaf RSS 为 7064-7140 KiB；数值无单调增长。
- 每轮 core FD 固定 31、Leaf FD 固定 11；core 线程固定 8、Leaf 线程固定 2。
- 正常停止耗时 105-209 ms；每轮旧 PID、子进程和 namespace 内 `tun0` 都回到 0。
- 60 秒空闲期间 core 消耗 12 ticks（HZ=100，约 0.20% 单核），Leaf 消耗 4 ticks（约 0.067% 单核）。
- 空闲期间 Leaf RSS/FD/线程保持 7096 KiB、11、2；core FD/线程保持稳定，RSS 的一次性变化未在重复生命周期中形成单调趋势。
- UoT 压测采样峰值：core RSS 60320 KiB、Leaf 9048 KiB、sing-box 42836 KiB。这里的 `ps %CPU` 是进程生命周期平均值，不作为瞬时峰值结论。

### 最小配置与组合方式

直接使用 UoT：

```yaml
proxies:
  ss-uot:
    type: shadowsocks
    server: 2001:db8::10
    port: 8388
    cipher: aes-256-gcm
    password: change-me
    via: native
    udp: uot-v2

rules:
  - NETWORK,udp,ss-uot
  - MATCH,ss-uot
```

使用传统 Shadowsocks UDP 时仅把 `udp` 改为 `native`；禁止 UDP 时使用 `off`。旧配置中的布尔值继续兼容：`true` 等价于 native，`false` 等价于 off。

peer 本机运行 SS 服务端时，通过已有 mesh SOCKS actor 组合 chain，而不是给 Shadowsocks actor 发明 `via: mesh`：

```yaml
proxies:
  mesh-hop:
    type: socks5
    server:
      virtual-ip: 10.44.0.8
    via: mesh
    udp: true

  peer-ss-uot:
    type: shadowsocks
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: change-me
    via: native
    udp: uot-v2

groups:
  mesh-ss-uot:
    type: chain
    members: [mesh-hop, peer-ss-uot]
```

fallback 继续复用现有组语义：

```yaml
groups:
  proxy-fallback:
    type: fallback
    members: [mesh-ss-uot, ss-uot]

rules:
  - GEOSITE,google,proxy-fallback
  - DOMAIN-SUFFIX,example.com,proxy-fallback
  - IP-CIDR,203.0.113.0/24,proxy-fallback,no-resolve
  - MATCH,DIRECT
```

本次没有增加通用 Leaf 协议抽象层。Shadowsocks 只是新的 actor kind；以后增加 Trojan、VMess、VLESS 时继续按 actor/compiler 插件逐个接入，只有出现两个以上协议共享且稳定的字段时才提取公共结构。

### 清理结果

- `lv1g2` 隔离 namespace `etuot182` 已删除。
- 标记为 `easytier-uot-1eb6f191` 的 IPv4/IPv6 firewall 规则均为 0。
- `lv1g2` 验证产物进程为 0。
- `lv1g3` 的 EasyTier、sing-box、Gust、iperf3、UDP echo 均按专用端口解析出的精确 PID 停止，专用监听行数为 0。
- `.37/.38` 的 28588-28590 临时 Gust/UDP 夹具均按精确 PID 停止。
- 原始日志、配置和性能结果保留在各 VPS 的 `/slab2/easytier-validation/uot-1eb6f191`，没有放入系统盘。

## 2026-07-18 官方 shadowsocks-rust ssserver 对照：Gust ss/ssu

### 精确组件

- 服务端：官方 `shadowsocks-rust ssserver v1.24.0`，x86_64-musl release。
- 官方归档 SHA-256：`0d84f5f350ec99396867d718f146fc3810975b2a7cd06192f158d96bdef460e7`。
- 服务端参数：`[::]:28700`、`aes-256-gcm`、`-U`（TCP_AND_UDP）。
- 客户端：Gust `v3.2.9-porty8`，二进制 SHA-256 `46ebef5815c6918f1c6e6102cc22a1af5398e92eee4070c05bddd62825c21647`。
- 测试主机：`lv1g2` 客户端、`lv1g3` 服务端；所有文件位于 `/slab2/easytier-validation/standard-ssserver-v1.24.0`。
- 对照目标：TCP iperf3 `:28720`、UDP echo `:28721`。

### 结果

| 路径 | 结果 |
|---|---:|
| 直连 TCP | receiver 7.96 Gbps |
| Gust `ss://` TCP，第 1 次 | receiver 3.92 Gbps |
| Gust `ss://` TCP，第 2 次 | receiver 3.80 Gbps |
| Gust `ss://` TCP，第 3 次 | receiver 2.84 Gbps |
| 直连 UDP echo | 20/20，0% loss，p50 0.158 ms |
| Gust `ssu://` UDP echo | 0/100，100% loss |
| UDP listener + Gust `ss://` | 明确返回 `network udp is unsupported` |

### SSU 故障定位

这次排除了 Gust 自身 SSU 服务端、第三方 sing-box 服务端以及公网 UDP 干扰：

1. Gust `ssu://` 客户端发出的 100 个加密 UDP 请求全部被官方 ssserver 成功解密。
2. 官方 ssserver 全部转发至 `205.185.113.193:28721`，并全部收到正确的 16-byte echo 回包。
3. 官方 ssserver 对 100 个回包全部重新加密并发送，计数为 `100 receive / 100 send_to`。
4. Gust 客户端每个 handler 都记录 `inputBytes=16, outputBytes=0`，本地探针最终为 `0/100`。
5. Gust 日志显示每个 UDP handler 在约 1 ms 后结束，同时每个数据包使用新的服务端源端口；标准 SS 回包没有被交回原本的本地 UDP socket。
6. 相同主机间的直连 UDP echo 为 20/20，说明目标服务与公网 UDP 路径正常。
7. `ss://` scheme 明确只接受 TCP；把 UDP listener 接到 `ss://` 会立即返回 `network udp is unsupported`，因此不能作为 SSU 替代。

结论：Gust v3.2.9-porty8 的 `ssu://` 请求编码可与标准 Shadowsocks UDP 服务端互通，但当前客户端回包生命周期/解密后回送路径存在缺陷。此前 Gust SSU server 的 `use of closed network connection` 与本次标准 ssserver 对照共同指向 Gust SSU 实现，而不是 EasyTier/Leaf 或服务端兼容性。

### SSU 源码根因与责任归属

源码走读基于与 release 对齐的精确版本：

- `lovitus/gust`：`0f150132b7c6bf1915b19dfdee2a4f50172ab486`。
- `lovitus/gust-x`：`d4fcbf5e746900e820ddddef36da3107b618e9e9`。
- Gust 使用的 `go-gost/go-shadowsocks2`：`v0.1.1`。
- 上游报告：[`lovitus/gust-x#1`](https://github.com/lovitus/gust-x/issues/1)。`lovitus/gust`
  本身关闭了 GitHub Issues，因此 issue 提交到实际承载 SSU 实现且启用 Issues 的
  `lovitus/gust-x`。

责任判断：

| 组件 | 判断 | 依据 |
|---|---|---|
| shadowsocks-rust `ssserver` | 无故障证据 | 100/100 请求解密、转发、收取目标回包并重新加密发送 |
| sing-box | 无故障证据 | 同一候选已通过标准 SS TCP、native UDP、UoT v2 和 IPv4/IPv6 目标矩阵 |
| Leaf | 本次 SSU 故障不归因于 Leaf | Leaf 不使用 Gust 的 `ssu://` scheme；Leaf native UDP/UoT 已与 sing-box 互通 |
| Gust | 确认故障 | 独立 Gust 客户端对官方 ssserver 为 0/100，且服务端已经发回全部响应 |

已确认的第一处硬错误是 UDP session 保存和查找使用不同 key：

- `internal/util/ss/conn.go::ReadFrom()` 使用
  `SessionHashFromAddrPort(clientAddr)` 查找，即 `<clientAddr>`。
- `internal/util/ss/conn.go::WriteTo()` 使用 `session.Hash()` 保存。
- 对 classic AEAD，`go-shadowsocks2 v0.1.1` 的 `session.Hash()` 实际为
  `aead-<clientAddr>`。

因此官方服务器响应到达 Gust 后无法命中发送请求时保存的 session，回包不会交给本地
UDP socket。这与 `inputBytes=16, outputBytes=0` 和 0/100 结果完全一致。把保存逻辑改为
`SessionHashFromAddrPort(clientAddr)` 是必要修复，但不是充分修复。

同一路径还存在以下明确问题：

1. classic AEAD UDP 的标准明文包含 `SOCKS address header + payload`；旧 wrapper 的客户端
   回包和 Gust 服务端回包没有一致地剥离/补入地址头。只修 session key 可能把地址头一起
   交给应用，Gust 服务端也可能生成不符合标准的响应。
2. `internal/net/udp/listener.go::WriteTo()` 在 `keepalive=false` 时首次写回后关闭虚拟 UDP
   connection，而默认值正是 false。这会造成 association 提前结束、源端口持续变化和
   `use of closed network connection`。配置 `keepalive=true` 只能缓解生命周期问题，不能
   修复 session key 和协议帧。
3. `handler/forward/local/handler.go` 忽略 `xnet.Pipe()` 返回值，导致 session lookup、解密或
   反向复制错误没有进入日志，外部只看到 handler 很快结束。
4. 旧 `handler/ss/udp` 手工维护 session 和 goroutine，并在并发收发路径复用 packet buffer，
   还缺少完整的 timeout/cancel/资源回收闭环。

推荐修复不是继续在旧 SSU 状态机上逐项打补丁，而是把上游已完成的统一 wrapper/session
实现按精确提交移植到 `lovitus/gust-x`，同时保留 Gust 的 porty/sings 定制：

- `go-gost/go-shadowsocks2` `901ceb6205e80eb05576b7d44b2dcb19b9f8463e`：统一 UDP/TCP
  wrapper 和共享 session 状态。
- `go-gost/x` `3c8995027a59e841e8ed51c591fa086f4a65af2a`：适配新的
  `go-shadowsocks2` API。
- `go-gost/x` `3f73c82d00760045ffec2f03e78b560b73cd9e04`：nil、资源泄漏和无效
  代码修复。
- `go-gost/x` `3f671abfb6f91cf23dbbfb3a57495abfd5fd2673`：SSU session cache 的
  goroutine/connection 泄漏修复。

此外，Gust release workflow 当前从未 pin 的 `gust-x main` 构建。后续应固定精确 SHA，
否则同一个 Gust tag 不能保证可复现。修复验收至少包括官方 shadowsocks-rust 的 classic
AEAD UDP 100/100、Gust client/server 双向角色、同一 association 多包/多目标、IPv4/IPv6/
域名、AEAD-2022，以及 timeout/close 后 FD、goroutine 和 session 回基线。

这不改变 EasyTier 当前选择：

- 标准 Shadowsocks TCP：可继续支持。
- 标准 Shadowsocks native UDP：Leaf 与 sing-box 的已有实测保持有效。
- UoT v2：继续使用 sing-box 作为权威互操作服务端。
- Gust `ssu://`：当前不能作为发布门槛或正确性基准，只可保留为已知失败夹具。

### 清理

- `lv1g2` 专用监听 `28710-28712`：0。
- `lv1g3` 专用监听 `28700`、`28720`、`28721`：0。
- 所有客户端、ssserver、iperf3 和 UDP echo 均按 PID 文件精确停止。
- 原始日志和测试输出保留在两台机器的 `/slab2/easytier-validation/standard-ssserver-v1.24.0`。
