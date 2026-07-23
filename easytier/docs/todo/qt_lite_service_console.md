# Qt Lite 服务控制台 TODO

状态：`TODO / USER CONFIRMED SCOPE`

## 目标

新增一套独立的 Qt Lite 服务控制台，用于不希望安装完整 GUI、又不想直接操作
`easytier-core` 的用户。

它不是现有 GUI 的替代品，也不是第二套完整 GUI。现有 Tauri GUI、Web 前端及其逻辑
禁止修改。

## 技术边界

- 使用 Qt 6 Widgets，不使用 QML、Qt Quick、WebView 或 Electron。
- Qt 运行库随安装包分发，不要求用户单独安装 Qt。
- 界面保持为一个简单的终端/状态窗口和必要的配置、服务操作入口。
- Qt Lite 不实现第二套配置表单。配置页面直接复用现有
  `easytier-web/frontend-lib` 的配置组件，通过系统默认浏览器按需打开。
- 直接使用当前 EasyTier 的配置格式、解析器、服务命令和状态接口，不实现第二套网络
  核心，不引入独立 FFI 配置模型。
- 除安装文件、系统服务定义和用户主动保存的配置外，运行期间几乎零写盘。
- 临时运行日志和操作记录仅保存在有容量上限的内存 ring buffer 中；退出即丢弃。

## 最小实现结构

只新增两个 Lite 组件：

1. `easytier-lite`：Qt 6 Widgets 可执行文件，只包含 `QSystemTrayIcon`、按钮、
   `QPlainTextEdit`、默认关闭的 `QTimer` 和 loopback 请求客户端。
2. `easytier-lite-agent`：随 Qt Lite 启动和退出的普通子进程，复用现有 Rust 模块，提供
   一个带一次性令牌的 loopback API，并按需提供临时配置页面。

`easytier-lite-agent` 不是系统服务或第二套 daemon：不得注册、自启动、脱离父进程、运行
网络数据面或持久化自身状态。Qt Lite 退出或父进程消失时它必须退出。安装后登录前运行的
仍然只有现有 `easytier-core` 系统服务。

Qt 状态窗口和系统浏览器配置页共用 agent 的同一个 loopback API，不再增加 stdio、
QLocalSocket、Tauri invoke 或 C++/Rust FFI 等第二条通信路径。

## 直接复用清单

- 服务安装、更新、查询、启停和异常恢复：直接复用
  `easytier::service_manager::{Service, ServiceInstallOptions, ServiceStatus}`；agent 的特权
  service-action 模式执行完成即退出，不复制 systemd、launchd 或 Windows Service 逻辑。
- Core 管理协议：直接复用生成的 EasyTier RPC client、`WebClientService`、
  `CollectNetworkInfoResponse` 和其他 protobuf 类型，不在 C++ 中实现 RPC framing。
- 配置解析与生成：直接复用 `TomlConfigLoader`、`NetworkConfig::new_from_config()` 和
  `NetworkConfig::gen_config()`，不自行实现 TOML 字段映射。
- 配置业务操作：优先复用现有 `WebClientService`/`RemoteClientManager` 行为；若服务停止，
  文件模式也必须使用同一解析器和同目录原子替换，不建立数据库。
- 配置 UI：直接复用 `frontend-lib` 的 `RemoteManagement`/`Config`、`NetworkConfig`、
  `Api.RemoteClient`、字段规范化、locales 和校验提示；Lite 页面设置
  `pauseAutoRefresh=true`，不运行 Web 状态轮询。
- 状态数据：直接复用 `CollectNetworkInfoResponse` 中的 peer 和
  `proxy_failover_entries`，并沿用现有最新 256 条排序/截断语义。
- 静态页面服务：只复用现有 Axum/RustEmbed 技术和构建产物，不启动完整
  `easytier-web` 后端。
- 视觉资源：复用现有 EasyTier 图标；Qt 托盘行为使用标准 `QSystemTrayIcon` 重新连接，
  不移植 Tauri tray 代码。

唯一需要新增的业务外代码是：Qt 壳、`LiteRemoteClient` 的 HTTP 传输、agent 的极少量
路由、完整 GUI 安装检测，以及跨平台 Core PID 的 CPU/RSS/FD-or-handle/线程采样。

## 明确不复用

- 不复用 Tauri `GUIRemoteClient`、Tauri commands、WebView 或 tray 外壳。
- 不复用完整 `easytier-web` 的 REST handler、SQLite、账户、session、OIDC 或 machine
  管理模型。
- 不用 `easytier-cli` 每 2 秒启动新进程轮询状态；自动刷新只复用 agent 中的长连接 RPC
  client，避免进程创建开销。
- 不把 EasyTier RPC、protobuf、配置解析或服务管理重新实现成 C++ 版本。
- 不新增长期运行的 Lite 后台服务、watchdog 或持久化 IPC。

## 功能 TODO

### 1. 服务管理

- [ ] 安装或卸载 Lite 套件。
- [ ] 注册、注销、启动、停止和重启 EasyTier 系统服务。
- [ ] 查询服务是否已注册、当前状态、PID、启动时间和最近一次退出结果。
- [ ] 使用 Windows Service Manager、systemd 或 launchd 自身的恢复机制，在进程异常
      终止后自动重启；Lite 不实现常驻 watchdog。
- [ ] 登录前可由系统启动 EasyTier 服务；Lite 控制台本身不要求登录前运行。

### 2. 配置管理

- [ ] Qt 窗口只提供“配置”入口，不实现任何原生配置字段或布局。
- [ ] 点击“配置”时临时启动仅监听 loopback 随机端口的本地配置会话，并用系统默认
      浏览器打开页面；禁止引入或打包 Qt WebEngine。
- [ ] 页面直接复用 `easytier-web/frontend-lib` 已有配置组件、字段、布局、校验提示和
      locales；核心字段增减时不维护第二份 Lite 布局。
- [ ] Lite 页面直接 import 现有 `frontend-lib` 的 `Api.RemoteClient`、配置类型和生成类型，
      不复制接口、DTO 或字段定义。
- [ ] 为共享前端新增一个最薄的 `LiteRemoteClient` 传输适配器，只把同一接口转换成临时
      localhost 请求；本地后端再调用现有 Core RPC。
- [ ] 不复用依赖 Tauri `invoke()` 的 `GUIRemoteClient`，也不复用依赖完整 Web 账户、数据库
      和 machine REST 路径的 `WebRemoteClient`；不得为此分叉共享接口语义。
- [ ] `LiteRemoteClient` 只负责读取、解析、校验和保存当前 EasyTier 配置，以及在用户确认
      后重启受管服务；字段和布局变化仍只在共享 `frontend-lib` 中维护一次。
- [ ] 不直接启动完整 `easytier-web`：不引入 SQLite、用户注册、登录、远程设备会话、
      OIDC 或常驻 Web 服务。
- [ ] 临时会话使用一次性随机令牌，不接受非 loopback 请求；不设置账户体系。
- [ ] 保存时使用当前 EasyTier 解析器校验；配置无效时禁止写入和重启服务。
- [ ] 保存时使用同目录临时文件加原子替换，避免中途退出产生半写配置。
- [ ] 用户执行“保存并关闭”后立即结束本地配置会话并释放页面后端资源。
- [ ] 用户直接关闭浏览器页面时，以页面心跳消失后的短超时结束会话；Qt Lite 退出时立即
      结束会话。页面关闭检测不承担配置正确性，未保存内容直接丢弃。
- [ ] 同一时刻最多存在一个配置会话；再次点击“配置”只重新打开现有会话。
- [ ] 静态页面资源内嵌在安装产物中，不解压临时目录；会话、令牌和未保存配置只存内存。

### 3. Core RPC 接口发现

- [ ] 共享前端的 `Api.RemoteClient` 只是 TypeScript 方法契约；Lite 本地适配器把这些方法
      转换为当前 Core 的 EasyTier TCP RPC 调用，它不是一个固定 HTTP 端口。
- [ ] Lite 注册的服务应保存并优先使用其明确的 `--rpc-portal`/`ET_RPC_PORTAL`；不要把
      `15888` 或 `15999` 写死为唯一接口。
- [ ] 已记录的 RPC portal 连接失败时，从系统服务管理器取得受管 EasyTier Core 的准确
      PID，只枚举该 PID 当前拥有的 TCP 监听端口，不扫描整机端口范围。
- [ ] 将枚举出的非标准监听端口作为候选列表交给用户选择“尝试连接”；每次尝试使用短
      超时调用只读的 EasyTier RPC 方法进行协议验证。
- [ ] 只有通过 EasyTier RPC 验证的候选才记为本次运行的管理接口；选择和探测结果不写盘。
- [ ] 连接时把 `0.0.0.0`/`::` 监听地址转换为本机 loopback 地址；不尝试远程地址。
- [ ] 找不到服务 PID、没有候选端口或所有候选均验证失败时，只报告无法连接，不猜测、
      不修改配置，也不自动重启服务。

### 4. 与完整 GUI 严格互斥

- [ ] Lite 安装、服务注册和每次启动时只检查完整 GUI 是否已经安装。
- [ ] 只要检测到完整 GUI 已安装，无论 GUI 进程或后台服务是否正在运行，立即弹出提示并
      终止当前操作。
- [ ] 错误信息明确要求用户先卸载完整 GUI，再重试 Lite 安装、注册或启动。
- [ ] 不自动卸载、停止、迁移或修改完整 GUI，不自动改端口规避冲突。
- [ ] 检测规则按平台集中实现并测试；不检查运行进程、服务状态或监听端口，也不提供
      “停止完整 GUI 后继续”的兼容分支。

### 5. 状态与资源展示

- [ ] 显示 EasyTier 进程 CPU、RSS、FD/handle、线程数和运行时间。
- [ ] 显示 peer 状态。
- [ ] 显示 proxy/failover 当前状态和切换结果。
- [ ] proxy/failover 展示与现有 GUI 保持相同上限：最多保留 256 条，按
      `start_time` 降序、再按 `generation` 降序，仅显示最新记录。
- [ ] 即使旧 Core 返回超过 256 条，Lite 也必须在送入 Qt model/文本窗口前完成排序和
      截断；刷新时替换上一份快照，不得持续追加或一次渲染数千条记录。
- [ ] 所有状态集中显示在一个简洁的终端式页面，不开发复杂仪表盘。
- [ ] 默认禁止自动刷新。
- [ ] 用户开启自动刷新后默认每 2 秒刷新一次。
- [ ] 允许用户为当前运行临时覆盖刷新间隔；不写盘、不持久化。
- [ ] 同一时刻最多存在一个状态查询；上一次尚未完成时不叠加新查询。
- [ ] 暂停刷新或关闭窗口后停止定时器和在途查询。

### 6. 内存日志

- [ ] 使用固定容量 ring buffer 保存 Lite 操作日志和最近查询错误。
- [ ] 达到容量后覆盖最旧记录，不随运行时间增长。
- [ ] 默认不写日志文件，不接管或复制 EasyTier Core 自身的日志系统。
- [ ] 如需导出，由用户显式执行一次性保存。

## 最小验收条件

- [ ] 未安装完整 GUI 时，Lite 可以安装、注册服务并在重启系统后于登录前运行服务。
- [ ] EasyTier 进程异常退出后，由系统服务管理器按注册策略恢复。
- [ ] 检测到完整 GUI 的安装记录时，即使其进程和服务均已停止，Lite 的安装、注册和启动
      仍然拒绝，且不会修改完整 GUI。
- [ ] 有效配置可以保存并重启服务；无效配置不会覆盖原文件。
- [ ] Qt Lite 不包含配置字段布局；同一份 `frontend-lib` 配置组件同时供现有 GUI 和 Lite
      临时页面使用。
- [ ] 关闭配置页面或退出 Qt Lite 后，本地临时监听端口在短超时内消失，且不留下数据库、
      日志或临时 Web 资源。
- [ ] Core 使用非默认 RPC portal 时，Lite 优先按服务参数连接；参数失效时能列出准确服务
      PID 的 TCP 监听候选，由用户选择并只接受通过 EasyTier RPC 验证的接口。
- [ ] 默认静止页面不产生周期查询；开启后默认 2 秒刷新，临时修改间隔不会写盘。
- [ ] Core 返回超过 256 条 proxy/failover 记录时，界面仍只持有和展示排序后的最新
      256 条；连续刷新不会累积旧快照。
- [ ] 连续启停服务、开关刷新和打开关闭窗口后，Lite 的 RSS、FD/handle 和线程数不持续
      增长，ring buffer 内存保持有界。
- [ ] peer、proxy failover 和进程资源信息查询失败时只显示错误，不导致服务被停止或配置
      被修改。
- [ ] 最终 diff 不包含现有 Tauri GUI 和 Web GUI 的逻辑修改。

## 最小构建与发布增量

- [ ] 只新增一个独立 `.github/workflows/lite.yml`；不要把 Lite job 加入 `gui.yml`，避免
      Lite-only 修改触发完整 Tauri GUI 矩阵。
- [ ] 不新增第二套 tag/Release workflow。现有 `release.yml` 只增加同一 SHA 的
      `lite.yml` 成功 run 解析、Lite artifact 下载和资产汇总；版本、tag、release notes
      和 Release 入口仍然只有一套。
- [ ] `lite.yml` 只构建支持平台的 Qt 壳、`easytier-lite-agent`、共享临时页面和安装包；
      不复制 Core、GUI、Mobile、OHOS 或 Test workflow 的测试内容。
- [ ] 正式安装包优先消费 `core.yml` 对同一 SHA 产生的 Core artifact，不在 Lite workflow
      另造一份不同构建参数的 `easytier-core`。
- [ ] Lite workflow 的自动触发范围只包含 Lite 源码、共享 `frontend-lib`、Lite 使用的
      Core RPC/config/service API、Qt 构建与打包文件和依赖锁；Markdown/验证结果不触发。
- [ ] Lite 版本只从 `easytier/Cargo.toml`/构建 SHA 注入 CMake 和安装包，不维护第二份
      手工版本号。
- [ ] Lite 安装包复用同一最终 SHA 的正式 `easytier-core`，不得自行产生另一份 Core
      候选；只新增 Qt 壳和 `easytier-lite-agent` 构建。
- [ ] `frontend-lib` 仍执行现有类型检查、测试和构建；Lite 只增加一次临时页面加载、配置
      读取/校验/保存的集成冒烟，不复制完整 GUI 测试矩阵。
- [ ] Lite 专属验证仅覆盖：完整 GUI 安装互斥、服务注册/恢复、RPC portal 发现、默认无
      刷新、2 秒刷新无重叠、256 条 failover 上限、配置会话销毁和资源不持续增长。
- [ ] Qt 依赖使用官方部署工具随包收集，不维护手写 Qt 动态库清单。
- [ ] 文档或结果记录变化不得重建 Lite；只有 Lite 源码、共享 `frontend-lib`、相关 Core
      RPC/config/service API、Qt 构建配置或依赖变化才触发 Lite 构建。

## 明确不做

- 不替换、不重构现有 Tauri GUI，也不复用它的 Tauri 外壳；只消费双方已有的共享
  `frontend-lib` 配置组件。
- 不复制现有 GUI/Web 的配置字段和布局；Lite 只消费共享 `frontend-lib` 组件。
- 不开发 QML/Qt Quick 界面。
- 不使用 Qt WebEngine，也不在 Lite 内嵌浏览器内核。
- 不实现网络数据面、第二套 daemon 或第二套配置语义。
- 不把完整 `easytier-web` 的数据库、账户或远程管理服务带入 Lite。
- 不提供拓扑图、图表仪表盘、历史数据库、遥测或后台日志索引。
- 不自动调整监听端口来兼容完整 GUI。
- 不让 Lite 控制台常驻承担服务故障恢复。
