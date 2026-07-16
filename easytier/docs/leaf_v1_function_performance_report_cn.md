# EasyTier Leaf v1 功能与性能验证报告

日期：2026-07-16

## 1. 结论

本报告把“代码已实现”“配置能编译”“实机流量通过”和“性能无回退”分开判断。

- Linux 与 Android 的 Leaf v1 基础功能、托管 HEV、mesh 共存、GeoSite/GeoIP、自定义域名/IP、TCP chain、连接级 fallback、显式 mesh UDP、故障恢复和清理已经取得实机证据。
- 境外分流使用 `lv1g2` 与 `lv1g3` 完成，未使用只能访问中国大陆网络的主机代替境外出口验证。
- 最终候选 `1a321cd6acfff4012836028d6615de04fb48be7c` 的 Linux 与 Android workflow、精确 SHA、制品哈希、Linux Build ID/符号/目标和 Android 签名均通过；它替代发生 smoltcp receive-window panic 的早期候选。
- Linux 与 Android 已取得同一环境、同一目标、同一参数下的 policy-off、DIRECT 和 managed HEV A/B 数据，Linux 另有显式 mesh SOCKS、native SOCKS chain 与 fallback/failback 对照。结果支持“首版范围内未见明显全局性能回退”；策略、SOCKS 和额外 mesh hop 的固有成本仍需按路径分别理解。
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

## 2026-07-16 Linux 最终 fallback、资源与退出证据

精确制品：`7b708f4009952aedfe009b03f80b601a29c0a8be`，Linux workflow `29471422081`。三节点测试网为隔离的 `10.250.0.0/24`：策略入口位于 `.37`，managed HEV 备出口位于 `.38`，chain + native SOCKS 主出口位于 `.160`；这里只记录内网地址，不在仓库中保存境外节点公网名称。

### fallback 故障与恢复

- 主 native SOCKS 在线：连续 `5/5` HTTP 200；`.160` 的 GOST 日志确认请求经过 loopback SOCKS 主链。
- 停止主 SOCKS：从 `t=0` 到 `t=28s` 的 `14/14` 请求全部 HTTP 200，随后稳定备份 `10/10` 全部 HTTP 200；总计观察到 `24/24` 成功，没有 Reject 或 timeout。
- 故障期间目标端源地址为 `.38`，证明流量已由 managed HEV 备出口承接，而非本机 DIRECT 假阳性。
- 重启主 SOCKS：两组恢复请求各 `10/10` HTTP 200；目标日志显示源地址从 `.38` 切回 `.160`，约 48 秒后全部恢复到主链。该时序符合“差异探测成功后新连接可先使用临时主节点，但 pinned actor 仍需满足既有观察门槛才替换”的稳定 fallback 语义。

### 运行中资源与正常退出

主链正常时的单点快照：EasyTier core RSS `18984 KiB`、线程 `9`、FD `34`；Leaf RSS `7020 KiB`、线程 `4`、FD `31`；HEV RSS `260 KiB`、线程 `2`、FD `12`。

正常发送 TERM 后，当前精确 core、Leaf、HEV PID 均消失，策略 TUN `etpf1`、当前策略路由/规则、当前临时文件均消失，当前运行创建的 `/tmp/easytier-hev-2MRYhr` 已删除。验证前已经存在的 hash 命名旧目录未归因给本次运行，也未作为本次清理成功的证据。

### 精确 policy-off CPU 基线

同一 workflow 制品、同一 `.37 -> 10.250.0.2:26500` 测试路径：TCP receiver `563 Mbit/s`、sender `562 Mbit/s`，本轮有 `1560` 次 retransmit。EasyTier core CPU `170.91%`、RSS `28828 KiB`、线程 `9`、FD `30`；常驻 managed egress HEV sidecar CPU `0%`、RSS `252 KiB`、线程 `2`、FD `12`。没有 Leaf worker、策略 TUN、策略规则或 `ET_POLICY_*` 环境变量。HEV sidecar 是 mesh 的常驻可选 egress 服务，其存在不能被解释为 policy 已启用。

该基线与此前 policy-enabled mesh bypass 的 `477 Mbit/s`、DIRECT 的 `943 Mbit/s`、managed HEV 的 `497 Mbit/s`、显式 mesh SOCKS 的 `506 Mbit/s`、chain 的 `518 Mbit/s` 一起构成当前 Linux 功能/性能矩阵。不同路径承担的用户态转发工作不同，不能仅按吞吐绝对值推导回退；首版结论是 policy-off 不创建 Leaf/TUN/rule，mesh bypass 保持原 mesh 数据面，策略出口按所选 actor 承担对应的 Leaf/HEV CPU。

### smoltcp 修复候选预检边界

修复候选 `1a321cd6acfff4012836028d6615de04fb48be7c` 将 registry smoltcp 从 `0.12.0` 升级到 `0.13.1`。远程 builder 上 `cargo check --locked --package netstack-smoltcp --package easytier-policy --features easytier-policy/leaf-runtime` 通过，无 EasyTier API 适配；锁文件保留了与本修复无关的 bindgen/itertools 原解析。官方单测源码及 `test_recv_out_of_recv_win` 已核对，但独立编译该依赖测试时 builder 205G 挂载耗尽，失败原因为 `No space left on device`，不是测试断言或源码编译错误。本轮创建的 24 MiB 临时测试 target 已单独删除，没有清理共享缓存。最终门槛仍由该精确候选的 Linux/Android workflow 和 Android DIRECT/managed HEV 实机复测关闭。

## 2026-07-16 最终 Android 同参数矩阵与候选结论

### 精确候选与验证条件

- 最终候选：`1a321cd6acfff4012836028d6615de04fb48be7c`。它在原验证基线 `d307a4e460a230599f595e1f59b832453d20b888` 上仅补入 smoltcp `0.13.1` receive-window 修复及本报告；Linux workflow `29473713550`、Android workflow `29473713549` 均成功。
- Android 制品的 `BUILD_INFO.txt`、SHA256、目标架构和 APK v2 签名均与上述精确候选一致；Linux musl 制品也已核对 commit、build ID、symbols、target 和 SHA256。
- 设备：`192.168.234.227:5555`；测试期间 Wi-Fi 始终开启，thermal status 为 `0`，电池温度约 `32 C`。没有通过断开 Wi-Fi 破坏 wireless ADB。
- 三种模式都使用同一个 Android 进程、同一个 EasyTier 网络和 `stealth_mode: true`，吞吐参数统一为 iperf3 TCP `3 x 10 s`。测试中发现服务端与客户端 stealth 设置不一致会造成握手 reset；恢复用户原配置后 TCP/QUIC mesh 均正常。该现象是测试拓扑配置不一致，不是候选回归。

### Android 功能、吞吐与资源对照

| 模式 | 规则/路径证据 | 3 次接收吞吐 (Mbps) | 中位数 | 进程 CPU | VmRSS (KiB) | FD | 线程 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| policy-off | 无 Leaf policy；访问 mesh 虚拟地址 `10.247.0.1`，服务端看到源 `10.247.0.3` | 82.7 / 83.1 / 83.8 | 83.1 | 99.55% | 269556 | 317 | 65 |
| DIRECT | `MATCH,DIRECT`；目标 `.38:25454` 看到源 `192.168.6.36`，证明由 Android 物理出口直连而非 mesh | 89.3 / 91.3 / 91.3 | 91.3 | 57.84% | 275400 | 330 | 69 |
| managed HEV | `MATCH,linux-hev`，actor 使用 `server.virtual-ip: 10.247.0.1`、`via: mesh`、`udp: true`；`.38` 三次均看到源 `192.168.1.37` | 54.9 / 57.2 / 56.8 | 56.8 | 114.66% | 273068 | 325 | 68 |

补充证据：

- DIRECT 连续 30 秒、3 次传输后 Android PID 始终为 `5862`，logcat 未出现 `panic`、`underflow`、`smoltcp` 或 `SIGABRT`。这覆盖了旧 smoltcp `0.12` 候选可稳定触发的 `SeqNumber::sub -> Socket::window_to_update -> dispatch` 路径。
- managed HEV 三次 sender 吞吐为 `67.6 / 71.0 / 69.3 Mbps`，重传为 `8 / 11 / 14`；`.37` 内置 `easytier-hev-socks-egress` 保持运行，采样为 VmRSS `276 KiB`、FD `22`、线程 `2`。
- Android 的 CPU 是采样窗口内进程总 CPU，可能超过 100%，因为该进程为多线程；RSS/FD/线程是每个场景结束时的点采样。它们适合用于同设备同参数相对比较，不应解释为跨平台绝对基准。
- 正常 stop 后运行实例列表为空且 `tun0` 消失；force-stop 后候选进程退出，Wi-Fi 仍开启。`vpn_management` 保留已授权包的 owner 记录，但 `Active vpn type: -1`、无 TUN、无候选进程和活跃服务，因此它不是残留 VPN 会话。

### smoltcp 修复闭环与证据边界

- 上游问题 `smoltcp-rs/smoltcp#1048`、`#1051` 与修复 PR `#1079` 对应 receive-window/sequence-number underflow；修复已包含在 smoltcp `0.13` 系列。本候选更新到 `0.13.1`，没有在 EasyTier 中复制或魔改 TCP 状态机。
- 远程 builder 上 `cargo check --locked --package netstack-smoltcp --package easytier-policy --features easytier-policy/leaf-runtime` 通过；Linux 精确候选 DIRECT smoke 为 receiver `939 Mbps`、sender `943 Mbps`、零重传，无 panic，与旧正常基线 `943 Mbps` 同量级。
- smoltcp 上游已有 `test_recv_out_of_recv_win` 回归测试。由于 `.160` 的 `/workspace` 205G 磁盘已满，独立执行该上游测试的临时编译没有完成；这里不把它记为已执行。产品路径由 Android 原故障复现矩阵、Linux smoke、远程 locked check 和两套精确候选 workflow 共同闭环。builder 磁盘容量是后续诊断环境维护项，不是 Leaf 产品发布阻塞。

### Leaf v1 最终发布判断

- 在首版边界“Linux + Android、单 policy-enabled 实例、DIRECT/REJECT、基础域名/GeoSite/GeoIP、managed HEV、显式 mesh SOCKS、native SOCKS chain、fallback/failback、Magic DNS 归 mesh”内，当前没有架构级或功能级重大发布阻塞。
- Linux 已覆盖 policy-off、mesh bypass、DIRECT、managed HEV、显式 `via: mesh` 用户 SOCKS 端口、native SOCKS chain、fallback/failback、TCP/UDP、资源采样和正常清理；Android 已完成本节 A/B/C 同参数矩阵和停止清理。境外 `lv1g2/lv1g3` 的 GeoSite、GeoIP、自定义域名/IP、chain、mesh UDP 与 fallback/failback 证据见本报告前文。
- 显式用户 SOCKS 的 UDP 能力仍以实际服务端为准：配置 `udp: true` 不会把不支持 UDP 的 peer SOCKS 自动变成可用。首版按既定语义失败并报告错误，不做隐藏回退；用户需要可靠 UDP 时应选择支持 UDP 的节点或 managed HEV 出口。
- 未纳入首版承诺的 split-DNS、在线 Geo 更新、HTTP actor、netns、多实例、高吞吐 UDP 和所有高级 chain/fallback 组合继续作为实验/后续矩阵，不应在首版文档中宣称完全兼容。
- 性能结论是“未见明显全局回退”，不是“所有策略路径零成本”：Linux policy-off 与原生 mesh 同量级，DIRECT 接近线速；Android policy-off/DIRECT 受无线链路限制约 `83-91 Mbps`，managed HEV 多一层 SOCKS 与 mesh 转发后约 `57 Mbps`。该差异符合路径复杂度，未观察到持续 RSS/FD/线程增长或进程重启。

## 2026-07-16 `c48816f4` 解耦、禁用成本与恢复复核（当前权威补充）

本节覆盖本文较早的“显式端口未验证”“policy-off 固定成本不明确”和“网络切换资源仅有单轮证据”等描述。产品代码保持 `de1a894b` 的 Android DNS 修复边界；`c48816f4300f5853525b62d5793d9778923aed80` 只为 profiling workflow 增加手动、默认关闭的 no-Leaf comparator，没有修改 mesh、Leaf 或策略行为。错误修改 mesh transport 的 `c5e8ba42` 已由 `f47657f2` 完整回退，本轮没有依赖该错误逻辑。

### 精确制品

- Linux workflow：`29486876174`，run number `157`，输入 `audit_comparator=true`。
- 同一 run 同时产生 `easytier-core`（`jemalloc,leaf-policy-proxy`）和 `easytier-core-no-leaf`（`jemalloc`）。两者均为 `x86_64-unknown-linux-musl` static PIE，带独立 Build ID 和 debug info。
- `BUILD_INFO.txt`、commit、run ID、target 和 `SHA256SUMS.txt` 全部匹配；测试只使用这一套精确制品。

### policy-off 固定成本

`.37/.38` 使用相同网络、相同虚拟 IP、相同显式 listener 和相同 peer。三种运行形态分别为：不编译 Leaf、编译 Leaf 但目录/PATH 中没有 HEV、编译 Leaf 且打包 HEV。

| 形态 | core RSS | core threads | core FD | 额外进程 | 10 秒空闲 CPU |
| --- | --- | --- | --- | --- | --- |
| no-Leaf | `16272/16560 KiB` | `9/9` | `25/27` | 无 | 两端 `0 tick` |
| Leaf、无 HEV 文件 | `17912/17644 KiB` | 稳定后约 `10/10` | `26/26` | 无 | `1/2 tick`，约 `0.1/0.2%` |
| Leaf、打包 HEV | core `17960/17564 KiB` | core `9/9` | `26/27` | 每端 HEV `252/256 KiB`、`2 threads`、`12 FD` | core+HEV 每端约 `0.1%` |

因此不能再写“禁用策略等于零资源开销”。portless `via: mesh` 为了让未配置本地策略的 peer 也能被远端选为 managed exit，当前会在 HEV 文件存在时常驻 HEV。它的 CPU/RSS 很小，但 `2 threads + 12 FD` 是真实固定成本。若未来要求严格零常驻成本，应改为由 mesh adapter 在首个 managed-exit 请求时惰性启动，而不是把启动条件简单绑定到本机 `policy-config`；后者会破坏“目标 peer 不需显式打开 exit-node 开关”的既定体验。

### 固定单 tunnel 数据面对照

为排除自动 P2P 发现的多个 UDP/QUIC tunnel 造成的路径抖动，两组均使用 `--disable-p2p true`，保留全部显式 listener，但只建立同一个显式 UDP peer。

| 形态 | TCP 5 秒成功样本 | 成功样本均值 | 控制连接 reset | UDP 50 Mbit/s loss |
| --- | --- | ---: | ---: | --- |
| no-Leaf | `702/721/685/693 Mbit/s` | `700 Mbit/s` | `2/6` | `17%/6.9%/12%` |
| Leaf + HEV，policy-off | `721/710/718/713/675 Mbit/s` | `707 Mbit/s` | `1/6` | `15%/14%/16%` |

TCP 没有可见回退。reset 在 no-Leaf 也出现且次数更多，不能归因给 Leaf。UDP 两组区间重叠，Leaf 组均值约高 3 个百分点，但该单 UDP tunnel 基线本身已经不稳定；现有证据只能写“未证明 Leaf 导致 UDP 回退”，不能写“UDP 无回退”。

### portless 与显式端口 actor

同一个 `.38` peer、同一个 EasyTier UDP mesh、同一个 `203.0.113.10:27000` 受控 HTTP 目标：

- portless actor 使用目标 EasyTier 托管 HEV，`20/20` HTTP 200；目标自建 HEV 日志保持为空。
- 显式 `port: 11081` actor 使用目标自建 HEV，`20/20` HTTP 200；自建 HEV 精确记录 20 条目标连接。
- 两组 mesh RTT 均约 `0.52 ms`，请求都出现约 `3 ms` 与约 `204-208 ms` 两档延迟，没有发现端口省略路径绕开或修改正常 mesh transport 的证据。

结论：有端口与无端口都通过现有 mesh adapter 到达同一 peer；端口只决定目标是用户 SOCKS 还是 EasyTier 托管 HEV。overlay、UDP/QUIC/KCP 和 smoltcp 仍由原 EasyTier mesh 能力与配置决定，Leaf 不应重新选择或改写。

### fallback、断链和 worker ownership

- fallback 主成员为显式 `11081`，备成员为同一 peer 的 portless managed HEV。主成员在线时 `5/5` HTTP 200，目标自建 HEV 日志 `186 -> 191`。
- 停止主 SOCKS 后，新连接 `10/10` HTTP 200，自建 HEV日志不再增长，证明由 managed secondary 接管。
- 用两条精确 iptables 规则只阻断唯一 EasyTier UDP peer 时，mesh `3/3` 丢失，策略 HTTP 在 10 秒内没有 DIRECT 逃逸。删除规则后 1 秒内 mesh 恢复，HTTP 200。
- core PID 和 managed HEV PID 保持不变；Leaf worker 从 `26470` 重建为 `27363`，旧 worker 已退出。恢复语义是 network-generation worker replacement，不是同一个 Leaf runtime 原地修复。
- 正常 SIGTERM 后 core、Leaf、HEV、TUN 和本轮临时配置均清理；Linux 隔离测试结束后 `.37/.38` 无残留进程、TUN、回环地址或防火墙规则。

### 失败 UDP association 的资源窗口

显式 actor 的第三方 HEV 接受 UDP association，但 payload 未到达受控 echo server。按既定 v1 语义直接向用户暴露失败，没有新增协议猜测或隐式 fallback。约 50 次两秒超时后资源峰值为：

- `.37` core FD `29 -> 91`，Leaf FD `23 -> 80`。
- `.38` core FD `25 -> 182`，自建 HEV FD `16 -> 111`。

70 秒时仍未回基线；约 190 秒时大部分回收；约 330 秒时两端 core 和 managed HEV 精确回到原 FD，Leaf 比初始多 3 FD，自建 HEV 多 2 FD，线程稳定、RSS下降。现有证据更像首次 UDP 使用后的常驻 runtime/socket 池，而不是每个 association 永久泄漏，但尚未完成多轮相同 wave 后的线性增长检验，因此不能写“所有 UDP association 资源完全回基线”。

### Android 三次 Wi-Fi 切换

设备保留原有三个 EasyTier 包，没有卸载、覆盖安装或模拟点击；活动包始终为 `com.kkrainbow.easytier.policycandidate`。每次关闭 Wi-Fi 前都先用设备侧 `setsid` 任务安排 10 秒后重新打开 Wi-Fi，截图没有参与控制或判断。

- 三次 Wi-Fi -> outage/蜂窝 -> Wi-Fi 后，PID 始终为 `11615`，`tun0` ifindex 始终为 `190`，地址始终为 `10.44.0.88/16`。
- 新 Wi-Fi network key 携带 DNS `fda9:52cf:9966::1` 和 `192.168.234.1`；Google/百度分别持续返回 204/200，FakeDNS 地址随 generation 重新分配。
- mesh `10.44.0.8` 恢复，但该公网路径前后均约 `20%` ICMP loss、RTT 约 `154 ms`，属于当前网络基线。
- RSS 从首轮前约 `230040 KiB` 上升到约 `238268-238424 KiB` 后趋于平台；后两轮没有线性增长。线程在 `69-71` 间波动。

Android 当前仍有一个需要归因的性能风险：稳定后 20 秒进程 CPU 为 `205 ticks`（约单核 `10.25%`），此前样本约 `14%`；同期有 25 次 SELinux denial，包括 12 次 `/proc/net` read、7 次 packet socket create 和 6 次 `sysfs/net` search。线程采样中两个长期 EasyTier `tokio-rt-worker` 各约 2%，Leaf worker 约 0.3%。这说明高空闲 CPU 主要不在 Leaf worker，但仍需同一 APK、同一 mesh 配置的 policy-off 对照才能判断是否由 policy 引入的全局网络观察 hook 放大；在取得该对照前，既不能归因给 Leaf，也不能宣称 Android 无空闲性能回退。

### 解耦与兼容结论

- 规则、DNS、Geo 数据、outbound 和 fallback 仍在 `easytier-policy`/Leaf 边界；EasyTier core 的职责应限于配置、TUN/worker lifecycle、mesh actor bridge 和 network-generation 通知。
- `via: mesh` 的有端口/无端口实测均复用原 mesh 数据面，错误 transport 补丁已经回退，没有证据显示当前 Leaf 改写 overlay/KCP/smoltcp 选择。
- 仍需收窄的耦合包括：policy 驱动但作用于普通 mesh 的全局 OSPF generation restart、policy-only KCP endpoint，以及 policy-off 时 managed HEV 常驻。它们必须保留明确 gate/adapter 边界，不能继续向普通 mesh 热路径扩散。
- 当前 Unix datagram/raw-FD bridge 和平台 `cfg` 只支持 Linux/Android v1 证据。Windows/macOS 不能据此声明运行时兼容；全平台设计应替换 transport adapter，而不是在 mesh core 复制平台分支。
- 当前 no-Leaf comparator 仍基于最新 core，包含最新 core 的全局 hook；它证明 feature/sidecar 相对成本，但不是 EasyTier 2.9.10 精确旧二进制对照。因此配置和 mesh 行为未发现明显破坏，不等于已经证明与 2.9.10 性能完全一致。

### 新增 core hook 逐项审计

审计基线为引入 Leaf 前的 `030d17e7`，范围覆盖 `instance`、`virtual_nic`、通用 SOCKS dataplane、KCP、OSPF route、underlay guard、peer RPC 和平台 runtime。结论不是“没有看到明显问题”，而是逐项判断是否满足三条硬边界：Leaf 只拥有规则/DNS/outbound，policy adapter 只拥有 bridge/lifecycle，mesh 继续拥有 route/overlay/KCP/smoltcp。

| hook | 判定 | 理由与处理 |
| --- | --- | --- |
| policy 配置 envelope、CLI/RPC 校验、单实例 lease | 保留：必要 lifecycle adapter | 不进入 peer route 或 overlay 算法；未启用时不创建 runtime。 |
| `POLICY_SOCKET_MARK` 传播到 underlay socket | 保留：必要 loop-prevention adapter | 只在启用 policy 时设置，防止 Leaf 捕获 EasyTier 自身 underlay；不改变路径选择。 |
| `VirtualNic` 的 `PacketClassifier`、Leaf writer queue、peer/policy TUN 合流 | 保留：必要 packet bridge | policy 关闭时 `policy=None`，不创建队列和额外转发任务；开启后有界队列 fail-closed。固定 tunnel A/B 未见吞吐回退。 |
| `data_plane_tcp_connect_mesh_only` 禁止 kernel fallback | 保留并通用化命名 | 这是 `via: mesh` fail-closed 的必要能力，仍调用原 `Socks5AutoConnector`/route/smoltcp；不应带 Leaf 规则语义。 |
| `MeshSocksRelayService` 与 peer RPC open/close UDP association | 保留 bridge 能力，但应去 policy 命名 | UDP association/UoT 需要跨 peer 生命周期控制；它不应叫 `PolicyUdpRelayRpc`，更合适的边界是通用 `MeshSocksRelayRpc`，供任何 mesh SOCKS consumer 使用。 |
| managed HEV 与 TCP ingress | 保留能力，收窄为 lazy lifecycle | 当前 feature build 无条件启动 sidecar，造成 policy-off `2 threads + 12 FD`。严格零常驻要求下应在首个 managed request 惰性启动，而不是要求本机 policy 或用户打开 exit-node 开关。 |
| `policy_kcp_endpoint` / `start_endpoint_only` | 可选性能 adapter，不是首版正确性依赖 | KCP 仍由 mesh endpoint 处理，但 policy 专用 endpoint 被直接存入通用 `Socks5Server`。应改为注入通用 mesh stream capability；首版也可退回 smoltcp，而不应扩散 policy 字段。 |
| KCP 首次尝试超时与失败后 smoltcp retry | 已从通用 connector 解耦 | `Socks5AutoConnector` 只按 endpoint/capability 连接；5 秒 KCP 尝试与一次 smoltcp fallback 归 `gateway::socks5::dataplane` 所有。`.160` 的 endpoint isolation、mesh-only fail-closed 和 UoT smoltcp burst 回归均通过。 |
| peer 删除后 OSPF `restart_local_generation` | 不属于 policy adapter | 这是普通 mesh route correctness 修改，policy-off 也执行。独立精确回归 `peer_removal_restarts_remaining_generation_and_invalidates_remote_cache` 已通过；发布前仍需放入 no-Leaf 部署比较，不能作为 Leaf 必需 hook 混入兼容结论。 |
| underlay preflight 的 `source_interface_signal()` | 不属于 policy adapter，移动端原生接口探测已门控 | 先前把 Android 扫描归因于 `collect_local_ip_addrs_now()` 不准确：bind-address 刷新本来就在 Android 禁用；实际每次 preflight 调用 pnet 的是软判据 `source_interface_signal()`。本候选在 Android、iOS、macOS Network Extension 和 OHOS 禁用它，保留 managed-IP 等硬判据；桌面 bind-address 刷新仍应按 network generation 缓存。 |
| public IPv6 updater 接受 `policy_owns_default_route` | 保留但抽象为 route-owner capability | 避免两个组件同时拥有默认路由是必要 lifecycle 协调；不应让 IPv6 updater直接理解 Leaf，只应接收“外部 route owner”状态。 |
| netlink route/rule 列举与 replace helper | 保留：平台 route adapter | 只有 policy routing 调用时产生行为，普通 mesh 热路径无新增工作。 |
| QUIC bind mode、loopback 不使用 `SO_BINDTODEVICE` | 独立通用网络修复 | 与 Leaf 规则无关。应单独保留测试和变更说明，不能拿它证明 Leaf 兼容，也不能把其风险归给 Leaf。 |

因此，“所有新增 mesh hook 都是必要 adapter”当前不成立。至少以下三项必须在最终兼容结论前处理：

1. 把通用 `Socks5AutoConnector` 内的 policy KCP timeout/fallback 移回 actor adapter，或将其重构为无 policy 语义的 transport strategy。
2. 将 OSPF generation restart 作为独立 mesh correctness 变更审查和验证，不再作为 Leaf 实现的一部分。
3. 将 uncached underlay interface scan 改为 network-generation 级缓存刷新；先用同 APK policy-off/`bind_device` 对照确认 Android CPU 与 denial 因果。

### Windows/macOS 未来平台边界

当前实现不能称为全平台宿主已解耦：

- `LeafPacketBridge` 只在 Unix 构建，底层是 `UnixDatagram::pair()` 和 raw FD。
- 外部 Leaf worker 继承固定 TUN FD；Linux 与传统 macOS 可以采用该模型，Windows 没有对应实现。
- Android in-process runtime 同样以 `RawFd` 为入口，并维护全局 runtime ID/reaper；这是移动宿主实现，不是跨平台接口。
- `run_for_mobile` 只有 Android 会启动 policy runtime；iOS 和 macOS Network Extension 虽能得到 TUN FD，当前没有等价 policy host。
- `VirtualNic` 中大量 `cfg(unix/android/linux/macos)` 直接包围 policy 字段和启动逻辑，新增 Windows 或 Apple NE 时会继续复制分支。

目标结构应先固定平台无关接口，再由各平台实现：

```text
PolicyRuntimeHost
  start(document, PacketIo, NetworkContext)
  update_network(NetworkContext)
  stop()

PacketIo
  send_to_policy(packet)
  recv_from_policy(packet)

ManagedMeshEgressHost
  ensure_started()
  endpoint()
  stop()
```

- Linux/传统 macOS：Unix datagram + external worker adapter。
- Android：in-process Leaf + VPN FD adapter。
- Windows：Windows TUN/channel 或 in-process FFI adapter，不模拟 Unix FD。
- iOS/macOS NE：packet-flow/NE-provided FD adapter，不启动不允许的外部进程。

mesh actor 只依赖 `PacketIo` 和通用 mesh stream/datagram capability，不能依赖 Unix socket、Android runtime ID 或 Leaf process。完成该 trait 边界前，只能声明 Linux/Android v1 运行证据，不能宣称未来 Windows/macOS 不会被当前特殊设计绑住。

### 网络变化语义参考与 EasyTier 整改边界

按仓库要求核对的参考实现与可观察语义：

- Mihomo `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go`：`tun.NewNetworkUpdateMonitor` 驱动 `tun.NewDefaultInterfaceMonitor`；默认接口变化回调才调用 `iface.FlushCache()` 和 `resolver.ResetConnection()`。它不在每次 dial/reconnect 失败时扫描接口。
- Mihomo `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/iface.go`：`defaultInterfaceFinder.Update` 明确负责 flush 后重建接口缓存；普通 `Interfaces`/`ByAddr` 使用缓存，只有发现系统对象存在而缓存缺失时才更新。
- Mihomo `/Users/fanli/Documents/mihomo-rev/component/dialer/dialer.go`：dial/listen 从原子 `DefaultInterface` 或 `DefaultInterfaceFinder` 取得当前接口，loopback 明确清空 interface bind；拨号器消费当前状态，不拥有网络变化轮询。
- Android 宿主 `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/clash/module/NetworkObserveModule.kt`：`ConnectivityManager.NetworkCallback` 接收 `onAvailable`、`onLosing`、`onLost`、`onLinkPropertiesChanged`，通过 conflated channel 和 2 秒 debounce 计算 network key/DNS，再通知 core。
- Android 宿主 `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`：旧 Android 版本把选中的系统 `Network` 传给 `setUnderlyingNetworks`，VPN ownership 与网络观察保持在宿主层。

EasyTier 可以因多 peer、bind-device 和 OSPF generation 与 Mihomo 不同，但不能把“网络变化恢复”实现为每次连接重试都无缓存枚举系统接口。应遵循同一外部语义：

1. Android 继续由现有 VPN network callback 提供 network key、DNS 和可用网络信息；network generation 变化时只触发一次接口/IP cache invalidation。
2. Linux/传统 macOS 由 netlink/route monitor 或现有 underlay generation 事件触发 invalidation；拨号器只读取该 generation 的缓存快照。
3. 移动端/受限 host 不调用 `source_interface_signal()` 的原生 pnet 枚举，由 host network callback 提供 generation；桌面端 `bind_device_sources` 在同一 generation 内不得重复枚举接口。首次构建失败可有界重试，但不能随每个 peer reconnect 放大。
4. 网络丢失时保持 fail-closed；恢复后新 generation 丢弃旧 bind address、DNS connection 和 Leaf runtime，不能保留旧地址继续拨号。
5. 修复验证必须同时记录 Android policy-off 与 policy-on 的 CPU、SELinux denial 数、mesh/DNS恢复时间，证明减少轮询没有牺牲切网恢复。

这是与 Mihomo 一致的“事件驱动失效、拨号消费快照”语义；有意差异仅是 EasyTier 还需把 generation 传播给 OSPF/Leaf lifecycle，而不是把接口发现放进 policy 或 connector 热路径。
