# 本 Fork 与上游 EasyTier 的差异总览

这份文档是 README 首页的 fork 差异入口，内容已经按当前本地代码和 `upstream/main`
交叉核对过，目的是把下面四类信息放在同一处：

- 这个 fork 修复了哪些真实问题？
- 这个 fork 新增了哪些上游默认没有的能力？
- 与上游相比，有哪些行为差异或兼容边界？
- 哪些参数是这个 fork 新增的，哪些是上游原有但在这里行为不同的？

Stealth、Proxy、回退和 rollout 细节仍然以
[udp_stealth_compatibility.md](udp_stealth_compatibility.md) 为准。
与 Mihomo/Clash/sing-box TUN 共存时的 CPU 异常和复现步骤见
[mihomo_tun_interop_cn.md](mihomo_tun_interop_cn.md)。
2026-07-08 的远端功能和性能验证报告记录在
[performance_validation_2026_07_08.md](performance_validation_2026_07_08.md)。
2026-07-09 的 direct 传输优先级、WAN/LAN 识别和 breaker 复核记录在
[transport_priority_breaker_validation_2026_07_09.md](transport_priority_breaker_validation_2026_07_09.md)。
Stealth/Secure 后续问题见
[known_bugs/stealth_secure_known_bugs.md](known_bugs/stealth_secure_known_bugs.md)。
v2.6.9 发布说明见 [release_notes/v2.6.9.md](release_notes/v2.6.9.md)。

## 1. 这个 Fork 修复了什么

### Stealth 与底层传输兼容性

- 把 stealth 从“仅 UDP”扩展为结构化的多传输 capability 广告与协商，不再靠单一布尔值
  粗略表达。
- 修正了 direct-connect 和 hole-punch 在 strict stealth、plain fallback 和显式
  plain 请求之间的行为边界。
- 明确并落实 `stealth_window_secs` 是网络级参数，不再只靠零散说明。
- 保留了 manual UDP stealth fallback 的完整预算，避免第一次 stealth 尝试失败后把整次
  外层超时预算提前耗尽。
- 修正了 datagram stealth 的 phase 切换时序，确保 gate key 和连接级 outer key 在预期
  的握手边界上切换。

### 回环避退与连接稳定性

- 把 self-loop 识别收敛为高置信的 underlay 后 `peer id conflict` 信号，普通超时、
  拒绝连接和一般网络错误不再被误记为回环。
- 把新的 direct / hole-punch 回环避退收敛为目标级 TTL blacklist，而不是把单个目标的
  问题直接升级为大范围 scheme 级 suppression。
- 旧的 scheme/scope suppression 状态仍保留为兼容安全栏，但不再作为新 direct /
  hole-punch 路径的主 gate。
- 保留 native TCP proxy 的 NAT entry 查找 / 交接正确性，让真实本地项优先于 fake-local
  fallback，并减少连接激活瞬间首个 ACK/Data 落入查找空窗的风险。
- 这属于“回环/避退强化”，不是“已经彻底消灭所有回环流量”；当前仍可能观察到残余回环。

### QUIC/KCP Proxy 可靠性

- 修正了 QUIC/KCP Proxy prepare 阶段“只要 transport stream 建立就算成功”的问题，
  让 source 侧可以等待远端目标真正 ready 再决定是否回退。
- 为 Proxy 故障转移增加了显式就绪 ACK 和分类错误原因。
- 修复了 QUIC/KCP Proxy 共用的本地 TCP capture 路径，它之前可能从虚拟网段里选出
  不可用的伪源地址。
- 修复了 KCP 关闭路径的尾包排空和坏连接清理问题，避免尾部数据丢失和 live state 常驻。

### 能力广告与策略选择

- 修复了 Proxy input capability 广告，使其同时受编译 feature 和运行时
  `disable_*_input` 控制，而不是只看运行时开关。
- 明确双栈 peer 同时命中 IPv4/IPv6 exact-IP 传输规则时的优先级。

## 2. 这个 Fork 新增了什么

- 支持 `udp`、`tcp`、`faketcp`、`quic`、`wg`、`ws`、`wss` 的多传输 stealth。
- 新增 `transport_priority`，可按 `global`、`wan`、`lan` 和精确虚拟 IP 规则重排
  direct-connect 底层协议顺序。
- 修正 direct-connect 的 WAN/LAN 候选分类：公网 IPv4/IPv6 不再因为处于本机接口前缀内
  就被当成 LAN；link-local 只有匹配本机 link-local 网段时才进入 LAN bucket。
- 新增 `disable_legacy_udp_hole_punch`，用于拒绝未携带 stealth 偏好的旧版 UDP 打洞
  RPC。
- 在原有 QUIC/KCP Proxy 基础上新增 readiness ACK、分类 fallback reason 和按传输维度
  的健康状态。
- 新增 Linux 原生 veth NIC 后端，供具备网络管理 capability、但无法打开或创建 TUN
  设备的容器使用。
- 新增默认开启的 `underlay_candidate_guard`，会从本机 IP 通告、direct candidate、
  direct UDP 路由源校验、hole-punch candidate 和 bind-source 列表中过滤配置的
  fake-IP/TUN CIDR 以及 EasyTier 运行态虚拟地址。
- `underlay_candidate_guard` 还包含内部运行时 breaker：对 guard hard hit 和明确
  peer mismatch 做 Endpoint/Peer 维度局部熔断；Peer/Endpoint 使用同一 lease 原子
  获取，TTL 后 half-open 单飞，取消的 preflight 会在真实网络副作用前回滚，只有认证
  连接收到首个 pong 才精确解除。可疑 TUN 接口只记录 soft signal，不默认硬拒绝。
- 新增 fork 自己的 GitHub Actions 发布顺序约束，要求先完成必要 build/test，再手动
  触发 release。

## 3. 与上游不同的行为和兼容边界

这些差异是运维和升级时必须知道的：

- 固定的 stealth `udp://` listener 不接受 legacy plain SYN 探测；旧节点主动拨 strict
  UDP stealth listener 时会被静默丢弃，这是设计上的 anti-probe 取舍。
  TCP strict listener 当前在一个同 secret 混合场景下不够严格，详见
  [known_bugs/stealth_secure_known_bugs.md](known_bugs/stealth_secure_known_bugs.md)。
- 默认 `stealth_protocols` 已列出所有支持的传输。显式把它设为空字符串时，才是
  “只保护 UDP”的兼容发布覆盖。
- `stealth_window_secs` 是网络级参数；`0` 等价于 60 秒，同一网络中所有 stealth 节点
  必须使用相同有效值。
- `transport_priority` 只影响 direct-connect，不重排 manual/bootstrap 显式 URL，也不
  改变 Proxy 固定顺序。
- QUIC/KCP Proxy 故障转移顺序固定为 `QUIC -> KCP -> Native`。
- `failover` 表是 TCP SYN 的短期选择状态，不是封装连接或回环抑制清单。
  内部 stream 交接会故意使用本机到本机的 PeerManager 标记；记录会按 TTL
  自动清理，不影响远端 QUIC/KCP 健康或交接。原始目标精确等于本机虚拟 IP 时会在
  建表前旁路 selector，防止本机 proxy 目标连接被 TUN 再捕获后递归放大；状态接口和
  GUI 只展示最近 256 条用于诊断。
- `disable_quic_input` / `disable_kcp_input` 只关闭 QUIC/KCP Proxy 入站能力，不关闭底层
  `quic://` 或 `kcp` 相关 listener 路径。
- 双栈 peer 同时命中精确 IPv4 和精确 IPv6 规则时，确定性采用 IPv4 规则，因为
  direct tunnel 是 peer 级共享连接。
- `default_protocol` 仍可保留作为兼容配置，但一旦设置 `transport_priority`，
  direct-connect 实际以 `transport_priority` 为准。
- 回环处理现在是目标级 TTL 避退，不应理解为“所有残余回环流量都会自动消失”。
- 与 Mihomo/Clash/sing-box TUN 同时运行时，默认 underlay guard 会减少明显污染的
  direct candidate，但它不是“所有 generic underlay socket 都跨平台强制绕过系统
  TUN”的硬保证。这不是 QUIC/KCP Proxy fallback 本身的无界状态；排查步骤见
  [Mihomo TUN 共存风险](mihomo_tun_interop_cn.md)。

## 4. 本 Fork 新增参数

这里只列出 `upstream/main` 没有、而这个 fork 对外新增的参数。

| CLI 参数 | 环境变量 | 用途 | 说明 |
| --- | --- | --- | --- |
| `--stealth-mode` | `ET_STEALTH_MODE` | 启用 stealth。 | 默认开启；需要非空 `network_secret`。未显式配置 `secure_mode` 时，握手密钥只为 Stealth-protected PeerConn 握手在运行期派生。 |
| `--stealth-window-secs <n>` | `ET_STEALTH_WINDOW_SECS` | 设置 gate-key 滚动窗口。 | `0` 表示 60 秒；同一网络所有 stealth 节点必须一致。 |
| `--stealth-protocols <list>` | `ET_STEALTH_PROTOCOLS` | 配置需要 stealth 的传输协议。 | 默认列出所有支持的传输；显式留空表示仅 UDP 进入 stealth，便于兼容 rollout。 |
| `--disable-legacy-udp-hole-punch` | `ET_DISABLE_LEGACY_UDP_HOLE_PUNCH` | 拒绝旧版 UDP 打洞 RPC。 | 只拒绝“没有 stealth 偏好字段”的旧请求，不拒绝新节点显式请求 plain。 |
| `--transport-priority <rules>` | `ET_TRANSPORT_PRIORITY` | 重排 direct-connect 协议顺序。 | 格式必须是 `scope:proto,...;scope:proto,...`，例如 `global:quic,faketcp,ws,wg,udp,tcp`。 |
| `--underlay-candidate-guard` | `ET_UNDERLAY_CANDIDATE_GUARD` | 过滤污染 underlay candidate。 | 默认开启；不改变 listener 绑定。 |
| `--underlay-exclude-cidrs <cidrs>` | `ET_UNDERLAY_EXCLUDE_CIDRS` | 用户附加的排除 CIDR，会用于 IP 通告、direct candidate、hole-punch candidate，以及相关路由源 / bind-source 校验。 | 默认 `198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,fd65:6173:7974::/48,192.19.0.0/24`；这组常见 fake-IP 网段在 guard 开启时也是内置 base set，清空后仍保留运行态 EasyTier 虚拟地址过滤和内置 base set。 |
| `--nic-backend <tun|veth|auto>` | 无 | 选择 Linux 虚拟 NIC 后端。 | 仅 Linux `tun` 构建的 CLI 提供；默认 `tun`，不序列化到 TOML/protobuf。 |

上游原本就有 `--enable-kcp-proxy`、`--enable-quic-proxy`、
`--disable-kcp-input`、`--disable-quic-input`。本 fork 改的不是这些参数是否存在，
而是其相关 Proxy 路径上的 readiness ACK、failover 分类、健康状态和 capability
广告行为。

## 5. 原有参数在本 Fork 下需要注意的行为

这些参数不是本 fork 新发明的，但在对比上游时仍应单独注意。

| CLI 参数 | 环境变量 | 原有用途 | 本 fork 相关变化 |
| --- | --- | --- | --- |
| `--enable-kcp-proxy` | `ET_ENABLE_KCP_PROXY` | 开启 source 侧 TCP→KCP Proxy。 | source 侧 prepare/fallback 现在会等待 readiness ACK，并使用更精确的失败分类。 |
| `--disable-kcp-input` | `ET_DISABLE_KCP_INPUT` | 关闭 KCP Proxy 入站能力。 | capability 广告现在应同时受运行时配置和编译 feature 约束。 |
| `--enable-quic-proxy` | `ET_ENABLE_QUIC_PROXY` | 开启 source 侧 TCP→QUIC Proxy。 | QUIC 失败后仍可回退 KCP 或 Native，但失败原因和健康统计更精确。 |
| `--disable-quic-input` | `ET_DISABLE_QUIC_INPUT` | 关闭 QUIC Proxy 入站能力。 | 仍然不会关闭 `quic://` listener，只影响 QUIC Proxy 入站能力。 |

## 6. 配置冲突与易错点

- `--transport-priority` 必须写成带 scope 的规则，例如
  `global:quic,faketcp,ws,wg,udp,tcp`；直接写 `quic,faketcp,ws,wg,udp,tcp`
  会报 `failed to parse transport_priority`。
- 一旦设置 `--transport-priority`，`default_protocol` 对 direct-connect 就只剩兼容兜底
  含义，不应再把两者理解为共同控制同一路径。
- 数据面协议偏好受延迟约束：Peer 会先排除 RTT 超过最低 RTT 125% 的连接，然后才在
  合格集合里按偏好顺序选择；亚毫秒链路会额外允许一个很小的绝对 RTT slack，避免
  0.x ms 级别差异让 QUIC/FakeTCP 永远无法进入合格集合。
- `--underlay-candidate-guard` 是 candidate 净化，不是进程级强制绕过
  Mihomo/Clash/sing-box TUN。它过滤的是 EasyTier 对外通告和主动拨打的 underlay
  candidate；listener 仍可绑定 `0.0.0.0`。命中 guard 的公网 IPv4 UDP 直连候选会
  直接 fail-closed 跳过，不再退回 generic direct UDP fallback。
- `--stealth-mode` 如果缺少非空 `network_secret`，不会变成硬错误；启动时会告警，并
  继续保持 plain。
- `secure_mode` 是显式高级 credential/Noise 配置。已有配置如果包含 `[secure_mode]`
  但没有显式 `stealth_mode=true`，会保持旧行为：Stealth 关闭。显式
  `stealth_mode=true` 同时 `secure_mode.enabled=false` 会被拒绝为冲突。
- 运行期派生的 Stealth 密钥不会作为 RoutePeerInfo 的 `noise_static_pubkey` 发布，也不会
  启用全局 RelayPeerMap/PeerManager secure relay/session 语义；这些路径仍只跟随显式
  `secure_mode`。
- 远端验证和代码路径审计确认派生 secure 会进入 Stealth-protected PeerConn
  secure-session 路径；不能因为吞吐接近 plain TCP 就理解成“没有加密”，也不能把它理解成
  显式全局 `secure_mode`。
- `secure_mode=true + stealth_mode=true` 在已测 TCP underlay 路径上存在已知吞吐回归；
  plain Stealth 和单独显式 secure 在 2026-07-08 验证中没有同类回归。
- 当前 GUI 只编辑 Stealth；显式 `secure_mode` 仍属于 CLI/TOML/RPC 高级配置。后续 GUI
  入口计划见 [todo/gui_global_secure_identity.md](todo/gui_global_secure_identity.md)。
- 自定义 `--stealth-window-secs` 时，同一网络内所有 stealth 节点必须使用相同有效值。
- `--disable-legacy-udp-hole-punch` 即使在 UDP stealth 当前未生效时，也仍会拒绝没有
  stealth 偏好的旧请求。
- `stealth_protocols` 里如果写入当前构建未编译的协议，启动时会告警并跳过，不会默默生效。
- `--nic-backend veth`、`auto` 与 `--no-tun` 冲突。veth 后端要求
  `CAP_SYS_ADMIN + CAP_NET_ADMIN + CAP_NET_RAW`，不是无特权容器 fallback。
- `auto` 只在 TUN open/create 返回预期的设备不可用或权限 errno 时回退；MTU、地址和
  路由配置失败不触发 veth。
- veth 后端保留 `169.254.255.254` 和 `fe80::e:1`；包含任一内部 gateway 的非默认
  静态或动态路由会被拒绝。

### veth 生命周期与已复核边界

下面这些行为已经按实际调用链和 Linux 3.10 运行结果复核，不属于需要继续修改的功能
问题：

- 缺少 `addr_gen_mode` 的旧内核使用有界 link-local 清理。清理失败发生在
  `TunDeviceReady` 之前，初始化错误会向上返回并销毁 veth，不会让未清理接口进入正常
  数据面。
- `VirtualNic` 销毁时立即清理 veth 是停止实例和 DHCP 重建路径的预期行为。此时内部
  转发任务正在取消，不需要为了等待每个 `Arc` 自然释放而延迟接口删除。
- veth stream 会丢弃满足内部特征的 NDP、MLD、IGMP 等链路控制流量，避免它们进入
  overlay。普通 IPv4/IPv6 单播、广播、组播 UDP 和其他用户数据仍会转发；EasyTier 对
  组播目标采用广播到相关 peer 的策略，不依赖 IGMP 成员关系，因此不影响常规组播。
  只有未来明确要支持 IGMP proxy/组播路由守护进程的原始控制包隧穿时，才需要另行设计。
- 地址回滚失败项保存在容量固定为 256 的 orphan registry 中，并在后续配置和 cleanup
  时重试。它不会形成无界内存增长；仅在外部程序恰好并发删除地址等极端情况下可能保留
  一个容量槽位，不需要作为当前功能阻塞项处理。
- Linux 在删除接口地址时可能同时删除依赖该地址的显式路由。后端把后续
  `ESRCH`/`ENOENT` 视为幂等删除成功，并继续清理 IPv4/IPv6 route cache 和
  directed-broadcast 状态。

## 7. 配置示例

### CLI 示例

下面最后两个参数本身是上游已有参数；这里把它们放进示例，是为了说明它们如何与本 fork
更新过的 QUIC/KCP Proxy 路径配合使用。

```bash
easytier-core \
  --network-name demo \
  --network-secret demo-secret \
  --stealth-mode \
  --stealth-window-secs 60 \
  --stealth-protocols udp,tcp,faketcp,quic,wg,ws,wss \
  --transport-priority 'global:quic,faketcp,ws,wg,udp,tcp' \
  --enable-quic-proxy \
  --enable-kcp-proxy
```

### TOML 示例

```toml
[flags]
stealth_mode = true
stealth_window_secs = 60
stealth_protocols = "udp,tcp,faketcp,quic,wg,ws,wss"
disable_legacy_udp_hole_punch = false
transport_priority = "global:quic,faketcp,ws,wg,udp,tcp"
underlay_candidate_guard = true
underlay_exclude_cidrs = "198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,fd65:6173:7974::/48,192.19.0.0/24"
enable_quic_proxy = true
enable_kcp_proxy = true
disable_quic_input = false
disable_kcp_input = false
```

## 8. 推荐阅读顺序

如果你是从上游迁移、对比两个分支，或者要判断某个参数是否属于本 fork 的行为变更，
建议按下面顺序阅读：

1. 先看本文，快速区分本 fork 和上游的差别。
2. 再看 [udp_stealth_compatibility.md](udp_stealth_compatibility.md)，了解 stealth、
   proxy、回退和优先级细节。
3. 调查 SOCKS5、`no_tun` 或 QUIC/KCP Proxy 吞吐时，看
   [流量、代理协议与封装层级](traffic_protocol_layers_cn.md)，先确定观察到的是内层
   Proxy 还是外层 overlay；再看
   [SOCKS5 性能调查与维护边界](socks5_performance_investigation_cn.md)，避免把目标端
   `no_tun` TCP 入站代理误判成 SOCKS5 source 瓶颈。
4. 最后回到 README 里的 `Fork-Specific Changes`，它是首页摘要，不替代详细设计文档。
