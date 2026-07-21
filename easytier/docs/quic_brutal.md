# quic-brutal 使用说明 / Usage

`quic-brutal` 是 EasyTier 3.0.2 新增的实验性 mesh overlay。它使用 QUIC
协议栈和 Brutal 发送节奏，但不是标准 Hysteria2 服务，不能与 Hysteria2 客户端或
服务端互通。

`quic-brutal` is an experimental mesh overlay added in EasyTier 3.0.2. It uses
the QUIC stack with Brutal sender pacing, but it is not a standard Hysteria2
service and does not interoperate with Hysteria2 clients or servers.

## 是否默认启用 / Is it enabled by default?

不是。普通 `quic://`、默认 listener 和现有传输优先级均保持不变。只有显式添加
`quic-brutal://` listener 或 peer 才会使用该协议。

No. Ordinary `quic://`, the default listeners, and the existing transport
priority remain unchanged. The protocol is used only when a
`quic-brutal://` listener or peer is explicitly configured.

- GUI：在 listener 或 peer 编辑器中显式选择 `quic-brutal`。GUI 为 listener
  预填端口 `11013`，发送带宽按 Mbps 填写，也可以留空。
- CLI/TOML/RPC：保持现有配置方式，只把所需 URL 写成 `quic-brutal://...`。

## CLI 示例 / CLI examples

```bash
# 监听端：本节点向对端发送时最多按 1 Gbit/s 的 Brutal 速率调度。
easytier-core --listeners 'quic-brutal://0.0.0.0:11013?tx_mbps=1000'

# 拨号端：本节点向监听端发送时最多按 100 Mbit/s 的 Brutal 速率调度。
easytier-core --peers 'quic-brutal://SERVER:11013?tx_mbps=100'
```

示例只展示相关参数；网络名称、网络密钥、虚拟地址等仍按现有方式配置。

The examples show only the relevant arguments. Configure the network name,
network secret, virtual address, and other settings as usual.

```toml
# TOML uses the same Mbps-based URLs.
listeners = ["quic-brutal://0.0.0.0:11013?tx_mbps=1000"]

[[peer]]
uri = "quic-brutal://SERVER:11013?tx_mbps=100"
```

## `tx_mbps` 怎么填 / Choosing `tx_mbps`

GUI、CLI、TOML 和 RPC URL 都按 **Mbps** 填写 `tx_mbps`。它只描述 URL
所在节点的**发送方向**：

- listener URL 上的值控制监听节点发送给拨号节点的流量；
- peer URL 上的值控制拨号节点发送给监听节点的流量；
- 允许范围是 `1` 到 `100000` Mbps，最多 6 位小数；
- 留空不会自动探测带宽，而是让该方向使用普通 QUIC 的 BBR。

例如拨号节点上传 100 Mbit/s、下载 1 Gbit/s 时，拨号节点的 peer URL 填
`tx_mbps=100`，监听节点的 listener URL 填 `tx_mbps=1000`。

`tx_mbps` is measured in **Mbps** and applies only to the **local sending
direction**. A listener value controls traffic sent by the listening node; a
peer value controls traffic sent by the dialing node. Omitting it uses ordinary
QUIC BBR for that direction; bandwidth is not auto-detected.

3.0.2 已有的 `tx_bps` 配置仍会继续读取，但它只是向后兼容别名；新配置应统一使用
`tx_mbps`，并且不能在同一个 URL 中同时填写两者。

如果无法准确知道所有节点的链路容量，可以只在确实需要 Brutal 的链路上填写保守值。
保守值不会破坏连接，但会限制该节点发送方向能利用的带宽；例如在 10 Gbit/s 链路上
统一填写 100 Mbit/s，就无法让该方向充分利用 10 Gbit/s。留空更安全，但该方向也
不会获得 Brutal pacing 的优势。

## Stealth

`quic-brutal` 复用 QUIC 的 Stealth 配置，不需要单独的 Stealth 开关。网络密钥和
Stealth 有效时，Brutal pacing 位于现有 Stealth UDP 外层封装之内。强制 Stealth
不会静默降级为 plain Brutal；现有兼容策略允许回退时，也只会在相同 Brutal/BBR
发送控制下切换外层封装。如果连接仍然失败，现有候选和故障转移逻辑会继续尝试下一个
协议；`quic-brutal` 不拥有单独的优先级或回退策略。

`quic-brutal` reuses the QUIC Stealth configuration and needs no separate
Stealth switch. When Stealth is effective, Brutal pacing runs inside the
existing Stealth UDP wrapper. Required Stealth never silently falls back to
plain Brutal. If the connection still fails, the existing candidate and
failover logic continues with the next protocol; `quic-brutal` adds no private
priority or fallback policy.

## 查看状态 / Checking status

```bash
# 查看实际连接；Tunnel/协议列应显示 quic-brutal。
easytier-cli peer list

# 查看当前 listener URL，包括本地 listener 的 tx_mbps。
easytier-cli node info

# 查看已配置的主动连接。
easytier-cli connector list
```

`peer list` 显示 `quic-brutal` 代表 overlay 已建立；它不会单独显示该方向正在使用
Brutal 还是因 `tx_mbps` 留空而使用 BBR，因此还应结合 `node info` 或
`connector list` 检查本地 URL 参数。
