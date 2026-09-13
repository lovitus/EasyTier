# Secure relay 首里程碑：设计与验收提案

状态：**DRAFT_PROPOSAL / SOURCE_REVIEW_ONLY**。整理日期：2026-09-13。

关联：[总议题 #1](https://github.com/lovitus/EasyTier/issues/1)、[首里程碑 #2](https://github.com/lovitus/EasyTier/issues/2)、[协作审查反馈](https://github.com/lovitus/EasyTier/issues/2#issuecomment-5654151293)。

本文件仅形成可评审的设计与测试合同，不修改业务代码，不批准实现、合并或发布，也不声称任何回归已运行或通过。两个行为分别提供证据，经批准后可作为一个完整实现批次交付。本文件不覆盖团队尚未提交的总研究 TODO。

## 1. 固定输入与证据边界

| 输入 | 固定值 | 证据状态 |
| --- | --- | --- |
| fork | `lovitus/EasyTier`，目标分支 `codex/current` | 本次已重新查询远端 ref |
| fork 基线 | `c6772dbfef2395ff96b39bd4801945d92212dffb` | 与本线程此前源码阅读基线一致；不等于检查了维护者本地未提交内容 |
| 上游来源 | `425a24273b192399d3de3509fd868382f8ec89cc` | 本线程此前已读取补丁；本提案选择性提取，不整体 cherry-pick |
| 授权范围 | #1 / #2 中的首里程碑提案与基线核对 | 业务代码实施仍需明确批准 |
| 运行证据 | 无 | 没有 baseline failure、修复通过、benchmark 或设备测试结果 |

以下源码事实继承本线程对固定版本的实际阅读，不表示本次重跑了全仓审计。正式实施前须补齐完整调用点、实际依赖和 feature 配置；新 HEAD 的差异不能默认为零。

## 2. 行为边界

**包含：** 下一跳 session 的目标保护；有效 session 的活动感知回收；与这两项直接相关的查询语义、对象身份保护及回归测试。

**保留：** 连接级 Stealth；派生模式的 connection-local session；显式 Secure Mode 的 direct/relay 共享；key/generation、轮换、replay 与现有 crypto/connection 锁语义；原有控制包豁免和 GC 调度。

**不包含：** ArcSwap、pinger、liveness/QUIC wire 协议、DNS、relay 路由或 bootstrap 修改、额外用户配置/timer、版本号、workflow 或发布流程改造。新增活动字段可能需要同步；其具体实现待评审，不承诺零开销，也不借此替换现有锁框架。

## 3. 源码事实与拟修改位置

| 位置 | 已读事实 | 待评审的修改方向 |
| --- | --- | --- |
| `peer_conn.rs::PeerSessionTunnelFilter::before_send` | 检查本机来源，但缺少最终目标与当前 next-hop peer 的一致性条件 | 保持控制包豁免；仅对本机发给当前 peer 的包使用该连接的 payload session |
| `peer_session.rs::PeerSessionStore::evict_unused_sessions` | 仅使用外部强引用计数保留 session | 有效性、活动宽限与外部持有共同决定保留 |
| `peer_session.rs::get` | 无活动时间；失效查询可能触发按 key 删除 | 保持 non-touch；评估纯查询或对象身份受保护的条件清理 |
| `peer_session.rs::upsert_responder_session / apply_initiator_action` | Join/Sync/Create 与算法、公钥、generation 校验在此发生 | 仅成功完成所定义的会话操作后记活动；审计失效替换路径 |
| `peer_conn.rs::existing_session_generation` | 只读取已存在 session 的 generation；connection-local 路径另行处理 | 查询不续期，保持派生 Stealth 不进入全局共享 store |
| `relay_peer_map.rs::has_session / ensure_session / handshake_session / handshake_session_once` | 多类检查和使用共用 get | 区分存在性检查、空 flush、建立、真实使用，不让所有 get 隐式续期 |
| `relay_peer_map.rs::decrypt_if_needed` | 在调用 session 解密之前更新 relay bookkeeping 时间 | 不能把该时间直接视为成功认证活动；不顺带改变独立 relay-state 清理策略 |
| `peers/tests.rs` 的 secure manager / ring 辅助 | 已读到 `create_mock_peer_manager_secure` 及连接辅助 | 作为候选 fixture；完整组合测试定位和生命周期仍待核实 |

#2 指出了既有 `run_peer_session_gc_routine` 的周期扫描。本提案维持既有调度；实施者须重新核对调用点、周期与任务所有权，而不是把 issue 描述当作新的运行证据。

## 4. 活动定义：建议合同，尚待团队确认

| 事件 | 是否建议刷新 idle | 限定 |
| --- | --- | --- |
| 有效 session 新建 | 是，给予初始宽限 | 对象已成功建立 |
| 真正完成 payload 加密 | 是 | 不等于网络发送或对端交付已成功 |
| 解密成功且认证/replay 检查通过 | 是 | 不能在验证前续期 |
| 成功 responder upsert / initiator Join、Sync、Create | 是 | 必须完成算法、公钥、generation 等相应检查 |
| get、has_session、generation/status 查询 | 否 | 只读检查不应延长生命周期 |
| 只取得引用、空 pending flush | 否 | 后续真正使用单独记账 |
| 失败认证、解密失败、replay 拒绝、失败握手校验 | 否 | 防止失败流量成为续期来源 |

实施前审计底层 `Ok` 的语义，包括可能的 no-op/已处理分支。不能把任意 `Ok(())` 自动描述为实际加密或成功认证。

候选保留条件：

```text
retain = valid && (externally_held || age < idle)
```

60 秒仅作为沿用上游和 #2 的初始设计讨论值，不宣称最优。`age == idle` 且无外部持有者时按该合同可回收。失效对象不因近期活动或外部持有而继续留在 store；外部 Arc 自身仍由其持有者释放。检查引用数时不得为检查本身 clone 并抬高计数。idle 门槛和周期扫描是两件事，不能承诺恰好在第 60 秒释放。

## 5. 活动时间与并发身份

建议首先评估把私有单调活动时间归属到 `PeerSession` 对象，而非在操作完成后只凭 `SessionKey` 更新 map 条目：同一个 Arc 的 direct/relay 活动自然共享，旧 S1 的完成不能刷新同 key 的新 S2。

具体同步类型仍待批准。要求：时间更新不倒退；不跨 await；不在时间临界区重入 store；核对锁定依赖和 32 位目标。不假定 AtomicCell 一定无锁，不新加未经验证的原生 64 位 atomic 要求。若选短临界区锁，必须写明新增锁顺序和真实开销；不替换现有 session/crypto 锁。

### 需要验证的替换交错

固定源码中的失效读取后按 key 删除，可推导如下交错：

```text
T1 读取同 key 的旧失效对象 S1
T2 发布同 key 的新有效对象 S2
T1 按 key 清理，可能删除 S2
```

这是源码推断，不是已复现故障。实现前用真实 store API 和确定性同步验证。优先评估查询不删除、由同步 GC 清理；若保留 eager purge，应在同一 entry/shard 临界区内检查当前对象身份及失效状态。只比 key 或 generation 不足以证明是原对象。显式管理性 remove 的语义另行核对，不一律改写。

测试须断言 S2 仍存在、可用且不被 S1 活动续期。不能仅测试简化 map 模型。持有 DashMap guard 时不重入同一 map、不跨 await。GC 和握手路径的更多竞争只有在真实调用审计证明相关时才进入本批，不扩展成整体握手并发重构。

## 6. 两项独立红绿证据

### A. next-hop 目标保护

- [ ] 在未改业务逻辑的固定基线上加入可独立编译的回归：A→C 的真实有效目标密文经过绑定 B、且 B session 已失效的过滤器，记录基线结果。
- [ ] 修改后该密文逐字节保持不变，C 可正常解密。
- [ ] A→B 的正常直接加解密仍成功。
- [ ] 错误来源、握手、Ping/Pong 及既有豁免语义保持。

### B. 活动感知 GC

- [ ] 成功使用 relay-only session，释放所有外部强引用，调用真实 GC，记录未修基线是否丢失。
- [ ] 修改后近期有效 session 保留；没有外部持有且超期的 idle session 回收。
- [ ] 用私有显式时间或受控时钟分别验证边界前、等于边界和边界后，不靠长 sleep 模糊断言。
- [ ] 有效外部持有者保留；失效对象即使近期或外部持有也从 store 移除。
- [ ] 重复只读查询、空 flush、失败解密及 replay 拒绝不续期。
- [ ] 成功/失败 Join、Sync、Create 的活动合同可观察；轮换、旧引用释放和同 key 替换保持正确。

新增 helper 在基线无法编译不是基线红灯。两项行为分别记录证据，不能证明其中一项后宣布整批通过。若当前 HEAD 已存在等效行为，记录实现依据和覆盖，不制造失败或重复补丁。

## 7. 组合回归与验证矩阵

- [ ] 定位并保留 `derived_stealth_peer_sessions_are_connection_local` 原测试与原断言。
- [ ] 用经过确认的现有 fixture 建立显式 Secure Mode A—B—C，禁止 A—C 直连，跨多轮真实周期 GC 持续收发。
- [ ] 区分正常连接建立与周期性非预期重握手；记录双方 session/generation 与重握手计数。
- [ ] 验证停止后任务和引用收敛，以及 direct/relay 共享和轮换期间 key/generation 不串用。
- [ ] 正式运行前定义可执行的停止期限、资源/引用边界、GC 周期覆盖和 feature/平台矩阵，避免结果出来后改变通过标准。
- [ ] 记录精确 base/patch SHA、Cargo.lock、工具链、feature、平台、命令或 workflow run、原始失败/修复输出与未测试项。

私有时间边界单测和连续手动调用 GC 不能代替真实周期集成证据。任何测试都不得放宽旧断言以换取通过。

## 8. 工作流政策、环境限制与合并门槛

#1 记录：维护者最新版会话 AGENTS.md 要求 GitHub workflows、批次交付及指定全局状态/log helper。这是维护者项目政策，不是本研究推导。远端固定 AGENTS.md 尚含旧 builder 规则；本提案不重写它，也不自行切回旧流程。执行前须读到实际最新版政策和 helper 定义，未读到的命令保持未执行。

本次隔离 checkout 尝试失败，命令及第一错误为：

```text
git clone --depth 1 --branch codex/current --single-branch \
  https://github.com/lovitus/EasyTier.git <isolated-checkout>
exit 128: Could not resolve host: github.com
```

GitHub connector 的读取和写入与该本地 shell 不同：前者已可用，但没有据此宣称本地 git checkout 或项目 pre-commit 成功。环境检查也未找到 `rustup` / `actionlint`。完整 `scripts/pre-commit-check.sh`、构建和测试均未运行；本 Draft 的文档提交不是正式门禁豁免或已验证候选。正式纳入前须由符合项目要求的环境补齐检查。本提案不调度 build/test/release。

实施前剩余确认：

- [ ] 当前源码和维护者未提交内容与固定基线的差异。
- [ ] 所有 store/get/remove/crypto/handshake 调用点清单及活动分类。
- [ ] 活动字段与同步开销、锁顺序、32 位兼容。
- [ ] 身份保护的最小修复范围和确定性生产 API 回归。
- [ ] 组合 fixture、feature/平台矩阵、量化 stop/资源边界。
- [ ] 运行环境、政策/helper 定义和项目要求的文档检查。
- [ ] 团队设计结论与维护者明确实施批准。

提案评审、实现批准、测试通过、PR 合并、发布是独立状态。不得因提案 PR 存在而自动关闭 #1 / #2。将来实现应为相关代码、测试及文档的完整批次，回退仅撤销该批次，不撤销其他 fork 功能。

## 9. 固定源码参考

- [上游 secure relay 补丁](https://github.com/EasyTier/EasyTier/commit/425a24273b192399d3de3509fd868382f8ec89cc)
- [fork peer_conn.rs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/peer_conn.rs)
- [fork peer_session.rs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/peer_session.rs)
- [fork relay_peer_map.rs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/relay_peer_map.rs)
- [fork secure_datagram.rs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/secure_datagram.rs)
- [fork peers/tests.rs](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/easytier/src/peers/tests.rs)
- [fork AGENTS.md](https://github.com/lovitus/EasyTier/blob/c6772dbfef2395ff96b39bd4801945d92212dffb/AGENTS.md)
