# Android Policy 性能、Bug 与耗电后续研究 TODO

状态：**需要进一步研究，当前仅完成源码走读和只读验证，尚未进入修改阶段。**

记录日期：2026-07-17  
基准快照：`5d71abed66a1ad1957834a40bc67b0d0092a95af`  
Android 候选包：`com.kkrainbow.easytier.policycandidate`  
锁定 Leaf：`2f62208187f7980d066e479bd70bb55613c066d2`  
锁定 rust-tun：`12378839e7985283df0e4fb536b7137230356db5`

本记录不授权代码修改、构建或工作流。文档应保持本地，直到它随需要实机验证的代码候选一起提交，或维护者明确要求推送。

## 已确认的研究发现

### 1. Leaf 统计清理存在固定 100 ms 唤醒

- 精确来源：`/Volumes/micron512g/code/leaf/leaf/src/app/stat_manager.rs` 的 `StatManager::cleanup_task`。
- 任务无条件随 Leaf runtime 启动；接收完成事件超时 100 ms 后，会取得 `StatManager` 写锁并调用 `move_to_recent()`。
- `move_to_recent()` 扫描所有活动 counter；默认 `MAX_RECENT_CONNECTIONS=0` 只禁止保存 recent counter，不会禁止活动 counter 扫描。
- Android 30 秒线程采样中，Leaf 线程组约消耗单核 1.1%；其中一个 Leaf Tokio worker 约有 12.4 次/秒上下文切换。上下文切换不是单一归因证据，但与固定 100 ms 任务方向一致。
- Mihomo 对照：`/Users/fanli/Documents/mihomo-rev/tunnel/statistic/manager.go` 的 `Join`/`Leave` 是事件驱动；1 秒 ticker 只更新聚合速率，不执行 100 ms 全连接写锁扫描。

进一步研究：

- [ ] 在同一 Android artifact 上分别记录 Policy 无连接、少量长连接和大量短连接时 Leaf 各线程 CPU、timer wakeup、context switch 与电量统计。
- [ ] 证明 100 ms 扫描在不同活动连接数下的 O(n) 成本，并分离 DNS、Leaf dispatcher 和统计任务的 CPU。
- [ ] 研究完成事件驱动清理加低频兜底的 Mihomo 兼容语义；不得直接删除异常关闭兜底。

### 2. Android Policy 每 5 秒无条件重建路由分类快照

- 精确来源：`easytier/src/instance/virtual_nic.rs` 的 `run_mobile_policy_updater`。
- 持久 `tokio::time::Interval` 每 5 秒触发 underlay refresh、proxy CIDR/IPv6 路由收集、prefix trie 重建、`ArcSwap` 快照替换、peer route clone 和 mesh endpoint 解析。
- 即使路由没有变化也会重建和替换快照；项目已有 `ProxyCidrsMonitor` 根据 generation 变化发出 `ProxyCidrsUpdated`，因此这里存在重复的固定轮询和分配。
- 已排除此前的“无关事件会重置 sleep 并令 5 秒任务饿死”怀疑：当前实现使用持久 Interval，事件不会重置其 deadline。

进一步研究：

- [ ] 对 0、5、50、500 条 route/proxy CIDR 的每轮分配量、锁等待、CPU 时间和 ArcSwap churn 做同 SHA 基准。
- [ ] 核对 Mihomo 路由/provider 更新的 generation、缓存失效和首次匹配语义，确定事件驱动替换的兼容边界。
- [ ] 验证仅在 snapshot 实际变化时替换，是否仍覆盖 DHCP、Public IPv6、ConfigPatched、peer generation 和 lagged event 恢复。

### 3. Android TUN 的 64 包批量没有减少 TUN write syscall

- `easytier/src/tunnel/common.rs` 的 `FramedWriter` 使用锁定的 `tokio-util 0.7.13`，会通过 `poll_write_vectored` 提交最多 64 个 slice。
- `easytier/src/instance/virtual_nic.rs` 的 `TunAsyncWrite::is_write_vectored()` 返回 true。
- 锁定 rust-tun `12378839...` 的 `src/async/unix_device.rs` 同样声明 vectored write，但 Android 使用的 `src/platform/posix/split.rs::Writer` 只实现标量 `write()`，没有实现 `write_vectored()`。
- 标准库 fallback 因此每次只写第一个非空 slice。正确性不受影响，但仍是逐包 TUN syscall；64 包 feed/flush 只能减少部分调度、flush 和锁开销。

进一步研究：

- [ ] 用 syscall/perfetto 证明确切的 TUN write 次数、每批包数和 BiLock 等待时间。
- [ ] 对比正确报告 `is_write_vectored=false`、Android 可用的真实批量接口和保持当前 fallback 三种方案；先建立正确性与平台能力边界再设计修改。
- [ ] 检查 Linux、Android、iOS/OHOS 的 rust-tun Writer，避免修复 Android 时错误扩大跨平台能力声明。

### 4. Policy 数据面有固定逐包 bridge syscall、复制和有限队列

- TUN 到 Leaf：逐包分类后进入 `mpsc(4096)`，再通过一对 `UnixDatagram` 逐包发送给 Leaf。
- Leaf 到 TUN：使用 65535 字节接收缓冲；每包调用 `ZCPacket::new_with_payload` 复制/分配，进入 `mpsc(256)` 后与 mesh 包合并写入 TUN。
- 反向队列满时 fail-closed 丢包并按 2 的幂记录告警，这是现有明确失败语义，后续优化不能静默改成无界缓存。
- Linux 一次显式 actor 对照约为 native SOCKS 27.8 Mbit/s、Leaf 15.5 Mbit/s，约下降 44%。这是单样本，只能作为进一步 benchmark 的方向证据。

进一步研究：

- [ ] 在 exact artifact 上分别测 TCP/UDP、小包/大包、单流/多流的 bridge syscall、copy、allocation、队列深度和 drop counter。
- [ ] 区分 Leaf 规则/DNS/outbound 成本、UnixDatagram bridge 成本和 TUN writer 成本。
- [ ] 研究 buffer pool、所有权转移或批量 bridge 的可行性，保持 packet boundary、generation 检查和 fail-closed 语义。

### 5. Transport priority 会对未满足的 peer 长期后台升级重试

- `PeerManagerForDirectConnector::list_peers` 允许已有 peer 在没有近期流量时进入 `priority_upgrade_allowed`。
- `do_try_direct_connect` 使用最长 60 秒 backoff 无限重试；每个 outer attempt 的候选扩展会调用 `IPCollector::collect_interfaces`。
- Android 的 `collect_interfaces` 走 `pnet::datalink::interfaces()`，会尝试受 SELinux 限制的 packet socket。
- 当前拓扑有一个仅 UDP 的 peer，而全局优先级要求 QUIC 优先，因此该 peer 持续处于未满足状态。这不是 Policy 独有问题，但会与 Policy/Leaf 固定唤醒叠加。
- 历史约 63 分钟日志中记录过 1642 次 `packet_socket` denial；但当前拓扑没有 FakeTCP URL，不能把全部 denial 归因于 FakeTCP。

进一步研究：

- [ ] 用 exact topology 记录每个 peer 的 retry reason、candidate enumeration、失败协议、backoff 和 CPU/wakeup。
- [ ] 分别关闭 transport priority、满足 QUIC、保持 UDP-only，比较主 EasyTier worker CPU 与 SELinux denial。
- [ ] 对照 Mihomo Android interface cache/default-network callback，研究近期流量门槛、平台能力缓存和失败冷却，不得破坏 priority/failover 语义。
- [ ] 单独验证 FakeTCP 在 Android 上的 capability failure；默认配置包含 FakeTCP，但当前运行态没有候选，不得提前定性为当前主因。

## 需要进一步验证的 Bug/生命周期风险

### 6. Android 网络恢复依赖 WebView/JavaScript 中转

- 当前链路：`VpnService NetworkCallback -> Tauri plugin JS event -> Vue listener -> Rust update_mobile_network`。
- ClashMeta Android 的 `NetworkObserveModule` 在 Service coroutine 内观察网络，并直接通知 native core 更新 DNS、刷新缓存、关闭旧连接；不依赖 Activity/WebView。
- EasyTier 的 network key 包含 Android Network、transport、接口名、全部地址、全部路由和 DNS hash。相同 Android Network 上的 DHCP、route 或 DNS 变化也会触发完整 Leaf runtime 重建。
- ClashMeta key 仅使用 `Network@transport`，DNS 独立更新，并在短暂无 DNS 时保留上一次非空 DNS。两者存在明确语义差异；EasyTier 当前行为是更严格的 fail-closed，但重建和耗电更重。
- Activity 重建时，页面初始化会重新加载运行实例并强制 TUN rebind，因此“重建后永远失联”已被排除。
- 仍待确认：WebView 冻结/后台期间的网络事件没有 native 持久队列或明确重放机制，可能导致 underlay/DNS 更新延迟或丢失。

进一步研究：

- [ ] 使用已经安排好设备端恢复脚本的无线 ADB 流程，验证前台、HOME 后台、息屏、Activity 重建四种 Wi-Fi outage/recovery。
- [ ] 每轮记录 native `outage!EPOCH`、恢复 key/DNS、Leaf generation、TUN route、mesh 流量、captured-UID TLS、FD/task/RSS 回落。
- [ ] 研究 native Service 保存最新网络 generation 并向 Rust 直接同步或在 UI 恢复时 replay 的边界；不得让 UI 成为安全关键 shutdown/recovery 的唯一所有者。

### 7. VPN plugin listener 在页面重新挂载时会累积

- `registerVpnServiceListener()` 调用三次 `addPluginListener`，但丢弃返回的 unlisten handle。
- `index.vue` 只清理 `listenGlobalEvents()` 返回的监听，不清理三个 VPN plugin listener。
- 如果页面或 WebView 在同一 JS runtime 中重新挂载，可能产生重复 start/stop/network callback，放大串行 VPN operation 和 Leaf rebuild。

进一步研究：

- [ ] 构造不杀进程的页面 remount/Activity recreation，统计 plugin listener 和每个 native event 对应的 JS/Rust调用次数。
- [ ] 核对 Tauri Android plugin 生命周期、WebView 销毁和静态 `triggerCallback` closure 的释放语义。
- [ ] 明确 listener 只注册一次或随组件注销的所有权，并增加 remount/recovery 测试后再修改。

## 当前已排除或需要保持的边界

- [x] 没有发现持续 FD、线程或 in-process Leaf runtime 泄漏。干净运行基线约 `323 FD / 67 threads`，停止基线约 `280 / 60`；一次隔离 Wi-Fi cycle 最终回到 `323 / 67`。
- [x] 没有发现 Policy 专属 held wakelock；当前耗电证据主要是 CPU、timer、retry 和 syscall，不是 wakelock。
- [x] `update_mobile_network` 选择第一个启用 TUN 实例在当前 Android 单活动 TUN invariant 下不是实际多实例 Bug；若未来放宽 invariant，需重新审计 API identity。
- [x] 5 秒 Policy route interval 不会被无关 GlobalCtxEvent 重置或饿死。
- [x] Android mesh 慢/挂在 Policy 关闭时也能复现，不能把普通 mesh underlay 吞吐问题归因于 Leaf。
- [x] 当前 VPN UID range 正确排除 EasyTier 自身 UID；当前 underlay 正确绑定 Wi-Fi。

## 下一轮候选前的研究门槛

- [ ] 先完成上述只读/诊断矩阵并记录 Mihomo/ClashMeta 对照函数与外部语义。
- [ ] 将问题拆成独立候选：Leaf 统计唤醒、Policy route snapshot、Android TUN batching/copy、direct retry、native network lifecycle；不要一次混合难以归因的行为变更。
- [ ] 每个候选先补 parity/compatibility test 和失败语义，再按仓库规定执行本地格式化、`.160` 完整 preflight、一次 workflow set 和 exact artifact Linux/Android 验证。
- [ ] 当前文档本身不触发构建、提交、push 或工作流。
