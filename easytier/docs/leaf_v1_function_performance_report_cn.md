# EasyTier Leaf v1 功能与性能验证报告

日期：2026-07-16

## 1. 结论

本报告把“代码已实现”“配置能编译”“实机流量通过”和“性能无回退”分开判断。

- Linux 与 Android 的 Leaf v1 基础功能、托管 HEV、mesh 共存、GeoSite/GeoIP、自定义域名/IP、TCP chain、连接级 fallback、显式 mesh UDP、故障恢复和清理已经取得实机证据。
- 境外分流使用 `lv1g2` 与 `lv1g3` 完成，未使用只能访问中国大陆网络的主机代替境外出口验证。
- `d307a4e460a230599f595e1f59b832453d20b888` 的 Linux 与 Android workflow、精确 SHA、制品哈希、Linux Build ID/符号/目标和 Android 签名均通过。
- 当前性能数据能够证明托管 HEV 和策略路径可实际工作且没有灾难性吞吐问题，但还不能严格证明“相对 policy-off 或 EasyTier 2.9.10 无明显性能回退”，因为缺少同一环境、同一目标、同一参数下的 A/B 基线、CPU 利用率和 chain 最大吞吐对照。
- 显式 `via: mesh` 加用户指定 SOCKS 端口的独立场景尚未单独执行。已通过的是端口省略的托管 HEV mesh actor，以及经 mesh 到 peer 本地 native SOCKS 的 chain。

因此，冻结的 Linux/Android Leaf v1 功能边界基本闭环，但原目标中的完整性能回退结论和全部 SOCKS 形态验证尚未完成，不能把整个目标标记为完成。

## 2. 发布与配置边界

首版声明范围：

- 平台：Linux、Android。
- 单进程、单 policy-enabled 实例。
- DIRECT、REJECT、基础域名规则、GeoSite、GeoIP、自定义域名/IP。
- portless `via: mesh` 托管 HEV actor。
- native SOCKS actor。
- TCP chain 与连接级 fallback。
- UDP 使用显式 `NETWORK,udp` 规则选择已经验证的 mesh actor。
- Magic DNS 继续归 EasyTier mesh。

首版不声明：

- SOCKS-over-SOCKS UDP chain。
- UDP payload 丢失后的自动 fallback。
- 进行中连接或多连接事务的无缝迁移。
- 完整 split-DNS、在线 Geo 更新、HTTP actor、netns、多实例。
- Windows、macOS、iOS、OHOS 等平台的实机发布保证。

## 3. 用户部署入口

- 完整部署、最小启用、字段和限制说明：[`leaf_policy_proxy_cn.md`](leaf_policy_proxy_cn.md)。
- 可直接解析并由 Rust 测试编译的示例：[`leaf_policy_v1_example.yaml`](leaf_policy_v1_example.yaml)。
- 发布门槛：[`todo/leaf_v1_release_gates.md`](todo/leaf_v1_release_gates.md)。
- 原始验证证据：[`todo/leaf_validation_journal.md`](todo/leaf_validation_journal.md)。

最小使用路径：

1. EasyTier mesh 按原配置启动。
2. 在 EasyTier 配置中启用 `[policy_proxy]` 并指定策略 YAML。
3. 最小策略只使用 DIRECT、REJECT 或一个 portless `via: mesh` actor。
4. 需要 peer 本地 SOCKS 时，定义 native SOCKS actor，并放在 mesh hop 后组成 TCP chain。
5. 将 chain 与另一个 `mesh-direct` 组成 fallback。
6. GeoSite、GeoIP、自定义域名/IP 的 TCP 规则指向该 fallback；UDP 单独指向 `mesh-direct`。

## 4. 实机功能验证矩阵

| 场景 | 平台/主机 | 结果 | 证据摘要 |
| --- | --- | --- | --- |
| Leaf 关闭时 mesh 保持原行为 | Linux、Android | 通过 | mesh ICMP/TCP 与 VPN ownership 正常 |
| Leaf 开启后 mesh 与策略共存 | Linux、Android | 通过 | mesh 流量与 policy TUN 流量同时可用 |
| DIRECT、REJECT、GeoSite、GeoIP | Linux、Android | 通过 | 配置编译、目标流量和 captured-UID probe 通过 |
| portless `via: mesh` 托管 HEV TCP | Linux | 通过 | TCP 接收吞吐 313 Mbit/s |
| portless `via: mesh` 托管 HEV UDP/UoT | Linux、Android | 通过 | Linux 与 Android 均取得真实 UDP 接收证据 |
| native SOCKS | Linux、`lv1g2` | 通过 | peer 本地 GOST 仅监听 loopback，日志记录实际目标 |
| TCP chain `[mesh-hop, peer-local-socks]` | Linux、`lv1g2` | 通过 | GitHub、example.com、8.8.8.8 和自定义 IP 均从 `lv1g2` 出口出现 |
| fallback `[peer-chain, mesh-direct]` | Linux、`lv1g2`、`lv1g3` | 通过 | 停止首选 SOCKS 后，新的单连接 HTTP 经 `lv1g3` 返回 200 |
| fallback 恢复与 failback | Linux、`lv1g2`、`lv1g3` | 通过 | 恢复 SOCKS 后连续四次请求重新出现在 `lv1g2` 日志 |
| `GEOSITE,github` | Linux、境外出口 | 通过 | HTTPS 200，`lv1g2` SOCKS 记录 `github.com:443` |
| `GEOIP,google` | Linux、境外出口 | 通过 | `8.8.8.8:443` 通过主 chain，目标出现在 SOCKS 日志 |
| 自定义 DOMAIN | Linux、境外出口 | 通过 | `example.com` HTTPS 200，目标出现在 SOCKS 日志 |
| 自定义 IP | Linux、境外出口 | 通过 | 受控 TCP 目标观察到 `lv1g2` 源地址 |
| 显式 UDP 到 `mesh-direct` | Linux、`lv1g3` | 通过 | 受控 UDP 目标观察到 `lv1g3` 源地址 |
| worker kill 与恢复 | Linux | 通过 | Leaf worker PID 变化，mesh 与 policy 恢复 |
| 默认路由丢失、恢复与 fail-closed | Linux | 通过 | mesh 保持可用，DIRECT 策略失败关闭，路由恢复后恢复 |
| Wi-Fi 中断与恢复 | Android | 通过 | 先安排设备侧重新打开 Wi-Fi；ADB 恢复后同进程/VPN/TUN 和探针通过 |
| stop/start 与配置保留 | Android | 通过 | 停止后 VPN/TUN 清理，配置保留，重新启动恢复 |
| 正常退出清理 | Linux、Android | 通过 | 测试进程、TUN、策略规则、路由表和临时服务清理 |
| 显式 `via: mesh` 加用户 SOCKS 端口 | 未单独执行 | 未完成 | 文档已说明，但不能用 portless 托管 HEV 或 native SOCKS chain 代替该证据 |

## 5. 境外分流拓扑与结果

验证拓扑：

```text
Linux policy client
  overseas-fallback
    1. peer-chain
       mesh-hop -> lv1g2 managed HEV -> lv1g2 loopback native SOCKS
    2. mesh-direct
       managed HEV -> lv1g3
```

规则分配：

- 国内和默认组：DIRECT。
- GeoSite GitHub、GeoIP Google、自定义域名和自定义 IP：`overseas-fallback`。
- UDP：显式 `NETWORK,udp,mesh-direct`。

资源加载：

- GeoIP CN：112008 条。
- GeoSite GitHub：63 条。
- GeoSite `geolocation-!cn`：26948 条。

故障边界：

- fallback 按新连接选择出口，不迁移已建立连接。
- 单连接 HTTP 在首选出口停止后成功回退。
- `iperf3` 控制连接和数据连接跨越首次切换窗口时可能落到不同成员，需要重试整个事务。
- UDP association 已建立但 payload 被丢弃时，fallback 不一定能观察到 actor 建立错误，因此首版 UDP 不依赖自动 fallback。

## 6. 性能与资源结果

| 路径 | 参数 | 结果 | 可得结论 |
| --- | --- | --- | --- |
| Linux 托管 HEV TCP | 内网受控目标 | 313 Mbit/s receiver，0 retransmission | 托管 HEV TCP 可达到数百 Mbit/s |
| Linux 托管 HEV UDP/UoT | 10 Mbit/s | 4641/4641，0 loss | 低负载 UDP 路径稳定 |
| Linux 托管 HEV UDP/UoT | 20 Mbit/s，20 秒 | 119/37692 loss，0.32% | 中等负载可用；不能据此声明零丢包 |
| Android 托管 HEV UDP | 10 Mbit/s，5 秒 | 0/4960 loss | Android policy TUN UDP 可用 |
| Android 托管 HEV UDP | 20 Mbit/s，10 秒 | 0/19840 loss | Android 20 Mbit/s 测试无接收丢包 |
| 境外 native SOCKS chain TCP | 受控自定义 IP，约 20 Mbit/s 测试负载 | 目标观察到 `lv1g2` 出口 | 证明 chain 数据面；不是最大吞吐测量 |
| 境外 `mesh-direct` UDP | 约 4.9 Mbit/s | 2321/2321，0 loss | 证明 `lv1g3` UDP 出口选择；不是容量上限 |

Linux policy client 境外验证前清理快照：

- EasyTier core：20872 KiB RSS，10 threads，32 FD。
- Leaf worker：17196 KiB RSS，4 threads，25 FD。
- HEV：256 KiB RSS，2 threads，12 FD。

Linux worker 恢复后的另一组快照：

- EasyTier core：约 19 MiB RSS，31 FD，9 threads。
- Leaf worker：6308 KiB RSS，12 FD，4 threads。
- HEV：252 KiB RSS，12 FD，2 threads。

Android 生命周期快照：

- Wi-Fi 恢复前后约 220076/222932 KiB RSS、369/359 FD、68/69 threads。
- stop/start 前后约 224128/222572 KiB RSS、366/371 FD、69/69 threads。
- 单轮数据没有显示持续单向增长，但不能替代长时间 soak。

## 7. 不能从现有数据推出的结论

现有数据不能严格证明以下说法：

- “Leaf 开启相对 policy-off 没有明显性能回退”。
- “与 EasyTier 2.9.10 在相同环境下性能完全一致”。
- “native SOCKS chain 的容量上限是 20 Mbit/s”。
- “所有 UDP 负载都不会丢包”。
- “fallback 能无缝迁移进行中的连接或多连接事务”。
- “显式用户 SOCKS 端口与 portless 托管 HEV 已取得完全相同的平台证据”。

## 8. 仍需补测的最小矩阵

要关闭原目标中的完整性能结论，需要在同一候选、同一机器、同一目标和同一测试参数下执行：

| 编号 | 路径 | 需要记录 |
| --- | --- | --- |
| A | policy disabled 的原生 mesh TCP/UDP | throughput、loss、CPU、RSS、FD、threads |
| B | policy enabled 的 DIRECT | 同 A，用于测量策略框架固定成本 |
| C | portless managed HEV mesh actor | 同 A，用于与原生 mesh 对照 |
| D | 显式 `via: mesh` 加用户 SOCKS 端口 | TCP/UDP 实际出口、故障行为、资源 |
| E | `mesh-hop -> native SOCKS` chain | 最大稳定 TCP throughput、CPU、资源 |
| F | `peer-chain -> mesh-direct` fallback | 正常、首选停止、新事务重试、恢复 failback |

建议将 Linux A-F 放在一次隔离验证轮次中完成，避免反复构建和部署。Android只补 A-C 的同参数对照与资源快照；境外 `lv1g2`/`lv1g3` 继续用于验证出口身份和故障切换，不把跨境公网带宽当作核心容量基准。

## 9. 当前完成度判断

- 功能实现与冻结边界：基本完成。
- Linux/Android 基础实机验证：完成。
- 境外 GeoSite、自定义域名/IP、chain/fallback/UDP 分流：完成。
- 部署和字段文档：完成。
- 独立功能/性能报告：本文件已补齐。
- 显式用户 SOCKS 端口独立验证：未完成。
- 同机 A/B 性能回退报告：未完成。

在最后两项完成前，原始目标不能标记为全部完成。
## Fallback 可用性修复依据（候选后续修订）

- 参考实现：`/Users/fanli/Documents/mihomo-rev/adapter/outboundgroup/fallback.go` 的 `findAliveProxy`、`DialContext` 与 `ListenPacketContext`。Mihomo 选择当前健康的第一个出口，并把拨号或首次写入失败反馈给健康状态；它不会在已经找到可工作的备用出口后故意拒绝观察期内的新连接。
- Leaf 当前实现：`/Volumes/micron512g/code/leaf/leaf/src/proxy/failover/stable.rs` 的 `StableFailover::decide` 在主出口失败、备用出口成功并进入 `provisional` 后，仍在 15 秒观察窗口返回 `Reject`。实机表现为第一次备用请求成功，后续请求出现超时或空响应。
- EasyTier 的有意差异：不引入依赖公网 URL 的主动健康检查，继续使用实际业务连接进行被动判断；保留 15 秒观察间隔、至少 3 次且跨度至少 30 秒才永久切换 `selected` 的防抖状态机。
- 修复边界：`provisional` 已经成功后，观察窗口内的新连接临时使用该备用出口；到达观察点后仍重新比较首选与备用出口。此修改只改变“已验证备用可用却拒绝新连接”的行为，不降低永久切换和回切门槛。
## 2026-07-16 权威进度更新（覆盖本文较早的待验证描述）

最终候选正在从 EasyTier `7b708f4009952aedfe009b03f80b601a29c0a8be` 构建；它只把 Leaf pin 从 `b1e33b50...` 更新为包含 fallback 可用性修复的 `2f62208187f7980d066e479bd70bb55613c066d2`。Linux workflow 为 `29471422081`，Android workflow 为 `29471422078`。在这两个 workflow、精确制品哈希和实机复测完成前，本报告不作“可发布”结论。

### Linux 同参数功能与性能结果（基线候选 `d307a4e460a230599f595e1f59b832453d20b888`）

验证拓扑使用 `.37` 作为入口、`.38` 作为 managed HEV 出口、`.160` 作为显式 SOCKS/目标端；所有 EasyTier listener 均使用隔离端口。TCP 为三次 iperf3 接收端中位数，UDP 为 20 Mbit/s 接收端结果。

| 场景 | TCP 中位数 | UDP | 一次 TCP 运行的 CPU / 资源快照 | 结论 |
| --- | ---: | ---: | --- | --- |
| policy-off 原生 mesh | 461 Mbit/s | 中位丢包约 0.77% | 旧采样只有进程生命周期均值，最终候选需补精确 delta | 基线通过 |
| policy-on，mesh 目的地址绕过策略 | 477 Mbit/s | 未重复 | core 161.10%，Leaf 0.41%，HEV 0%；RSS 39460/6816/256 KiB | 相对 policy-off 吞吐 +3.5%，无可见 mesh 回退 |
| DIRECT 到 `.160` | 943 Mbit/s | 三次 0 丢包 | core 43.75%，Leaf 76.20%，HEV 0%；RSS 27492/9512/256 KiB | 通过 |
| portless managed HEV，经 `.38` | 497 Mbit/s | 最大约 0.095% | core 120.10%，Leaf 41.20%，HEV 0%；RSS 39196/7696/260 KiB | 通过；目标观察到来源为 `.38` |
| 显式 `via: mesh` 用户 SOCKS，经 `.160:26480` | 506 Mbit/s | 启用服务端 UDP 后中位丢包 0.27% | core 118.42%，Leaf 39.64%，HEV 0%；RSS 42112/7632/252 KiB | TCP/UDP 通过；目标观察到来源为 `.160` |
| `mesh-hop -> native SOCKS` TCP chain | 518 Mbit/s | v1 不承诺 UDP chain 自动修复 | core 112.87%，Leaf 39.95%，HEV 0%；RSS 41092/7580/252 KiB | 通过 |

上述 policy-on mesh 绕过结果与 policy-off 基线的差异在正常波动范围内；策略流量经 Leaf/TUN/出口转发，不能与同机 10 Gbit/s underlay 直连结果混用。早期把 `.38` 的物理地址作为目标会绕过 mesh/HEV，该组数据已废弃，并改为 `.160` 独立目标。

### 显式 SOCKS UDP 能力边界

EasyTier 配置 `udp: true` 只声明该 SOCKS 出口允许 UDP，不会把不支持 UDP 的外部 SOCKS 服务变成可用。验证用官方 GOST v3.2.6 在默认配置下明确记录 `socks5: UDP relay is disabled`，三次 UDP 均超时；按 [GOST SOCKS5 handler 文档](https://v3.gost.run/en/reference/handlers/socks5/) 使用 `?udp=true` 后，三次均成功，目标来源为 `.160`。这符合首版约定：UDP 失败直接暴露给用户，不做协议猜测或隐式改写；需要可靠 UDP 时使用 managed HEV、支持 UDP 的 SOCKS，或显式 TCP/UoT/KCP 方案。

### Fallback 实机发现与候选修复

旧候选中，primary native SOCKS 停止后，第一次新 HTTP 连接可经 managed HEV secondary 成功，但后续连接在 15 秒观察窗口出现超时、空响应或拒绝。目标 HTTP 服务从 `.38` 和 `.160` 本地始终可用，因此故障定位到 Leaf `StableFailover::decide`：备用出口真实成功并进入 `provisional` 后仍返回 `Reject`。

Leaf `2f622081...` 保留永久切换的三次、至少 30 秒防抖门槛，但观察期新连接使用已经成功的 `provisional` 备用出口。`.160` 已完成 EasyTier 实际 feature 集合的 `cargo check --locked --offline`；最终结论仍需由 `7b708f40...` 精确 Linux 制品验证连续 fallback、30 秒稳定切换、primary 恢复回切和资源回基线。

### 尚未闭环

- Linux：补 policy-off 精确 CPU delta；验证修复后 fallback 连续请求、稳定切换、failback；stop 后核对 RSS/FD/线程/TUN/路由/临时文件回基线。
- Android：安装 `7b708f40...` 精确签名 APK，完成 policy-off、DIRECT、managed HEV 的同参数功能、吞吐、CPU、RSS、FD、线程对照；断网测试必须在同一脚本中重新打开 Wi-Fi，截图只用于最终状态确认。
- 在上述证据完成前，首版仍有一个验证门槛，但没有新的架构级实现阻塞。

## 2026-07-16 Android DIRECT TCP 崩溃与 smoltcp 上游修复边界

> 本节是修改 netstack 行为前的参考记录。Android 精确候选 `7b708f4009952aedfe009b03f80b601a29c0a8be` 在 DIRECT TCP 吞吐测试中触发进程级 panic，因此该候选不能发布，Android managed HEV 场景暂停到修复候选产生后再继续。

### 实机复现

- 制品：Android workflow `29471606443` 的精确候选，APK、`BUILD_INFO.txt`、SHA256 和签名均已核对。
- policy-off 基线：mesh TCP receiver `72.5 Mbit/s`，CPU `78.32%`，RSS `278584 KiB`，FD `311`，线程 `65`。
- DIRECT：UID probe 成功，iperf3 已建立并运行约 2 秒，随后候选进程退出；目标端确认连接源为 Android 当前外网地址，而不是 mesh 地址。
- 当前候选 panic：smoltcp `0.12.0` 的 `wire::tcp::SeqNumber::sub` 在 `Socket::window_to_update -> dispatch -> Interface::socket_egress -> netstack_smoltcp::TcpListenerRunner` 路径发生 sequence-number 下溢。该条记录与日志中旧候选的 `PolicyDocument::actor_supports_udp` panic 无关。

### 上游与 Mihomo 参考

- smoltcp issue `#1048` 与 `#1051` 报告了相同的 `window_to_update`/sequence underflow，其中 `#1051` 同样来自 Android TUN 场景。
- smoltcp PR `#1079`（merge commit `ac32e643a4b7e09161193071526b3ca5a0deedb5`）修正了根因：接收数据可用窗口必须按“最近一次实际通告给对端的 ACK 与 scaled window”计算，不能按本地 receive-buffer 当前容量推导，否则应用读取数据后会错误接受对端尚不可见的额外字节，并破坏后续窗口更新的不变量。
- 上游回归测试 `smoltcp::socket::tcp::tests::test_recv_out_of_recv_win` 覆盖延迟 ACK、应用释放一个接收字节、对端继续发送越过最近通告窗口一个字节的序列；修复后最后一个字节不被接受且窗口更新不 panic。
- 修复已包含在官方 smoltcp `0.13.0`，本项目远程最小预检使用兼容版本 `0.13.1`，`netstack-smoltcp` 无源码改动通过 `cargo check`。
- Mihomo 参考入口：`/Users/fanli/Documents/mihomo-rev/listener/parse.go` 的 `IN.NewTun`/`C.TunGvisor`，以及 `listener/sing_tun`。Mihomo 使用 gVisor/sing-tun 而非 smoltcp，外部可观察语义是异常或窗口边界流量不得让整个 VPN 进程 panic。

### 有意差异与修复选择

EasyTier 的 Leaf TUN 数据面已经通过 `netstack-smoltcp` 集成 smoltcp；为了维持现有解耦边界，不引入 Mihomo 的整套 gVisor/sing-tun，也不在 EasyTier 内重写 TCP sequence/window 算法。采用包含官方修复和官方回归测试的 smoltcp `0.13.1`，失败行为保持为丢弃最近通告窗口之外的数据并继续连接状态机。兼容门槛是远程 `--locked` crate/Leaf 预检、Android 同参数 DIRECT 吞吐复现、managed HEV 复测和 policy-off 对照；在这些证据完成前不得声称 Android 门槛关闭。
