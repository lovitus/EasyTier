# 上游 7 个提交选择性整合计划

## 状态与基线

- 总体状态：**待调查 / 待决定**。
- 本文只记录选择性整合方案，不表示已经批准实现，也不授权合并上游分支。
- 评估日期：2026-07-22。
- 当前 fork 产品基线：`82ff7f18`。
- 当前文档工作树 HEAD：`117a0ac1`。
- 上游评估终点：`346f32d3`。
- 双方共同基线：`15e5d89f`。
- 上游相对共同基线有 7 个提交；fork 已有大量独立演进，禁止直接 merge `upstream/main`，也禁止不审查地批量 cherry-pick。
- 当前工作树已有尚未形成候选 SHA 的功能修改。在这些修改完成、预检并固定为独立候选前，不得混入本文的上游移植。
- 文档变更不触发构建或 workflow。只有代码候选完成远程 `.160` 预检并形成完整候选清单后，才允许推送一次对应候选。

### 状态定义

- **待调查**：已确认当前 fork 仍存在对应缺口或值得移植，但实现细节、兼容边界或验证证据尚未完整闭环。
- **待决定**：是否引入本身需要维护者作出范围或价值判断；在决定前不进入实现候选。

## 2026-07-22 评审修正

- 不再把 U1–U4 合并为一个代码候选。减少 workflow 不能以混合 Web 鉴权、加密 session、packet 热路径和 protobuf/GUI 四个故障域为代价。
- `192.168.2.160` 作为主要风险隔离面：每一项在独立 worktree 中完成多轮最小 no-run、精确测试和重复测试；只有 `.160` 证据稳定后才产生 GitHub 候选。
- U1 是已复现的跨用户会话信息泄漏，必须脱离其他上游整合立即修复。
- U2 是显式 Secure Mode 的正确性修复，但涉及加密热路径和 session 生命周期，必须独立候选。
- U3 是有条件性能优化；字节等价和稳定收益缺一不可，没有可重复收益就不合入。
- U4 只实现 pinned peer key 和缺省字段等缺失能力，等待当前 GUI/protobuf WIP 固定后独立处理。
- U5 不增加任何打洞协议，也不影响现有 IPv4/IPv6 直连、UDP/TCP 打洞、QUIC、KCP 或 relay；它只扩展 public IPv6 provider 在 SLAAC/IA_NA、无 PD 场景下的公网地址下发和 NDP 可达性，保持产品决定状态。
- U6 建议保留默认关闭的 hotpath；U7 建议不引入产品候选。

## 结论总表

| 上游提交 | 状态 | 当前判断 | 整合方式 |
| --- | --- | --- | --- |
| `346f32d3` Web 会话按用户隔离 | **建议立即实现** | 已确认普通用户路径返回所有用户的 authorized token；属于安全问题 | 独立安全候选，不等待其他六项 |
| `425a2427` Secure relay session 修复 | **建议实现** | 显式 Secure Mode 的中继加密范围和 session 回收缺口已确认 | 独立高风险候选，手工语义移植 |
| `f24735a8` 减少 packet buffer 切片开销 | **有条件实施** | 当前仍使用 `BytesMut::split_off`，方向适用但收益尚未证明 | 独立性能候选；无稳定收益则终止 |
| `4e616129` Web managed config/status 兼容 | **建议部分实现** | status 已是上游超集；pinned key 和缺省字段仍有缺口 | 等现有 GUI/protobuf WIP 固定后独立候选 |
| `7756a15c` IPv6 IA_NA/SLAAC 与 NDP proxy | **待产品决定** | 扩展无 PD 场景的公网 IPv6 下发，不是打洞能力 | 独立实验和产品候选，禁止直接移植完整提交 |
| `13412895` 删除 hotpath profiling | **建议不移植** | 可选且默认关闭，删除没有明确产品收益 | 只审计正式构建未启用，不建立代码候选 |
| `741460e1` TX Criterion benchmark | **建议不移植** | 仅开发基准，会引入依赖和 lockfile 变更 | 优先使用现有 profiling 工具 |

## U1：`346f32d3` Web 会话按用户隔离

**状态：建议立即实现 / 独立安全候选**

### 已确认的问题

普通用户的会话查询路径当前调用全局 `list_sessions()`，没有按当前用户过滤。多用户部署中，这可能暴露其他用户的 token、连接地址和机器信息。上游提交整体可干净应用，但仍必须结合 fork 当前鉴权语义复核，不能只以“能应用补丁”作为完成标准。

### 具体实施方案

1. 在 `easytier-web/src/client_manager/storage.rs` 增加按 `user_id` 返回该用户 client token 的只读接口。
2. 在 `easytier-web/src/client_manager/mod.rs` 增加 `list_sessions_by_user_id`：
   - 只遍历该用户已登记的 token；
   - 只返回当前仍存在且已授权的会话；
   - 不改变内部管理员或可信内部接口的全局会话语义。
3. 在 `easytier-web/src/restful/mod.rs` 的普通用户 handler 中：
   - 必须先取得经过鉴权的当前用户；
   - 用户上下文不存在或无效时返回 `401`；
   - 使用 `list_sessions_by_user_id`，禁止回退到全局列表。
4. 保留提交来源追踪，在实现提交信息中记录上游 SHA `346f32d3`。

### 测试用例方案

- `list_sessions_isolated_between_users`：用户 A、B 各有一个已授权 client，A 只能看到 A，B 只能看到 B。
- `list_sessions_excludes_pending_or_unauthorized_client`：用户已登记但未授权的 client 不得出现在普通用户列表中。
- `list_sessions_requires_authenticated_user`：缺失或无效登录上下文返回 `401`，不得返回空列表掩盖鉴权错误。
- `list_sessions_does_not_leak_after_revoke`：撤销或删除用户 A 的 client 后，A 与 B 的查询均不得出现残留会话。
- `internal_list_sessions_remains_global`：受信内部接口仍按既有约定返回全局会话，避免误伤管理功能。
- `list_sessions_concurrent_registration`：会话注册、撤销与列表读取并发时不 panic、不跨用户泄露。
- 前端或 API 集成测试使用两个真实用户完成登录、登记、授权、查询和撤销闭环，并检查响应中 token、URL、machine ID、user ID 均未越权。

### 完成条件

- 普通用户路径不存在任何调用全局 `list_sessions()` 的分支。
- 上述隔离和并发测试通过。
- 内部管理接口语义没有变化。

## U2：`425a2427` Secure relay session 修复

**状态：建议实现 / 独立高风险候选**

### 已确认的问题

fork 已把派生 Stealth session 调整为连接级状态，但显式 Secure Mode 仍存在两个独立问题：

1. 发送过滤器只检查 `from_peer_id`，没有确认目标就是当前直连 peer。三节点中继时，转发包可能被错误地使用下一跳 session 加密。
2. `PeerSessionStore::evict_unused_sessions` 只依赖 `Arc::strong_count`，没有空闲宽限期；活跃中继 session 可能在暂时没有外部引用时被提前回收。

上游 `peer_session` 部分可直接参考，`peer_conn` 与 fork 当前测试和 Stealth 重构存在冲突，必须手工移植语义。

### 具体实施方案

1. 在 `easytier/src/peers/peer_conn.rs` 收紧发送过滤条件：
   - 只有 packet 的来源是本地 peer；
   - 且 packet 的目标是当前直连 peer；
   - 才允许使用该连接的 session 加密。
   - 中继包、握手包和不属于该直连会话的包保持原样，不得错误套用下一跳密钥。
2. 首轮修复不主动替换现有锁模型。只有 profiling 证明锁本身是热点，且并发与生命周期测试已经闭环时，才另行评估 `ArcSwapOption`；不得把锁重构与正确性修复绑定。
3. 在 `easytier/src/peers/peer_session.rs` 引入带最后使用时间的 entry：
   - `get`、插入和更新时刷新时间；
   - 有外部引用的有效 session 保留；
   - 暂无外部引用但仍处于空闲宽限期的有效 session 保留；
   - 无效 session 立即清理；
   - 超出宽限期且无外部引用的 session 才清理。
4. 默认空闲宽限期先按上游的 60 秒语义评估，不把它暴露为新用户配置，除非测试证明 fork 的网络恢复时间需要不同边界。
5. 派生 Stealth session 继续保持连接级，不写入全局 `PeerSessionStore`；显式 Secure Mode 与 Stealth 的 session 所有权必须分别测试。

### 测试用例方案

- `secure_filter_encrypts_direct_local_packet`：本地发往直连 peer 的数据仍正常加密和解密。
- `secure_filter_does_not_encrypt_relay_packet`：A → B → C 时，B 转发给 C 的包不能使用 B-C session 重复加密属于 A-C 或 A 端语义的数据。
- `secure_filter_rejects_wrong_source_or_destination`：来源或目标任一不匹配直连关系时不加密。
- `secure_handshake_and_ping_bypass_data_filter`：握手、保活和控制包语义不变。
- `recent_session_survives_gc_without_external_reference`：刚使用的 session 即使暂时 `strong_count == 1`，也能跨一次 GC 存活。
- `idle_session_expires_after_grace_period`：超过宽限期、无外部引用的 session 被清理。
- `invalid_session_expires_immediately`：失效 session 不因最近使用而滞留。
- `explicit_secure_mode_three_peer_relay_survives_repeated_gc`：三节点中继流量跨多轮 GC 持续可用，不出现周期性断流或重协商风暴。
- `derived_stealth_session_remains_connection_local`：两条连接不得互相读取或复用派生 Stealth session。
- `session_rekey_does_not_mix_generations`：重协商后旧、新 session 不交叉使用。
- `session_cleanup_on_connection_drop`：连接退出后引用与定时任务可收敛，无泄漏。

### 完成条件

- 普通非 Secure QUIC、QUIC Brutal、Stealth 及其他 overlay 行为不变。
- 显式 Secure Mode 的直连和三节点中继均通过功能、GC、重连和清理测试。
- 包热路径没有新增阻塞锁或全局跨连接状态。

## U3：`f24735a8` 减少 packet buffer 切片开销

**状态：有条件实施 / 独立性能候选**

### 已确认的问题

当前 `ZCPacket` 与 TUN 转换路径仍使用 `BytesMut::split_off` 取得 payload。该操作会拆分缓冲区管理状态；上游改为 `Buf::advance` 的方向适用于当前 fork。`packet_def.rs` 变更可直接参考，`virtual_nic.rs` 因代码位置变化需要手工适配。

### 具体实施方案

1. 在 `easytier/src/tunnel/packet_def.rs` 引入 `bytes::Buf`，将仅用于丢弃前缀的 `split_off(offset)` 改为 `advance(offset)`。
2. 覆盖以下入口，不扩大到无关 packet 重构：
   - payload bytes 提取；
   - tunnel payload 提取；
   - packet 类型转换后的前缀移动；
   - foreign header 丢弃。
3. 在当前 `virtual_nic` 的 `TunZCPacketToBytes` 实际位置做同样适配，保持 packet-info、IP header 和 payload 的现有边界。
4. 删除因此变得多余的复制或中间 buffer，但不改变公开 packet 格式、加密尾部布局或 MTU 语义。
5. 不把 `741460e1` 的 Criterion 依赖自动带入本候选；性能证据优先使用现有 profiling 构建和定向计数。

### 测试用例方案

- `zc_packet_payload_bytes_exact`：payload 长度为 0、1、MTU、4096 时，结果逐字节一致。
- `zc_packet_tunnel_payload_exact`：包含 tunnel header、加密尾部和空 payload 的边界均正确。
- `zc_packet_convert_type_preserves_payload`：TCP、UDP、WG、NIC、Dummy 等现有类型转换后 payload 和 header offset 不变。
- `zc_packet_drop_foreign_header_exact`：有无 foreign header 均不会多丢或少丢一个字节。
- `tun_packet_info_prefix_exact`：存在 4 字节 packet-info 的平台路径保持正确。
- 运行现有 tunnel、virtual NIC、加解密和分片/聚合测试，比较修改前后的固定输入输出 fixture。
- 使用同一 profiling artifact 对代表性 TUN TX/RX workload 比较分配次数、吞吐和 CPU；要求无性能回退，并能观察到切片/复制开销下降。
- 在旧内核兼容主机上验证普通 UDP/QUIC/WG 与 TUN 通信，避免只用单元测试推断驱动边界。

### 完成条件

- 所有 packet fixture 字节完全一致。
- 无新增拷贝、panic 或越界条件。
- 性能至少不回退；若无法稳定证明收益，则只保留有明确代码简化且无风险的部分，或终止该项。

## U4：`4e616129` Web managed config/status 兼容

**状态：建议部分实现 / 等待现有 WIP 固定**

### 已确认的问题

fork 当前 status 解析已经是上游方案的超集，覆盖 snake/camel case、uint64 字符串/BigInt 和多种 feature flag，禁止用上游版本覆盖。仍需处理的缺口是：

- GUI/TOML 往返只保留 peer URL，可能丢失 `peer_public_key`。
- ACL 或其他 repeated 字段缺省时，前端部分路径直接调用 `.map()`，可能再次出现 `Cannot read properties of undefined`。
- 上游把 `NetworkConfig.peers` 放在 protobuf field 68，但 fork 已发布 field 68 为 `stealth_mode`，69–80 也已占用。绝不能复用上游 field 68。

### 具体实施方案

1. 保留现有 status display 和兼容解析，不移植上游的替代实现。
2. 为结构化 peer 增加 fork 自有 protobuf 字段：
   - 实施前再次扫描当前 schema 和生成物；
   - 若 81 仍为空闲，则使用 field 81；否则使用下一个经确认未占用的字段；
   - field 68 永久保留 `stealth_mode`；
   - 保留 legacy `peer_urls`，不得强制旧客户端升级。
3. launcher/config 转换：
   - 结构化 `peers` 非空时优先使用，并保留 URL 与 pinned public key；
   - 结构化字段为空或缺失时回退到 legacy `peer_urls`；
   - 输出 managed config 时同时保留结构化信息与兼容 URL 表达。
4. 前端类型和编辑状态保存 peer metadata；只编辑 URL 时不得静默删除原有 public key。
5. 对 protobuf 缺省 repeated 字段统一在读取边界正规化为空数组，禁止在业务组件中直接假定 `.map()` 对象一定存在。
6. 缺省 ACL 不得被保存为“显式启用的空 ACL”；只有用户实际编辑后才生成对应配置。
7. 明确记录 wire 边界：fork 的结构化 peer field 与上游 field 68 不具备 protobuf wire 级互通，兼容路径是 legacy `peer_urls`。

### 测试用例方案

- `managed_config_preserves_peer_public_key_roundtrip`：TOML → protobuf → GUI 编辑/保存 → protobuf → TOML 后 URL 和 pinned key 完全一致。
- `managed_config_accepts_legacy_peer_urls`：旧配置只有 `peer_urls` 时行为不变。
- `managed_config_prefers_nonempty_structured_peers`：结构化 peer 存在时不被 legacy URL 覆盖。
- `managed_config_empty_structured_peers_falls_back`：空结构化数组仍使用 legacy URL。
- `managed_config_modes_preserve_peers`：PublicServer、Manual、Standalone 等现有模式均不丢 peer metadata。
- `managed_config_legacy_3_0_x_fixture`：读取已发布 3.0.x 配置 fixture，不改变其语义。
- `protobuf_field_numbers_are_stable`：断言 field 68 仍是 `stealth_mode`，新增 peers 使用经批准的新 field。
- `protobuf_upstream_field_68_payload_is_not_stealth`：构造上游 length-delimited field 68 fixture，fork 绝不能错误开启 Stealth；允许明确拒绝不兼容 payload，只有当前 protobuf runtime 可证明安全时才要求忽略未知 wire type。
- `protobuf_fork_field_68_bool_is_not_peer`：构造 fork field 68 bool fixture，结构化 peer 列表保持为空。
- `managed_config_omitted_repeated_fields_do_not_crash`：ACL chains、rules、groups 及其他 repeated 字段缺失时不发生 `.map()` 异常。
- `managed_config_absent_acl_remains_absent`：未编辑 ACL 的加载与保存不会凭空改变配置含义。
- 执行现有 frontend-lib Vitest、frontend-lib build、frontend build、VPN plugin build、GUI build，严格使用远程前端预检顺序。

### 完成条件

- pinned peer key 可无损往返。
- 缺省数组不会导致 GUI 运行时异常。
- 已发布 protobuf field 编号和现有 status 行为完全不变。
- wire 不兼容边界已在配置文档中明确说明。

## U5：`7756a15c` IPv6 IA_NA/SLAAC 与 NDP proxy

**状态：待产品决定 / 不影响现有打洞能力**

### 已确认的问题

当前 public IPv6 provider 主要处理已路由到本机的前缀，没有完整覆盖家庭或普通接入网络常见的 IA_NA / SLAAC 地址派生，以及同链路前缀需要 NDP proxy 的情况。上游核心 provider 与 netlink 辅助代码可参考移植，但 fork 的实例生命周期、移动端边界、policy/Leaf 集成已经变化，不能直接应用完整提交。

### 与打洞和现有连接能力的边界

- 本功能不是 UDP/TCP/QUIC/KCP 打洞协议，也不参与 EasyTier 现有候选交换和 NAT traversal。
- 不实现本功能不会减少已有 IPv4 打洞、主机自身 global IPv6 直连、QUIC/KCP、native 或 relay 的使用率。
- 当 Linux 网关只有 SLAAC/IA_NA 的 on-link `/64`、没有 DHCPv6-PD 或静态 routed prefix 时，现有 provider 无法安全地把同前缀 `/128` 下发给远端 mesh 成员并让 ISP 路由器通过 NDP 找到它们。
- 实现后得到的是“经 public IPv6 provider 网关转发的公网 IPv6 可达路径”，不是远端成员之间新增的 P2P underlay；它可能让部分业务不再依赖 relay，但不能按打洞成功率提升进行宣传或验收。
- 如果没有用户需要从无 PD 家庭/普通接入网络向 mesh 成员提供公网 IPv6，本项没有首发必要性。
- 上游会启用 WAN 接口 `proxy_ndp`，但没有完整恢复 sysctl 原值；fork 若实现，必须把 sysctl 原值、entry 所有权和退出清理一起设计，不能直接 cherry-pick。

### 具体实施方案

1. 先逐函数复核并移植以下核心能力：
   - 默认路由接口的全局 IPv6 地址发现；
   - IA_NA / SLAAC 地址推导可用前缀；
   - netlink NDP proxy 的查询、添加和删除；
   - public IPv6 provider 对路由前缀与 on-link 前缀的区分。
2. 前缀选择规则必须确定且可测试：
   - 只接受 global unicast；
   - 排除 link-local、ULA、multicast、unspecified；
   - 自动推导不得生成比 `/64` 更窄且不适合下游分配的前缀；
   - 多地址时按明确的 route/interface/index 规则稳定选择，禁止依赖 netlink 返回顺序。
3. 只有 on-link 前缀才启用 NDP proxy；已明确路由到本机的 delegated prefix 保持现有路径，不增加代理。
4. NDP proxy 资源必须有所有权记录：
   - 启动前查询既有条目；
   - 只删除本实例实际创建的条目；
   - 预先存在或其他实例创建的条目不得删除；
   - WAN 切换、TUN 重建、配置关闭、运行失败和实例退出均执行幂等清理。
5. 手工适配 fork 生命周期：
   - `GlobalCtx` 只通过既有 TUN ready/error 状态记录真实设备名；
   - `NicCtx`、`Instance` 和 launcher 使用当前 cancellation/closing 机制；
   - provider task 每实例最多启动一次；
   - shutdown 后不得被迟到事件重新启动；
   - 初始化中途失败必须回滚已经创建的 sysctl、route 和 proxy entry。
6. Linux-only 代码继续受严格 `cfg` 保护，不向 Android、macOS 或其他目标泄漏 netlink 类型和行为。
7. 不新增静态全局所有权表，避免多实例互相清理资源。

### 测试用例方案

- `public_ipv6_routed_prefix_path_unchanged`：已有 delegated route 时继续使用旧路径，不创建 NDP proxy。
- `public_ipv6_derives_prefix_from_iana_address`：只有 IA_NA global address 时可稳定推导前缀。
- `public_ipv6_derives_prefix_from_slaac_address`：SLAAC 地址路径可用且结果确定。
- `public_ipv6_rejects_non_global_addresses`：link-local、ULA、multicast、unspecified 均拒绝。
- `public_ipv6_address_selection_is_deterministic`：多 route、多 interface、多 global address 时选择不依赖枚举顺序。
- `public_ipv6_on_link_prefix_requires_ndp_proxy`：on-link 前缀正确创建 proxy entry。
- `public_ipv6_preexisting_proxy_is_preserved`：预先存在的 proxy entry 在实例退出后仍存在。
- `public_ipv6_owned_proxy_is_removed`：本实例创建的 entry 在关闭、失败和 WAN 切换时删除。
- `public_ipv6_cleanup_is_idempotent`：重复关闭和部分初始化失败不报错、不误删。
- `public_ipv6_provider_starts_once`：重复 TUN ready 或网络事件不产生重复任务。
- `public_ipv6_provider_does_not_restart_after_shutdown`：closing 后的迟到事件不能复活任务。
- `public_ipv6_tun_rename_and_recreate`：实际 TUN 名变化时资源从旧接口干净迁移。
- Linux network namespace 集成测试使用独立 CIDR，验证邻居发现、NDP、路由和端到端 IPv6 通信。
- 在 `192.168.1.37`、`192.168.1.38` 的 CentOS 7 / Linux 3.10 环境验证 netlink 兼容、创建、查询和清理。
- 在 10 Gbps 公网双栈验证主机对上验证真实 IPv4/IPv6、重连和性能，不从内部编译机或内网主机推断公网结论。
- 非 Linux 目标执行 compile-only `cfg` 检查，确认无平台泄漏；Android 不增加后台轮询或唤醒。

### 完成条件

- routed prefix 旧路径零回退。
- IA_NA/SLAAC 的自动路径在命名空间、旧内核和真实双栈网络均通过。
- 所有 NDP/sysctl/route 资源有明确所有权并可幂等清理。
- 任何失败都不会遗留网络状态或启动重复 provider task。

## U6：`13412895` 删除 hotpath profiling

**状态：待决定**

### 当前倾向

fork 中 hotpath profiling 是可选且默认关闭，并有 no-op shim。它对日常构建没有已知运行时影响，同时对后续 packet、Leaf 和 overlay 性能诊断仍有价值，因此当前倾向是**保留，不移植删除提交**。

### 决定前调查方案

1. 检查所有生产 workflow、默认 features、发布脚本和平台构建是否显式或间接启用 `hotpath`。
2. 使用 `cargo tree -e features` 和 build metadata 确认默认发布产物没有引入 profiling 依赖或初始化路径。
3. 对默认关闭构建检查符号、依赖和运行时任务，确认 no-op shim 不产生后台线程、定时器或分配。
4. 确认 hotpath feature 在当前 Rust/toolchain 上仍能独立编译，避免保留一个已经失效的诊断开关。

### 测试用例方案

- 默认 feature 的 `--locked` no-run 构建不包含 hotpath 依赖。
- 显式启用 hotpath 的诊断构建能够编译，并可启动/停止采样。
- 默认构建的空闲线程、任务和分配基线与关闭诊断代码一致。
- workflow 配置扫描测试或脚本断言正式 Release 不启用 hotpath，除非以后显式改变政策。

### 待决定事项

- **方案 A（建议）**：保留默认关闭的诊断 feature，只修复其独立编译问题。
- **方案 B**：确认维护成本高于诊断价值后完整删除 feature、依赖、shim、文档和 workflow 引用。
- 未作决定前，不把删除提交混入任何产品候选。

## U7：`741460e1` TX Criterion benchmark

**状态：待决定**

### 当前倾向

该提交只增加开发基准，并会带来 Criterion 依赖与 `Cargo.lock` 变化，不修复产品行为。当前倾向是**不进入产品候选**，避免为了验证 U3 而扩大依赖和发布差异。

### 具体可选方案

1. **方案 A（建议）**：不移植。U3 使用现有 profiling artifact、固定 packet fixture 和分配/吞吐计数完成验证。
2. **方案 B**：如果现有工具无法稳定度量 U3，单独建立开发基准改动：
   - 只覆盖 packet payload extraction / TUN TX 的目标路径；
   - 不把基准依赖加入生产 feature；
   - 与产品候选分开提交和评审；
   - 文档或基准变更不得触发 Release。

### 测试用例方案

- `cargo bench --no-run --locked` 在远程构建机通过。
- 固定 packet 大小分布：64、512、1500、4096 字节，以及空 payload。
- 分别度量提取 payload、类型转换和 TUN TX 转换，禁止把网络 I/O 抖动混入纯 buffer 基准。
- 每项同时记录吞吐、时间和分配次数；结果方差过大时不得据此宣称性能提升。
- 基准依赖不得出现在正式默认 feature 或发布产物 dependency tree 中。

### 待决定事项

- 是否确实需要长期维护 Criterion 基准。
- 若只是一次性验证 U3，优先使用临时/现有诊断工具，不修改产品 lockfile。

## 分批实施方案

### 阶段 0：完成调查与范围冻结

1. 先完成并固定当前工作树已有功能候选，禁止把两批未验证修改混在同一 SHA。
2. 为 U1–U5 逐项记录实际采用的上游文件、函数和可观察语义。
3. 对 U4 固定 protobuf field 编号与 wire 边界。
4. 对 U6、U7 作出明确“保留/删除”和“引入/不引入”决定。
5. 调查阶段只允许只读检查、测试设计和文档更新，不触发 workflow。

### 候选 A：U1 Web 会话隔离安全修复

- 从当前产品基线建立独立干净 worktree，不使用已有 GUI/protobuf WIP 工作树。
- `.160` 先做最小 `--locked` no-run，再运行双用户隔离、未授权、撤销、缺失用户和内部全局接口测试；并发测试从同一已编译测试二进制重复执行。
- `.160` 全部通过后才形成一次 GitHub 候选。U1 不等待 U2–U5，不因减少 workflow 推迟安全修复。
- 真实验证只覆盖 Web 双用户登录、登记、授权、查询和撤销闭环，不混入 overlay 或 packet 性能结论。

### 候选 B：U2 Secure relay session

- 独立 worktree、独立 `.160` 编译和测试，不与 Web、GUI、protobuf 或 packet buffer 修改共用候选。
- `.160` 覆盖直连、中继、错误来源/目标、GC 宽限期、rekey、连接退出和 Stealth 连接级隔离；三节点与共享端口测试保持串行。
- 同一 debug 测试二进制重复运行 GC/relay 场景，先排除偶发断流，再启动一次 GitHub 候选。
- Linux 实机验证显式 Secure Mode 三节点中继、重连和多轮 GC，同时确认普通 QUIC、QUIC Brutal、Stealth 与非 Secure 路径不回退。

### 候选 C：U3 packet buffer 优化

- 当前 `virtual_nic.rs` WIP 固定前不实现；之后从干净基线建立独立候选。
- `.160` 先验证全部 packet fixture 字节等价，再使用现有工具采集分配/CPU；可多轮重复 `.160` 测量，不用 GitHub workflow 充当基准反馈。
- 只有收益跨多轮稳定且无字节、吞吐和平台回退时才推送 profiling 候选；收益不稳定则终止，不产生产品提交。

### 候选 D：U4 managed config 兼容

- 等当前 GUI/protobuf/launcher WIP 独立预检和固定后再开始，禁止整文件覆盖或并行修改同一生成物。
- `.160` 按 protobuf/Rust focused tests、frontend-lib Vitest、frontend-lib build、frontend build、VPN plugin build、GUI build 的顺序执行。
- field 编号、legacy fallback、pinned key、缺省数组和 ACL absent 语义全部在 `.160` 闭环后，才形成一个 GUI/config 候选。

### 候选 E：IPv6 provider 与 NDP proxy

只有维护者明确批准无 PD SLAAC/IA_NA public IPv6 provider 产品范围后才建立。它涉及 netlink、sysctl、实例生命周期、真实 WAN 行为和旧内核，必须一次包含完整 provider、所有权清理、namespace 测试、平台 `cfg` 和配置文档。

远程与真实环境验证至少包括：

- `.160` 最小 feature `--locked` no-run 和精确单元/namespace 测试；
- `Cargo.lock`、Linux `cfg`、非 Linux compile boundary、workflow pin 检查；
- CentOS 7 / Linux 3.10 netlink 功能与清理；
- 公网双栈主机对的 IA_NA/SLAAC、NDP、WAN 切换、重连和端到端 IPv6；
- 多实例、初始化失败、TUN 重建与退出后的零残留检查。

### 候选 F：可选开发工具

默认不建立。只有 U6 或 U7 的决定改变时才建立；不得为了文档、基准结果或 evidence 重跑产品 workflow。基准工具原则上不与产品功能候选混合。

### `.160` 风险隔离规则

1. U1、U2、U3、U4、U5 各自使用独立 worktree、独立同步目录、独立 focused-test 清单；失败不得污染其他候选。
2. `.160` 可以在代码范围稳定后多轮使用：第一次解决编译、cfg、生成物和类型问题；第二次运行精确测试；第三次只在并发、生命周期或性能需要重复证据时使用。
3. 同一轮已完成的 no-run 产物应直接重复执行精确测试二进制，不为每个测试重新编译。
4. `.160` 失败只回到对应 worktree 修复，不触发 GitHub。只有该项完整 `.160` 证据通过后才允许一次候选 push。
5. 不再为了节省 workflow 强制合并不同故障域。只有代码依赖、验证矩阵和回退边界一致的修改才允许共享 GitHub 候选。
6. post-build run ID、性能数字和 PASS/FAIL 只写入绑定不可变 SHA 的 evidence，不移动候选 ref、不重建 artifact。

## 通用回归与发布约束

1. 不 merge `upstream/main`，只移植经确认仍适用的语义。
2. 自动应用检查仅说明文本冲突情况，不代表兼容性结论：
   - `346f32d3` 整体可干净应用；
   - `425a2427` 的 `peer_session` 部分可参考直接应用，`peer_conn` 必须手工适配；
   - `f24735a8` 的 `packet_def` 部分可直接参考，`virtual_nic` 必须手工适配；
   - `7756a15c` 的 provider/ifcfg 核心部分可参考直接应用，生命周期集成必须手工适配；
   - `4e616129` 与 fork 的 proto/launcher 已冲突，只能选择性移植。
3. 每个手工移植点必须在 TODO、测试名、代码注释或提交说明中记录来源 SHA、采用的函数语义及 fork 的差异理由。
4. 不重新编号或复用任何已发布 protobuf field。
5. 不以单元测试代替安全隔离、中继、旧内核、双栈网络和资源清理证据。
6. 每个候选推送前必须具备：完整 candidate manifest、成功的 `.160` `--locked` no-run、精确 focused tests、完整 diff/lockfile/生成物/`cfg`/workflow pin 审查。
7. 文档和 post-build evidence 只能绑定已验证的不可变 SHA，不得移动候选分支或重启 workflow。
8. 验证失败时只对对应候选使用可审计的 `git revert`；不得用 reset 隐藏失败，也不得因此回退不相关且已验证的 fork 功能。
9. 只有代码、生成物、依赖或构建配置实际变化才允许产生新候选 workflow；测试记录、run ID、哈希、报告和本 TODO 的更新不构成重建理由。

## 最终决定门槛

在以下事项全部明确前，本文保持“待调查 / 待决定”，不得标记为可合并：

- U1 的多用户越权测试已经复现并由修复关闭；
- U2 的显式 Secure Mode 中继、GC 和连接级 Stealth 边界已验证；
- U3 有字节等价证据，并证明无性能回退；
- U4 的新 protobuf field、legacy fallback 和上游 field 68 wire 边界已批准；
- U5 的 NDP 资源所有权、旧内核和真实双栈证据闭环；
- U6、U7 已由维护者作出明确决定；
- 当前工作树已有候选与本文候选完全分离，不存在来源不明的混合改动。
# Protobuf 字段 68 分叉冲突核实（2026-07-22）

U4 在实现前必须先处理 `NetworkConfig` 的 protobuf 字段号分叉，禁止直接 cherry-pick 上游 proto 变更。

- 当前分叉的 `easytier/src/proto/api_manage.proto` 已将字段 `68` 定义为 `optional bool stealth_mode`，字段 `69..80` 也已连续用于 stealth、underlay guard 和 policy proxy 配置。
- 上游提交 `4e616129446cd26c035ac608975a43271a60e4e5` 将同一消息的字段 `68` 定义为 `repeated NetworkPeerConfig peers`。
- 两者不仅名称不同，wire type 也不同：当前 `bool` 使用 varint，上游 `repeated message` 使用 length-delimited。直接覆盖会造成分叉客户端和上游客户端对同一 tag 作不同解释；安全结果可能是解码失败，不能假设旧客户端一定会将它当作未知字段跳过，更不能接受它被误解释为 `stealth_mode=true`。
- U4 只移植“managed config 保留 `peer_public_key`”的行为，不移植上游字段号。实施时必须在当前分叉未占用的新 field number 上增加结构化 peers 字段，并保留现有 `peer_urls` 兼容面。
- 分配新字段前必须扫描 `NetworkConfig` 的完整字段号及 `reserved` 声明，并将选择的字段号加入项目 protobuf 字段注册记录；禁止只检查上游文件或只检查当前单一分支。
- protobuf 兼容测试必须覆盖：旧分叉 payload、新分叉 payload、上游含 tag 68 peers 的 payload、未知字段 round-trip，以及错误 wire type 输入。验收条件是旧配置不丢失、`peer_public_key` 可 round-trip、上游 tag 68 不会启用 stealth；对于冲突 payload，明确的安全拒绝或安全忽略都可接受。
- GUI、REST、Tauri 和生成代码必须使用同一个重新分配后的字段号；生成文件、TypeScript 类型和 Rust prost 输出不一致时，`.160` 预检必须失败。
- 在该字段号和兼容策略确定前，U4 保持独立候选并标记 `NEEDS_REVIEW`，不得与其他上游提交一起 cherry-pick。
