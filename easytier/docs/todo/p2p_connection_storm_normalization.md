# P2P 打洞风暴降频

## 状态

实现候选基于已发布的 `v3.0.7` 最新代码 `deed0cf2`，在独立分支
`codex/p2p-storm-normalization` 开发。

本批只处理重复失败的 hole-punch burst，不重构连接生命周期，也不改协议选择。

## 目标和非目标

目标：

- 已有可用连接时，同一 peer、同一打洞协议、同一远端 `IP:port` 在 10 秒内出现
  超过 10 个失败的 underlay socket fanout 后，静默该目标后续 burst 60 秒。
- 当前已开始的 burst 必须完整执行；静默只能作用于下一轮。
- 新目标地址立即获准尝试；peer 已无可用连接时立即绕过静默恢复连通性。
- 状态固定大小、task-local、零堆分配、零锁，不受随机 endpoint 数量影响。

非目标：

- 不处理 Direct、普通 connector、DNS、握手或业务流量。
- 不改变 `lazy_p2p`、`need_p2p`、`disable_p2p`、transport priority、upgrade、
  fallback、RTT 防抖或现有 BackOff。
- 不改变 UDP/TCP 打洞的 socket 数、猜端口、发包次数、等待窗口或并发方式。
- 不增加指数冷却、持久化、公开 API、配置项或 protobuf 字段。
- 不复用或修改受 `underlay_candidate_guard` 设置控制的 `UnderlayBreaker`。
- 路由净化仍是后续独立事项。

## 最小实现

每个现有 UDP/TCP peer task 在自己的异步任务栈中持有一个
`PunchStormGuard`。它只保存：

- 10 秒窗口起点；
- 60 秒静默截止时间；
- 最近目标的固定 16-byte IP、port 和地址族 key；
- 饱和在 11 的 `u8` 失败 fanout 数；
- 一个目标有效位。

目标完整 `SocketAddr` 只存在于当前 burst 的栈上 `PunchBurstTrace`，用于精确比较和
诊断日志。guard 不保存 endpoint 列表，也没有全局 Map、LRU、时间戳队列、锁或原子。

target key 不使用哈希，没有碰撞语义。peer 失联会无条件清空并绕过该状态。

状态生命周期与现有 peer task 完全一致：task 被 collector 取消、peer 被移除或任务正常
退出时自动释放。异常随机 IP/端口只能覆盖当前 target，内存不会增长。

## 准入和结算

每个协议先按原逻辑取得这一 burst 将使用的稳定 mapped/listener `IP:port`，然后调用唯一
的私有接缝：

```text
PunchBurstTrace::begin_target(
    task_local_guard,
    monotonic_now,
    peer_has_live_connection,
    remote_ip_port,
)
```

规则：

1. peer 没有可用连接：清空 guard 并允许，保证恢复能力。
2. target 与被静默 target 不同：清空旧窗口并允许，新地址不继承旧地址失败。
3. target 相同且仍在 60 秒静默期：本轮不进入昂贵的本机批量发送。
4. target 相同但静默已到期：清空窗口并允许一次完整 burst。

获准的 burst 内不再读取 guard，不会在第 11 个 socket、猜测端口或数据包处中断。burst
结束后只结算一次：

```text
成功 tunnel 且成功加入 PeerManager
    -> 清空 guard

完整执行后 Ok(None)
    -> 一次性提交本 burst 的 socket fanout 数

busy、取消、blacklist、self-loop、RPC、STUN、bind、本地资源或其他 Err
    -> 不提交失败
```

只有失败 fanout 累计值从 `<= 10` 变为 `> 10` 时记录一次 transition warning；静默
轮询不重复打印 warning。

## 计数口径

UDP 使用本轮参与发送的不同本地 socket 数，而不是数据包数：

- `send_with_all()` 虽然每个 socket 重发 3 个包，只返回 `sockets.len()`。
- 同一 socket 在等待循环中重发不再次累计。
- cone 的单 socket burst 计 1。
- hard symmetric 的 84-socket fanout 计 84，立即饱和为 11。
- both-easy-symmetric 的 25-socket fanout 只在该 burst 第一次批量发送时计 25。
- cone fallback 与后续 symmetric 阶段仍属于一个 burst；只有最终结果结算一次。
- 阶段切换到不同 target 时，当前 trace 清空旧 target 的 fanout，防止把两个地址混算。

TCP initiator 的一次主动 simultaneous-connect 计 1；listen/accept 等待不伪装成额外
目标连接。现有 TCP BackOff 本身已使它很难触发 10 秒阈值，但仍使用相同语义。

## 协议接入边界

### UDP cone

`select_punch_listener` 是轻量地址协商。取得 target 后先准入；被静默时不 bind 本地打洞
socket、不做 UPnP 映射、不调用远端批量发包 RPC。

### UDP symmetric-to-cone

取得 listener target 后先准入；被静默时不创建/复用 84-socket fanout，也不进入
predictable/random 扫描。获准后原有 direct-connect、predictable、random 和等待顺序
不变。

### UDP both-easy-symmetric

该既有算法要求 25 个接收 socket 在 `send_punch_packet_both_easy_sym` RPC 前监听，否则
会漏掉远端首批包并降低成功率。因此本批严格保留这一时序。

RPC 返回 target 后立即准入；被静默时不执行本机后续 `send_with_all` 批量发送。这里不能
在不改变 RPC/打洞协议的前提下同时省掉前置接收 socket 和远端已经开始的首批发送。本批
选择保持可用性，不虚假宣称该子路径能在 RPC 前完全静默。

### TCP

`exchange_mapped_addr` 返回 target 后准入；被静默时不执行主动 simultaneous-connect，
也不进入后续 listen/accept。RPC、STUN 或本地端口错误不计作 target failure。

## 自动化契约

新增或补强以下测试：

1. 10 次失败仍允许，第 11 次失败只阻断下一 burst。
2. 60 秒到期自动恢复并清空窗口。
3. 10 秒窗口边界丢弃旧失败。
4. 新 target 在旧 target 静默期间立即允许且不继承计数。
5. target 在一个 burst 内变化时只保留最后 target 的 fanout。
6. 成功 tunnel 清空；peer 无可用连接时清空并绕过。
7. 空 trace、取消和未产生 underlay fanout 的失败不改变 guard。
8. 计数饱和在 11；guard 大小受固定预算约束。
9. `send_with_all()` 对 2 个 socket、每个重发 3 包时返回 2 而不是 6。
10. 现有 cone、symmetric、both-easy-symmetric、TCP、lazy P2P、blacklist、
    stealth 和 socket-mark 测试保持通过。

## 性能接缝

`tools/punch-storm-guard-probe` 对 unchanged baseline 与 task-local guard 做独立微基准：

- 1,024 个并发 task 状态；
- 5,000,000 次 burst-boundary 准入/结算；
- exact target key、10 秒窗口、60 秒静默和成功复位；
- 统计每次操作耗时、结构大小和全局 allocator 调用数。

这是 burst 边界成本，不在数据包热路径。进入候选前必须在 `.160` 确认固定大小、零
分配、零锁，且单次开销远小于一次系统调用；微基准不代替真实 artifact 场景验收。

## Pre-build candidate manifest

### 预期 build snapshot

- 基线：`deed0cf2`。
- 新增：`connector/punch_storm.rs` 和 `tools/punch-storm-guard-probe`。
- 修改：UDP cone/symmetric/both-easy-symmetric 的 burst trace 与目标准入，
  TCP initiator 的相同边界，以及 `send_with_all()` 的 socket fanout 返回值。
- 文档：本 TODO 和既有路由净化 TODO；文档不扩大本批实现范围。
- 不修改 Cargo dependency、feature、protobuf、公开 API、设置定义和 workflow。

### `.160` 门禁

最终快照必须依次完成：

1. `cargo test --locked --no-run --package easytier --lib`。
2. 直接执行同一测试二进制中的 `connector::punch_storm` 契约测试。
3. 执行 `send_with_all_counts_socket_fanout_not_retransmit_packets`。
4. 串行执行 UDP/TCP hole-punch 现有 focused tests。
5. 构建并运行 `tools/punch-storm-guard-probe`。

2026-07-29 在基于 `deed0cf2` 的最终未提交快照上完成：

- `--locked --no-run`：通过，未见编译警告。
- `connector::punch_storm::tests::`：6/6 通过。
- `connector::udp_hole_punch::`：24 通过、1 个既有 SO_MARK 权限测试按预期忽略。
- `connector::tcp_hole_punch::`：4/4 通过。
- `connector::tests::`：14/14 通过，覆盖 lazy/disable/public-server/priority 语义。
- 微基准：5,000,000 次操作、1,024 个 task，guard 为 56 bytes；baseline
  38.673 ns/op，candidate 121.476 ns/op，增量 82.803 ns/op；两者 allocator
  调用均为 0。

以上只证明编译、契约和边界开销；真实降频效果仍必须由 immutable profiling artifact
场景验证，不能用微基准替代。

### GitHub 和实机计划

- 一个 `profiling-beta` workflow 构建 immutable optimized x86_64-musl artifact。
- 校验 release asset 的 SHA、build ID、commit、target 和符号后，再部署该 exact artifact。
- `192.168.1.37/.38`：CentOS 7 启动、基础互联、UDP/TCP 打洞和资源回归。
- 公网双栈验证对：性能/互操作基线；所有文件放各自主机专用的持久化子目录。
- Mihomo 场景机：对比稳定 peer 下 60 秒窗口内 EasyTier UDP 会话新增速率，并验证
  60 秒后允许一轮、新 target 立即允许、peer 失联恢复不受阻。
- Android 设备未由维护者重新开放时不等待、不连接；保留 workflow 编译门禁，并在
  post-build evidence 明确真实 Android 场景未覆盖。

## Post-build evidence

按 immutable candidate SHA 单独记录 workflow run、artifact 校验、主机命令、会话增速、
CPU/内存、功能矩阵和清理结果。补录这些证据不得触发第二次构建。
