# P2P endpoint 失败冷却

## 状态

**核心候选 `bc829e58` 已完成远端与实机验证；新增兼容开关必须随最终发布快照重新完成
`.160`、GUI 和正式 artifact 门禁。**

上一版候选 `518bf8df` 只覆盖 TCP/UDP hole-punch initiator，明确排除了 Direct，
因此没有覆盖实测风暴的主要来源，已经回退。当前候选从该回退基线重新实现，不继承
上一版的 task 级限流。

## 目标

只增加一张有界、高效的 endpoint 失败表：

- 记录自动 P2P 实际尝试的 peer、协议和远端 `IP:port`；
- Direct candidate 的完整内部重试失败，或 hole-punch 已耗尽原有渐进重试阶段后，
  按 `1、2、4、8、10、10...` 分钟冷却；
- 冷却只跳过这个 endpoint，不阻断同 peer 的其他地址和协议；
- 成功立即删除对应表项，新 endpoint 首次尝试不受旧 endpoint 影响。

不重构 P2P 调度，不改变打洞算法，不管理普通连接，也不重新解释：

- `lazy_p2p`
- `need_p2p`
- `disable_p2p`
- UDP/TCP hole-punch 开关
- transport priority
- 协议 upgrade、fallback/降级和 probe 的触发条件
- RTT 与网络抖动防切换
- 现有协议内部重试、广播、端口猜测、fanout、重发和等待窗口

已有代码仍是任务是否应当执行的唯一准入来源。失败表不读取当前连接是 stable、
relay 还是失联，也不关心尝试来自冷启动、恢复、upgrade、fallback 或 probe。

## 兼容开关

提供一个实例级逃生开关，默认保持节流启用：

```text
TOML: [flags] disable_p2p_storm_throttle = true
CLI:  --disable-p2p-storm-throttle
ENV:  ET_DISABLE_P2P_STORM_THROTTLE=true
GUI:  Disable P2P Storm Throttle
```

该开关只在 `P2pEndpointRetryTable` 的唯一判定入口生效，不散布到 Direct、UDP/TCP
hole-punch 或 P2P 调度代码。开启时原子切换为无状态放行并清空旧冷却；`begin` 始终
允许，后续成功或失败均不写表。再次关闭后从空表重新开始，旧冷却不会复活。这样开启
开关的可观察行为等同于没有实现本失败冷却，不改变任何现有设置、协议尝试或内部重试。

## 实测边界

在 macOS 场景机上显式使用 `lazy_p2p=false`、`need_p2p=true`，发布版 120 秒内首帧
之后产生 499 个新 UDP 会话，其中 98.4% 指向 EasyTier 通告的默认协议监听端口。
上一版仅限 hole-punch 的候选没有改善连接数。

按精确目标统计时，任一 `IP:port` 在任意 10 秒窗口内最多出现 5 个新连接。因此不再
使用“10 秒超过 10 次”作为主触发条件。一次 Direct candidate 本来已经包含多次底层
connect，完整失败即可进入第一阶段。hole-punch 则必须先完整保留原有 BackOff 的渐进
尝试；只有 BackOff 已到最终档位后的完整 burst 失败，才进入第一阶段。这样既截断
长期 steady-state 风暴，也不删减 symmetric 猜端口等建立连接所需的初始 round。

## 覆盖范围

只覆盖 EasyTier 自动 P2P：

1. Direct/priority candidate：
   - QUIC / QUIC-Brutal
   - FakeTCP
   - WireGuard
   - UDP
   - TCP
   - WS / WSS
   - 其他由 `DirectConnectorManager` 从 peer listener 展开的 IP 协议
2. UDP hole-punch：
   - cone-to-cone
   - symmetric-to-cone
   - both-easy-symmetric
3. TCP simultaneous-open hole-punch。

Direct 的 upgrade、fallback/降级和 probe 共用相同入口；失败表不增加新的状态判断。
当当前连接丢失、经 relay、冷启动或配置要求主动 P2P 时，只要既有代码生成同一个
endpoint 尝试，就应用同样的失败冷却。

明确排除：

- 用户手工 connector；
- listener/server accept；
- 已建立 tunnel 的业务流量；
- DNS、STUN 和轻量地址发现 RPC；
- Mihomo、GOST、Leaf 等业务代理连接；
- 路由净化。

## 数据结构

每个 `GlobalCtx` 即每个 network instance 持有一张 crate-private 表：

```text
P2pEndpointRetryTable
  64 shards
  each shard: Mutex<HashMap<P2pEndpointKey, Entry>>
  each shard capacity: 1024
  total hard cap: 65,536 entries

P2pEndpointKey
  peer_id
  IpScheme
  SocketAddr

Entry
  stage
  blocked_until
  revision
  last_touched
```

表的硬上限为 65,536。随机 IP、端口或 peer 最多引起有界淘汰，不会无限增长。每个
shard 满时只扫描本 shard 的最多 1,024 个条目并淘汰最久未使用项；常规查找、插入和
删除只锁一个 shard，不使用全局锁，不在 packet send/recv 热路径执行。

key 包含 peer 和协议，避免：

- 一个 peer 的失败误伤碰巧复用同一 NAT endpoint 的另一个 peer；
- TCP、UDP、QUIC 等不同 transport 因相同端口相互阻断。

Direct UDP 与 UDP hole-punch 如果属于同一 peer、同一协议和同一远端 endpoint，则
共享失败记录；触发原因和调度入口不构成新的 key。

## 一次性 attempt lease

协议代码在已经解析出远端 endpoint、但尚未开始昂贵网络动作时调用：

```text
begin(peer_id, scheme, remote_addr, now)
    -> Cooled(remaining)
    -> Lease(key, observed_revision)
```

一次 lease 对应一次**外层逻辑尝试**：

- Direct：`try_connect_to_ip()` 对一个已展开 candidate 的完整现有重试；
- UDP：一个 NAT 算法的完整现有 burst；原有 BackOff 到最终档位前只执行
  `no_work()`，不累计失败；
- TCP：一次完整 mapped-address exchange 后的 simultaneous-open；同样只在原有
  BackOff 到最终档位后累计失败。

以下内部动作不再次查表，也不单独累计：

- Direct candidate 内现有多次 connect；
- UDP 多 socket、广播、端口猜测、cone fallback、symmetric round 和重发；
- TCP connect/listen/accept 循环；
- 发起端触发的远端 RPC fanout。

lease 不实现 `Clone`/`Copy`，结束接口按值消费，编译期保证同一路径只能结算一次：

```text
lease.succeeded()
    删除当前 endpoint 表项

lease.failed_after_network_work()
    仅当 revision 仍匹配时增加一个 stage 并设置 block_until

lease.no_work() / drop
    不修改失败阶段
```

revision 防止两个并发旧尝试都把同一次风暴累计成多个阶段。第一个完成的失败更新
revision，其他基于旧 revision 的失败不再递增。任何实际成功都可以删除表项。

取消、busy、blacklist、没有 candidate、地址发现 RPC 失败、DNS 失败、本地 bind 或
资源错误均不惩罚远端 endpoint。只有已经执行远端网络尝试、并由现有逻辑判断完整失败
时才结算。对于 hole-punch，“可结算”还要求既有 BackOff 已达到最终档位；这是防止
冷却降低广播、猜端口和 simultaneous-open 初始成功率的兼容边界。

## 冷却

阶段固定为：

```text
1m -> 2m -> 4m -> 8m -> 10m -> 10m ...
```

不再有 30 分钟阶段，也不永久封禁。

冷却到期只允许现有逻辑再次尝试，不主动创建新任务。下一次完整失败进入下一阶段；
下一次成功删除记录。新 peer、新协议或新 `IP:port` 是新 key，可立即尝试。

同一个 `IP:port` 在底层网络静默恢复但没有任何新通告时，最坏会等待当前阶段，最高
10 分钟。这是 endpoint 失败冷却唯一明确的恢复延迟；新 listener/IP 不继承旧地址的
冷却。

## 接入边界

公共表和 lease 放在 `common`/`GlobalCtx`，具体协议仅在现有外层 attempt 边界调用：

- Direct 在 `DirectCandidate` 已展开为具体 `SocketAddr` 后获取 lease，完整保留当前
  candidate 内部重试；
- UDP 在 mapped/listener 基准 endpoint 已知后、开始批量 socket/send/RPC fanout 前
  获取一次 lease，整个 burst 最终只结算一次；既有 BackOff 未饱和时失败 lease 直接
  丢弃；
- TCP 在 exchange 获得远端 mapped endpoint 后、simultaneous-open 开始前获取一次
  lease；既有 BackOff 未饱和时失败 lease 直接丢弃。

不把 guard 放入通用 `TunnelConnector::connect()`，否则会影响手工 connector；不复用
受 `underlay_candidate_guard` 设置控制的 `UnderlayBreaker`，否则会改变既有设置语义
和 self-loop 安全策略。

响应端不根据“只发送了打洞包但没有同步看到 tunnel”猜测失败。发起端的 lease 包含
其触发的整个 RPC fanout；发起端冷却后不会再触发同轮远端工作。混合版本中旧发起端
仍可能请求新响应端执行 fanout，这是兼容边界，不用误判失败的 responder limiter
牺牲成功率。

Direct 的 peer listener/IP 通告仍会在每次轻量 `get_ip_list` 后展开；新增地址形成新
key，可立即尝试。TCP simultaneous-open 与 both-easy-symmetric 的 mapped endpoint
只能由会启动 responder fanout 的 RPC 得知，因此已知旧 endpoint 冷却期间不额外调用
该 RPC；这两类 NAT 映射变化最晚在当前冷却到期后发现，最高 10 分钟。这个边界只影响
未通告的临时 NAT mapped endpoint，不影响 peer 新增 listener/IP 的 Direct 尝试。

## 自动化契约

1. 首次 endpoint 立即允许。
2. 完整失败后阶段依次为 1、2、4、8、10、10 分钟。
3. 冷却中只阻断相同 peer、scheme、`IP:port`。
4. 新 peer、新 scheme、新 IP 或新 port 不受旧 key 影响。
5. 成功立即删除记录并恢复首轮语义。
6. Direct 内部多次 connect 只结算一次。
7. UDP 广播、端口猜测和 fanout 的一个 burst 只结算一次；原有 BackOff 到最终档位
   前的多个必要 round 不结算。
8. TCP simultaneous-open 多次 connect 只结算一次；原有 BackOff 到最终档位前不结算。
9. cancellation、busy、空 candidate、RPC/DNS、本地 bind/资源错误不结算。
10. 两个相同 revision 的并发失败只增加一个阶段；成功可清除旧失败。
11. 65,536 硬上限与每 shard 确定淘汰有效，随机 endpoint 不扩大结构。
12. BackOff 饱和门槛精确保留每种 hole-punch 的完整原始渐进序列。
13. 所有既有设置准入、priority、upgrade、fallback、probe 和打洞测试保持原语义。
14. 手工 connector、listener 和业务 tunnel 不访问此表。
15. 兼容开关开启时重复失败始终放行、表保持为空；动态关闭后从第一阶段重新开始。

## 提交前候选清单

### 精确构建快照

核心候选基线为 `707939a1`（净代码内容等同 `deed0cf2` 回退基线）。最终发布快照包含：

- `common/p2p_endpoint_retry.rs` 及 `GlobalCtx` 私有持有点；
- Direct 精确 endpoint lease；
- UDP cone、symmetric-to-cone、both-easy-symmetric lease；
- TCP simultaneous-open lease；
- 既有 BackOff 饱和只读判断；
- `disable_p2p_storm_throttle` 在 TOML、CLI/env、management protobuf 和 GUI 上的
  完整配置链路；
- 表内唯一原子 bypass、动态开启清表和重新关闭后从空表开始的契约；
- Rust 配置 round-trip、CLI、managed config、GUI checkbox 和全字段 protobuf
  round-trip 测试；
- 同文件 focused tests；
- 本 TODO 与独立性能探针。

新增 protobuf 字段固定为 `FlagsInConfig=51`、`NetworkConfig=86`，不复用已发布字段。
TypeScript protobuf 在标准 frontend build 中从精确 schema 生成，不手工维护生成物。
不包含依赖、`Cargo.lock`、workflow 或平台 `cfg` 变更。提交前必须再次确认完整 diff、
schema 字段号、生成结果和 untracked 文件列表。

### `.160` 强制预检

已使用完整同步快照和 Rust 1.95 debug test profile 完成：

```text
cargo test --locked --no-run --package easytier --lib
common::p2p_endpoint_retry::tests
connector::direct::tests（排除已由原始 HEAD 复现的同机 IPv4 基线坏例）
connector::udp_hole_punch
connector::tcp_hole_punch::tests
connector::tests
```

最终一次 no-run 无 warning；结果分别为 7/7、Direct 17/17 加 IPv6 集成 3/3、
UDP 24/24、TCP 4/4、设置契约 14/14。
`hole_punching_symmetric_only_random` 在初版 endpoint 门槛下真实失败，修正为 BackOff
饱和后才结算后连续单跑 3/3，并随 UDP 全套通过。原始 HEAD 和候选的 Direct IPv4
同机 mock 均为 3/6 失败，错误发生在 `DirectConnectorRpc` 注册发现阶段，未进入本
候选 endpoint guard。

性能探针在 65,536 硬上限下采集满表查找、begin/fail/success、异常随机淘汰、分配和
RSS；结果属于提交前机制门槛，不替代 immutable artifact 实机验证。

兼容开关扩展必须在 `.160` 重新执行完整 `--locked --no-run`、endpoint/global
context/config/CLI/launcher/managed-config focused tests，以及 frontend-lib
protobuf codegen、Config/RemoteManagement Vitest、全字段 round-trip 和依赖顺序的
frontend-lib、frontend、VPN plugin、GUI build。旧 `bc829e58` 的成功结果不能冒充
新增 schema 和 GUI 的编译证据。

### 兼容开关扩展预检证据（未提交工作树）

已把包含开关的完整工作树同步到 `.160`，并完成以下门禁：

- `cargo test --locked --no-run --package easytier --lib`：通过、无 warning；
- `cargo test --locked --no-run --package easytier-web`：通过；
- endpoint 表 8/8、开关启动/动态切换及 CLI 3/3、TOML 1/1、launcher
  3/3、managed config 2/2：通过；
- Direct 既定有效子集 17/17、Direct IPv6 3/3、UDP 24/24（另有 1 项原有
  ignored）、TCP 4/4、connector 设置契约 14/14：通过；
- frontend-lib protobuf codegen、Config/RemoteManagement Vitest 29/29、全字段
  round-trip 7/7：通过；
- frontend-lib、frontend、VPN plugin、GUI 按依赖顺序完成 production build。

一次包含全部 Direct 测试的宽过滤仍复现原始 HEAD 已有的 3 个同机 IPv4 mock
注册发现失败；它们未进入 endpoint guard，不能记作本开关回归，也不能记成通过。

以上仅证明未提交源码的编译和契约。新增 Rust、protobuf 与 GUI 尚无新的 immutable
candidate SHA、workflow artifact 或实机证据；因此最终发布仍须提交完整快照，并对
该精确 SHA 重走要求的 artifact 和实机门禁。

### 必需 workflow 与实机证据

只允许对同一候选 SHA 执行：

1. `profiling-beta`：生成优化、带符号的 x86_64-musl 核心包；
2. `EasyTier GUI macOS ARM64 Test`：为明确授权的 macOS `.162` A/B 生成 GUI 包。

不触发 Core、完整 GUI、Mobile、OHOS、Test、tag 或 Release。

计划证据：

- `192.168.1.37/.38`：显式端口、设置组合、冷启动、relay、失联恢复和混合版本；
- 两台 10 Gbps 双栈主机：跨主机 IPv4/IPv6、协议建立、RTT/丢包与资源基线；
- macOS `.162`：备份并恢复原配置，A/B 均显式
  `lazy_p2p=false, need_p2p=true`，分阶段采样至少 10 分钟；
- Android 当前不可用，记录平台不可用例外，不等待或连接设备。

`.160` 等待期间完成完整 diff、lockfile、平台 `cfg` 和 workflow pin 审计；GitHub
等待期间准备 artifact 校验、验证主机清理、显式 listener 端口、A/B 配置备份和有界
采样命令，不修改正在构建的 immutable snapshot。

## 验证门槛

### `.160`

- `--locked --no-run`；
- endpoint 表全部契约；
- Direct、UDP/TCP hole-punch、priority、lazy/need/disable、upgrade/fallback focused tests；
- 小型基准比较无表与 begin/finish，覆盖 65,536 表项和并发 shard，报告吞吐、锁等待、
  分配和内存上限；
- 证明 packet send/recv 热路径没有新增查表。

### 实机 A/B

使用同一网络和配置，发布版与 immutable candidate 都显式确认：

```text
lazy_p2p=false
need_p2p=true
```

采样必须长于 10 分钟并按冷却阶段分段，记录：

- 每秒新建/活跃连接；
- endpoint、scheme 和失败/冷却阶段的低基数计数；
- EasyTier GUI 与 Mihomo CPU；
- 当前 tunnel、业务 RTT 和丢包。

功能场景至少包括：

1. 冷启动首次 P2P；
2. 已有 stable P2P 的 priority upgrade/probe；
3. relay 场景；
4. peer 失联后恢复；
5. 同 endpoint 短暂失败后恢复；
6. peer 新增 listener/IP；
7. UDP cone、symmetric 和 TCP simultaneous-open；
8. lazy/need/disable 与 transport priority 设置组合；
9. 同一 endpoint 的候选成功后清除冷却；
10. 混合版本互操作。

硬门槛：

- `lazy=false, need=true` 下新建连接率和活跃连接必须明确、可重复下降；
- 不能只把风暴平移到下一个阶段；
- 单次完整 candidate/burst 的内部行为和成功率不变；
- 新 endpoint 首次尝试不延迟；
- upgrade、fallback、失联恢复、relay 和设置项语义无回退；
- 任一门槛失败即停止候选，不提交正式发布。

## 候选验证证据（`bc829e581540b1ffa188fded23546bd30a2e35f9`）

本节是 immutable candidate 完成后的证据记录，不改变候选代码，也不触发新构建。

### 构建、契约和边界

- `.160` 的 `--locked --no-run` 无 warning；endpoint 表 7/7、Direct 17/17、
  Direct IPv6 3/3、UDP 24/24、TCP 4/4、设置契约 14/14 通过。
- 65,536 项压力下 RSS 约 10 MiB；满表 blocked lookup 约 965 万次/秒，
  begin/fail/success 约 700 万次/秒，随机满表淘汰约 60.8 万次/秒；查表不在
  packet send/recv 热路径。
- profiling-beta run `30463656142`、macOS ARM64 GUI run `30463695099` 和自动触发的
  Android Policy Candidate run `30463655993` 均在上述精确 SHA 成功。Linux 包的
  内外层校验和、BUILD_INFO、SHA、target、static-pie 和符号已核验；macOS app 的
  arm64 架构、可执行权限、deep/strict 签名和候选版本字符串已核验。
- Android 设备不可用，因此本候选只有编译/打包证据，没有把 workflow 成功表述为
  Android 实机功能证据。

### macOS `.162` 正式 A/B

发布版和候选版都通过 GUI 明确确认 `lazy_p2p=false, need_p2p=true` 后运行 720 个
样本；最初一轮被 GUI 持久化值覆盖为 lazy 的采样已经作废，未进入下列比较。

| 指标 | 发布版 | 候选版 | 变化 |
|---|---:|---:|---:|
| 平均活跃连接 | 284.94 | 99.68 | -65.0% |
| 活跃连接中位数 | 293 | 78.5 | -73.2% |
| 失败新连接率 | 4.234/s | 1.279/s | -69.8% |
| GUI CPU | 2.25% | 1.05% | -53.3% |
| Mihomo CPU | 4.63% | 1.89% | -59.2% |
| 最后 180 样本平均活跃连接 | 262.77 | 72.37 | -72.5% |
| 最后 180 样本新连接 | 803 | 117 | -85.4% |
| 最后 180 秒 endpoint churn | 789 | 44 | -94.4% |

候选早期 180 秒 endpoint churn 为 358，发布版为 704，且候选后段继续下降，因此
不是把风暴平移到下一冷却阶段。两者早期观测到的唯一 endpoint 数分别为 75 和 78，
候选没有通过放弃 endpoint 发现来换取较低连接数。

测试后已通过 GUI 恢复 `lazy_p2p=true, need_p2p=false` 并重新运行发布版；运行配置
SHA-256 与测试前备份完全一致，候选进程未运行。

### Linux exact-artifact 场景

- 两节点冷启动从 TCP 自动升级到 QUIC；连续约 15 分钟只有最初 TCP、QUIC 两次建立，
  无协议跳动，双向 ping 20/20。
- 三节点场景中两端经 relay 相识后建立 UDP/TCP/QUIC Direct；对一端公网地址注入
  35 秒阻断时 Direct 消失、路由回到 `relay(2)`，解除后恢复 Direct TCP 和 QUIC。
  注入期间底层 WAN 本身有丢包，因此这里只证明 relay 路由仍存在且能恢复，不声称
  零丢包切换。
- macOS 候选加入现有发布版 mesh 成功；公网短样本噪声较大，不据此声称 RTT/丢包
  改善或回退。
- peer 新增通告 listener/IP 的“新精确 key 立即允许”由 key 隔离契约测试覆盖。
  当前 `--machine-id` 重启未保留 peer ID，不能把该实机重启误记为同 peer 证据。
  未通告临时 NAT mapped endpoint 仍遵守前述最高 10 分钟发现边界。

所有 Linux 测试核心、测试 TUN 和注入的防火墙规则均已清理；没有触发 Core、完整
GUI、Mobile、OHOS、Test、tag 或 Release。
