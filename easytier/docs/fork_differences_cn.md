# 本 Fork 与上游 EasyTier 的差异总览

这份文档是 README 首页的 fork 差异入口，内容已经按当前本地代码和 `upstream/main`
交叉核对过，目的是把下面四类信息放在同一处：

- 这个 fork 修复了哪些真实问题？
- 这个 fork 新增了哪些上游默认没有的能力？
- 与上游相比，有哪些行为差异或兼容边界？
- 哪些参数是这个 fork 新增的，哪些是上游原有但在这里行为不同的？

Stealth、Proxy、回退和 rollout 细节仍然以
[udp_stealth_compatibility.md](udp_stealth_compatibility.md) 为准。

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
- 新增 `disable_legacy_udp_hole_punch`，用于拒绝未携带 stealth 偏好的旧版 UDP 打洞
  RPC。
- 在原有 QUIC/KCP Proxy 基础上新增 readiness ACK、分类 fallback reason 和按传输维度
  的健康状态。
- 新增 fork 自己的 GitHub Actions 发布顺序约束，要求先完成必要 build/test，再手动
  触发 release。

## 3. 与上游不同的行为和兼容边界

这些差异是运维和升级时必须知道的：

- 固定的 stealth `udp://` listener 不接受 legacy plain SYN 探测；旧节点主动拨 strict
  stealth listener 时会被静默丢弃，这是设计上的 anti-probe 取舍。
- `stealth_protocols` 留空时，仍保持“只保护 UDP”的兼容发布行为；只有显式列出的协议
  才会进入 stealth。
- `stealth_window_secs` 是网络级参数；`0` 等价于 60 秒，同一网络中所有 stealth 节点
  必须使用相同有效值。
- `transport_priority` 只影响 direct-connect，不重排 manual/bootstrap 显式 URL，也不
  改变 Proxy 固定顺序。
- QUIC/KCP Proxy 故障转移顺序固定为 `QUIC -> KCP -> Native`。
- `disable_quic_input` / `disable_kcp_input` 只关闭 QUIC/KCP Proxy 入站能力，不关闭底层
  `quic://` 或 `kcp` 相关 listener 路径。
- 双栈 peer 同时命中精确 IPv4 和精确 IPv6 规则时，确定性采用 IPv4 规则，因为
  direct tunnel 是 peer 级共享连接。
- `default_protocol` 仍可保留作为兼容配置，但一旦设置 `transport_priority`，
  direct-connect 实际以 `transport_priority` 为准。
- 回环处理现在是目标级 TTL 避退，不应理解为“所有残余回环流量都会自动消失”。

## 4. 本 Fork 新增参数

这里只列出 `upstream/main` 没有、而这个 fork 对外新增的参数。

| CLI 参数 | 环境变量 | 用途 | 说明 |
| --- | --- | --- | --- |
| `--stealth-mode` | `ET_STEALTH_MODE` | 启用 stealth。 | 需要 secure mode 和非空 `network_secret`。 |
| `--stealth-window-secs <n>` | `ET_STEALTH_WINDOW_SECS` | 设置 gate-key 滚动窗口。 | `0` 表示 60 秒；同一网络所有 stealth 节点必须一致。 |
| `--stealth-protocols <list>` | `ET_STEALTH_PROTOCOLS` | 配置需要 stealth 的传输协议。 | 留空表示仅 UDP 进入 stealth，便于兼容 rollout。 |
| `--disable-legacy-udp-hole-punch` | `ET_DISABLE_LEGACY_UDP_HOLE_PUNCH` | 拒绝旧版 UDP 打洞 RPC。 | 只拒绝“没有 stealth 偏好字段”的旧请求，不拒绝新节点显式请求 plain。 |
| `--transport-priority <rules>` | `ET_TRANSPORT_PRIORITY` | 重排 direct-connect 协议顺序。 | 格式必须是 `scope:proto,...;scope:proto,...`，例如 `global:quic,faketcp,ws,wg,udp,tcp`。 |

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
  合格集合里按偏好顺序选择。
- `--stealth-mode` 如果缺少 secure mode 或非空 `network_secret`，不会变成硬错误；启动
  时会告警，并继续保持 plain。
- 自定义 `--stealth-window-secs` 时，同一网络内所有 stealth 节点必须使用相同有效值。
- `--disable-legacy-udp-hole-punch` 即使在 UDP stealth 当前未生效时，也仍会拒绝没有
  stealth 偏好的旧请求。
- `stealth_protocols` 里如果写入当前构建未编译的协议，启动时会告警并跳过，不会默默生效。

## 7. 配置示例

### CLI 示例

下面最后两个参数本身是上游已有参数；这里把它们放进示例，是为了说明它们如何与本 fork
更新过的 QUIC/KCP Proxy 路径配合使用。

```bash
easytier-core \
  --network-name demo \
  --network-secret demo-secret \
  --secure-mode \
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
   [SOCKS5 性能调查与维护边界](socks5_performance_investigation_cn.md)，避免把目标端
   `no_tun` TCP 入站代理误判成 SOCKS5 source 瓶颈。
4. 最后回到 README 里的 `Fork-Specific Changes`，它是首页摘要，不替代详细设计文档。
