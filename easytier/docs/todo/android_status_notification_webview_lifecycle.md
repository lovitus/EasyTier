# Android 状态通知与后台 WebView 生命周期 TODO

状态：**状态通知候选的实施规格已冻结，满足开工要求；WebView 销毁或统一暂停仍不满足
开工要求。**

记录日期：2026-07-18

重新审计基线：`225b0a223a311c184ebd489ac436e52b779088b1`

锁定 UI runtime：Tauri `2.10.3`、tauri-runtime-wry `2.10.1`、Wry `0.54.2`

本文冻结实现和验证边界，但不单独触发构建、push 或工作流。文档保持本地，直到它随
Android 状态通知代码候选一起提交，或维护者明确要求推送。

## 结论摘要

当前收益、复杂度和回归风险之间最好的候选是：

> 复用 `MainForegroundService` 增加无按钮、事件驱动的 Mesh/Leaf 状态通知；保持现有
> Activity/WebView 生命周期，不增加后台 10 秒销毁、全局 timer pause 或新的网络轮询。

可行性分级：

| 方案 | 可行性 | 近期价值 | 回归风险 | 当前决定 |
| --- | --- | --- | --- | --- |
| 状态通知，无按钮 | 高 | 高 | 低到中 | 规格已冻结，可以开工 |
| 后台 10 秒停止纯 UI 刷新 | 已大部分存在 | 低到中 | 低 | 先测量，不重复实现 |
| 后台 10 秒统一暂停 JS timers/listeners | 技术上可做 | 低 | 高 | 禁止；会暂停 VPN 安全关键中转 |
| 单进程内直接 `WebView.destroy()` | 当前 Tauri/Wry 生命周期不支持安全重建 | 表面内存收益未知 | 极高 | 禁止 |
| Core service/UI 双进程后销毁 UI | 架构上可行 | 潜在高 | 高、工作量大 | 长期独立项目 |

## 当前实现事实

### 前台服务和通知

- `easytier-gui/src-tauri/gen/android/app/src/main/java/com/kkrainbow/easytier/MainActivity.kt::onCreate`
  每次 Activity 创建都会启动 `MainForegroundService`。
- `MainForegroundService::onStartCommand` 当前创建固定通知：标题 `easytier Running`，正文
  `easytier is available on localhost`，没有状态模型或 action。
- 服务使用 notification ID `1355`、`FOREGROUND_SERVICE_TYPE_DATA_SYNC` 和
  `START_STICKY`。
- Manifest 已声明 `POST_NOTIFICATIONS`，但当前应用代码中没有发现 Android 13+ 的运行时
  通知权限请求。候选必须补充一次性运行时请求；权限被拒绝时不得影响 mesh、Leaf 或 VPN
  运行。
- 长期使用 `dataSync` foreground-service type 是否符合目标 Android API 的时长和启动限制
  需要单独平台审计；状态通知候选不得顺便改变 service type。

### 主要纯 UI 刷新已经在后台停止

- `easytier-gui/src/pages/index.vue` 的 2 秒 client status 和 5 秒 config-server status timer
  都在 `document.hidden` 时直接返回。
- `easytier-web/frontend-lib/src/components/RemoteManagement.vue` 的实例/状态刷新 scheduler
  在 `visibilitychange` hidden 时清除 timer，恢复可见时才重新拉取完整状态。
- `easytier-web/frontend-lib/src/components/Status.vue` 的 2 秒速率计算在 hidden 时不更新，
  恢复时重置统计基线，避免把后台累计字节显示成瞬时速率。

因此，“后台 10 秒后再停止页面刷新”与当前行为高度重叠。没有 CPU/wakeup 证据前再增加
一套延时状态机，价值有限且会增加前后台竞态。

### WebView 仍是网络生命周期的关键中转

以下逻辑不能按“UI timer”暂停：

- `mobile_vpn.ts::registerVpnServiceListener` 在 JS 中接收 `vpn_service_start`、
  `vpn_service_stop` 和 `vpn_network_changed`。
- `mobile_vpn.ts::onVpnNetworkChanged` 把 Android DNS/network key 转发给 Rust
  `update_mobile_network`。
- `mobile_vpn.ts::onVpnServiceStart` 把 TUN FD 交给 Rust `set_tun_fd`。
- `mobile_vpn.ts` 的 DHCP retry 和串行 VPN operation 负责 TUN address/routes/DNS rebind。
- `composables/event.ts` 在 JS 中处理中继的 DHCP、proxy-CIDR、event-lagged 和 VPN stop
  事件，并触发 `onNetworkInstanceChange`/`syncMobileVpnService`。
- Rust `GUIStorage::{save_configs,save_enabled_networks}` 通过 Tauri event 通知 JS，再由 JS
  写入 `localStorage`；WebView 不存在时没有原生持久化接收者。

已有 `android_policy_performance_power_followup.md` 还记录了 VPN plugin listener 没有保存
unlisten handle 的累积风险。任何 Activity/WebView 重建候选都必须先解决 listener 所有权，
不能把重复 callback 当作可接受副作用。

## 为什么当前禁止直接销毁 WebView

锁定的 Wry `0.54.2` 中：

1. `WryActivity::onDestroy` 调用 native `destroy()` 和 `onActivityDestroy()`。
2. `onActivityDestroy()` 向 Wry main pipe 发送 `WebViewMessage::OnDestroy`。
3. main pipe 返回 `MainPipeState::Destroyed` 后注销自身；这不是普通的隐藏或暂停。
4. tauri-runtime-wry 在最后一个 window 收到 `Destroyed` 时发送 `ExitRequested`；当前
   `run_gui()` 的 `app.run(|_app, _event| {})` 没有阻止退出。

EasyTier mesh、Leaf、HEV、VPN bridge、GUIClientManager 和 WebView 当前处于同一 Tauri/Rust
进程。因此单独 `webView.destroy()` 或 `Activity.finish()` 可能同时退出 Rust event loop 和
数据面。仅拦截 `ExitRequested` 也不能证明安全：Wry main pipe 已经注销，锁定版本没有
已验证的“保持旧 Rust app、创建第二个 Android Activity/WebView 并重新注册全部 plugin”
路径。

在完成进程分离前，以下操作均禁止：

- 后台固定 10 秒调用裸 `WebView.destroy()`。
- 用 `finishAndRemoveTask()` 期待 Rust core 自动留存。
- 全局 `WebView.pauseTimers()`；它会影响同进程 WebView timer，且无法区分 UI 与 VPN 中转。
- 注销所有 JS/Tauri listener；其中包含网络恢复和 TUN ownership listener。
- 通过阻止 Tauri exit 强行保留一个已经失去 Wry main pipe 的半存活 runtime。
- 用 `Process.killProcess()` 回收 UI；当前数据面在同一进程，会一起被杀死。

## 参考实现语义与 EasyTier 边界

本候选开始实现前采用本地 Mihomo Android 源码的以下行为作为参考：

- `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`
  的 `TunService::onCreate`：Service 自己创建 channel、先发布 loading notification，再启动
  runtime；通知不由 Activity/WebView 持有。
- `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/clash/module/StaticNotificationModule.kt`
  的 `StaticNotificationModule::{run,createNotificationChannel,notifyLoadingNotification}`：状态
  channel 使用低重要性，通知为 ongoing、`onlyAlertOnce`、不显示时间，并使用 content intent
  打开主 Activity；状态事件使用 conflated channel，不累积无意义刷新。
- `/Users/fanli/Documents/clashmeta-android-rev/app/src/main/java/com/github/kr328/clash/MainActivity.kt`
  的 `MainActivity::onCreate`：Android 13+ 在 Activity 中申请 `POST_NOTIFICATIONS`，权限结果
  不参与 VPN 启停结果。
- `/Users/fanli/Documents/clashmeta-android-rev/common/src/main/java/com/github/kr328/clash/common/compat/Services.kt`
  的 `Service::startForegroundCompat`：Android 14+ 显式传 foreground-service type。

EasyTier 的有意差异：

- Mihomo dynamic notification 的 1 秒流量 ticker 不移植。EasyTier 第一版只显示状态，完全
  由状态转换驱动，原因是降低后台 wakeup；边界是不显示实时速率或累计流量。
- Mihomo 显示单 profile 状态；EasyTier 聚合本地多个 network instance，原因是 GUI 支持
  多实例。聚合语义在下一节精确定义，不宣称所有 peer 健康。
- 第一版没有通知按钮、停止命令或配置写操作；失败时保留旧通知或初始化/停止文案，不影响
  数据面。
- 本候选保持现有 `dataSync` service type 和 `START_STICKY`，只覆盖当前 targetSdk 34 行为。
  Android 15+/targetSdk 35 的 FGS 类型和 6 小时时限另行审计；本候选不得宣称解决该兼容性。
- EasyTier 不采用 Mihomo 的独立 `:background` 进程；因此本候选不改变 WebView、Tauri event
  loop 或 core 进程所有权。

## 已冻结的近期候选：状态通知，无按钮

### 产品语义

第一版通知只承诺已经有权威来源的状态：

```text
EasyTier
Mesh：初始化中 / 已启动 / 已停止
Leaf：初始化中 / 已启用 / 未启用
```

- `Mesh 已启动` 表示至少一个本地 `NetworkInstance::is_easytier_running()` 为 true；不得使用
  `list_network_instance_ids()` 或 `running_inst_ids` 代替。后两者反映 `instance_map` 成员，
  实例异常停止后仍可能留在 map 中。
- `Mesh 已停止` 表示完成启动配置 reconciliation 后，没有本地实例实际运行。远程管理页面
  连接的其他设备不得计入 Android 本机通知。
- 第一版不显示 `错误`。当前没有同时覆盖启动失败、自行退出、部分实例失败和历史错误清理
  的稳定聚合 API；把 GUI/RPC 错误直接映射为通知状态会产生持久误报。错误仍由现有 UI 和
  日志呈现，未来如增加必须先定义独立权威状态和兼容测试。
- `Leaf 已启用` 表示至少一个**实际运行**的本地实例，其 effective
  `TomlConfigLoader::get_policy_proxy_config()` 返回 `enabled = true`。只保存但未运行、已经
  停止或远程设备的配置均不计入。
- `Leaf 已启用` 只表示运行实例的有效配置启用了 policy proxy，不得显示为“Leaf 运行正常”。
- exact Leaf runtime `Applying/Ready/Outage/Dormant` 当前没有暴露给 GUI；如未来需要，应
  单独设计只读状态 API 和 compatibility tests，不得混入第一版通知。
- 多实例使用 `any(actual_running)` 和 `any(actual_running && policy.enabled)` 聚合；不得隐式
  假设选中的 GUI instance 是唯一运行实例。
- 通知无停止按钮、无 notification action、无配置或生命周期写操作。允许一个只打开
  `MainActivity` 的 immutable content `PendingIntent`；它不是后台命令，点击外的任何路径
  都不能改变网络状态。

### Rust 权威状态与并发模型

- snapshot 固定为三个布尔语义字段：`initialized`、`mesh_running`、`leaf_enabled`。不得携带
  GUI 当前选择、peer 数、流量、错误字符串、配置正文或用户隐私数据。
- Rust 从本地 `INSTANCE_MANAGER` 的实际实例状态和 effective config 生成 immutable
  snapshot；Kotlin 只负责翻译资源和渲染，不能自行推断 Mesh/Leaf 语义。
- Android publisher 使用一个异步串行锁。每个调用者取得锁后重新读取当前权威状态，比较
  上一次成功 snapshot，再调用 Kotlin；不得在锁外计算后排队，避免旧 run/stop 结果晚到
  覆盖新状态。
- 只在 load reconciliation 完成或失败、run 成功或失败、显式 stop/disable/remove 完成、
  以及已运行实例自行退出后刷新。保存未应用配置、DHCP 地址、DNS/network key 和 VPN
  network-change 不改变本通知语义，不触发刷新。
- `post_run_network_instance_hook` 已持有 Android 实例 event receiver；receiver 关闭或实例
  stop notifier 完成时必须触发一次重新读取，而不是只退出监听任务。
- Kotlin 对相同 snapshot 再做一次幂等保护；相同状态不得重复调用
  `NotificationManager.notify()`。
- 禁止固定 1/2/5/10 秒轮询通知状态。
- 通知更新是 best-effort side effect。plugin 注册、Intent、序列化或 `notify()` 失败只记录
  有界日志，不得改变原 run/stop/load/remove 的返回值，不得回滚或停止 network instance、
  Leaf 或 VPN。
- App/WebView 暂时不可见时通知仍应保持最后一个原生确认状态；不得要求 JS ACK 才提交
  状态。
- Activity 正常冷启动时先显示“初始化中”，待 GUI manager 完成 config/load reconciliation
  后再显示最终状态，避免短暂假报“已停止”。

### Android Service、channel 与权限契约

- `MainActivity` 使用 Activity Result API 在 Android 13+ 首次进入时申请
  `POST_NOTIFICATIONS`。使用 native `SharedPreferences` 记录已经请求，Activity recreation
  不重复弹框；拒绝或异常不阻止 Service、Mesh、Leaf 或 VPN。
- `MainActivity` 只用 `ACTION_ENSURE_STARTED` 启动现有 service。service 已有 snapshot 时
  Activity recreation 不得重置通知；没有 snapshot 时才显示“初始化中”。
- Rust plugin 使用显式 `ACTION_UPDATE_STATUS` Intent 更新同一个未导出的 service。Intent
  只包含固定 snapshot 字段，Kotlin 对缺字段、未知 action 或非法值 fail closed。
- action 固定为 `com.kkrainbow.easytier.action.ENSURE_STATUS_SERVICE` 和
  `com.kkrainbow.easytier.action.UPDATE_STATUS_NOTIFICATION`；不得使用 implicit Intent 或
  exported receiver/service。
- 保持 notification ID `1355`；使用新的低重要性 channel ID `easytier_status_v2`，避免
  已安装版本原有 `IMPORTANCE_DEFAULT` channel 的行为影响新状态 channel。不得删除或重建
  用户已有 channel。
- 通知固定使用资源化字符串、合规的单色 small icon、`ongoing`、`onlyAlertOnce`、
  `showWhen(false)` 和 service category；至少提供默认英文及简体中文资源。
- `START_STICKY` 的 null-intent 仅可能来自进程被杀后的系统重启。此时同进程 Rust core 已经
  消失，service 必须丢弃进程内旧 snapshot 并显示 `Mesh 已停止 / Leaf 未启用`，不得恢复或
  持久化上一次“已启动”。重新打开 Activity 后才进入“初始化中”并重新 reconciliation。
- 权限被拒绝时 Android 可能不在普通通知抽屉显示 FGS notification；这是系统权限边界，
  不是网络启动失败。必须分别验证允许和拒绝路径。

### Rust -> Kotlin 桥接与跨平台隔离

桥接实现冻结为 app-local Android Tauri plugin，不复用也不修改
`tauri-plugin-vpnservice`：

- 新增 `easytier-gui/src-tauri/src/android_status_notification.rs`，只包含纯 snapshot reducer、
  Android publisher 和 app-local mobile plugin handle。
- 新增
  `easytier-gui/src-tauri/gen/android/app/src/main/java/com/kkrainbow/easytier/StatusNotificationPlugin.kt`，
  直接引用同 app module 的 `MainForegroundService`，避免通用 VPN plugin 依赖 GUI service。
- plugin 只从 Rust 内部调用，不暴露新的 JS command；前端不参与通知更新。
- Rust module 使用 `#[cfg(any(target_os = "android", test))]` 声明；其中 Kotlin/mobile handle、
  plugin 注册、publisher state 和所有运行时调用继续使用 `#[cfg(target_os = "android")]`。
  `test` 只允许 Linux builder 编译纯 reducer 测试，不得创建桌面 runtime 路径。
- `run_gui()` 中 plugin 注册放在独立的 `#[cfg(target_os = "android")]` block。不得把它加入
  当前所有平台共用的 `.plugin(...)` chain。
- shared run/stop/load/remove hook 中的 publish 调用逐处使用
  `#[cfg(target_os = "android")]`；macOS、Windows、Linux 编译后不存在调用、锁、状态或日志。
- 不新增 Cargo dependency，不修改 feature resolution，不修改 desktop plugin 列表、tray、
  window close、single-instance 或 dock 行为。
- 不修改 `easytier-gui/src/**`、`easytier-web/**` 或任何 Vue/TypeScript 文件；因此桌面 GUI
  bundle、JS event/listener 和 localStorage 语义保持逐字不变。

### 近期候选的改动边界

允许修改：

- `MainActivity.kt`：一次性通知权限请求和 `ACTION_ENSURE_STARTED`。
- `MainForegroundService.kt`：snapshot、显式 action、幂等渲染和 null-intent 语义。
- 新的 app-local `StatusNotificationPlugin.kt`。
- Android string/icon resource：默认英文、简体中文、单色 small icon 和 channel 文案。
- 新的 `android_status_notification.rs` 及其纯 reducer tests。
- `easytier-gui/src-tauri/src/lib.rs` 中 Android-cfg plugin 注册和既有 GUI manager
  run/stop/load/remove/instance-exit hook；只发布只读状态。
- Android app module 的 Kotlin unit tests，以及现有 Android candidate workflow 中必要的
  contract/Gradle test 接线。

第一版禁止修改：

- `api_manage.proto`、Leaf supervisor、HEV、mesh 数据面、TUN packet path。
- `tauri-plugin-vpnservice` 以及 `mobile_vpn.ts` 的现有 ownership、network-change、DHCP 和
  rebind 语义。
- foreground-service type、`START_STICKY`、VPN plugin service 和进程模型。
- WebView/Activity destroy、pause、restart 或配置持久化。
- 通知按钮、stop-all、deep link 或后台配置修改。
- macOS/Windows/Linux GUI plugin、tray、window lifecycle、frontend bundle 或 Cargo feature。

## 是否还需要“后台 10 秒门控”

当前结论是：**近期候选不需要。**

先用 exact Android artifact 测量 HOME 后台 30 秒和 10 分钟的：

- UI/WebView process CPU、线程 context switches 和 timer wakeups；
- Rust core、Leaf 和 VPN service 的分项 CPU；
- RSS/PSS、WebView renderer RSS、FD 和 tasks；
- 是否仍有 frontend RPC/HTTP 请求在 hidden 状态发生。

只有测量证明仍有具体 UI-only 任务在 hidden 后运行，才按任务逐个修复。修复必须使用现有
`visibilitychange`/`document.hidden` 所有权，不再创建一个全局“10 秒后暂停一切”的开关。

## 长期方案：Core service 与 UI 进程分离

如果硬需求是“后台 10 秒后真正释放 WebView/renderer 内存，同时 mesh、Leaf 和 VPN
继续运行”，必须先完成以下架构：

```text
Android :core process
  Core foreground service
  EasyTier InstanceManager
  Mesh / Leaf / HEV / VPN ownership
  native durable config
  authoritative notification status

Android UI process
  MainActivity + Tauri WebView
  Binder/local-RPC client
  disposable UI-only state
```

前置要求：

1. network configs、enabled instance IDs、VPN owner/epoch 从 WebView `localStorage` 迁到原生
   原子持久化；JS 只保存语言、主题、页面和表单草稿。
2. Android NetworkCallback、TUN FD、DHCP/DNS/network key 更新直接进入 core service/Rust，
   不再经 WebView。
3. UI 使用 snapshot + generation 重建，不能依赖销毁期间收到每个 event。
4. 通知完全由 core process 拥有；UI 进程死亡不改变通知和网络状态。
5. Activity 后台 10 秒后通过正常 lifecycle 结束 UI process，不调用内部裸 WebView API。
6. 点击通知启动新的 UI process，绑定 core，取得完整 snapshot 后再渲染。

该方向能完整实现可销毁 UI，但属于 Android 平台架构项目，不能包装成通知优化或几行
`onStop` 代码。

## 回归风险和强制验证

### 状态通知候选

Rust reducer 和 publisher contract：

- [ ] `initialized=false` 总是生成 Mesh/Leaf“初始化中”，不得泄露默认 false 为“已停止”。
- [ ] 空 manager、只有已停止实例、只有实际运行实例的 stopped/running 组合测试。
- [ ] 运行实例分别为 policy disabled/enabled；停止实例配置即使 policy enabled 也不得显示
  Leaf 已启用。
- [ ] 多实例测试覆盖全部停止、一个运行、混合运行/停止、多个运行且只有一个启用 Leaf。
- [ ] 回归证明 `instance_map` 中存在但 `is_easytier_running() == false` 的实例不会误报运行。
- [ ] 两个并发刷新串行后必须重新读取当前状态；旧 snapshot 不得覆盖新 snapshot。
- [ ] 相同 snapshot 不调用 mobile sink；sink 失败不更新 last-success snapshot，下一次转换可
  重试，且调用者业务结果保持成功或原错误。
- [ ] Rust/Kotlin command 名、payload 字段和 Kotlin export 有 source contract test。

Kotlin/Android unit contract：

- [ ] snapshot 的缺字段、未知 action、非法布尔/版本 fail closed，不生成“已启动”。
- [ ] `ACTION_ENSURE_STARTED` 在已有 snapshot 时不重置；首次启动显示初始化。
- [ ] `ACTION_UPDATE_STATUS_NOTIFICATION` 相同 snapshot 幂等，不重复 `notify()`。
- [ ] null-intent sticky restart 丢弃旧状态并渲染 stopped/disabled。
- [ ] notification model 固定 channel ID/notification ID、ongoing、only-alert-once、无 action
  button，content intent 只指向 `MainActivity`。
- [ ] Android 12 及以下不请求通知权限；Android 13+ 只请求一次，允许、拒绝、异常均不改变
  service/network 返回结果。

Exact Android artifact 实机验证：

- [ ] 使用 `adb install -r` 保留现有 app data；验证升级前后 configs、enabled networks、
  WebView Local Storage 和 IndexedDB 不变。
- [ ] `dumpsys notification --noredact` 验证 channel、ID、标题、Mesh/Leaf 文案、无 action；
  最终截图只用于图标和中英文视觉确认。
- [ ] Android 13+ 分别 grant/revoke `POST_NOTIFICATIONS`：允许时通知栏可见，拒绝时网络仍正常
  且无启动错误。
- [ ] 无配置、无 TUN mesh、policy TUN、多个本地实例分别验证实际运行与 Leaf 配置聚合。
- [ ] run、stop、disable、remove、启动失败、实例自行退出、配置 reload 后状态最终一致；DHCP、
  DNS/network key 和 VPN network-change 不触发无关 notify。
- [ ] HOME、息屏、返回前台和 Activity recreation 后不短暂假报、永久陈旧或重复提示权限。
- [ ] 用同 UID 的有界进程终止方式验证 sticky null-intent；不得用卸载/清数据或
  `am force-stop` 代替系统重启语义。
- [ ] captured-UID TCP/TLS、UDP、FakeDNS、HEV chain/fallback 和 mesh 流量不变。
- [ ] 后台 30 秒和 10 分钟 CPU/RSS/FD/tasks、timer wakeup 与同 SHA Android 基线没有显著
  回退；通知状态转换期间没有固定周期 wakeup。

macOS/Windows/Linux 不受影响的硬门禁：

- [ ] candidate diff 不包含 `easytier-gui/src/**`、`easytier-web/**`、desktop native source、
  Cargo dependency/feature 或 desktop Tauri plugin 改动。
- [ ] `rg` 审计所有 `android_status_notification` 和 `StatusNotificationPlugin` 引用：运行时
  module、plugin 注册、publisher state、hook 调用均在 `target_os = "android"` 下。
- [ ] `.160` 对完整 snapshot 执行 Linux desktop `--locked` no-run 和纯 reducer focused tests；
  证明 Android module 的 test-only reducer 不创建 Linux runtime side effect。
- [ ] 检查展开后的 candidate diff：macOS/Windows 的 tray、single-instance、dock、window
  close、invoke handler、VPN plugin 注册顺序与候选前一致。
- [ ] 本候选不因 Android-only 文档或代码单独触发 macOS workflow；正式 release 前仍由
  exact validated SHA 的正常 GUI matrix 编译 macOS/Windows，禁止绕过正式平台门禁。

### 任何未来 WebView 生命周期候选

- [ ] WebView 销毁前后 core PID/TID 不变，mesh 与 Leaf 连续流量不中断。
- [ ] UI 进程可重复冷启动并从 authoritative native snapshot 恢复，不依赖丢失的 event。
- [ ] 前后台/销毁/重建 1000 次没有 plugin callback、FD、task、renderer 或 Activity 泄漏。
- [ ] Wi-Fi outage 恰好发生在 UI 销毁窗口时仍能恢复 DNS、TUN routes、mesh 与 policy TLS。
- [ ] 系统回收 UI process、core process、VPN ownership 被其他应用夺走三种路径均 fail closed。
- [ ] 升级安装保留配置和 enabled 状态；不得用清数据或卸载掩盖迁移问题。

## 候选执行要求

- 状态通知是一个 Android-only build-affecting workstream。实现前在
  `leaf_parallel_workboard.md` 冻结与其他已经 ready 的 Android/Leaf workstream 的批处理
  边界；不得为了单个字符串或 icon 单独触发 workflow，也不得混入未准备好的 WebView 销毁
  实验。
- 本地只格式化，不编译。候选先同步到 `192.168.2.160`，使用
  `scripts/leaf-remote-preflight.sh` 完成完整 batch 的 `--locked` no-run 和 exact reducer/
  lifecycle focused tests。候选不改 TypeScript/Vue，因此不运行、也不声称需要 frontend
  production build；Android Gradle/SDK-only 部分记录 workflow exception，由 Android
  candidate workflow 执行 `testDebugUnitTest` 和 APK build。
- 推送前更新 `leaf_parallel_workboard.md` candidate manifest，明确没有数据面、Proto、Leaf
  runtime、VPN ownership 或非 Android GUI 语义变化，并记录：完整文件白名单、`.160` 命令和
  结果、Android unit tests、自动 Linux/Android workflow、实机矩阵以及每段等待期间任务。
- 推送前检查 `Cargo.lock`、Android/desktop `cfg`、workflow pins、Android manifest merge、
  generated Android source 和完整 candidate diff；candidate manifest 与 `.160` 证据未通过时
  不得 commit/push/dispatch。
- push 前先查询 exact SHA workflow；`codex/profiling-beta` 自动启动 Linux profiling 和
  Android policy candidate 时不得重复手动 dispatch。
- 只接受 exact SHA Android artifact；保留 app data 使用 `adb install -r`，运行完整通知、
  生命周期、网络恢复、captured-UID policy 和资源基线矩阵。
- 任一状态误报、listener 累积、网络恢复失败、后台 CPU 回退或配置持久化问题均 literal
  revert；不得用延长 10 秒 timeout 或增加轮询掩盖。

## 当前决定

- [ ] 先测量 hidden 状态下剩余 WebView/UI CPU、wakeups、RPC 和 renderer PSS。
- [x] 第一候选仅做无按钮、无轮询、事件驱动状态通知；实施规格已冻结，可以进入代码阶段。
- [x] 第一候选不得增加后台 10 秒门控；现有主要 UI scheduler 已在 hidden 时停止。
- [x] 第一候选通过 Android-only `cfg`、文件白名单和无 frontend/Cargo dependency 改动隔离；
  macOS/Windows/Linux GUI 不增加任何运行时路径。
- [x] 第一候选不显示 Mesh `错误`，避免把 `instance_map`/历史错误误当作实际运行状态。
- [ ] 修复 VPN plugin listener unlisten/只注册一次所有权后，再做 Activity recreation soak。
- [ ] 只有确认 WebView/renderer PSS 是必须回收的实际问题，才建立 Core/UI 双进程项目。
- [x] 当前明确不实施单进程 `WebView.destroy()`。
