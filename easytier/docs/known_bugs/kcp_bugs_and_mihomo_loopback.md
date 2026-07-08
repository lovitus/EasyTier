# KCP SOCKS 生命周期缺陷与 Mihomo TUN 回环放大

**状态**

- 已复现。KCP endpoint 生命周期缺陷已在本分支修复；idle 状态下 generic connector 经 Mihomo/sing-box/Clash TUN 选源形成的系统层回流也已补一层 underlay guard。
- 结论来自实机压测、系统进程/TUN 计数观察、当前代码路径审计，以及 `kcp-sys`/Proxy targeted 测试。
- 该问题不是单一 bug：KCP 生命周期缺陷会放大系统 TUN 回流，Mihomo TUN 回流又会放大 KCP 的重试、hedge 和关闭失败。本次修复先切断 KCP state 泄漏放大器，不承诺 generic underlay socket 永远绕过系统 TUN。

**结论**

目前存在两个独立但会互相放大的问题：

1. **KCP 本身存在真实的连接生命周期缺陷。**压力下会出现连接失败、关闭超时，以及 hedged 建连遗留会话。该部分已通过 `KcpEndpoint::connect()` lifecycle guard、同步 cleanup、best-effort RST 和 stale cleanup 修复。
2. **Mihomo 回环位于更底层的 PeerConn/系统 TUN。**KCP 不是直接创建系统 UDP socket 的根因，但会通过高频分片、重传、ping 和重复建连迅速放大回环。
3. **idle 回环可由 generic connector 触发。**即使没有 SOCKS/KCP 压测，后台 P2P/连接维护解析到 IPv6 underlay candidate 时，系统可能选择 Mihomo fake IPv6 作为本地源地址，导致 EasyTier 和 Mihomo 在空闲状态下互相拉高 CPU。

**影响范围**

- 已在实机上明确复现的高风险入口是 **SOCKS 私有 KCP fast path**。该路径绕过通用 Proxy Failover，固定 legacy prepare，不等待 READY，也没有 QUIC/Native 自动回退。
- 普通 TCP Proxy 的 KCP 路径也复用 `NatDstKcpConnector` 和 `KcpEndpoint::connect()`，因此底层 hedge cancellation / `state_map` 遗留缺陷不是 SOCKS 独有。
- 普通 TCP Proxy 风险较低，因为它通过 selector 请求 READY ACK，并按 `QUIC → KCP → Native` 回退；KCP 失败通常会被分类并尝试下一候选，不会像 SOCKS 私有 KCP 一样直接成为单点。
- 因此不能把 KCP endpoint lifecycle fix 只做在 SOCKS 层。SOCKS 是优先修复入口，底层 `kcp-sys` 也必须修。
- 直接关闭 SOCKS KCP 不是合适的长期修复：no-TUN / smoltcp SOCKS 路径已经验证存在明显性能缺口。
- 本次修复不改变 SOCKS KCP-only 语义：KCP 可用时仍使用 SOCKS 私有 KCP fast path；KCP connect/prepare 失败仍返回 SOCKS 错误，不自动回退 smoltcp/native。

**KCP 问题**

- SOCKS KCP 绕过了通用 `QUIC → KCP → Native` selector。目标允许 KCP 时直接选择 `Socks5KcpConnector`，见 [socks5.rs](../../src/gateway/socks5.rs#L423)。
- 该路径固定发送 `proxy_prepare_version=0`，见 [kcp_proxy.rs](../../src/gateway/kcp_proxy.rs#L160)，不等待 READY，也不会在失败后自动回退 QUIC 或 Native。因此 KCP 异常时 SOCKS 请求直接失败，而且 Proxy Failover 面板不一定记录。
- 每个逻辑连接最多启动 5 个 KCP 建连，间隔 200ms，见 [kcp_proxy.rs](../../src/gateway/kcp_proxy.rs#L142)。路径延迟超过 200ms 后会并发产生多个会话。
- 修复前，`KcpEndpoint::connect()` 在等待握手前先写入 `state_map`。hedge 返回首个成功结果后，其他 future 被取消，却没有 cancellation cleanup。
- 修复前，被取消的连接仍可能收到 SYN-ACK 并进入 Established。远端也可能 `accept()` 它、连接真实目标端口，然后永久等待没有应用数据的 KCP stream。
- 修复后，connect future 在取消、超时、发送失败、状态异常或 `add_conn()` 失败时都会清理 `state_map` 和可能存在的 `conn_map`。取消路径只依赖同步 `Drop` cleanup 和 `try_send(RST)`，不依赖 async Drop。
- 远端 RST 丢失时仍依赖 pong timeout + cleanup tick 回收 orphan；这是异常网络下允许存在的瞬时 orphan 边界，不是长期泄漏。
- 关闭阶段的 forced cleanup 本身不是泄漏：6 秒后发送 RST 并删除 `conn_map/state_map`。大量 `KCP graceful close timed out` 仍说明 FIN/FIN-ACK 在当前路径上不能稳定完成，但不会再导致未归属 state 长期残留。
- 另一个独立实现错误也已修复：`create_kcp_endpoint()` 设置的 `kcp_config_factory` 现在会被 `add_conn()` 用于创建连接，proxy endpoint 的 5ms interval 配置实际生效。

**已观察到的实机结果**

KCP 短连接压测结果：

- 63 成功。
- 22 明确失败。
- 15 个在终止测试时尚未完成。
- 远端 HTTP 服务收到 89 个请求且没有服务端错误。
- 客户端出现 connect timeout、proxy closed 和大量 forced cleanup。

这说明失败同时发生在建连阶段和响应/关闭阶段，不只是 HTTP 服务或单纯测试超时。

**Mihomo 回环**

KCP packet 在 [kcp_proxy.rs](../../src/gateway/kcp_proxy.rs#L91) 被封装为 `KcpSrc/KcpDst`，然后调用 `PeerManager::send_msg_for_proxy()`。它复用现有 PeerConn，并不直接创建独立的公网 UDP underlay。

实际链路是：

```text
SOCKS KCP
→ KcpSrc/KcpDst overlay packet
→ PeerManager 路由/relay
→ 当前 PeerConn
→ macOS 路由进入 Mihomo utun
→ Mihomo 再转发 underlay
```

第一次异常时观测到：

- EasyTier 主实例约 39–44% CPU。
- Mihomo 约 113–122% CPU。
- `.99` KCP 测试节点自身只有约 0–2% CPU。
- Mihomo TUN packet counter 每秒增加数万。
- Proxy Failover 表没有同步膨胀。
- 停止 `.99` 后循环仍持续；终止主 EasyTier 后 Mihomo CPU 立即恢复。

因此，**KCP是触发器和放大器，持续回环状态实际保存在主实例的 underlay PeerConn 和 Mihomo TUN 路径中**。

Overlay 的 `forward_counter > 7` 只能阻止 overlay 路由循环，见 [peer_manager.rs](../../src/peers/peer_manager.rs#L1334)。系统 TUN 捕获后重新进入 underlay 属于另一层，计数器无法识别。

现有 underlay guard 也不是完整 bypass：

- 能过滤广告地址、direct candidate、IPv6 hole-punch，以及 public UDP direct 的路由源。
- 修复后，generic connector 解析出的候选目标地址也会被 guard 检查；建立连接前还会使用同一 netns、同一 address family、同一 `socket_mark` 创建临时 connected UDP socket，验证系统实际选择的本地源地址。如果目标或源地址命中 guarded 网段，则跳过该候选。
- 该修复堵住了 idle 状态下 TCP/WS/QUIC/WG/FakeTCP 等通用 connector 因源地址选择进入 Mihomo TUN 的主要路径，但仍不承诺所有 manual/bootstrap/generic underlay 在所有平台上具备内核级 TUN bypass。
- `198.18.0.0/15`、`fc00::/18`、`fdfe:dcba:9876::/48` 和 `192.19.0.0/24` 已作为 guard 内置 base set；即使用户配置的 `underlay_exclude_cidrs` 为空或缺项，常见 Mihomo/Clash、sing-box、V2Ray/Xray、Surge fake-IP 源/目标仍会被过滤。
- Mihomo 的 `10/8 DIRECT` 只处理 overlay 地址；EasyTier underlay 通常连接公网 IP，不属于 `10/8`。
- 进程 DIRECT 规则也不等于路由层 bypass，数据仍可能先进入 utun。

最可能的正反馈过程是：

```text
Mihomo 捕获 underlay
→ RTT/丢包增加
→ KCP 启动更多 hedge 和重传
→ 遗留 KCP 会话、ping、目标 TCP 增加
→ PeerConn/TUN 包量继续上升
→ Mihomo 更拥塞
```

**临时规避**

- 需要稳定运行时，避免在 Mihomo/Clash/sing-box TUN 开启状态下做 KCP-only 或高并发 SOCKS KCP 压测。
- 优先使用 QUIC 或 Native 路径；如果必须使用 SOCKS，避免把 SOCKS chain 设计成经同一个 overlay 反复回到本机或同一下一跳。该规避会牺牲 no-TUN / smoltcp SOCKS 性能，不应视为最终修复。
- 确认 `underlay_candidate_guard=true`。默认 fake-IP base set 已内置；如果使用了其他 fake-IP 网段，再追加到 `underlay_exclude_cidrs`。
- 触发高 CPU 后，重启 EasyTier 主实例和 Mihomo 可以释放当前循环状态；这只是恢复手段，不是修复。

**修复顺序**

1. 已完成：修复 KCP hedge cancellation cleanup。被取消的 connect future 会删除 `state_map`，裁剪可能存在的 `conn_map`，并 best-effort 发送 RST。
2. 已完成：修复 `kcp_config_factory` 未生效问题，确保 proxy 创建的 endpoint 参数确实作用到连接。
3. 已完成：增加 KCP endpoint 内部 stats：`state_map_len`、`conn_map_len`、`connect_cancel_cleanup_total`、`forced_cleanup_total`、`orphan_timeout_cleanup_total`。首版只用于 tracing 和测试断言，不进入 RPC/protobuf/GUI。
4. 明确不做：本轮不让 SOCKS KCP 回退 QUIC/Native，也不把 SOCKS legacy `proxy_prepare_version=0` 改为 READY-aware。这样避免把 lifecycle 修复扩大为策略重构，并保留 no-TUN / smoltcp 场景下已经验证过的 KCP 性能路径。
5. 已完成：修复 `should_deny_proxy()`（见 [global_ctx.rs](../../src/common/global_ctx.rs)）只检查目标端口是否在
   EasyTier 自身 `running_listeners`/`protected_port` 里的漏洞。当目的地址等于本机 EasyTier 虚拟 IP
   （`dst_is_local_et_ip`）且端口不属于 EasyTier 自身监听器时，新增一次针对该精确地址的 bind 探测
   （`is_local_port_occupied`）：如果 bind 失败说明本机有其他进程（例如 Mihomo/Clash 的通配监听）
   占用了该端口，直接拒绝代理，避免把流量转发进入无关本地进程并形成回环。
   该检查刻意不应用于 `dst_is_local_phy_ip`（物理网卡地址），因为向本机物理 LAN 地址上真实存在的服务
   转发正是 `proxy_cidrs` 出口节点场景的正常用法，不能一并拒绝。
6. 已完成：generic connector 的候选目标和临时 connected UDP 源地址验证接入 `underlay_guard`。
   这覆盖了运行后未访问、未压测时仍由后台连接维护触发的 Mihomo fake IPv6 源地址选择问题。
   该修复复用 `bind()` 的 netns、address family、`socket_mark` 和 bind-source 语义；收到的
   hole-punch RPC connector 地址也会先净化。不改 listener 绑定、不改 Proxy/KCP/SOCKS/wire。
7. 已完成：新增运行时局部 breaker。目标 IP 或系统实际 source IP 命中内置 fake-IP base set、
   用户附加 CIDR 或 EasyTier 运行态虚拟地址时，只记 `Endpoint(remote_addr, scheme, scope)`
   hard strike；handshake peer mismatch 有 expected peer 时才记 `Peer(expected_peer_id, scheme,
   scope)`。3 次 hard strike / 30 秒触发 TTL；Peer/Endpoint key 使用同一 generation lease
   原子进入 half-open，取消的 preflight 只回滚自身 lease，真实连接开始后失败才触发超时退避。
   Direct、generic 与 TCP/UDP hole-punch 都在首个认证 pong 后精确清理，握手成功不会提前清理。
8. 已完成：source-interface 反查只作为 soft signal。`utun`/`tun`/`tap`/`wintun`/point-to-point
   等接口如果没有命中 CIDR/IP guard，不会被硬拒绝，也不会触发熔断，只用于 warning 和诊断。
9. 明确边界：`underlay_candidate_guard=false` 时，本轮新增的 guard、breaker gate、hard/soft
   strike 和 TTL 都不生效；只保留历史 EasyTier-managed IPv6 保护。

**验收条件**

- KCP hedge 超时、成功和取消后，`state_map/conn_map` 都能回到基线。
- 高并发短连接后，远端不残留长期空 TCP 连接，KCP endpoint 不接近 4096 上限。
- SOCKS KCP 失败时以明确错误返回且不会留下后台 KCP 会话；不要求回退 smoltcp/native。
- Mihomo TUN 开启时，KCP 压测不会导致 EasyTier 主实例和 Mihomo 长时间互相拉满 CPU。
- Proxy Failover 面板、RPC proxy entries 与 KCP endpoint 内部 stats 能区分“TCP SYN selector 状态”和“SOCKS 私有 KCP 会话”。当前内部 stats 不通过 RPC 暴露。
