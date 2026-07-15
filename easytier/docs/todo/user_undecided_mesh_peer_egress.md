# [用户未决定是否实现 / USER UNDECIDED / DO NOT IMPLEMENT] 无端口 Mesh Peer 出口与链式 Dialer 设计评估

> **状态：用户尚未决定是否实现。本文仅记录讨论、现状证据、候选架构和决策边界；不得据此直接开始编码、修改配置语义、触发构建或安排发布。**

## 1. 文档目的

本文记录关于 Leaf policy proxy 中 `via: mesh`、用户自建 SOCKS5、无端口 EasyTier peer 出口、chain/fallback、UDP、DNS、性能、跨平台设计、授权和 EasyTier 2.9.10 兼容性的完整讨论。

需要防止后续再次混淆以下两个概念：

- “通过 EasyTier Mesh 到达 peer 上由用户运行的 SOCKS5 服务”。
- “把 EasyTier peer 本身作为内建出口或 chain 前置 dialer，不要求用户运行 SOCKS5”。

这两个能力必须具有不同配置类型、不同生命周期和清晰的兼容边界。

## 2. 用户提出并确认的目标

- 用户预期 chain 前面的 EasyTier peer 可以只指定 peer 身份，不指定端口。
- 只有用户明确使用 gost 或其他自建 SOCKS5 服务时，才应该配置 SOCKS5 端口。
- 用户不希望内建 peer 出口依赖 peer 上的 SOCKS5 UDP 支持。
- 用户接受 UDP 出口实际不可用时 UDP 直接失败，不要求根据首个 UDP 包自动切换 fallback。
- 用户可以在默认 UDP 出口中加入一个确定支持 UDP 的 mesh peer。
- DNS 不能因为 UDP 不可用而错误回退；DNS 可以使用 DoH、DoT 或 TCP。
- 域名应按照匹配到的出口进行解析：DIRECT 域名按 DIRECT 路径解析；通过某个节点的域名应通过该节点对应的路径解析。
- 用户认为 Mesh 中再套用户 SOCKS 的性能不如 EasyTier 原生数据面。
- 第一版只要求 Linux 和 Android 完成并验证，但架构必须从开始就考虑 Windows、macOS，以及移动平台受限运行环境。
- 不能为了 Linux 的短期性能把公开语义绑定到 Linux TUN、iptables、NAT、network namespace 或特定内核功能。
- 用户认为为了使用 `mesh-peer` 必须另外显式打开传统 `enable_exit_node` 不够合理，希望存在安全的小妥协。
- EasyTier 不应为了参考 Mihomo 而被严重魔改；Leaf/EasyTier 集成需要保持解耦。

## 3. 已确认的当前实现事实

### 3.1 当前所有 proxy 都强制要求端口

`easytier-policy/src/config.rs` 中当前 `Proxy.port` 是必填 `u16`，不是可选字段；当前验证也不接受端口 0。因此现有 schema 没有“只指定 peer、不指定端口”的 actor。

参考：

- `easytier-policy/src/config.rs`：`Proxy`、`ProxyKind`、proxy validation。
- `easytier-web/frontend-lib/src/components/policy/PolicyEditor.vue`：当前编辑器代理端口模型。
- `easytier-web/frontend-lib/src/components/policy/policyDocument.ts`：当前默认模板中的 mesh SOCKS 示例。

### 3.2 当前 `via: mesh` 实际是“通过 Mesh 连接 peer SOCKS 端口”

当前 mesh proxy target 由选中的 peer 虚拟地址和 `proxy.port` 共同构成：

```rust
MeshProxyTarget {
    peer_id: route.peer_id,
    endpoint: SocketAddr::new(address, proxy.port),
}
```

Leaf 连接本机随机 loopback SOCKS bridge；bridge 再通过 EasyTier DataPlane 连接 `peer_virtual_ip:configured_port`，并在该连接上执行 SOCKS5 协议。

因此当前：

```yaml
peer-gost:
  type: socks5
  server:
    instance-id: peer-instance-id
  port: 1080
  via: mesh
```

表示：

> 通过 EasyTier Mesh 到达该 peer 虚拟地址的 1080 端口，并要求该端口确实运行 SOCKS5，例如 gost。

它并不表示 EasyTier 内建 peer 出口。

参考：

- `easytier/src/instance/virtual_nic.rs`：`resolve_policy_mesh_endpoints`、`build_policy_runtime`。
- `easytier/src/policy_proxy/mesh_socks_bridge.rs`：`MeshProxyBridgeSet::start`、`relay_socks5`、`connect_remote`。

### 3.3 当前 UDP 同样依赖远端 SOCKS5

当前 UDP relay 会连接同一个 `peer_virtual_ip:configured_port`，在远端执行 SOCKS5 `UDP ASSOCIATE`。用户把配置写成 `udp: true` 不能证明实际 SOCKS 服务支持 UDP。

以下情况当前都可能导致 UDP 不可用：

- peer 自带或用户启动的 SOCKS 只支持 TCP。
- gost/SOCKS server 没有启用 UDP。
- 中间网络允许 TCP 但阻断 UDP relay。
- 配置声明支持 UDP，但服务端实际行为不匹配。

参考：

- `easytier/src/policy_proxy/mesh_udp_relay.rs`：`RemoteUdpAssociation::open`、`open_local_socks_udp`、SOCKS UDP ASSOCIATE。

### 3.4 当前不存在 portless peer actor

当前 Leaf chain 的成员是已定义 outbound actor 名称。现有 policy schema 只有 SOCKS5/HTTP 等代理 actor，没有只接受 `instance-id` 或 `virtual-ip` 的 EasyTier peer actor。

因此下列用户预期配置目前不存在：

```yaml
peer-kr:
  type: mesh-peer
  peer:
    instance-id: peer-instance-id
```

### 3.5 对先前口头描述的纠正

先前曾把当前 `via: mesh` 描述为 EasyTier 管理的 peer UDP 出口，并暗示它不依赖 peer SOCKS UDP。该描述错误。

当前代码证据表明：

- TCP 目标仍然是 peer 地址加用户配置端口。
- UDP 仍然对该端口执行 SOCKS5 UDP ASSOCIATE。
- 当前 `mesh-direct` 不是已实现的独立 actor 类型。

后续文档和 UI 不得把当前 `via: mesh` 简称为“内建 mesh peer 出口”。

## 4. 术语边界

### 4.1 `DIRECT`

`DIRECT` 表示运行 Leaf/EasyTier 的本机通过本机原生网络出口连接目标。

### 4.2 `socks5 + via: native`

本机通过 underlay/native 网络连接用户配置的 SOCKS5 `server:port`。

### 4.3 `socks5 + via: mesh`

本机通过 EasyTier Mesh 到达指定 peer，再连接该 peer 上用户配置的 SOCKS5 `port`。该能力必须继续要求端口。

### 4.4 候选 `mesh-peer`

候选 `mesh-peer` 表示 EasyTier 内建 peer dialer/egress：

- 只配置 durable peer identity。
- 不配置用户端口。
- 不要求用户运行 gost/SOCKS5。
- peer 可以作为最终出口。
- peer 可以作为 chain 前置 dialer，由该 peer 发起到下一代理节点的连接。
- UDP 使用 EasyTier 内建数据面和目标 peer 原生 UDP socket，不依赖 SOCKS5 UDP ASSOCIATE。

### 4.5 “peer 作为 route hop”与“peer 作为 chain actor”

普通 EasyTier 路由经过某个 relay peer，不等于该 peer 是 Leaf chain actor。

- route hop 只负责 Mesh 内的数据转发，不改变公网出口身份。
- chain actor 必须能够接受目标地址，并代表上游连接下一个 actor 或最终目标。

不能仅通过改变 EasyTier route preference 来假装实现 Leaf chain。

## 5. 配置候选语义

以下仅为候选，不是已决定 schema：

```yaml
proxies:
  # EasyTier 内建 peer 出口，不需要端口。
  peer-kr:
    type: mesh-peer
    peer:
      instance-id: 01234567-89ab-cdef-0123-456789abcdef
      # 可选校验条件，不应替代 durable identity。
      virtual-ip: 10.144.144.2

  # 用户自行运行的 SOCKS5，仍然要求端口。
  peer-gost:
    type: socks5
    server:
      instance-id: 01234567-89ab-cdef-0123-456789abcdef
    port: 1080
    via: mesh

groups:
  proxy-chain:
    type: chain
    members:
      - peer-kr
      - socks-node

  preferred-exit:
    type: fallback
    members:
      - proxy-chain
      - peer-kr
```

候选 chain 语义：

```text
本机 Leaf
  -> peer-kr
  -> 由 peer-kr 发起到 socks-node 的连接
  -> socks-node
  -> 最终目标
```

候选 standalone 语义：

```text
本机 Leaf
  -> peer-kr
  -> 由 peer-kr 原生连接最终目标
```

## 6. 为什么单纯复用用户 SOCKS 不够优雅

当前路径：

```text
应用流量
  -> Leaf TUN
  -> 本机随机 SOCKS bridge
  -> EasyTier DataPlane
  -> peer 上的 SOCKS5/gost
  -> peer 原生网络
```

额外成本包括：

- 每个 TCP flow 的 SOCKS5 协商。
- 本机 bridge relay。
- peer SOCKS 服务 relay。
- UDP ASSOCIATE 和额外 UDP 映射。
- 更多任务、FD、缓冲区和用户态复制。
- 对用户 SOCKS 配置和真实 UDP 能力的依赖。
- 用户需要管理服务端口、鉴权、启动和恢复。

这一路径适合兼容用户已有 gost/SOCKS 服务，不适合作为 EasyTier 内建 peer 出口的基础语义。

## 7. 全平台设计约束

最终目标包括 Linux、Android、Windows、macOS，并应为受限移动平台保留实现空间。

公开语义不得依赖：

- Linux `iptables`/nftables、policy routing 或 network namespace。
- Linux 内核 NAT 必须可用。
- Android 能创建第二个 VpnService TUN。
- Windows Wintun 具有与 Linux 相同的 route mark 行为。
- macOS utun 或 NetworkExtension 允许动态库和独立代理进程。
- 出口 peer 拥有 root/Administrator 权限。

推荐把平台差异限制在流量入口和 Leaf 运行方式：

- Linux：TUN + 独立 Leaf worker，首版可保留本机 IPC adapter。
- Android：VpnService TUN + in-process Leaf。
- Windows：Wintun/现有虚拟网卡入口，优先静态/in-process handler。
- macOS：utun 或 NetworkExtension，必须支持静态链接和 in-process handler。

远端 egress 应尽量只依赖跨平台能力：

- Tokio/native TCP socket。
- Tokio/native UDP socket。
- EasyTier 已认证 Mesh 数据面。
- 平台无关的超时、背压、取消和资源限制。

## 8. 候选方案比较

| 方案 | 性能 | 开发难度 | Chain | 全平台 | 旧 peer 兼容 | 主要问题 |
|---|---:|---:|---:|---:|---:|---|
| 当前用户 SOCKS over Mesh | 中 | 低 | 是 | 是 | 是 | 依赖用户服务和 UDP 能力，层次多 |
| 显式 peer 的现有 exit-node packet path | 高 | 中高 | 需要 Leaf dialer adapter | 需逐平台确认 | 较好 | 现有授权开关和包路径语义较宽 |
| 跨平台内部 Mesh egress service | 高 | 中高 | 是 | 是 | 需要 capability | 新内部服务和生命周期 |
| 新建专用 stream mux 协议 | 高 | 高 | 是 | 是 | 否 | 新 wire protocol 和复杂背压 |
| Linux TUN + 内核转发/NAT | 最高 | 很高 | 不自然 | 否 | 部分 | root、NAT、路由 ownership |
| 每个 peer 独立 TUN/路由表 | 中高 | 极高 | 复杂 | 否 | 不适用 | MTU、路由、清理和多平台分歧 |

## 9. 当前综合候选：平台无关 `MeshEgressDialer`

当前讨论中的综合候选不是最终决定。

建议定义一个平台无关的 EasyTier egress 抽象：

```rust
trait MeshEgressDialer {
    async fn dial_tcp(peer: PeerSelector, destination: Destination) -> Stream;
    async fn open_udp(peer: PeerSelector) -> DatagramAssociation;
    async fn resolve(peer: PeerSelector, domain: DomainName, qtype: QueryType)
        -> ResolvedAddresses;
}
```

Leaf 只把 `mesh-peer` 当作一个普通 outbound actor。EasyTier policy integration 将 Leaf session 转换成 `MeshEgressDialer` 请求。

公共数据路径：

```text
Leaf rule/group selection
  -> mesh-peer outbound
  -> MeshEgressDialer
  -> EasyTier authenticated DataPlane
  -> selected peer
  -> peer native TCP/UDP socket
  -> destination
```

该抽象允许以后替换内部传输，而不改变用户配置：

- 首版可复用现有 DataPlane smoltcp TCP/UDP 能力。
- 支持时可复用现有 exit-node packet flag 和 gateway。
- 后续如基准证明必要，可增加原生 stream mux fast path。
- Linux 可增加可选内核 packet/NAT fast path，但不能改变语义。

## 10. 内部服务标识，不使用用户端口

候选实现不应要求 YAML 中出现端口。

内部实现可以采用：

- EasyTier 保留的 `service-id`。
- DataPlane 内部保留 virtual port，由实现映射，不暴露给用户。
- capability-negotiated egress protocol identifier。

内部标识必须满足：

- 不绑定 OS 公网或 LAN 监听端口。
- 不与用户 gost/SOCKS 端口混淆。
- 不允许普通应用抢占。
- 可进行 capability/version negotiation。
- 生命周期由 EasyTier instance 管理。

内部使用 virtual port 不等于用户配置 SOCKS 端口；但长期设计优先使用 service ID，避免把端口号变成公开协议约束。

## 11. Leaf 集成候选

本地 Leaf 源码已有通用 external outbound handler 概念：

- `ExternalOutboundStreamHandler`。
- `ExternalOutboundDatagramHandler`。
- `OutboundConnect`。
- `DatagramTransportType`。

参考：

- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/app/outbound/plugin.rs`。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/proxy/chain/outbound/stream.rs`。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/proxy/chain/outbound/datagram.rs`。

当前 EasyTier pinned Leaf feature list没有启用 `plugin`，且 Leaf 当前 plugin 路径偏向动态库加载。动态 `.so` 不适合作为全平台基础：

- Android 和 macOS NetworkExtension 不适合动态 plugin 部署。
- Windows 动态库打包和 ABI 管理复杂。
- Linux worker 与移动端会形成两套不同注册模型。

候选 Leaf 最小改动：

- 增加通用的、静态/in-memory external handler 注册入口。
- handler 通过 `StartOptions` 或等价 runtime builder 注入。
- Leaf 不知道 EasyTier、peer、Mesh 或 egress protocol。
- EasyTier 专用逻辑全部留在 `easytier-policy`/`easytier` adapter。
- 添加 Leaf chain/fallback parity tests，证明 external actor 与内建 actor 组合语义一致。

这应当是通用、可独立维护的 Leaf API 扩展，而不是把 EasyTier 逻辑写入 Leaf。

## 12. Mihomo 参考语义

Mihomo 不要求前置 dialer 一定是 SOCKS server。它通过 dialer proxy 将“连接下一目标”委托给另一个 proxy actor。

参考：

- `/Users/fanli/Documents/mihomo-rev/component/proxydialer/byname.go`：`byNameProxyDialer.DialContext`、`ListenPacket`、`NewByName`。
- `/Users/fanli/Documents/mihomo-rev/component/proxydialer/proxydialer.go`：`proxyDialer.DialContext`、`ListenPacket`。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/base.go`：`BasicOption.NewTunnel`、dialer composition、chain tracking。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/openvpn.go`：L3 outbound 通过 tunnel device 暴露 `DialContext`/`ListenPacketContext`，而不是要求用户提供 SOCKS 端口。

需要遵循的外部语义：

- 前置 actor 接收到下一 actor 的 endpoint。
- 前置 actor 负责建立到下一 actor 的 transport。
- 最后一个 actor 接收到最终目标。
- chain 不应重新执行规则匹配。
- fallback 依据 actor 建连/初始化错误选择候选，不依据任意首个 UDP 包自动切换。
- actor 名称、能力和错误必须可观察，不得静默变成 DIRECT。

EasyTier 可因 mesh identity、现有 DataPlane API 和平台生命周期而采用不同内部实现，但不能改变上述可观察 chain 语义。

## 13. TCP 语义候选

### 13.1 `mesh-peer` 作为最终 actor

```text
Leaf session destination
  -> MeshEgressDialer.dial_tcp(peer, final_destination)
  -> selected peer native TCP connect
```

### 13.2 `mesh-peer` 作为 chain 前置 actor

```text
Leaf chain computes next actor endpoint
  -> MeshEgressDialer.dial_tcp(peer, next_actor_endpoint)
  -> returned stream passed to next Leaf actor
```

### 13.3 错误和 fallback

- peer 不存在或 selector 不唯一：建连前失败。
- peer 未广告 egress capability：建连前失败。
- peer 拒绝授权：明确 permission error。
- 远端 DNS 失败：明确 resolve error。
- 远端 TCP connect 失败：返回可分类 connect error。
- timeout：必须有确定上限并可取消。
- Leaf fallback 可根据这些错误尝试下一个成员。
- 不得在失败时静默改为本机 DIRECT。

### 13.4 背压和关闭

- 双向 relay 必须传播 half-close。
- caller cancel 必须撤销远端 flow。
- peer 断线必须及时唤醒本地 stream。
- 网络代际切换后不能继续复用旧 egress lease。
- 每个 flow 的任务、FD、缓冲区和统计项必须在关闭后回基线。

## 14. UDP 语义候选

- `mesh-peer` UDP 不依赖用户 SOCKS5 的 `udp: true`。
- association 固定 selected peer，不因单个 datagram 临时切换 peer。
- peer 网络代际改变后撤销旧 association。
- UDP 无响应本身不能证明路径失效。
- 第一版不要求通过首个 UDP 包失败自动触发 fallback。
- 用户接受 UDP 实际不可用时直接失败。
- fallback 对 UDP 的可用性只能根据 actor capability、association 创建错误或明确控制面错误判断。
- 不能因为一个 SOCKS actor 配置 `udp: true` 就认为远端真实可用。

UDP association 至少需要：

- requester peer identity。
- selected egress peer identity。
- association ID。
- source/destination mapping。
- idle timeout。
- maximum datagram size/MTU 行为。
- network generation。
- cancellation and cleanup。
- per-source quotas。

## 15. DNS 语义候选

### 15.1 基本原则

- DIRECT 匹配的域名通过 DIRECT resolver 路径解析。
- `mesh-peer` 匹配的域名通过该 peer 对应的 resolver 路径解析。
- `mesh-peer -> socks-node` chain 中，下一 SOCKS 节点 hostname 应通过前置 peer 路径解析，除非该代理协议明确要求把域名交给更后面的节点。
- DNS 不得因 UDP 不支持而自动走错误出口。
- DoH、DoT、TCP DNS 应当可以通过 selected peer 使用。

### 15.2 缓存隔离

缓存键至少应包含：

```text
resolver identity
+ selected egress peer identity
+ network generation
+ qname
+ qtype
```

不能把 DIRECT 解析结果无条件复用到 peer egress，也不能把 peer A 的结果复用到 peer B。

### 15.3 域名携带

内部 egress request 应能携带 domain destination，而不是强制本地预解析为 IP。候选行为：

- 远端 peer 使用其受控 resolver 解析。
- 或通过 selected peer 访问配置的 DoH/DoT/TCP resolver。
- bootstrap resolver 必须有明确路径，避免递归依赖。
- resolver 不可用时 fail-closed，不得静默改成本机系统 DNS。

### 15.4 TUN 输入限制

如果应用已经自行解析域名并只向 TUN 发送目标 IP，系统无法凭空恢复域名。完整按域名出口解析可能需要：

- Leaf DNS 接管。
- DNS sniff/cache correlation。
- FakeDNS/fake-IP。

不得宣称所有 TUN IP flow 都天然具备远端域名解析语义。

## 16. `enable_exit_node` 与授权妥协

### 16.1 为什么传统开关不应直接成为新功能 UX

传统 `enable_exit_node` 表示节点允许承担较宽泛的全局出口行为。候选 `mesh-peer` 是一个已认证成员明确指定目标 peer 的按 flow egress 请求。

两者权限边界不同。普通用户选择 `mesh-peer` 后还必须登录远端手工打开传统开关，UX 不合理，也会让用户误解新功能依赖传统全局 exit-node 配置。

### 16.2 候选拆分

保留传统行为：

```yaml
enable-exit-node: true
```

它继续控制传统全局 exit-node/未知目标路由行为。

新增独立内部能力，候选名称：

```yaml
policy-egress:
  mode: trusted-network
```

普通私有网络用户不一定需要显式配置该段；服务可以懒启动，并通过 capability 广告。

### 16.3 候选默认授权

- 有密码、已认证的私有 EasyTier 网络：允许同网络成员明确指定本 peer 作为公网出口。
- 无密码或公开网络：默认禁止。
- foreign network：默认禁止。
- 默认只允许公网目标。
- 默认拒绝 loopback、link-local、RFC1918、mesh CIDR 和管理地址。
- 访问 peer 所在 LAN CIDR 必须单独授权。
- 用户可显式设置 `disabled`。
- 用户可配置 requester peer allowlist。
- 每个 requester 必须有并发、UDP association 和带宽限制。

这是安全妥协，不是零风险默认。私有网络成员使用其他 peer 的公网 IP、带宽和信誉仍然是额外权限，最终默认值尚未由用户决定。

### 16.4 懒启动不等于无授权

候选 egress gateway 可以总是具备按需启动能力，但每个请求仍必须：

- 来自已认证 Mesh peer。
- 明确指定当前 peer。
- 通过来源 peer ACL。
- 通过 destination ACL。
- 通过资源 quota。
- 记录可审计的 requester、目标和结果。

不得因为“不需要手动开开关”就无条件允许任意网络成员借用出口。

## 17. EasyTier 2.9.10 兼容边界

### 17.1 不启用 Leaf/policy 时

- 新功能不得改变默认 EasyTier 数据路径。
- 不得增加常驻出口服务资源。
- 不得改变旧配置解析和旧 peer 路由。
- 2.9.10 行为和性能应保持不变。

### 17.2 新客户端连接旧 peer

旧 peer 不理解候选 `policy-egress-v1` capability，因此：

- 若旧 peer 已开启传统 `enable_exit_node`，可考虑使用现有 exit-node packet path 作为兼容后端。
- 若旧 peer 未开启传统 exit node，只能使用该 peer 上已存在的用户 SOCKS 服务。
- 新客户端不能绕过旧 peer 的授权决定。
- 新客户端必须明确报告 capability unavailable 或 requires legacy exit-node。
- 不得静默降级为 DIRECT。

### 17.3 新 peer 与旧客户端

- 旧客户端不知道 `mesh-peer` actor，不会主动使用新 egress service。
- 新 peer 的按需 service 不应影响旧客户端。
- 传统 SOCKS、传统 exit-node 和普通 mesh 路由继续按旧语义运行。

### 17.4 Wire compatibility 候选

两种候选兼容策略：

- 尽量复用现有 `EXIT_NODE` packet flag，仅增加发送端显式 peer pinning。
- 新增 `policy-egress-v1` service capability，并保留旧 exit-node compatibility backend。

前者旧 peer 兼容更好，后者权限和跨平台服务边界更清晰。是否组合使用尚未决定。

## 18. 性能评估

### 18.1 预期层级

| 方案 | 预期吞吐 | 预期延迟 | CPU/资源 | 备注 |
|---|---:|---:|---:|---|
| 用户 SOCKS over Mesh | 中 | 较高 | 较高 | 两端 SOCKS/relay，UDP ASSOCIATE |
| 跨平台 MeshEgressDialer | 高 | 中低 | 中 | 无用户 SOCKS，仍是用户态 flow |
| 现有 exit packet fast path | 高 | 低到中 | 中 | 需验证不同平台 gateway |
| Linux kernel TUN/NAT | 最高 | 最低 | 较低 | 不适合作为全平台公共语义 |

### 18.2 Android 候选路径

Android in-process Leaf 可以直接把 external handler 连接到 `MeshEgressDialer`，避免本机 TCP SOCKS bridge：

```text
Leaf external handler
  -> EasyTier DataPlane
  -> selected peer
```

### 18.3 Linux 首版候选路径

为了保留 Leaf worker 崩溃隔离，首版可以保留本机随机 bridge 作为 IPC adapter：

```text
Leaf worker
  -> local adapter
  -> MeshEgressDialer
  -> selected peer
```

该本机 adapter 的端口是内部临时资源，不进入用户 schema，也不连接远端用户 SOCKS。

只有 benchmark 证明本机 adapter 是主要瓶颈后，才考虑：

- Unix domain socket/专用 IPC。
- 静态 external handler worker protocol。
- 受控 in-process 模式。
- native stream fast path。

不能仅凭理论性能取消已经验证的 Linux worker 崩溃隔离。

### 18.4 不得提前作出的性能结论

- 不能把用户态 DataPlane 宣称为等价于 Linux kernel NAT。
- 不能仅根据移除 SOCKS 握手宣称吞吐提升比例。
- 不能假设 Android in-process 结果等于 Linux worker。
- 不能忽略 UDP packet size、copy、task 和 association 成本。
- 不能用短连接延迟代替长流吞吐和资源基线。

## 19. 如用户决定实施后的性能验证矩阵

该矩阵当前只是候选，不得在用户未决定时执行。

- TCP 单连接吞吐：DIRECT、当前 mesh SOCKS、新 mesh-peer。
- TCP 多连接吞吐：1/8/32/128 并发。
- TCP 小包延迟和短连接建立时间。
- UDP PPS、吞吐、loss、乱序和 MTU 边界。
- chain：`mesh-peer -> socks5` 与当前 `socks5 via:mesh`。
- fallback：peer 不存在、peer 离线、远端 connect refused、timeout。
- DNS：DIRECT、peer DoH、peer DoT、TCP DNS、network generation replacement。
- Android Wi-Fi 断开时同时保留 Wi-Fi 打开，以便 wireless ADB 恢复后继续验证。
- Linux worker crash/restart。
- RSS、FD、线程、任务、DataPlane route、UDP association 回基线。
- 2.9.10 old peer interoperability。
- Windows/macOS 后续至少进行协议和生命周期 parity 验证。

## 20. 开发难度和风险拆分

| 工作项 | 难度 | 风险 |
|---|---:|---|
| `mesh-peer` schema/UI/docs | 低 | 与现有 `via:mesh` 混淆 |
| durable peer selector/capability | 中 | peer churn、重复 identity、IP 变化 |
| DataPlane explicit peer egress | 中高 | 路由 pinning、防环、backpressure |
| TCP external outbound | 中 | half-close、cancel、fallback error |
| UDP association | 中高 | lifecycle、MTU、网络代际、资源增长 |
| per-egress DNS | 高 | 泄漏、缓存污染、bootstrap recursion |
| Linux worker adapter | 中 | crash isolation、IPC cleanup |
| Android direct handler | 中 | runtime ownership、network callback |
| Windows/macOS 后续接入 | 中 | 平台生命周期和打包，不应改变语义 |
| legacy exit-node compatibility | 中 | capability negotiation、错误可观察性 |
| policy egress ACL | 中高 | 出口滥用、LAN 探测、带宽和审计 |

## 21. 不推荐作为公共基础的方案

### 21.1 隐藏固定 SOCKS 端口

把远端固定端口从 YAML 隐藏起来仍然是 SOCKS 服务架构，没有解决协议层次和用户态 relay 问题。

### 21.2 为每个 peer 创建 TUN

会引入接口数量、MTU、route ownership、移动端限制和清理复杂度，且 chain 仍需要 L4 dialer。

### 21.3 把所有平台改为 in-process Leaf

会牺牲 Linux 已验证的 worker crash isolation，也不能解决 Windows/macOS 所有生命周期问题。

### 21.4 在 EasyTier 中重新实现 Leaf chain/fallback

会形成双重状态机，破坏首匹配、actor order、错误传播和 failover 语义。

### 21.5 把传统 `enable_exit_node` 默认无条件打开

会允许网络成员借用任意 peer 的公网 IP、带宽和信誉，并增加 LAN 探测风险。若取消手动开关，必须以更窄的 authenticated policy-egress ACL 代替。

## 22. 解耦边界候选

### `easytier-policy`

- 配置 schema 和验证。
- Leaf config compilation。
- 通用 `MeshEgressDialer` trait 或 adapter contract。
- Leaf external handler adapter。
- 不依赖具体 PeerManager 实现。

### `easytier`

- peer selector 解析。
- capability negotiation。
- DataPlane explicit-peer implementation。
- egress service lifecycle。
- TCP/UDP socket backend。
- ACL、quota、metrics、network generation。

### Leaf fork

- 仅提供通用静态 external outbound handler 注入。
- 保留原有 chain/fallback/first-match 行为。
- 不出现 EasyTier 配置、peer identity 或 Mesh wire details。

### UI

- 清楚区分 `mesh-peer` 与 `socks5 via:mesh`。
- `mesh-peer` 不显示 port 字段。
- capability unavailable 和 permission denied 必须可见。
- 不把传统 exit-node 开关描述为新 actor 的必需配置。

## 23. 如用户决定实施后的候选阶段

以下阶段当前全部处于未批准状态。

### 阶段 A：语义和 contract

- 决定 actor 名称、selector、DNS 和授权默认值。
- 记录 Mihomo/Leaf parity semantics。
- 添加 schema/compilation tests。
- 不改变数据面。

### 阶段 B：Linux/Android portable egress

- 实现 platform-neutral egress contract。
- Android direct handler。
- Linux worker adapter。
- TCP、UDP、DNS、cleanup 定向验证。

### 阶段 C：旧 peer 兼容

- capability negotiation。
- legacy exit-node backend。
- 2.9.10 fail-closed interoperability。

### 阶段 D：Windows/macOS

- 接入现有虚拟网卡和 Leaf runtime 生命周期。
- 不改变 schema、chain 或 wire semantics。
- 验证安装、休眠、网络切换和退出清理。

### 阶段 E：性能优化

- 依据 profiling 决定是否需要 native stream mux。
- 依据平台决定是否增加 packet/kernel fast path。
- fast path 必须与 portable backend 行为一致，并可安全回退。

## 24. 尚未决定的问题

- 是否实现 `mesh-peer`。
- actor 最终名称是 `mesh-peer`、`easytier-peer` 还是其他名称。
- 是否复用现有 exit-node packet path 作为首版数据面。
- 是否先实现独立 portable egress service。
- 内部服务使用 reserved virtual port 还是新 service ID。
- Leaf fork 是否接受静态 external handler 注入扩展。
- Linux 首版是否继续使用 loopback bridge 作为 worker adapter。
- `policy-egress` 在有密码私有网络中是否默认允许。
- 默认是否只允许公网目标。
- peer LAN CIDR 如何授权。
- 旧 peer 缺少 capability 时是否允许显式 legacy mode。
- per-egress DNS 第一版支持哪些协议和 bootstrap 方式。
- 是否首版公开 UDP chain，还是先只支持 standalone UDP exit。
- 是否在第一版 UI 中显示高级 actor，还是先仅允许配置文件使用。
- Windows/macOS 的首个支持版本和验证门槛。

## 25. 决策前硬性停止条件

在用户明确决定实施前：

- 不新增 `mesh-peer` schema。
- 不修改当前 `via: mesh` 语义。
- 不删除当前用户 SOCKS over Mesh 能力。
- 不改变传统 `enable_exit_node` 默认值。
- 不修改 Leaf fork。
- 不新增 wire protocol/service ID。
- 不触发构建、workflow 或远程验证。
- 不把本文候选方案写成已发布功能文档。

用户明确决定后，实施前仍必须再次检查对应 Mihomo/Leaf/sing-box 行为，并把最终选择、差异原因、失败行为和测试名称记录到实施 ledger。

## 26. 代码和参考索引

### EasyTier 当前实现

- `easytier-policy/src/config.rs`：`Proxy`、`ProxyKind`、proxy/group validation。
- `easytier-policy/src/leaf_config.rs`：Leaf outbound 和 chain/failover config compilation。
- `easytier-policy/src/inprocess.rs`：Android/in-process Leaf runtime ownership。
- `easytier-policy/src/leaf_process.rs`：Linux Leaf worker lifecycle。
- `easytier/src/instance/virtual_nic.rs`：policy runtime、mesh endpoint resolution、network recovery。
- `easytier/src/policy_proxy/mesh_socks_bridge.rs`：当前本机 SOCKS bridge 和 Mesh TCP relay。
- `easytier/src/policy_proxy/mesh_udp_relay.rs`：当前远端 SOCKS UDP ASSOCIATE relay。
- `easytier/src/gateway/socks5/dataplane.rs`：`DataPlaneTcpStream`、`DataPlaneUdpSocket`、connect/bind API。
- `easytier/src/peers/peer_manager.rs`：exit-node selection、packet target 和 header marking。
- `easytier/src/tunnel/packet_def.rs`：`is_exit_node`、`set_exit_node`。
- `easytier/src/gateway/tcp_proxy.rs`：`NatDstTcpConnector`、exit-node TCP gateway。
- `easytier/src/gateway/udp_proxy.rs`：exit-node UDP NAT/gateway。

### Leaf

- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/app/outbound/plugin.rs`：external outbound interfaces。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/app/outbound/manager.rs`：outbound construction 和 plugin loading。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/proxy/chain/outbound/stream.rs`：TCP chain actor order 和 next destination。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/src/proxy/chain/outbound/datagram.rs`：UDP chain transport selection。
- `/Users/fanli/.cargo/git/checkouts/leaf-6b5c4f97d0d0212b/1d20301/leaf/tests/test_out_chain_1.rs`：chain externally observable behavior。

### Mihomo

- `/Users/fanli/Documents/mihomo-rev/component/proxydialer/byname.go`：named proxy dialer。
- `/Users/fanli/Documents/mihomo-rev/component/proxydialer/proxydialer.go`：TCP/UDP through selected proxy。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/base.go`：dialer proxy composition。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/direct.go`：DIRECT TCP/UDP dial semantics。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/openvpn.go`：L3 outbound 暴露 dialer，而不是用户 SOCKS endpoint。
- `/Users/fanli/Documents/mihomo-rev/adapter/outbound/wireguard.go`：跨 tunnel outbound 的 dial/listen packet 模型。

## 27. [用户未决定] 内建 UDP SOCKS 作为近期妥协方案

本节记录用户后续提出的近期妥协，不表示已经决定实施：

> 在每个候选出口 peer 内建一个支持 UDP ASSOCIATE 的 SOCKS5 服务，只允许通过该 peer 的 EasyTier virtual IP 访问。Leaf 继续使用已经完成的 mesh SOCKS actor、chain 和 fallback，长期再决定是否替换成无 SOCKS 的 `MeshEgressDialer`。

### 27.1 已确认的验证事实

- 当前 policy UDP 合格基线确实使用目标 peer 上的外部 GOST SOCKS5 actor。
- EasyTier 当前内建 `--socks5` 已经是 Rust/Tokio 实现，不是 Go 实现。
- 当前内建服务使用 vendored/modified `fast-socks5` 代码处理 SOCKS5。
- 当前 EasyTier 明确没有启用它的 UDP ASSOCIATE；收到该命令返回 SOCKS reply `0x07`。
- 已验证的 GOST actor 在 qualified buffer 配置下通过 TCP、UDP、认证、错误密码、actor kill/restart 和 association cleanup 检查。
- 验证记录显示 TCP 已达到约 492 Mbit/s 且无重传；UDP 损失与 socket receive buffer、burst 和 association 数据路径密切相关，不能仅根据实现语言推断性能。

参考：

- `easytier/docs/policy_proxy_validation_2026_07_13.md`：GOST UDP ASSOCIATE、buffer、kill/restart、120 秒回收和性能矩阵。
- `easytier/docs/socks5_performance_investigation_cn.md`：EasyTier 内建 SOCKS TCP 路径和性能边界。
- `easytier/src/gateway/fast_socks5/README.md`：当前 vendored Rust SOCKS 来源。
- `easytier/src/gateway/socks5.rs`：当前 EasyTier SOCKS portal、DataPlane 和生命周期。
- `easytier/src/gateway/fast_socks5/server.rs`：当前 UDP ASSOCIATE 实现。

### 27.2 不能直接打开当前 `allow_udp`

当前 vendored Rust UDP ASSOCIATE 代码虽然存在，但不能仅调用 `set_udp_support(true)` 后发布。已确认的问题包括：

- association UDP socket 当前绑定 `[::]:0`，不是 EasyTier virtual IP。
- 代码明确说明没有使用客户端在 UDP ASSOCIATE 中提供的地址限制访问。
- 第一个能够向随机 UDP 端口发送数据的来源可能抢占 association client endpoint。
- UDP relay 没有与 TCP control connection EOF/close 建立清晰的并发 ownership。
- control connection 关闭后，当前 `transfer_udp` 没有独立监控任务保证立即取消 UDP relay。
- 每个 association 建立两个 65,536-byte heap buffer，缺少全局和 per-peer association 限额。
- 域名通过本机通用 `ToSocketAddrs` 解析，没有 policy egress resolver identity 和网络代际隔离。
- 没有完整定义虚拟 IP 变化、peer 断线、网络切换、shutdown 和 late task cleanup。
- 没有把实际 socket buffer、drop counter 和 association ownership 纳入验收。

因此“使用 Rust”不能自动保证 UDP 生命周期完善。需要把 UDP association 作为 EasyTier 自己拥有的资源状态机，而不是依赖第三方 crate 默认 relay loop。

### 27.3 候选公共配置仍应使用 `mesh-peer`

即使首版内部以 SOCKS5 实现，也不建议把内部动态端口暴露成用户 contract。候选配置仍为：

```yaml
peer-kr:
  type: mesh-peer
  peer:
    instance-id: peer-instance-id
```

内部实现可以暂时解析为：

```text
mesh-peer actor
  -> selected peer advertised built-in policy SOCKS endpoint
  -> SOCKS5 TCP CONNECT / UDP ASSOCIATE
```

这样以后把 backend 替换为 `MeshEgressDialer` 时，不需要修改用户配置和 group/rule 引用。

用户明确配置的外部服务继续使用：

```yaml
peer-gost:
  type: socks5
  server:
    instance-id: peer-instance-id
  port: 1080
  via: mesh
```

### 27.4 监听地址候选

内建 policy SOCKS 不应监听 `0.0.0.0` 或 `[::]`。候选要求：

- IPv4 TCP control listener 只绑定当前 EasyTier IPv4 virtual address。
- IPv6 listener 只绑定当前 EasyTier IPv6 virtual address，并显式设置 IPv6-only 行为。
- UDP association socket 只绑定对应 virtual address。
- virtual address 尚未就绪时不启动 listener。
- virtual address 变化时关闭旧 TCP listener 和全部 UDP associations，再绑定新地址并更新 capability。
- listener bind 失败时 fail-closed，不回退到 wildcard。

只绑定 virtual IP 能显著减少 LAN/WAN 暴露，但不能称为绝对“没有安全问题”：同一 Mesh 中能到达该 virtual IP 的成员仍可能访问服务。无密码公开网络和 foreign network 不应默认开放；私有网络仍需要来源 peer ACL、认证或等价的受控授权。

### 27.5 不建议使用固定默认端口

若内建服务固定使用 1080 或其他端口，会与以下 listener 冲突：

- 已存在的 `0.0.0.0:PORT` TCP listener。
- 已存在的 exact virtual IPv4 `VIRTUAL_IP:PORT` listener。
- 已存在的 `[::]:PORT` listener；是否同时占用 IPv4 取决于平台和 `IPV6_V6ONLY`。
- 同一主机上的另一个 EasyTier instance。
- 用户自己启动的 gost/SOCKS 服务。

`SO_REUSEADDR` 或 `SO_REUSEPORT` 不应被用来掩盖 ownership 冲突。不同平台对 wildcard、IPv4-mapped IPv6 和 reuse 的行为不完全一致。

近期妥协的首选方案是：

- TCP control listener 原子地绑定 `virtual-ip:0`，由 OS 或 EasyTier user-space stack选择可用端口。
- listener 保持打开，不采用“先探测端口、关闭、再绑定”的 TOCTOU 流程。
- 通过经过认证的 peer capability 广告实际 endpoint 和 UDP 支持版本。
- UDP ASSOCIATE 单独绑定 `virtual-ip:0`，在 SOCKS reply 返回实际 UDP 端口。
- source 根据 capability generation 更新 endpoint；peer 重启后端口允许变化。
- 如未来提供手动固定端口，高级配置 bind 失败必须阻止该 capability 广告并报告明确冲突。

使用动态端口后，现有 wildcard listener 仍可能占用部分端口，但内核/用户态 stack 可选择另一个无冲突端口；不需要猜测 `0.0.0.0` 或 `[::]` 的占用情况。

### 27.6 OS socket 与 EasyTier user-space listener

两种内部 listener backend 尚未决定：

| Backend | 优点 | 缺点 |
|---|---|---|
| OS socket 绑定 exact virtual IP | 改动较小，可复用当前 portal | 受平台地址 ownership、wildcard listener 和 VPN/utun/Wintun 行为影响 |
| EasyTier smoltcp/DataPlane 内部 listener | 不占 OS wildcard port，平台行为更一致，天然只在 Mesh 数据面 | 需要让 SOCKS UDP relay 使用抽象 datagram socket，并补充 internal service ownership |

从长期全平台角度，EasyTier user-space listener 更干净；从 Linux/Android 首版改动量看，exact virtual-IP OS listener 更省事。公共 schema 和 capability 不应暴露 backend，以允许后续替换。

### 27.7 UDP association 必需生命周期

若用户决定采用该妥协，Rust 内建 UDP 必须至少满足：

- 每个 UDP association 必须由对应 TCP control connection 唯一拥有。
- TCP EOF、RST、认证失败、worker/core shutdown 必须立即取消 association。
- TCP control 与 UDP relay 必须使用同一个 cancellation token/owner guard。
- UDP client endpoint 必须按 RFC 请求地址或第一个合法数据包锁定，锁定后拒绝其他来源。
- 来源必须与建立 TCP control 的 Mesh requester 一致；不能只依赖可伪造的 UDP source address。
- virtual IP/network generation 变化必须取消全部旧 association。
- peer route 消失必须有有界清理。
- association 必须有 idle timeout；候选基线可参考已验证 GOST 的 120 秒回收窗口，但最终值未决定。
- 必须限制每 peer、每 instance 和全局 association 数量。
- UDP socket receive/send buffer 必须设置并记录内核实际授予值。
- relay queue 必须有界，drop 必须计数，不允许无限内存增长。
- FRAG 非零在不支持 fragmentation/reassembly 时按 RFC 行为丢弃并计数。
- remote response 必须映射回正确 association 和目标，不允许跨 association 注入。
- DNS cache 必须按 egress identity 和 network generation 隔离。
- 所有 task、socket、map entry、buffer 和 metrics label 必须在 association 结束后回基线。
- actor kill/restart、Leaf kill/restart、Wi-Fi 切换和 peer IP 变化必须重新建立新 association，而不是复用旧状态。

### 27.8 是否更换 Rust SOCKS crate

当前 EasyTier 内建 SOCKS 已经是 Rust。是否换 crate 不能只看语言，需要先比较：

- UDP control-channel ownership。
- client endpoint pinning。
- IPv4/IPv6 exact bind。
- async cancellation。
- bounded buffers and quotas。
- domain resolver injection。
- custom TCP connector 和 EasyTier DataPlane integration。
- Android、Windows、macOS socket 行为。
- 项目维护状态、license 和 unsafe/FFI 边界。

候选优先级：

1. 保留当前经过使用的 TCP parser/connector，只重写 EasyTier-owned UDP association lifecycle。
2. 若另一个 Rust 实现已经具备上述 ownership，并允许注入 connector/resolver/socket backend，再做隔离替换 spike。
3. 不应复制一个大型代理项目只为获得 UDP ASSOCIATE。
4. 不应仅根据 microbenchmark 替换当前 TCP path。

Rust 内建实现的主要收益是单一制品、统一生命周期、跨平台打包、metrics 和 capability 集成；性能提升需要实测，不能预设一定快于 GOST。

### 27.9 该妥协的当前评价

| 维度 | 评价 |
|---|---|
| 首版开发量 | 明显低于完整 `MeshEgressDialer`/Leaf external outbound |
| 复用当前 Leaf chain/fallback | 高 |
| 用户配置复杂度 | 使用动态 endpoint/capability 后低 |
| TCP 性能 | 预计接近当前合格路径，是否更快需实测 |
| UDP 风险 | 仍是主要工作，不能直接启用现有 crate 实现 |
| 全平台可演进性 | 公共 schema 不暴露 SOCKS backend 时可接受 |
| 长期优雅程度 | 低于原生 egress dialer，但可作为明确的临时 backend |

当前最省心且不锁死长期设计的候选是：

> 对用户公开无端口 `mesh-peer` actor；peer 内部按 virtual IP 动态绑定并广告一个 EasyTier-owned Rust SOCKS endpoint；继续复用 Leaf chain/fallback；严格重写 UDP association ownership；以后允许无配置迁移到原生 `MeshEgressDialer`。

该候选仍须用户明确决定后才可实施。
## 28. 过渡方案收窄：原样 Rust SOCKS sidecar（用户未决定是否实现）

### 28.1 用户确认的边界

- 这是替代手工部署 GOST 的过渡方案，不是新的安全边界。
- SOCKS 可供能够进入 mesh/内网的用户使用；不为 sidecar 另加 ACL、认证或暴露面设计。
- 不把 UDP ASSOCIATE、UDP 会话状态、超时和回收逻辑重写进 EasyTier。
- EasyTier 只负责 sidecar 的启动、停止、崩溃重启、端口选择和实际 endpoint 发布。
- sidecar 保持独立进程，不与 Leaf、policy worker 或 EasyTier 内置 TCP-only SOCKS 融合。

### 28.2 现有 Rust 项目调查

首选候选为 `cfal/shoes`（MIT）：

- 上游仓库：<https://github.com/cfal/shoes>
- 配置语义：<https://github.com/cfal/shoes/blob/master/CONFIG.md>
- SOCKS 请求处理：<https://github.com/cfal/shoes/blob/master/src/socks_handler.rs>
- UDP relay：<https://github.com/cfal/shoes/blob/master/src/socks5_udp_relay.rs>
- 它是可独立运行的 Rust 代理服务端，SOCKS5 明确支持 `UDP ASSOCIATE`，可绑定指定 IP，direct routing 可直接使用默认 `allow-all-direct`。
- `handle_udp_associate` 在 TCP 监听地址相同的 IP 上以端口 `0` 创建每关联 UDP socket，使用 2 MiB socket buffer，并向客户端返回实际临时 UDP endpoint。
- `run_udp_associate` 用 `tokio::select!` 同时运行 UDP routing 和 TCP-close monitor；SOCKS TCP 控制连接结束时，UDP association 随即结束。这符合 RFC 1928 的关联生命周期，不需要 EasyTier 重写。
- `Socks5UdpRelayStream` 只接受首个 UDP 数据报学习到的客户端地址；shutdown 会 abort socket reader task。
- UDP fragment 明确不支持并返回错误；这与常见 SOCKS5 实现一致，但必须纳入与 GOST 的对照验证，不能仅凭源码宣称达到 GOST 等级。

排除的候选：

- `tokio-socks` 是 SOCKS client/library，不是可直接运行的 SOCKS server sidecar。
- `rusty_socks` 的 UDP ASSOCIATE 仍是未完成项。
- `rust-socksd` 虽宣称完整 SOCKS5，但公开说明没有提供足够 UDP ASSOCIATE 证据。
- `shadowsocks-rust` 的 `sslocal` SOCKS UDP 生命周期成熟，但它是 Shadowsocks client，需要远端 Shadowsocks server，不是 direct SOCKS-to-Internet sidecar。

因此当前调查中，`shoes` 是唯一与“Rust 原生、独立 sidecar、direct 出口、TCP+UDP SOCKS5”直接吻合的候选。它是否达到 GOST 的实际吞吐、丢包和长期回收水平，必须复用现有 GOST 验证矩阵实测后才能确认。

### 28.3 最小集成设计

sidecar 使用生成的最小配置，监听 EasyTier virtual IPv4，而不是改写其 SOCKS 实现：

```yaml
- address: "<virtual-ip>:29980"
  protocol:
    type: socks
    udp_enabled: true
  rules: allow-all-direct
```

- 以 `--no-reload` 启动，让 EasyTier supervisor 独占配置和生命周期。
- 固定尝试三个候选 TCP 监听端口，例如 `29980`、`29981`、`29982`。
- 不采用“先探测端口、释放、再启动”的方式，避免 probe/release race；直接逐个生成单端口配置并启动 child，以 sidecar 的真实 bind 结果为准。
- 某个端口因 `0.0.0.0:port`、`[::]:port` 或同 IP listener 冲突而启动失败时，终止该 child 并尝试下一个端口。
- 三个端口都失败时，只将该 peer 的内置 SOCKS endpoint 标记为不可用；EasyTier mesh、现有用户 SOCKS 和 policy worker继续运行。
- 成功后发布的是实际 `<peer-virtual-ip>:<selected-port>`；调用端不硬编码默认端口。
- virtual IP 变化、EasyTier stop 或实例重启时，停止旧 child、删除临时配置，并在新 IP 上重新执行三端口选择。
- `shoes` 的 UDP ASSOCIATE 使用临时 UDP 端口，所以三端口回退解决的是 SOCKS TCP control listener 冲突；每个 UDP association 的数据端口由 sidecar 自行管理。

### 28.4 引入方式

- 首轮应 pin 一个经过验证的 `shoes` commit，按上游原样构建 sidecar，不先 fork 和删功能。
- MIT vendoring 必须保留 license 和 attribution。
- 先以独立二进制随 EasyTier 制品一起分发；若体积或跨平台依赖不可接受，再决定是否 vendor/fork 并裁剪无关协议。
- 目标是减少实现和维护成本，以及统一 Rust 构建链；没有实测前不得宣称它比 GOST 更快。

### 28.5 决策前验证门槛

- Linux、Android 能构建和启动 pinned `shoes` sidecar；后续设计仍须评估 Windows/macOS 打包。
- 完整复用 GOST 基线：TCP throughput/retransmit、UDP 10/20/50/100 Mbit、突发 buffer、120 秒 association cleanup、sidecar kill/restart。
- 独立验证三个 TCP 端口中前两个冲突时可落到第三个，并且发布的 endpoint 是第三个端口。
- 验证 SOCKS TCP 控制连接关闭后 UDP socket、task、FD 和 RSS 回基线。

状态：**用户未决定是否实现；当前推荐先做 `shoes` 原样 sidecar 的小型资格验证，不在 EasyTier 内重写 UDP SOCKS。**
## 29. Rust SOCKS5 sidecar 扩大候选调查（用户未决定是否实现）

用户要求：不要预设 `shoes`；先扩大搜索并比较可直接作为 sidecar 的现成 Rust 项目，只有没有更优选择时才考虑 `shoes`。

### 29.1 筛选硬条件

- 独立可执行 SOCKS5 server，不是仅有 client/library。
- 同时支持 TCP `CONNECT` 和标准 UDP `ASSOCIATE`。
- direct 出口，不要求配套的 Shadowsocks/Trojan/自定义远端 server。
- 可绑定 EasyTier virtual IP；TCP listener 端口冲突可由 EasyTier supervisor 顺序重试。
- UDP association 生命周期至少绑定 TCP control connection；不能留下永久 UDP task/socket。
- 第一版需要评估 Linux、Android；设计上不能排除 Windows、macOS。
- sidecar 可原样 pin commit 运行，不要求融合进 EasyTier。

### 29.2 第一梯队：应先于 `shoes` 做资格验证

#### A. `ZingerLittleBee/next-socks5`

- 仓库：<https://github.com/ZingerLittleBee/next-socks5>
- UDP 实现：<https://github.com/ZingerLittleBee/next-socks5/blob/main/src/server/udp.rs>
- 性能说明：<https://github.com/ZingerLittleBee/next-socks5/blob/main/docs/PERFORMANCE.md>
- MIT、纯 Rust、无 C 依赖；支持 headless build，官方说明静态 musl binary/container 约 3.5 MB。
- 明确支持 TCP CONNECT、UDP ASSOCIATE、IPv4/IPv6/domain、source-IP/full endpoint filtering、UDP idle reclaim、graceful shutdown。
- association 绑定 TCP control connection；`tokio::select!` 同时监视 control EOF、UDP receive idle timeout 和全局 shutdown。
- 单次 UDP `send_to` 有 1 秒上限，避免 socket backpressure 阻塞 control EOF cleanup。
- DNS cache 有 30 秒 TTL 和 256 项上限；known targets 有上限；UDP relay port 可使用显式 range。
- 有可复现 TCP benchmark；上游报告单机约 2 GB/s、约 6k CPS，但没有 GOST 对照和 UDP loss/pps benchmark。
- 截至调查时约 151 commits、7 个 release，最新 v0.5.0（2026-07-07）；功能设计比 `socks5-rs` 完整，但项目仍很新。
- 官方现成制品只明确覆盖 Linux musl x86_64/aarch64；Android、Windows、macOS 构建尚未证明。

初步判断：**最符合“小而完整的过渡 sidecar”，当前应作为第一验证候选。**

#### B. `madeye/meow-rs`

- 仓库：<https://github.com/madeye/meow-rs>
- SOCKS inbound：<https://github.com/madeye/meow-rs/blob/main/crates/meow-listener/src/socks5.rs>
- UDP relay：<https://github.com/madeye/meow-rs/blob/main/crates/meow-listener/src/socks5_udp.rs>
- UDP round-trip：<https://github.com/madeye/meow-rs/blob/main/crates/meow-listener/tests/socks5_udp_associate.rs>
- QUIC multi-reply 回归：<https://github.com/madeye/meow-rs/blob/main/crates/meow-listener/tests/socks5_udp_multi_reply.rs>
- MIT；支持指定 `bind-address`、独立 `socks-port`、`mode: direct`。
- association 绑定 TCP control connection并按 destination 建 NAT session；control close 后销毁所有 session/reply task。
- 有 30 秒 sweep 和统一 UDP idle timeout，按 destination 回收 session；存在真实 UDP echo 和多包 server-first-flight 回归测试。
- 官方 TCP direct benchmark 报告 stripped binary 约 7.2 MB、约 5.23 Gbps，且与 mihomo 同机比较；这仍不是本项目 GOST 对照证据。
- 截至调查时约 507 commits，并持续发布 v0.8.0 至 v0.16.0；Linux、Windows、macOS/OpenWrt 已有明确运行或打包路径。
- 缺点是它本身是完整 Clash/Mihomo 风格内核，包含 DNS、规则、API、dashboard 和大量 EasyTier 不需要的功能；作为只做 direct SOCKS 的 sidecar 有明显重复。
- Android sidecar 构建未在上游公开说明中得到证明。

初步判断：**UDP 测试和发布纪律强于 `shoes`，但功能过重；若 Android 构建和 idle resource 验证通过，可优先于 `shoes`。**

#### C. ByteDance `g3proxy`

- 仓库：<https://github.com/bytedance/g3>
- UDP association task：<https://github.com/bytedance/g3/blob/master/g3proxy/src/serve/socks_proxy/task/udp_associate/task.rs>
- SOCKS client protocol 文档：<https://github.com/bytedance/g3/blob/master/sphinx/g3proxy/protocol/client/index.rst>
- Apache-2.0；企业级 forward proxy，长期维护、完整 metrics/ACL/limit/idle wheel/egress abstraction。
- UDP ASSOCIATE 不是名义支持：实现绑定 TCP control EOF、first-packet timeout、idle wheel、client endpoint locking、socket buffer/speed limits、batch recv/send、task counters 和 Drop cleanup。
- UDP batch paths源码显式包含 Linux、Android、BSD、macOS；但上游目标平台文档只把 Linux列为 fully supported，并声明 macOS/Windows/BSD 可编译，没有承诺 Android 完整 `g3proxy` 制品。
- 上游文档说明 UDP ASSOCIATE 默认关闭且只支持部分 escaper，需要生成正确 server/escaper 配置。
- 缺点是依赖和配置体系远重于单 SOCKS sidecar，构建/打包成本最高。

初步判断：**工程成熟度最接近 GOST 或更高，但不是最省心的过渡方案；作为质量上限和压力测试参考保留。**

### 29.3 第二梯队

#### `Watfaq/clash-rs`

- 仓库：<https://github.com/Watfaq/clash-rs>
- Apache-2.0，约 1,132 commits、55 个 release、约 1.7k stars。
- 官方明确列出 SOCKS inbound TCP+UDP、direct outbound、Linux/macOS/Windows/iOS。
- 缺点同 `meow-rs`：完整代理内核过重；构建依赖包括 cmake、libclang、nasm/protoc 等，Android没有明确支持声明。
- 在 sidecar 只做 direct SOCKS 的需求下，不优于 `meow-rs`；保留作跨平台和协议行为参考。

#### `0x676e67/vproxy`

- 仓库：<https://github.com/0x676e67/vproxy>
- GPL-3.0，约 430 commits、约 407 stars；明确支持 SOCKS5 CONNECT/BIND/ASSOCIATE、指定 bind 和 kernel-space zero-copy。
- 项目明显面向 Linux/CIDR 出口池和 sysctl/route 管理；未找到 Android、Windows、macOS sidecar 支持证据。
- 可作为 Linux throughput 对照，但不适合当前全平台方向的首选。

#### `jkshfanfbun/ruci`

- 仓库：<https://github.com/jkshfanfbun/ruci>
- MIT/Apache-2.0/CC0，约 1,289 commits；明确包含 SOCKS5 UDP ASSOCIATE、direct、chain 和 Android JNI。
- 上游 README 仍标记 WIP；Lua/JSON Map/chain framework 的使用和裁剪复杂度高，缺少足够 SOCKS UDP lifecycle/performance 公开证据。
- 功能过于通用，不优于第一梯队。

### 29.4 轻量但证据不足

#### `WANG-lp/socks5-rs` 2.0.0

- 仓库：<https://github.com/WANG-lp/socks5-rs>
- MIT、纯 Tokio、CLI 可指定 bind IP/port；支持 TCP CONNECT 和 UDP ASSOCIATE。
- UDP relay 用 `tokio::select!` 同时等待转发与 TCP control close，按 TCP peer IP 过滤并从首包学习 client endpoint。
- 但 UDP 支持直到 2.0.0（2026-06-25）才公开，仓库仅约 26 commits、无 GitHub release；没有 UDP idle timeout、association cap、buffer tuning和长期资源证据。
- 适合作为最小实现参考或小型对照，不应直接称为 GOST 级 sidecar。

### 29.5 已排除

- `shadowsocks-rust sslocal`：SOCKS UDP成熟，但必须连接 Shadowsocks server，不是 direct sidecar。
- `rust-socksd`：README虽称完整 SOCKS5，但源码/说明没有 UDP ASSOCIATE 证据。
- `tun2proxy/socks-hub`：没有 UDP ASSOCIATE 支持证据。
- `rusty_socks/rsocks`：公开 issue仍显示 UDP ASSOCIATE 不支持。
- `fast-socks5`、`socks5-impl`、`rama`：是 library/framework 或 example，需要自行补完整 server/lifecycle，不符合“直接原样 sidecar”。
- `tokio-socks`：client library。
- `merino`：没有 UDP ASSOCIATE 证据。
- 各类 Trojan/AnyTLS/Geph/WSTunnel client：本地 SOCKS只是专用隧道入口，需要配套远端，不是 direct出口。

### 29.6 当前排序和下一步

当前不做最终选型，资格验证顺序建议：

1. `next-socks5`：最符合小型过渡 sidecar。
2. `meow-rs`：更强的 UDP 测试/发布证据，但更重。
3. `g3proxy`：工程成熟度上限，用于判断前两者是否遗漏关键 lifecycle/性能处理。
4. `shoes`：降为后备，不再默认首选。

下一步若用户允许小型资格验证，只验证 pinned commits，不接入 EasyTier：Linux/Android build feasibility、精确 virtual-IP bind、GOST 同一 TCP/UDP matrix、control-close/idle cleanup、RSS/FD、三端口 TCP listener fallback。任何候选未通过都直接淘汰，不修改其 UDP 状态机。

状态：**用户未决定是否实现；`shoes` 已降级为后备，当前优先调查/验证 `next-socks5`、`meow-rs` 和 `g3proxy`。**

## 30. 用户未决定：Rust SOCKS sidecar 可行性验证（2026-07-15）

> **状态：用户未决定是否实现。** 本节仅记录候选资格验证，不代表已经决定集成，也没有修改 EasyTier 代码。后续一旦进入集成、Android App 打包、mesh virtual-IP 监听、端口 supervisor 或正式性能验证，禁止使用维护者本机 Mac 构建 EasyTier；应回到 profiling beta workflow、Android workflow 和精确制品实机验证路径。

### 30.1 固定候选和取舍

- `next-socks5`: `92083da82b7de3deefe3f4d6a8f11f12e13662d5`，对应 tag `v0.5.0`。MIT，纯 Rust，独立 headless SOCKS5 server，支持 TCP CONNECT、UDP ASSOCIATE、域名目标、UDP idle reclaim 和 graceful shutdown。
- `meow-rs`: `335aa55ff0d20e61ede08d93b7075382790c35f0`，当前 workspace 版本 `0.17.0`。MIT，Android socket-protect 等嵌入式基础存在，但完整 CLI 是 Clash/Mihomo 风格代理核，不是小型 sidecar。
- `shoes`: `386b11532424b8665ee3e46340c6236fb3c47595`。由于 `next-socks5` 已通过三平台初步资格验证，本轮不再编译；仅保留为后备。
- `g3`: `624be5b156d63c6e1b58a6fd423d0addb5ca81c1`。UDP 生命周期实现成熟，但 server/escaper/config 体系明显过重；保留为行为参考，不作为过渡 sidecar。

### 30.2 `next-socks5` 资格证据

#### macOS arm64 初步回环

- 使用 headless debug 构建：`cargo build --no-default-features`，Rust 1.96.1，限制 `CARGO_BUILD_JOBS=4`；首次构建约 26 秒，未编译 EasyTier。
- TCP CONNECT 成功。最初 CPS 用例在 2 秒内成功 8079 次后耗尽 Mac 临时端口；这是同机短连接压测方法导致的 `EADDRNOTAVAIL`，不是 SOCKS worker 崩溃。后续均改用单长连接。
- 两条 UDP ASSOCIATE、64-byte payload、每条 200 pps、3 秒：1200/1200 echo，0% loss；server UDP socket 活跃时为 2，控制连接结束 2 秒后为 0。
- 域名 ATYP 使用 `localhost`：200/200 echo，0% loss。
- SIGTERM 后 5 秒内退出。
- 端口占用时立即以 exit code 1 返回 `Address already in use`；supervisor 随后改用第二端口可正常启动。这满足“固定尝试 2～3 个端口”的实现前提，不需要使用或覆盖用户已有的 `7890`。

#### Android arm64 实机初步回环

- 设备：`192.168.234.227:5555`，全程保持 Wi-Fi/ADB 在线；未做断网测试、截图或模拟点击。
- 使用 NDK `29.0.14206865`、API 24 linker、`aarch64-linux-android` target 成功构建 Android PIE。必须显式固定 rustup `RUSTC`；Homebrew cargo/rustc 与 rustup target 混用会产生假的 `can't find crate for std`，该错误不是候选兼容问题。
- 二进制经 ADB 放入 `/data/local/tmp`，仅在设备 loopback 运行；未修改 VPN、路由、TUN 或 App 配置。
- TCP CONNECT 单长连接成功，`fail=0`。
- 两条 UDP ASSOCIATE：1196/1196 echo，0% loss；server FD 从 11 增至 15，控制连接结束后恢复为 11。
- 域名 ATYP 使用 `localhost`：200/200 echo，0% loss。
- SIGTERM 正常退出，日志中的每条 UDP association 均有对应 `closed`。
- **边界**：这里只证明 Android bionic/arm64 上的协议和进程可运行性。尚未证明正式 App UID、APK `nativeLibraryDir`/sidecar 打包、SELinux、VpnService ownership、冷启动和升级覆盖；这些属于后续集成验证，不能引用本节宣称完成。

#### Linux x86_64 初步回环

- 使用官方 `v0.5.0` x86_64-musl static PIE；tag 精确指向 `92083da82...`。
- GitHub asset digest 期望和实际均为 `cf3ebbf3de2f0dcd8880dd517effe3332a6cd55b08ca206364132d94cbf4db37`。
- 在 `192.168.2.160` 的 `easytier-debug-builder` 容器 `/tmp` 内运行，不编译 EasyTier、不修改路由/TUN/宿主网络。
- 单 TCP 长连接回环约 1503.2 MB/s，`fail=0`。该数字仅是资格 smoke，不是 EasyTier/Leaf 正式性能结论。
- 两条 UDP ASSOCIATE：1200/1200 echo，0% loss；server FD 从 10 增至 14，结束后恢复为 10。
- `localhost` 首次域名用例因 Debian 优先返回 `::1`、而 echo sink 仅监听 IPv4 而 100% loss；改用只解析为 `127.0.0.1` 的 `127-0-0-1.nip.io` 后 200/200 echo、0% loss。这是测试地址族不一致，不是域名 UDP relay 缺陷。
- 首个 shell 清理函数把空 PID 展开为 `kill 0`，误杀外层 `timeout` 并得到 exit 139；sidecar 已在此之前正常 SIGTERM 退出。修正清理函数后补测 exit 0。

### 30.3 `meow-rs` 对照结果

- `meow-app --no-default-features --features minimal` 在 macOS arm64 构建成功，但所谓 `minimal` 仍引入 API、DNS、rules、Shadowsocks、HTTP/WebSocket、metrics 等多个 workspace crate。首次构建约 40 秒，debug 二进制 54 MB；`next-socks5` 对应为约 26 秒、41 MB debug。
- Mac 单 TCP 长连接回环约 2239.8 MB/s；UDP IP 和域名均 0% loss；SIGTERM 正常退出。
- 五轮两 association UDP 周期后 FD 在 17/18 间波动并最终回到 17，没有逐轮增长证据。
- Android arm64 编译失败于 `crates/meow-app/src/main.rs`：CLI 无条件调用 `install_service`、`uninstall_service`、`service_status`，实现只为 Linux/macOS/Windows 编译，没有 Android 分支。
- 仓库内部确有 Android `VpnService.protect(fd)`、DNS socket factory 等嵌入式支持，因此不是核心完全不支持 Android；但要作为独立 Android sidecar，必须维护 CLI 补丁或自行编写 wrapper。对“直接拿现成项目当 sidecar”的过渡目标而言，这一维护成本使其劣于 `next-socks5`。

### 30.4 当前推荐（仍待用户决定）

1. 若决定实现过渡 sidecar，首先固定评估 `next-socks5 v0.5.0 / 92083da82...`，不要先选 `shoes`。
2. 集成保持进程边界：EasyTier 只负责生成最小配置、绑定 mesh virtual-IP、按顺序尝试 2～3 个保留端口、监督启动/退出和向 Leaf 暴露 SOCKS endpoint；不要把 SOCKS UDP 状态机重写进 EasyTier。
3. 首选端口被 `0.0.0.0:PORT`、`[::]:PORT` 或具体地址占用时，依赖 sidecar 的快速非零退出后尝试下一端口；不得占用、覆盖或假设用户的 `7890` 可用。
4. 正式接受前仍需补：Linux/Android 精确集成制品、Android App UID/SELinux/APK 打包、真实 mesh virtual-IP 远端 TCP/UDP、peer 离线/恢复、sidecar crash/restart、三端口全冲突、重复生命周期资源基线，以及 GOST 同条件性能/UDP 行为对照。
5. macOS 本轮证据只能作为候选资格筛选。后续 EasyTier 集成、构建和正式验证禁止在维护者本机 Mac 执行。

## 31. 用户未决定：`next-socks5` 解耦与全平台边界评估（2026-07-15）

> **状态：用户未决定是否实现。** 本节是架构审查，不表示已选择 `next-socks5`，也没有修改或编译 EasyTier。

### 31.1 直接结论

- 上游 `next-socks5 v0.5.0` **不能原样宣称支持 EasyTier 所有现有平台**。
- SOCKS5 TCP/UDP server 核心本身适合复用，且可以与 EasyTier/Leaf 保持清晰边界；但应该提取/裁剪为独立小 crate，而不是把上游完整 CLI、TUI、Unix admin socket 和 metrics 原样合入核心。
- 若要求“所有平台都必须是独立外部进程”，Android/OHOS 的打包和进程模型会成为不必要的高风险约束。推荐保持同一核心 API，桌面/服务器使用 sidecar process，Android/OHOS 必要时使用 in-process host；代码边界不变，只替换宿主方式。

### 31.2 已确认的平台缺口

1. **Windows 当前不能编译。** 对 `x86_64-pc-windows-gnullvm` 执行 `cargo check --no-default-features`，因 `src/admin/server.rs` 无条件使用 `tokio::net::{UnixListener, UnixStream}`、`flock`、Unix permissions/OpenOptions/FileType API 而失败。`--no-admin` 是运行时开关，不能消除编译期 Unix 依赖。
2. **MIPS 当前有确定风险。** EasyTier Core 发布 `mips-unknown-linux-musl` 和 `mipsel-unknown-linux-musl`，两者 `max-atomic-width=32`；`next-socks5` metrics 无条件使用多组 `AtomicU64`。需要移除非必要 metrics，或改用 `portable-atomic`/锁保护计数后才能进入 MIPS 矩阵。
3. **Android 只验证了 arm64/bionic 进程 smoke。** EasyTier Mobile 还发布 armv7、i686、x86_64；这些 ABI 尚未验证。正式 APK 还需处理 native executable/package UID/SELinux/升级覆盖；不能用 `/data/local/tmp` 结果代替。
4. **OHOS 当前发布的是 HAR + `libeasytier_ohrs.so`，不是可执行程序包。** 把外部 sidecar 强行加入现有 OHOS 交付形态会扩大平台集成面；同 crate 内嵌 host 更符合当前 OHOS 架构。
5. **FreeBSD、macOS x86_64、Windows i686/arm64、Linux ARM/RISC-V/LoongArch 尚无候选矩阵证据。** 纯 Rust server 核心有较高可移植性，但必须在正式 workflow 中逐目标证明，不能根据 Linux x86_64 外推。

### 31.3 Android 与 Mihomo 的差异

- Clash Meta Android 在 `TunModule.attach` 中把 `vpn::protect` 作为 `markSocket` 回调传给核心，逐 socket 绕过 VPN。
- EasyTier 当前 `TauriVpnService` 在建立 VPN 时把 `(disallowedApplications + packageName)` 全部传给 `Builder.addDisallowedApplication`，即 EasyTier 自身 package 默认整体绕过 VPN。
- 因此，同 package 的 SOCKS core/sidecar 使用普通 `TcpStream::connect`、`UdpSocket::bind` 在当前 EasyTier Android 模型下原则上不需要 Mihomo 式逐 socket protect；但这一差异必须在正式 App UID + VPN ownership + 网络切换实机矩阵中验证，不能只依赖源码推断。

### 31.4 推荐的解耦结构

```text
Leaf / policy rules
        |
        | only sees a standard SOCKS endpoint
        v
EasyTier mesh egress service
        |
        | loopback TCP/UDP + lifecycle/health contract
        v
`easytier-socks-egress-core`
        |
        +-- desktop/server host: separate `easytier-socks-egress` process
        +-- Android/OHOS host: same isolated crate hosted in native process when sidecar packaging is unsuitable
```

- `easytier-socks-egress-core` 不得依赖 EasyTier、Leaf、Tauri 或平台 UI；只拥有 SOCKS handshake、TCP relay、UDP association、DNS target resolution、limits 和 shutdown。
- EasyTier 只拥有 `EgressRuntime`/`EgressRuntimeFactory` 一类窄生命周期接口、端口选择、mesh service 发布、健康检查和退出清理。不要复用 `PolicyRuntime`：exit service 可以在本机未启用 Leaf policy 时独立运行。
- Leaf 继续只消费 `ResolvedMeshServer`/标准 SOCKS endpoint，不知道 server 是 GOST、用户 SOCKS、外部 sidecar 还是内嵌 core。
- 桌面/server 独立进程可以复用当前 Leaf worker 的 bounded start/stop、`kill_on_drop`、临时配置清理思路，但应建立独立 supervisor，避免生命周期相互绑死。
- 移动端即使采用 in-process host，也必须保持独立 crate、独立 runtime owner 和同一 health/shutdown contract，不能把 UDP association 状态散落进 EasyTier peer/policy 代码。

### 31.5 不建议 sidecar 直接绑定 mesh virtual-IP

- 直接绑定要求该 virtual-IP 在每个平台都作为可绑定的内核地址存在；Android VpnService、OHOS native library、EasyTier 用户态 data-plane 与桌面 TUN 的地址模型并不相同。
- 更稳妥的方式是 SOCKS core 只绑定 `127.0.0.1`/`::1` 的动态端口；EasyTier 使用自己的 data-plane TCP/UDP listener 把逻辑 `mesh virtual-IP:service` 发布到 mesh，再转发到 loopback SOCKS。
- 这也使用户配置不需要知道 peer 上的物理 SOCKS 端口，并避免 `0.0.0.0`、`[::]` 和用户已有 `7890` 的冲突。
- 若实现成本要求第一阶段仍直接绑定 virtual-IP，只能作为 Linux/Android 精确验证过的临时路径，不能作为全平台长期接口。

### 31.6 最小必要改造

1. 删除或 feature-gate TUI、admin socket、attach protocol、安装脚本和非必要 metrics。
2. Windows 提供无 Unix admin 依赖的 headless host；shutdown 由 supervisor contract 控制，而不是 CLI 独占 Ctrl-C。
3. 移除 `AtomicU64` 的目标假设，进入 EasyTier 全 Core target matrix。
4. 抽象 listener/socket creation，至少允许 loopback OS socket host；若移动端需要 socket protect/network binding，再通过平台 host 注入，不污染 SOCKS protocol core。
5. 将 MIT LICENSE 和固定 upstream commit 保留在 vendored/derived crate 中；后续上游更新按明确 patch ledger 审核，不自动漂移。
6. feature 默认关闭。关闭时不增加监听器、线程、进程、路由、配置字段解释或制品依赖，从而保持 2.9.10 行为和性能边界。

### 31.7 当前可作出的承诺边界

- **可以承诺**：按上述结构实现时，Leaf、mesh 和 SOCKS server 可以保持足够解耦；Linux/Android/macOS 已证明 server 核心具备基础可行性。
- **不能承诺**：上游 `next-socks5` 原样合入即可支持所有 EasyTier 平台。
- **全平台可达但尚未证明**：完成 Windows/32-bit/MIPS 修整，并采用 desktop-process + mobile-native 双 host 后，设计上可以覆盖现有 Core、Mobile 和 OHOS 交付形态；必须由各自 workflow 和真机/目标运行证据闭环。

## 32. 用户结论修正：`next-socks5` 尚不满足优雅、解耦、全平台完备性（2026-07-15）

> **用户当前判断：不接受把“核心可整理成 crate”当作“已经全平台兼容”。在需要 desktop sidecar + mobile in-process 两套宿主、Windows/MIPS 修补、Android/OHOS 特殊打包和 mesh virtual-IP 适配的前提下，`next-socks5` 还达不到目标中的优雅解耦完备性。是否实现仍未决定。**

- 更正上一节的推荐强度：`next-socks5` 不再是可直接进入集成的首选实现，只保留为轻量 SOCKS5 TCP/UDP、association 生命周期、timeout、DNS cache 和 graceful shutdown 的参考实现。
- “独立 crate”只说明源码依赖边界可以整理干净，不说明运行宿主、socket ownership、listener 地址模型、打包、进程生命周期和所有 target ABI 已统一。
- 如果一个候选需要 EasyTier 为 Windows、Android、OHOS、MIPS 分别修补其核心假设，或需要 process/in-process 双实现才能覆盖发布矩阵，就不能称为已经满足用户要求的统一全平台 sidecar。
- 后续候选必须提高门槛：同一核心、同一生命周期契约、同一 listener/socket 抽象、同一 UDP association 语义，能直接进入 Windows/macOS/Linux/FreeBSD/Android/OHOS 和现有 CPU target 矩阵；平台层最多提供薄的 socket/packaging adapter，不能重写协议或资源状态机。
- 在找到满足该门槛的现成实现之前，不进入 `next-socks5` 集成，不因前期 Linux/Android/macOS smoke 通过而扩大结论。
## 33. 用户未决定是否实现：HEV SOCKS5 Server 作为统一出口宿主

> **用户未决定是否实现。** 本节只记录 2026-07-15 的候选筛选、源码语义和可行性证据，不代表已经选择 HEV，也不授权开始 EasyTier 集成、发布或 workflow 构建。

### 33.1 Leaf 不应直接视为完整 SOCKS5 出口实现

实际依赖是 `lovitus/leaf@b1e33b50e37ea3b396e3cee2a1d60bb0c599655c`，而不是只依据 `eycorsican/leaf` README 或 release 结论。固定提交与上游 `v0.14.2` 的相关行为一致：

- `leaf/src/proxy/socks/inbound/stream.rs::Handler::handle_socks5` 在 `UDP ASSOCIATE` 后只启动任务等待 TCP 控制连接结束；源码仍保留“通知 NAT manager”的 TODO，控制连接结束不会立即删除 UDP association。
- `leaf/src/app/nat_manager.rs::NatManager` 使用无容量上限的 `HashMap<DatagramSource, ...>`，仅按全局 `UDP_SESSION_TIMEOUT` 和扫描间隔回收会话。
- Leaf runtime shutdown 会结束整个 Tokio runtime，但这不能替代每个 SOCKS5 UDP association 与控制连接的 RFC 生命周期绑定。
- 因此 Leaf 仍是跨平台代理核心和现有 policy runtime 的合适基础，但不能仅凭“支持 SOCKS5 inbound + DIRECT”认定其 UDP 生命周期已经达到 GOST 等级。

### 33.2 新的优先候选：`heiher/hev-socks5-server`

审计提交：`4cee82477755d115d5b113572a8d68920d76d6a2`（2026-06-28）。项目为 MIT 许可，定位是独立、轻量的 SOCKS5 server，而不是多协议代理核心。

满足当前需求的能力：

- 标准 SOCKS5 `CONNECT` 和 `UDP ASSOCIATE`，IPv4/IPv6、域名解析、用户名密码认证。
- `src/core/src/hev-socks5-udp.c::task_io_yielder` 在 UDP relay 等待 IO 时同时检查 SOCKS TCP 控制连接；控制连接 EOF 会把 session timeout 置零并结束 relay。
- TCP、UDP 分别具有可配置 read/write timeout；UDP 默认 60 秒。
- `src/hev-socks5-worker.c` 在 worker 停止时遍历并终止活动 session；`src/hev-socks5-proxy.c::hev_socks5_proxy_run` 等待工作线程 `pthread_join` 后才释放 worker 和 task system。
- `hev_socks5_server_main_from_str`、`hev_socks5_server_main_from_file` 和 `hev_socks5_server_quit` 提供窄 C API，可静态或动态链接，不要求运行外部 CLI。
- `main.udp-port` 原生支持单端口、随机端口或端口范围，可避免每个 UDP association 的端口冲突。
- 官方构建覆盖 Windows、FreeBSD、macOS、Android NDK 和 Apple XCFramework。Linux CI 覆盖 x86、ARM、AArch64、MIPS/MIPSel、MIPS64、RISC-V、LoongArch、PPC、s390、m68k、MicroBlaze、OpenRISC、SH 等大量 musl 目标，比目前调查过的 Rust SOCKS server crate 更接近 EasyTier 的 target 集合。

### 33.3 已完成的可行性验证

本次只编译和运行外部 HEV 候选，没有编译 EasyTier，也没有修改主机或 Android 路由、VPN、Wi-Fi。

macOS loopback：

- 源码提交：`4cee82477755d115d5b113572a8d68920d76d6a2`。
- TCP `CONNECT` 到本地 echo：通过。
- 使用客户端实际 UDP 源地址和端口执行 `UDP ASSOCIATE`：通过。
- 关闭 SOCKS TCP 控制连接后 UDP relay 不再转发：通过。
- SIGINT/公开 quit 路径正常退出：通过。

Android 实机：

- 设备：`arm64-v8a`，Android API 35；通过 wireless ADB 自动化执行，没有截图或模拟点击。
- 使用 NDK `29.0.14206865` 和项目官方 `Android.mk` 构建 `arm64-v8a` 共享库：通过。
- `libhev-socks5-server.so` 大小 183168 bytes；只调用公开 start/quit API 的测试宿主大小 6328 bytes。
- 测试监听 TCP `31280`，UDP relay 范围 `31281-31283`；实际选择 `31281`。
- 跨局域网 TCP `CONNECT`：通过。
- 跨局域网标准 UDP `ASSOCIATE`：通过。
- 关闭 TCP 控制连接后再次发送 UDP，客户端无响应且目标 echo 未收到该包：通过，确认不是目标服务提前退出造成的假阳性。
- SIGTERM 调用 `hev_socks5_server_quit` 后进程退出，未残留测试进程：通过。

### 33.4 仍未闭环的兼容边界

1. `0.0.0.0:0` 请求：当前 HEV 会尝试把 UDP relay `connect()` 到 TCP peer IP 和请求中的端口 0；macOS 实测握手返回通用失败。携带客户端实际 UDP 源端口时正常。候选补丁应在端口为 0 时跳过预连接，让 `hev_socks5_udp_recvmmsg_udp` 已存在的“从首个 UDP 包学习 client address”路径接管。该改动很小，但必须补 RFC/互操作测试并优先反馈上游。
2. Windows：README 中“标准 UDP ASSOCIATE 暂不支持 Windows”的脚注来自 2025-03；当前 HEAD 在 2026-06-28 新增 `HevSocks5Session: Fix failure to bind on Windows`，修复了 bind 地址结构未初始化。官方 Windows CI 能构建，但尚无 Windows 实机标准 UDP 证据，因此不能宣称该脚注已经失效。
3. OHOS：C、pthread、socket、共享库形式没有发现结构性阻碍，但尚未针对 `aarch64-unknown-linux-ohos`/OHOS NDK 构建或运行。
4. 主 TCP listener 只接受一个端口，不像 UDP relay 原生接受范围。EasyTier wrapper 仍需按顺序尝试 2 至 3 个固定候选端口，或向 HEV 增加返回实际 listener address 的窄 API。不能只做预探测后假定 bind 一定成功。
5. HEV 配置和 runtime 使用全局静态状态，只支持单进程内一个 server 实例。这与首版“单 policy/单出口服务实例”一致，但不满足未来同进程多实例；若未来需要多实例，必须选择进程隔离或先完成 upstream runtime handle 重构。
6. HEV 是 C 而不是 Rust。FFI 边界很窄且源码规模、产物和依赖都较小，但是否接受一个 MIT C 静态库仍由用户决定。

### 33.5 建议的解耦边界（若用户以后选择实现）

建议新增独立、feature-gated 的 `easytier-socks-egress`，而不是把 HEV 状态机写入 mesh、Leaf 或 policy 规则代码：

```rust
pub struct SocksEgressConfig {
    pub listen_candidates: Vec<SocketAddr>,
    pub udp_port_range: std::ops::RangeInclusive<u16>,
    pub bind_interface: Option<String>,
    pub socket_mark: Option<u32>,
}

pub trait SocksEgressRuntime {
    fn start(&self, config: SocksEgressConfig) -> Result<SocketAddr>;
    fn shutdown(&self) -> Result<()>;
}
```

约束：

- HEV 只负责 SOCKS 协议、DIRECT socket、UDP association 和资源回收。
- EasyTier 只负责选择 listener、发布 mesh endpoint、生命周期 ownership 和平台 loop prevention 参数。
- Linux 可把 HEV 已支持的 `mark`、`bind-interface` 或 `bind-address` 接入现有 underlay 选择，避免出口 socket 再进入 policy TUN。
- Android 沿用 EasyTier 应用已被自身 `VpnService` 排除的模型，不新增截图/点击自动化，也不另造一套 Java SOCKS 实现。
- 不修改 Leaf 的 SOCKS/UDP 状态机，不把 HEV 代码复制进 EasyTier mesh hot path。

### 33.6 当前候选排序

1. `hev-socks5-server`：目前最接近“单一库宿主、完整 UDP 生命周期、桌面/移动/多架构统一”的候选；需要闭环 port-zero、Windows 和 OHOS。
2. Leaf：平台和现有依赖优势明显，但 UDP association ownership 与容量治理不足，除非在 Leaf fork 中修复后再验证。
3. `fast-socks5`：协议 library 干净，但仍需 EasyTier 自己实现长期运行资源治理。
4. `shoes`：功能完整但依赖和协议面过重，平台证据不如 HEV。
5. `3proxy`：标准 UDP、Windows、Linux、macOS、FreeBSD 很成熟，但没有 HEV 这样明确的移动静态库/公开 start-quit API，不适合优先作为统一 in-process host。

在 Windows 标准 UDP、OHOS 构建和 `0.0.0.0:0` 互操作完成前，仍不得声称“支持 EasyTier 全部现有平台”。

### 33.7 Windows 最新源码构建与 TCP/UDP 实测（用户未决定是否采用 HEV）

验证日期：2026-07-15。验证主机：Windows 10 Pro N for Workstations 19045，AMD64。未在维护者 Mac 上编译，也未修改 EasyTier 实现。

- 上游 HEAD：`4cee82477755d115d5b113572a8d68920d76d6a2`（`HevSocks5Session: Fix failure to bind on Windows.`）。
- 构建方式：严格沿用上游 Windows workflow 的 MSYS2/MSYS ABI，使用 GCC 15.3.0、GNU Make 4.4.1、递归子模块和 `make -j$(nproc)`；原生 MinGW/Win32 ABI 不是本次产物路径。
- 构建成功，`hev-socks5-server.exe` SHA-256 为 `5cc7bac6d9a5cbe2dbfe3a4ce8d24a1fa1e2b4b582a0815a362a6faa37185cc7`；配套 `msys-2.0.dll` SHA-256 为 `1410599ee2efcede0869abec8910398d688755952e59cf7983b15cac6de78201`。
- 相同回环配置下，官方 `2.12.0` Release 的 SOCKS5 TCP CONNECT 会在 session bind 阶段超时；最新 HEAD 的原生 `curl.exe --socks5-hostname` 和独立协议测试均通过，证明该提交修复了 Windows TCP bind 回归。
- 最新 HEAD 的 UDP ASSOCIATE 能完成握手、绑定 relay 并接收客户端数据。使用真实 UDP 客户端源端口和 `0.0.0.0:0` 两种请求时，代理包都能到达本机 UDP echo 目标，但 echo 回包均未交还 SOCKS 客户端。
- 将 `udp-read-write-timeout` 从 1 秒提高到 5 秒不改变结果。关闭 Windows Public 防火墙后使用同一产物、配置和测试脚本复测，结果仍完全一致，因此排除防火墙阻断。
- 当前兼容性边界：HEV 最新 Windows sidecar 的 TCP 已可用，但 UDP 回程路径仍未闭环；在定位并修复 Windows UDP 接收/转发问题前，不能把 HEV 声明为 EasyTier 全平台 TCP+UDP SOCKS egress 的完备过渡方案。

### 33.8 Windows UDP 根因与最小修复可行性（用户未决定是否采用/上游化）

根因已通过源码追踪、原生 Winsock 探针和临时补丁实测闭环，不是防火墙，也不需要重写 IOCP：

- 公共 UDP splice 位于 HEV core `1.6.3` 的 `src/hev-socks5-udp.c::hev_socks5_udp_splicer`。它创建出口 UDP socket `fd_b` 后，在收到并向目标发送第一个客户端数据报之前，就先调用 `hev_socks5_udp_fwd_b` 对该未绑定 socket 执行非阻塞 `recvmmsg/recvmsg`。
- Linux/Android 的未绑定非阻塞 UDP 接收表现为暂时无数据（`EAGAIN`），macOS/iOS 的 kqueue 路径也不会把该状态永久关闭，因此后续首次 `sendmmsg` 自动绑定 socket 后仍会继续接收回包。
- Windows Winsock 对未绑定 UDP socket 的接收返回 `WSAEINVAL`。Windows 原生探针确认：首次 send 前为 `10022/WSAEINVAL`；首次 `sendto` 自动绑定后，同一 socket 的无数据状态变为 `10035/WSAEWOULDBLOCK`。
- `hev_socks5_udp_fwd_b` 只把 `EAGAIN` 视为暂时状态，其他错误返回 `-1`。`hev_socks5_udp_splicer` 随后把 `res_b` 永久保留为负值，循环条件 `if (res_b >= 0)` 使回包方向永远不再执行。客户端数据仍能通过 `fd_b` 发到目标，所以表现为“目标收到请求，但 SOCKS 客户端收不到回包”。
- Windows 使用 hev-task-system `5.10.2` 的 `WSAEventSelect/WSAEnumNetworkEvents` reactor，Unix 使用 epoll/kqueue；本次故障发生在进入 reactor 等待前的首次反向 recv 探测，不要求替换或重写 Windows reactor。

临时最小补丁只改变公共 UDP splice 的启动顺序：

1. 将 `res_b` 初始值从 `1` 改为 `0`，避免未启用反向读取时形成忙循环。
2. 仅在首个正向数据报已进入 bind/send 阶段后调用 `hev_socks5_udp_fwd_b`，即把条件从 `res_b >= 0` 收窄为 `res_b >= 0 && bind`。

Windows MSYS2 全量重构建成功，补丁产物 `hev-socks5-server.exe` SHA-256 为 `a9aaf60ea30bae10490eac23cc1f5a74b6b089698e06ca08c967228bd662899a`。相同配置和测试脚本结果：

- SOCKS5 TCP CONNECT：通过。
- UDP ASSOCIATE，客户端声明真实源地址/端口：通过。
- UDP ASSOCIATE，客户端声明 `0.0.0.0:0`：通过。
- 控制 TCP 关闭后的 UDP relay 清理：通过，关闭后数据报不再到达目标。
- relay 端口范围回退：本轮依次使用 `31381`、`31382`，行为符合配置。

难度判断：代码修复本身很小，约两行状态调整，属于低实现难度；但上游化前仍需补充 Linux、Android、macOS、Windows 的首包竞态、首次 send 失败、并发 association、UDP timeout、控制连接关闭和资源回基线回归，避免公共状态机改动造成其他平台行为回退。该补丁目前仅是临时可行性证据，尚未修改 EasyTier，也未提交到 HEV 上游。

## 34. 用户已决定：采用 HEV 作为首版内置 SOCKS egress

> 状态更新（2026-07-15）：本节覆盖第 33 节的“用户未决定”状态，但保留前文作为方案调查和取舍记录。首版继续采用 HEV，目标是单一协议实现、全平台一致配置，并与 Leaf 和 EasyTier mesh 生命周期解耦。

### 34.1 已确认的代码基线与修复

- server fork：lovitus/hev-socks5-server，分支 codex/windows-udp-unbound-recv。
- core fork：lovitus/hev-socks5-core，分支 codex/windows-udp-unbound-recv。
- core commit cd8793a 修复 Windows 上未绑定 UDP socket 首次 recv 返回 WSAEINVAL 后任务永久退出的问题：前向方向初始为可写、后向方向初始为不可读，等待真实事件再进入接收。
- server commit 0f5f9fc 更新 core submodule 到上述修复。
- Windows 已用当前源码构建并验证 TCP、UDP、UDP ASSOCIATE 0.0.0.0:0、控制连接关闭回收以及 UDP relay 端口候选切换。
- 本轮补充 server 修复：当客户端在 UDP ASSOCIATE 中声明端口 0 时，不对零端口提前执行 connect()；保持 association 未绑定，由现有 UDP 接收路径从第一个真实数据包学习客户端源地址。

### 34.2 Mihomo 对照语义

以下行为以本地 /Users/fanli/Documents/mihomo-rev 为准：

- listener/sockscommon/sockscommon.go::handleSocks5WithBindPolicy
  - 未显式拒绝时允许 UDP ASSOCIATE 请求不携带可绑定客户端端点。
  - TCP 控制连接保持打开，并通过 io.Copy(io.Discard, conn) 等待关闭；控制连接关闭即结束 association 生命周期。
- listener/sockscommon/sockscommon.go::ServePacketConn
  - UDP listener 持续接收并解码数据包，listener 关闭后退出，而不是把声明的零端口当成真实远端连接。
- listener/socks/tcp.go 与 listener/socks/udp.go
  - listener 对象拥有底层 socket 与关闭状态；关闭动作必须解除 accept/read 阻塞并让服务循环退出。
- transport/socks5/socks5.go::ServerHandshakeWithReplyAddrPolicy
  - UDP ASSOCIATE 可允许缺失/未指定的客户端绑定地址；服务端返回可访问的 UDP relay 地址，真实客户端来源由 UDP 数据面确认。

HEV 首版兼容语义：

1. 0.0.0.0:0 或 [::]:0 是“客户端端点尚未知”，不是应连接的目标。
2. 非零声明端口继续沿用 HEV 现有预连接/校验行为。
3. 零端口时由首个 UDP 包确定 association 的客户端来源。
4. TCP 控制连接仍拥有 association；本修复不改变 HEV 的 UDP 状态机、超时或回收所有权。

### 34.3 Android 宿主边界

以下生命周期以 /Users/fanli/Documents/clashmeta-android-rev 为平台参考：

- service/.../TunService.kt：Android VpnService coroutine 拥有 TUN、路由、DNS 和网络变化处理；退出在 finally/NonCancellable 路径中完成资源释放。
- service/.../TunModule.kt：native core 通过宿主提供的 vpn::protect 和 TUN 接口运行，requestStop() 显式停止 native 模块。
- app/.../util/Clash.kt：应用层只负责启动/停止 service，不让协议 listener 自行接管 Android VPN 生命周期。

EasyTier 的对应边界：

- HEV 不创建或修改 Android TUN、路由、DNS、VPN ownership。
- HEV 从 EasyTier Instance/Android service 生命周期启动和停止；停止必须可等待，不能只发送全局退出信号后立即丢弃宿主。
- Android 首版沿用 EasyTier 应用自身排除 VPN 的既有路径；是否需要逐 socket protect 由集成验证决定，不在 HEV 中发明第二套 VPN 管理。
- Wi-Fi/地址切换恢复由 EasyTier/Android 网络代际管理负责，HEV 只重建由宿主要求重建的 listener/association。

### 34.4 集成结构与最简配置

首版采用独立 egress 组件，不复用 Leaf PolicyRuntime：

    EasyTier Instance
      -> SocksEgressRuntime (单实例生命周期、端口候选、发布)
           -> HEV SOCKS5 TCP/UDP server
                -> DIRECT outbound
      -> mesh service/endpoint registry
           -> via: mesh 解析到成员的内置 egress
    Leaf
      -> 只消费已解析的 SOCKS endpoint

配置原则：

- 用户启用成员的 mesh egress 能力后，不需要手工填写该成员上的 SOCKS 端口。
- EasyTier 内部从少量端口候选中直接尝试监听，成功后发布实际 endpoint；不得先 probe 再释放，以免产生 TOCTOU 端口竞争。
- 用户自建 SOCKS（例如 gost）仍使用显式 host/port，与内置 mesh egress 分开表达。
- HEV 为单一全局 runtime，首版每进程只允许一个内置 egress 实例；多实例独立 runtime/API 改造记录到后续版本。
- Leaf、HEV、mesh endpoint registry 三者只通过窄配置/解析接口连接，不共享内部任务、DNS cache 或 UDP association 状态。

### 34.5 回归边界修正

此前“上游化前必须完成 Linux、Android、macOS、Windows 全套长期资源基线”的要求过宽，现修正为：

- Windows 两行 UDP event 修复：定向验证 TCP、UDP 首包、零端口 ASSOCIATE、控制连接关闭即可；已有实测证据。
- server 零端口修复：Linux/macOS 做零端口冒烟，Windows确认无回退；Android在集成宿主中覆盖。
- 并发 association、timeout、RSS/FD/线程回基线和网络切换属于 EasyTier 集成生命周期验收，不是两个局部 HEV 修复进入 fork 的前置条件。
- 不要求为未改变的 HEV UDP 状态机重新做所有平台长期基线，但集成发布前仍要证明 EasyTier 启停、崩溃恢复和网络变化不会持续泄漏资源。

### 34.6 后续批次

1. 完成独立 SocksEgressRuntime、端口候选与可等待退出。
2. 增加 mesh egress endpoint 发布/解析，使 via: mesh 不要求用户端口。
3. 让 Leaf 配置编译器只接收解析后的 endpoint，并保持现有显式 SOCKS 配置兼容。
4. 一次性在远程 builder/GitHub 构建较完整批次，先做 Linux/Android 生命周期与数据面验证。
5. 使用同一精确提交完成 Windows/macOS 构建和定向回归；OHOS与多进程实例若收益不足，明确记录到下一版本。

本节仅记录设计与修改 HEV fork，不因此触发 EasyTier profiling beta workflow。

## 35. HEV 集成批次 1：runtime 与免端口 mesh actor

### 35.1 新发现

- HEV 的 main_from_str/main_from_file 是阻塞入口，quit 解除阻塞；Android 官方 JNI 同样使用独立线程、quit、join，适合包装成可等待 runtime。
- HEV Makefile 默认在源码树写 build/bin，不能直接由多个 Cargo target 并行调用；后续 native backend 必须在 OUT_DIR 隔离构建，不能共享源码输出目录。
- Windows 官方构建依赖 MSYS2 runtime，而 EasyTier 正式目标是 windows-msvc。首版不得声称同一个 C FFI backend 原生覆盖 MSVC：
  - Linux/macOS/FreeBSD 可使用受监管 sidecar，数据面仍是同样的 kernel SOCKS listener。
  - Android 使用 in-process native backend，遵循 service quit/join 所有权。
  - Windows 首版使用同配置、同 supervisor 的 HEV sidecar backend；原生 MSVC backend 作为后续优化，不能阻塞 Linux/Android Leaf v1。
- 现有 MeshProxyBridgeSet 已经将 Leaf loopback SOCKS 与远端 SOCKS endpoint 隔离；RemoteUdpAssociation 也由目标 peer 建立真正的 HEV UDP association。因此端口候选只影响远端 endpoint 建连，不改变 Leaf DNS、规则或 UDP association 状态机。

### 35.2 本批实现

新增独立 workspace crate easytier-socks-egress：

- 不依赖 EasyTier、Leaf、路由或 DNS 类型。
- 生成私有 HEV YAML，直接尝试 11080、11081、11082 三个 TCP listener 候选。
- 每个候选由 HEV 自己 bind，失败后再启动下一个，不采用 probe/release，避免 TOCTOU。
- UDP 使用 udp-port: 0，由内核分配并通过 SOCKS5 UDP ASSOCIATE reply 返回，不需要固定 UDP 端口回退。
- 支持 bind-interface 和 socket mark，供 Linux 防止出口重新进入 policy TUN。
- 子进程 kill_on_drop，显式 shutdown 有界等待；后续 Instance wiring 必须拥有该 handle。
- 当前默认 workers 为 1，先保证低空闲开销；性能验证后再决定是否按 CPU/负载调整，避免未测量地默认创建 4 个 worker。

policy schema 兼容变更：

- native SOCKS actor 仍必须显式填写非零 port。
- via: mesh actor 可以省略 port；显式 port 保持旧语义和旧配置兼容。
- 省略 port 时，resolver 生成相同虚拟 IP 上的 11080/11081/11082 候选。
- TCP CONNECT 与 UDP association 共用候选顺序；显式 port 只尝试一次，不会被默认候选覆盖。
- Leaf 只看到本地 MeshProxyBridgeSet 的临时 loopback endpoint，不感知候选机制。

### 35.3 仍待本批后半段完成

1. 将 ProcessRuntime 接入 EasyTier Instance 生命周期，并自动解析相邻的 easytier-hev-socks-egress sidecar。
2. Linux policy 节点给 HEV outbound 设置与 policy routing 一致的 mark/interface，证明不会递归进入 Leaf。
3. 增加 Android in-process backend；不让 HEV拥有 VpnService、DNS、路由或 TUN。
4. 在 profiling/mobile workflow 构建并打包精确 HEV fork 97e74f1；所有 checkout/build 输出必须隔离。
5. 远程一次性更新 Cargo.lock、格式化并编译完整批次，然后执行显式端口兼容、三候选回退、TCP/UDP、停止清理测试。

本批尚未构建或运行 EasyTier；记录的是已写源码与下一验证边界，不将“代码存在”误记为“集成已通过”。

## 36. HEV 集成批次 1 后半段：宿主生命周期与平台构建

### 36.1 精确 HEV fork

- server fork 当前集成 commit 更新为 97e74f1068bd924e740032382cdc94ca83741ae6。
- ab5deaa 包含 UDP ASSOCIATE 零端口修复。
- 97e74f1 进一步把 src/core submodule URL 改为 lovitus/hev-socks5-core；否则干净 CI 无法从上游 URL 获取 fork-only commit cd8793a。
- profiling 和 Android candidate workflow 均固定 fetch 97e74f1 并递归初始化 submodule，不跟随可变分支头。

### 36.2 EasyTier 宿主生命周期

- 非 mobile 平台自动查找与 EasyTier 主程序相邻的 easytier-hev-socks-egress；找不到时再尝试 PATH。
- 不新增 exit-node 开关，也不要求用户配置内部端口。
- ProcessRuntime 由 SocksEgressGuard 监督：Instance drop 取消 token，监督任务执行有界终止并等待 child；HEV 意外退出只记录能力降级，不停止 mesh。
- Linux HEV outbound 设置 POLICY_SOCKET_MARK，复用现有 policy routing bypass 语义，避免 DIRECT egress 再次进入 Leaf TUN。
- sidecar 使用 kernel SOCKS listener；进程内/进程外不会改变 Leaf 到 listener 的 socket 数据面结构。选择 sidecar主要解决打包、MSVC和崩溃隔离，不建立第二套 SOCKS 协议实现。

### 36.3 Android in-process backend

- 使用 HEV 官方 main_from_str/quit 阻塞 API，Rust 独立线程运行。
- 全局 ACTIVE 只允许一个 runtime；占用直到线程 join 后才释放，避免旧 runtime Drop 误停止新代际。
- Instance guard 取消后调用 quit 并通过 spawn_blocking join，不在 Tokio worker 上阻塞。
- Drop 兜底把 JoinHandle 转移给清理线程，不能只 detach 并提前释放全局 runtime ID。
- Android HEV 不拥有 VpnService、TUN、DNS或路由；沿用 EasyTier app 自身 VPN 排除边界。
- Android workflow 使用 NDK aarch64 API 24 clang 构建 server、yaml、hev-task-system 三个静态库，通过 HEV_SOCKS5_LIB_DIR 交给 Rust build script 链接；APK 不增加独立 HEV service/process。

### 36.4 Linux sidecar workflow

- profiling workflow 使用 x86_64-linux-musl-gcc 构建静态 HEV executable。
- 制品重命名为 easytier-hev-socks-egress，与 easytier-core、easytier-cli、easytier-leaf-worker 同包。
- BUILD_INFO.txt 记录 hev_server_commit，SHA256SUMS.txt 覆盖 sidecar。
- workflow 检查 file 输出必须是 statically linked，避免 CentOS 7 验证机被新 glibc 或 runner 动态库污染。

### 36.5 当前验证状态

- 已完成源码、feature wiring、workflow wiring和配置兼容测试代码。
- 尚未更新 Cargo.lock、编译或运行测试。
- 下一步应先在远程 builder 做最小 GNU debug no-run，集中修复机械编译错误；随后再推 profiling beta，避免用一次完整 workflow发现普通类型错误。
- Android NDK Makefile能否在 clang/API 24下无补丁生成三个静态库仍属待证据项；失败时优先修复独立构建包装，不修改 HEV 协议状态机。

## 37. 首轮构建证据与机械修复

### 37.1 run 证据

- 旧短 SHA run：
  - Linux 29412925636 仅因 git fetch 无法解析短 commit 97e74f1 失败。
  - Android 29412925632 在新 push 后按 concurrency 规则取消。
- 完整 SHA快照 6852763e：
  - Linux 29413035603：HEV x86_64-musl sidecar构建成功，并通过 statically linked 检查；随后 easytier-policy test no-run 在 leaf_config.rs 的测试断言发现 Option<u16> 机械类型错误。
  - Android 29413035683：HEV server、yaml、hev-task-system 使用 NDK aarch64 API 24 clang 的三个静态库全部构建成功；既有前端 persisted-selection 测试通过，随后进入 debug APK Rust构建。
- 上述证据证明桌面 sidecar 与 Android静态库两种构建包装均可行；尚未证明最终 Rust链接和运行时行为。

### 37.2 修复

- 将 resolver 测试断言从 port == 1080 改为 port == Some(1080)，与新 trait签名一致。
- 前端 PolicyProxyRow.port 改为 number | null：
  - via: mesh 缺省 port解析为 null，序列化时省略。
  - native仍走 requiredPort，继续要求显式非零端口。
  - 新增 mesh actor默认 port为空，UI显示 auto占位。
  - 默认模板的内置 mesh示例不再填写 1080；用户自建 native SOCKS示例仍保留 7890。
  - 新增缺省 mesh port与显式旧 port同时往返的 codec测试。
- 该前端修改同时包含此前已完成但未提交的 GeoX默认组、规则、chain/fallback注释模板，现与免端口 HEV语义一并进入验证快照。

## 38. 第二轮 Linux 编译传播修复

- 精确候选 4d467a12，Linux run 29413455082：
  - HEV musl sidecar再次成功。
  - easytier-policy通过上一轮失败点。
  - EasyTier主 crate test no-run发现两处旧类型测试调用：
    1. policy_proxy.rs 的 resolver closure仍声明 port: u16。
    2. virtual_nic.rs 的显式端口测试仍读取已替换的 MeshProxyTarget.endpoint字段。
- 修复：
  - closure改为 Option<u16>。
  - MeshProxyTarget只读暴露 endpoints()，显式旧配置测试断言候选切片只有 10.44.0.7:1080。
- 这些都是 schema传播的测试断点，不改变运行时连接或回退语义。

## 39. 2026-07-15 Android HEV 精确候选实机证据与运行时包自排除修复

> 本 TODO 仍属于“用户未决定是否实现原生 mesh egress”的方案记录；本节只闭环当前 HEV 过渡后端，不把 HEV 固化为最终数据面。

### 39.1 参考实现与有意差异

- Android 参考：`/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`，`TunService` 创建 `VpnService.Builder` 时使用运行时 `packageName` 参与 access-control 集合，而不是硬编码 application id；`AcceptSelected` 将自身加入 allowed 集，`DenySelected` 则从 disallowed 集移除自身。
- EasyTier 有意不同：Mihomo/ClashMeta 的 native core 可以对 underlay socket 使用 Android `VpnService.protect()`；当前嵌入的 HEV SOCKS server 直接创建 egress socket，没有可注入的 `protect()` 回调。若 EasyTier/HEV 所在应用仍被 VPN 捕获，HEV DIRECT 会重新进入 TUN 并形成回环。
- 因此 EasyTier 的兼容边界是：`TauriVpnService` 必须无条件把自身运行时 `packageName` 加入 disallowed 集；调用者仍可增加其他排除项，但不能移除当前宿主。失败行为从“application-id suffix/flavor 下可能回环”收窄为“当前宿主始终绕过 Android VPN”。
- 修复位置：`tauri-plugin-vpnservice/android/src/main/java/TauriVpnService.kt::mergeDisallowedApplications` 与 `createVpnInterface`；前端 `easytier-gui/src/composables/mobile_vpn.ts::doStartVpn` 不再硬编码 `com.kkrainbow.easytier`。
- 兼容测试：`tauri-plugin-vpnservice/android/src/test/java/ExampleUnitTest.kt` 覆盖运行时包自动加入及已存在时去重。

### 39.2 精确候选与构建证据

- 候选 SHA：`0aa7c3dbd691d1f1c9fa2ff0559657485c177356`（以下实机数据面证据尚不包含本节 runtime-package 修复，修复需下一次批量 Android workflow 重建）。
- Android workflow `29413722793` 成功；下载制品的 `SHA256SUMS.txt`、`BUILD_INFO.txt`、commit SHA、target、HEV 完整 pin 和签名均已核对。
- Linux workflow `29413722615` 的 HEV musl sidecar、优化构建、metadata、bundle、artifact upload 和 profiling prerelease 均成功；GitHub runner 当时只停留在 post-cache 收尾。

### 39.3 Android 真实自动链路

- 设备：`192.168.234.227:5555`，Wi-Fi 始终保持开启；未使用截图或坐标点击。
- 通过 WebView CDP 调用应用已有 Tauri/Rust API，启用持久化实例 `c17a8c16-5016-4d09-a1c3-e97c6fddcaf5`。
- `post_run_network_instance` 触发应用自己的 Android VPN 流程；生成路由为 `0.0.0.0/0`、`::/0`，底层 DNS 为 `fda9:52cf:9966::1`、`192.168.234.1`，network key 为 Wi-Fi 代际键。
- `tun0` 建立为 `10.245.0.2/24`，内置 HEV 自动监听 `[::]:11080`。
- TCP：Mac 经 `192.168.234.227:11080` 使用 SOCKS5 访问 Cloudflare trace，HTTP 200，总耗时约 0.80 秒。
- UDP：标准 `UDP ASSOCIATE 0.0.0.0:0` 成功，HEV 返回随机 UDP relay `0.0.0.0:46318`；经该 relay 向 `1.1.1.1:53` 查询 `example.com`，41 ms 获得 2 个答案。该证据同时覆盖 fork 的零端口修复。

### 39.4 端口冲突、所有权和停止清理

- 正常停止后 HEV 监听与 VPN/TUN 均释放，应用线程从运行态回到 59。
- 使用独立 Android `toybox nc` 占用 TCP `11080` 后重新启动相同实例，HEV 自动选择 `[::]:11081`。
- `11081` 上 TCP SOCKS 访问返回 HTTP 200；UDP ASSOCIATE 返回随机 relay `0.0.0.0:47447`，DNS 查询 6 ms 获得 2 个答案。
- 停止实例后 `[::]:11081` 被释放，线程由 67 回到 59；外部占用者的 `11080` 保持存活，证明 EasyTier 只清理自己拥有的候选端口。
- 为候选 application-id suffix 手工二次 restart VPN 时曾留下 VPN/TUN，日志显示前端 operation epoch 忽略了迟到 stop；HEV 本身及其监听已经停止。该人工路径不是正常用户路径，随后显式 `stop_vpn` 完整清理。runtime-package 修复使下一候选无需该二次 restart，应重新验证正常单路径 stop。

### 39.5 UDP-over-TCP / KCP 当前边界

- 当前 HEV 过渡后端默认使用标准 SOCKS5 `UDP ASSOCIATE`，没有默认使用 UoT 或 KCP。
- HEV 的 `FWD UDP` 是其私有 UDP-over-TCP 扩展，不等同于 SagerNet UoT；Leaf 标准 SOCKS outbound 不会自动协商该扩展。
- EasyTier `enable_kcp_proxy` 是 underlay/代理能力，不等于 Leaf-to-HEV 的 UDP 自动 KCP，当前验证配置也未启用。
- 首版维持 native UDP：DNS 可独立使用 DoH/DoT/TCP；QUIC、语音和游戏避免 UoT 队头阻塞或叠加 KCP 重传。后续若用户决定实现，应在 egress 抽象下增加显式 `native`/`tcp`（或兼容 UoT）模式，不做无法可靠判定失败的静默逐包切换。
