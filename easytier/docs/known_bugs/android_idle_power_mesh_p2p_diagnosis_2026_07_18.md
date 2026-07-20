# Android 空闲耗电与 Mesh P2P 探测诊断记录（2026-07-18）

## 目的与状态

本文保留一次 Android 候选包空闲耗电排查的证据、误判修正和后续诊断顺序，避免再次把
瞬时 P2P 收敛窗口误判成长期 UDP 风暴，或在没有归因证据时修改 Leaf、
`transport_priority`、`lazy_p2p` 和打洞常量。

- 状态：只读诊断完成；**未形成代码修改结论**。
- 源码快照：`225b0a223a311c184ebd489ac436e52b779088b1`。
- Android 包：`com.kkrainbow.easytier.policycandidate`。
- 本次文档不授权构建、提交、push 或工作流。
- 诊断过程中未改变网络配置；临时 simpleperf 文件、CDP 端口转发已清理，临时 debug
  日志级别已恢复为 info。

## 先读结论

1. 当时的长期空闲 CPU 异常是真实的，但采样热点在主 EasyTier runtime 的 direct
   connector，不在 Leaf、隐藏 WebView 轮询或 wakelock。
2. 一次约 30 秒的 UID 网络统计捕获了完整的 Symmetric NAT 打洞收敛窗口；其中约
   `3,669` 个发送包不能外推为长期稳态 `110 pps`。
3. 后续连续 socket 采样显示高 socket 状态只持续十几秒，随后 84-socket 数组被释放。
   稳定 UDP P2P 会停止打洞，当前没有证据表明 UDP hole punch 在稳态持续风暴。
4. `transport_priority` 允许未满足的协议升级任务存在，但 direct 和 UDP hole punch 都有
   退避；它不是无退避死循环。
5. `lazy_p2p` 只能减少部分无流量 Peer 的主动 P2P。已有 peer 的
   `priority_upgrade_allowed` 可以绕过 lazy，因此它不是本问题的通用修复，也不应为了省电
   默认削弱用户的协议优先级语义。
6. 暂不修改 200 ms 检查、84 个 socket、每 socket 三连发或 64 秒最大退避。下一次应先
   统计任务为何创建/重建，以及每个阶段的真实结果和发包量。

## 运行时证据

### 电池、唤醒和 UI

- 设备当时接通 AC、屏幕唤醒且 Android Settings 在前台，因此 SystemUI、
  `system_server` 和 SurfaceFlinger 的瞬时 CPU 不能归因给 EasyTier。
- EasyTier UID 没有持有 partial wakelock；`dumpsys power` 未发现应用 wakelock。
- WebView 处于 `document.hidden=true`、`visibilityState=hidden`。对
  `window.__TAURI_INTERNALS__.invoke` 做 10 秒包装计数为零，排除了该窗口中隐藏页面持续
  调用 Rust 后端的假设。
- TUN 10 秒样本只有约 `894 B RX + 562 B TX`，业务 VPN 负载接近空闲。

### CPU 与线程归因

- 进程 RSS 约 `264-271 MiB`，线程约 `73-75`。
- 5 秒 `/proc/.../sched` 差分：两个主 `tokio-rt-worker` 合计约
  `292.7 ms / 5 s`，即单核约 `5.85%`；Leaf 主线程同期约 `34.2 ms / 5 s`，即约
  `0.68%`。
- UID batterystats 在约 `6 h 24 min` running 时间中记录约 `1 h 46 min` CPU 时间。
  该数据说明长期后台 CPU 风险真实存在，但它是累计值，不能单独解释当前一次瞬时事件。
- simpleperf caller/children 路径主要落在：
  - `DirectConnectorManagerData::do_try_connect_to_ip`
  - `try_direct_connect_with_peer_id_hint_timeout`
  - `PreparedUnderlayConnector::connect`
  - libc `connect`
- simpleperf self time 可见 SHA-256、Curve25519 和内核路径，符合反复建立加密连接的成本。
  这份 CPU 证据属于 direct connector，不能用来证明 UDP hole punch 是长期 CPU 主因。

## UDP 打洞发包量：观测、源码解释与纠正

### 瞬时观测

- 打洞窗口内进程 FD 约在 `354-427` 间波动，socket 约在 `145-216` 间波动。
- 一轮探测会临时增加约 70 个 socket，随后回收；没有观察到单调 FD 泄漏。
- 一个约 30 多秒的 Wi-Fi UID 差分记录 `3,669` 个发送包、约 `406 KiB`，平均包长约
  `111 B`。同期 TUN 业务流量接近零，因此这个窗口主要是控制/打洞流量。

### 单轮放大的源码边界

Hard Symmetric NAT 使用固定 84 个 UDP socket：

- `easytier/src/connector/udp_hole_punch/sym_to_cone.rs`
- `UDP_ARRAY_SIZE_FOR_HARD_SYM = 84`

`UdpSocketArray::send_with_all` 对每个 socket 连续发送三份相同 packet：

- `easytier/src/connector/udp_hole_punch/common.rs`
- 单次调用理论发送 `84 × 3 = 252` 个 UDP datagram。

`check_hole_punch_result` 每 200 ms 调用一次 `send_with_all` 并检查结果；远端任务结束后仍
保留约 1 秒接收迟到包。即使远端任务很快完成，一个阶段也可能发送约七批：

```text
252 × 7 ≈ 1,764 packets
```

Sym-to-cone 可能依次执行 predictable 和 random 两个阶段，因此一次完整尝试的数量级是：

```text
1,764 × 2 ≈ 3,528 packets
```

它与观测到的 `3,669` 个发送包高度接近，说明该样本更像**一个完整打洞窗口**，不是长期
每 30 秒都持续相同包速率。

### 连续采样对误判的纠正

后续约 90 秒 socket 采样观察到：

- 一次高 socket 窗口持续约十几秒；
- 随后 socket 基线从约 150 降到约 60；
- 下降量与共享的 84-socket array 一致。

`UdpHolePunchPeerTaskLauncher::all_task_done` 会清理该数组。这个回落说明当时已没有 eligible
hole-punch task；可能是 UDP P2P 成功、Peer/路由消失或候选条件不再满足。现有 release
包没有足够计数器区分这三种原因，禁止把其中一种当作已证实事实。

## 退避语义：哪些判断成立，哪些不成立

### 正常无隧道结果会推进指数退避

Symmetric 路径使用：

```text
1s, 1s, 2s, 4s, 4s, 8s, 8s, 16s, 64s, 64s, ...
```

`Ok(None)` 调用 `op(false)`，会推进 punch round 和 backoff。失败任务不会“永久休眠”，
而是最大每 64 秒再尝试一次；多个 Peer 必须按各自 task 分开统计。

### `Err` 会 rollback，但通常在批量发送前暴露控制链路失败

`handle_punch_result` 对 `Err` 调用 `backoff.rollback()`。这会重复当前较短档位，而不是
继续增长到 64 秒。该设计可能是为了让 Mesh RPC/逻辑链路恢复后尽快重新协调。

但不能据此认定 `Err` 造成了本次数千 UDP 包：

1. `select_punch_listener` RPC 在批量发包之前执行；
2. RPC/逻辑链路错误会在 `handle_rpc_result(...)?` 返回；
3. 只有成功得到远端 mapped address 并通过 preflight 后才进入 84-socket 批量发送。

因此，控制链路丢失时快速 retry 可以是合理语义，而且通常只产生控制 RPC，不会执行完整
的 `84 × 3 × 200ms` 发包阶段。若以后考虑修改 rollback，必须先按错误阶段分类，不能把
所有 `Err` 等同于本地热循环。

### 成功或已满足的 UDP P2P 会停止任务

collector 在下列情况不会继续为 Peer 创建 UDP hole-punch task：

- 已有 live UDP transport；或
- UDP candidate 不会改善当前 path/transport preference；或
- Peer 不再满足 P2P、NAT、blacklist 等候选条件。

因此“稳定 UDP P2P 不应继续打洞”是正确预期。若未来观测到稳定 UDP connection 存在时仍
重复打洞，应优先审计 `has_live_transport`、connection churn 和 candidate-improves 判断，
而不是先调退避常量。

### 任务重建会重置退避

`PeerTaskManager` 会在 candidate key 消失或 task 完成后删除 task；再次 eligible 时创建新
task。`PunchTaskInfo` 的 key 包含：

- destination Peer ID；
- 对端 NAT 类型；
- 本地 NAT 类型；
- UDP stealth compatibility 状态。

网络切换、NAT 重新判定、Peer 加入/离开、UDP tunnel 掉线或上述兼容状态变化，都可能让
一个运行很久的实例重新进入快速收敛阶段。这是解释“运行很久后为何又出现一次 burst”的
第一检查项。

## `transport_priority` 与 `lazy_p2p` 的准确边界

- `transport_priority` 不会绕过 punch task 内部退避。它只会让已有连接但仍可升级协议的
  Peer 进入 `priority_upgrade_allowed`。
- 将这种行为称为“无退避循环风暴”是误报。正确说法是“有退避的协议升级任务；每轮内部
  可能有较大发包量”。
- `lazy_p2p=true` 会阻止部分无近期流量的 background P2P，但
  `priority_upgrade_allowed` 在已有 peer 且配置了 priority 时不检查 lazy。
- 强制 lazy 覆盖 priority 会改变用户“持续争取更高优先级协议”的功能语义。没有明确产品
  决策和兼容测试前不得这样修改。

## 原始 200 ms 设计的历史边界

- 最初的 nat4-nat4 实现在发送一次 `send_with_all` 后，每 200 ms 只检查结果。
- 2024 年 `refactor sym to cone punch (#515)` 主动把 `send_with_all` 移入 200 ms 循环，
  并增加远端重复发送。
- PR 没有正文、review 或基准解释 200 ms 的取值。能够从代码推断的目标是提高 UDP 丢包、
  NAT 映射建立和远端端口扫描时间窗口重叠下的穿透成功率；这是推断，不是作者明示结论。
- 在没有多种 NAT、弱网和移动网络 A/B 穿透率前，不得宣称把发送间隔改成 1 秒“完全不影响
  功能”。

## 下次排查的最短路径

不要从修改常量开始。先加入一分钟聚合、默认低开销的诊断计数，至少覆盖：

1. UDP hole-punch task create/remove 次数及原因；
2. task key 变化：peer、local/remote NAT type、stealth compatibility；
3. 当前 backoff index 和实际 sleep；
4. `Ok(Some) / Ok(None) / Err` 次数，`Err` 按以下阶段分类：
   - select-listener RPC；
   - underlay/preflight；
   - STUN/public IP；
   - bind/send；
   - remote predictable/random RPC；
   - tunnel creation/addition；
5. `send_with_all` 调用次数和理论 datagram 数；
6. collector 判定：live UDP、candidate-improves、priority、lazy、recent traffic；
7. direct connector 的 peer/candidate/协议/结果计数，避免再次把 direct CPU 与 UDP radio
   burst 混为一谈。

验证至少分四个阶段，并使用同一 exact artifact：

1. 已稳定 UDP P2P 的一小时息屏空闲；
2. 新 Peer 加入和 UPnP/STUN 延迟就绪；
3. 控制链路短时断开后恢复；
4. UDP P2P 失败但其他 transport 可用。

每阶段同时记录：UID physical packet/byte delta、TUN byte delta、socket/FD/task、线程 CPU、
batterystats、当前 live transport、task lifecycle 和网络 epoch。只有 physical UID 流量而没有
TUN 对照时，不能区分业务转发和控制流量。

## 明确避免的重复错误

- 不用一次 30 秒 burst 推导长期稳态 pps。
- 不用一次 `top` 峰值推导长期 CPU；使用 sched delta、simpleperf 和 batterystats 交叉验证。
- 不因 Leaf 与主进程同 PID 就把主 Tokio worker CPU 归因给 Leaf。
- 不把 `transport_priority` 的后台升级等同于无退避循环。
- 不把所有 `Err` 等同于已进入大规模 UDP 发包阶段。
- 不因开启 `lazy_p2p` 可能省电，就静默改变 priority 语义。
- 不先改 84、三连发、200 ms 或 64 秒常量；先证明是任务频次、任务重建、单轮放大还是
  direct connector。
- 不在日志、文档或命令输出中保存网络 secret。
