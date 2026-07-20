# EasyTier Leaf 策略路由部署与配置指南

本文面向 EasyTier Leaf 策略路由 v1。当前正式验证范围是 Linux 和 Android，配置格式为 EasyTier 自己的严格子集，不是 Mihomo、Leaf 或 sing-box 完整配置兼容层。

上一轮完整实机验证候选为 `824ac5a1d47d568113a7e2190d57fecf049dd47b`。Linux 与 Android 已验证 mesh 共存、DIRECT/REJECT、GeoSite/GeoIP、内置 HEV、TCP、UDP/UoT、Wi-Fi/路由恢复、worker 崩溃恢复、配置保留和正常清理。Shadowsocks/UoT v2 的精确制品结果单独记录在 [验证报告](leaf_shadowsocks_uot_validation_report_cn.md)。Trojan、VMess、VLESS 的配置字段已经实现，但正式可用性和性能结论必须以对应精确候选报告为准。chain/fallback 的已验证安全边界是：TCP 可以使用 chain/fallback；UDP 使用显式 `NETWORK,udp` 规则指向已验证的 actor，不依赖 payload 超时后的自动回退。

## 1. 组件与流量关系

如果不确定 overlay QUIC、QUIC/KCP TCP Proxy、用户 SOCKS portal、Leaf managed
mesh SOCKS 和 smoltcp 分别处于哪一层，先阅读
[`traffic_protocol_layers_cn.md`](traffic_protocol_layers_cn.md)。该文按入口、内层代理、
EasyTier packet、外层 overlay 和物理网络拆解完整封装路径。

Linux 发布包中的四个文件应放在同一目录：

| 文件 | 用途 |
| --- | --- |
| `easytier-core` | mesh、TUN、策略路由和生命周期所有者 |
| `easytier-cli` | EasyTier 状态与诊断 |
| `easytier-leaf-worker` | Linux 上的 Leaf 策略 worker |
| `easytier-hev-socks-egress` | 内置 TCP/UDP SOCKS5 出口，由 EasyTier 管理 |

Android APK 内嵌 Leaf 和 HEV，不需要用户启动 sidecar。

`via: mesh` 不会把 Leaf 直接绑定到远端公开 SOCKS 端口。EasyTier 在本机创建私有 loopback bridge，经 mesh 数据面连接目标 peer 的 SOCKS 服务，并为 bridge 使用临时凭据。mesh 路由失效时该 actor fail-closed，不会逃逸到系统直连。

Magic DNS 始终由 EasyTier mesh 处理；其他 DNS 和应用流量才进入 Leaf 策略路径。

## 2. 最小启用方式

### 2.1 Linux

继续使用现有 EasyTier 网络配置，只追加：

```toml
[policy_proxy]
enabled = true
config_file = "leaf-policy.yaml"
outbound_interface = "eth0"
# 四个二进制同目录时通常无需填写：
# leaf_executable = "easytier-leaf-worker"
```

`config_file` 相对路径以 EasyTier 网络 TOML 所在目录为基准。`outbound_interface` 必须是实际 underlay 出口，例如 `eth0`、`ens3` 或 `wlan0`，不能填策略 TUN。

启动前检查：

```bash
./easytier-core --check-config --config-file ./network.toml
```

启动：

```bash
sudo ./easytier-core --config-file ./network.toml
```

也可以不修改 TOML，使用进程级参数：

```bash
sudo ./easytier-core \
  --config-file ./network.toml \
  --policy-config ./leaf-policy.yaml \
  --policy-outbound-interface eth0 \
  --policy-leaf-executable ./easytier-leaf-worker
```

不要手工启动 `easytier-hev-socks-egress`。EasyTier 负责候选端口选择、进程父子关系、重启和清理。

### 2.2 Android

1. 安装带 policy runtime 的 Android 包。
2. 为网络配置固定虚拟 IPv4；v1 不使用 DHCP 作为策略 TUN 启动依据。
3. 在策略编辑器启用策略路由，保留默认模板或粘贴本文示例。
4. 保存并启动网络，首次启动授予 Android VPN 权限。

Android 由 `TauriVpnService` 持有系统 TUN；Leaf 和 HEV 在应用内运行。停止网络应同时移除 VPN service 和 `tun0`。

### 2.3 最小 policy 文件

只要求 mesh 正常、国内直连、其余流量从一个 peer 出口时：

```yaml
version: 1
dns:
  direct:
    - system
    - doh:dns.alidns.com@223.5.5.5
    - 223.5.5.5
    - 119.29.29.29
  proxy:
    - doh:cloudflare-dns.com@1.1.1.1
    - doh:dns.google@8.8.8.8
    - doh:dns.quad9.net@9.9.9.9

proxies:
  mesh-exit:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    via: mesh
    udp: true

rules:
  - GEOIP,LAN,DIRECT,no-resolve
  - GEOSITE,CN,DIRECT
  - GEOIP,CN,DIRECT,no-resolve
  - NETWORK,udp,mesh-exit
  - MATCH,mesh-exit
```

把 `10.144.144.2` 改成实际 EasyTier peer 虚拟 IP。省略 `port` 表示使用目标 peer 的 EasyTier 托管 HEV；用户不需要在 peer 上另外启动 SOCKS。

## 3. actor 的三种用法

### 3.1 托管 HEV：端口省略的 `via: mesh`

```yaml
proxies:
  mesh-direct:
    type: socks5
    server:
      virtual-ip: 10.144.144.3
      # 也可以改用稳定 instance UUID：
      # instance-id: 00000000-0000-0000-0000-000000000000
    via: mesh
    udp: true
```

这是最省事的 mesh 出口。目标 peer 使用 EasyTier 托管 HEV，支持 TCP 与 UDP。`udp: true` 是 actor 能力声明，不会让一个实际不支持 UDP 的第三方 SOCKS 自动获得 UDP 能力。

### 3.2 peer 上用户自建 SOCKS：显式 mesh 端口

```yaml
proxies:
  peer-gost:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    port: 1080
    via: mesh
    udp: true
```

此时 EasyTier 通过 mesh 访问 `10.144.144.2:1080`，端口由用户进程提供。用户负责认证、UDP 开关、超时和服务生命周期。如果服务只支持 TCP，应删除 `udp: true` 或改为 `udp: false`。

### 3.3 native SOCKS

```yaml
proxies:
  local-socks:
    type: socks5
    server: 127.0.0.1
    port: 7890
    via: native
    # 只有确认服务端 UDP ASSOCIATE 可用时才打开：
    # udp: true
    # username: user
    # password: password
```

native SOCKS 地址由运行 Leaf 的主机直接访问，必须显式指定端口。它可以是 gost、Mihomo、sing-box 或其他标准 SOCKS5 服务。

现有 EasyTier `--socks5` portal 与托管 HEV 不是同一个功能。不要因为配置了 EasyTier TCP SOCKS portal 就假定它支持本策略路径要求的 UDP。

### 3.4 Shadowsocks 与 UoT v2

Shadowsocks 是 Leaf 的编译期协议 actor，不修改 EasyTier mesh 数据面：

```yaml
proxies:
  ss-native:
    type: shadowsocks
    server: ss.example.com
    port: 8388
    cipher: aes-256-gcm
    password: change-me
    via: native
    udp: native

  ss-uot:
    type: shadowsocks
    server: ss.example.com
    port: 8388
    cipher: aes-256-gcm
    password: change-me
    via: native
    udp: uot-v2
```

`udp: native` 使用标准 Shadowsocks UDP；`udp: uot-v2` 使用 SagerNet v2
magic destination 和 unconnected framing，每个数据包保留自己的目标或来源地址。
两者都是显式选择，失败时不会静默切换。

经 EasyTier mesh 使用 Shadowsocks 时，复用现有 chain，而不是给 Shadowsocks
增加第二套 peer 配置：

```yaml
proxies:
  mesh-hop:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    via: mesh
    udp: true

  peer-visible-ss:
    type: shadowsocks
    server: 127.0.0.1
    port: 8388
    cipher: chacha20-ietf-poly1305
    password: change-me
    via: native
    udp: uot-v2

groups:
  mesh-ss-uot:
    type: chain
    members: [mesh-hop, peer-visible-ss]
```

这里 `127.0.0.1:8388` 是 mesh peer 通过前序 actor 看到的地址。不要单独把
`peer-visible-ss` 选为规则出口，否则该地址会指向运行 Leaf 的本机。

首期 cipher 为 `aes-128-gcm`、`aes-256-gcm`、`chacha20-poly1305` 和
`chacha20-ietf-poly1305`。Shadowsocks 不使用 `username`。

锁定 Leaf 只实现传统 Shadowsocks AEAD 数据路径，不支持 `2022-blake3-*`。配置解析器不会接受 SS2022 cipher，不能用传统 AEAD 成功结果替代 SS2022 证据。

### 3.5 Trojan、VMess 与 VLESS

三种协议是独立的 Leaf actor 编译器，不修改 EasyTier mesh 数据面。TLS 和 WebSocket 会编译为私有 Leaf actor，并由一个保持用户名称不变的内部 chain 包装；用户 group 仍只引用自己配置的名称。

Trojan TLS：

```yaml
proxies:
  trojan-native:
    type: trojan
    server: edge.example.com
    port: 443
    password: change-me
    tls:
      server-name: cdn.example.com
      insecure: false
    udp: true
```

Trojan 必须配置 `password` 和 `tls`。`server-name` 省略时使用 `server`；生产环境不建议启用 `insecure`。

VMess WebSocket：

```yaml
proxies:
  vmess-ws:
    type: vmess
    server: edge.example.com
    port: 80
    uuid: 00000000-0000-0000-0000-000000000000
    alter-id: 0
    cipher: auto
    transport:
      type: websocket
      path: /vmess
      headers:
        Host: cdn.example.com
    udp: true
```

VMess 只接受 AEAD `alter-id: 0`。`cipher` 支持 `auto`、`aes-128-gcm`、`chacha20-poly1305` 和 `chacha20-ietf-poly1305`；`auto` 与 Mihomo 一致，在 x86_64、aarch64、s390x 使用 AES-128-GCM，其他目标使用 ChaCha20-Poly1305。

VLESS WebSocket + TLS：

```yaml
proxies:
  vless-wss:
    type: vless
    server: edge.example.com
    port: 443
    uuid: 00000000-0000-0000-0000-000000000000
    transport:
      type: websocket
      path: /vless
      headers:
        Host: cdn.example.com
    tls:
      server-name: cdn.example.com
      insecure: false
    udp: true
```

WebSocket `path` 必须以 `/` 开头，`headers` 是字符串 mapping。当前可视化编辑器直接编辑 Host，并在 YAML 往返时保留其他 header。

三种协议经 mesh 使用时不写 `via: mesh`，而是复用现有 mesh SOCKS actor：

```yaml
proxies:
  mesh-hop:
    type: socks5
    server: { virtual-ip: 10.144.144.2 }
    via: mesh
    udp: true

groups:
  vless-through-mesh:
    type: chain
    members: [mesh-hop, vless-wss]
```

`vless-wss` 仍为 `via: native`。在 chain 中，它使用前序 mesh actor 提供的流，而不是绕过 mesh 自行连接。这个组合首先用于 TCP；UDP-over-stream 和多跳 UDP 必须以精确候选互操作结果为准，不能仅凭每个 actor 都写了 `udp: true` 推断可用。

当前不接受或不宣称 Trojan fingerprint/uTLS、smux、Brutal，VMess legacy alter-id，VLESS flow/XTLS/XUDP/XHTTP，以及 Reality、WebSocket early-data。未知字段会 fail-closed，不能把 Mihomo/sing-box 配置原样粘贴后假定全部生效。

## 4. chain 与 fallback

### 4.1 mesh peer 后接 peer 本地 SOCKS

```yaml
proxies:
  mesh-hop:
    type: socks5
    server:
      virtual-ip: 10.144.144.2
    via: mesh
    udp: true

  peer-local-socks:
    type: socks5
    server: 127.0.0.1
    port: 7890
    via: native

groups:
  peer-chain:
    type: chain
    members: [mesh-hop, peer-local-socks]
```

chain 按 `members` 声明顺序建立 TCP hop。上例先进入 `mesh-hop`，然后由该 hop 连接它所看到的 `127.0.0.1:7890`，最后访问目标。`peer-local-socks` 不应被规则单独选择，否则 `127.0.0.1` 指的是 Leaf 所在本机。

v1 不承诺 SOCKS-over-SOCKS UDP chain。固定的 Leaf SOCKS datagram 实现不会复用 chain transport，因此多跳 UDP 即使每个 actor 都写了 `udp: true` 也可能超时。

### 4.2 一个 chain 加一个 mesh direct 组成 fallback

```yaml
groups:
  overseas-fallback:
    type: fallback
    members: [peer-chain, mesh-direct]
```

fallback 按顺序偏好成员，并在 actor 建立失败时做有界、稳定的被动切换。EasyTier v1 没有复刻 Mihomo 的 URL 主动健康检查。

fallback 是连接级选择，不会迁移已经建立的连接。多连接协议在第一次故障切换窗口中，控制连接和随后建立的数据连接可能落在不同的 fallback 状态，因此应重试整个事务。境外实机验证中，单连接 HTTP 在主出口停止后可直接回退并返回 200；`iperf3` 的控制连接和数据连接跨越切换窗口时需要整次重试，不能把它描述为进行中多连接会话的无缝迁移。

以下情况不会可靠触发下一个成员：SOCKS UDP ASSOCIATE 已成功，但随后 payload 被服务端或中间网络丢弃。此时 fallback 看不到 actor 建立错误。因此 UDP 不应依赖这个 group 自动救援。

如需失败后允许本机直连，可显式加入 `DIRECT`：

```yaml
groups:
  overseas-fallback:
    type: fallback
    members: [peer-chain, mesh-direct, DIRECT]
```

加入 `DIRECT` 意味着代理不可用时可能绕过代理。要求 fail-closed 时不要加入。

### 4.3 推荐的 TCP/UDP 分离规则

```yaml
rules:
  - GEOIP,LAN,DIRECT,no-resolve
  - GEOSITE,CN,DIRECT
  - GEOIP,CN,DIRECT,no-resolve
  - NETWORK,udp,mesh-direct
  - GEOSITE,geolocation-!cn,overseas-fallback
  - MATCH,DIRECT
```

规则是 first-match。国内规则放在 `NETWORK,udp` 前面，国内 UDP 仍为 DIRECT；剩余 UDP 固定走 `mesh-direct`；境外 TCP 才进入 chain/fallback。

## 5. 默认出口组

GUI 默认生成以下组，初始成员都只有 `DIRECT`，所以没有节点时配置仍可运行：

| 组 | 默认匹配 |
| --- | --- |
| `default-exit` | 最终 `MATCH` |
| `google-exit` | Google、YouTube 及 Google GeoIP |
| `social-exit` | Twitter 及 Twitter GeoIP |
| `telegram-exit` | Telegram 及 Telegram GeoIP |
| `media-exit` | Netflix、Bilibili、Bahamut、Spotify |
| `github-exit` | GitHub |
| `domestic-exit` | `GEOSITE,CN` 与 `GEOIP,CN` |
| `other-exit` | `GEOSITE,geolocation-!cn` |

如果只测试一个 chain 和一个 mesh direct，可以把二者组成 `overseas-fallback`，再把 `google-exit`、`social-exit`、`telegram-exit`、`media-exit`、`github-exit`、`other-exit` 的成员改成 `[overseas-fallback]`。保持 `domestic-exit` 和 `default-exit` 为 `[DIRECT]` 没有结构问题。

完整可执行示例见 [`leaf_policy_v1_example.yaml`](leaf_policy_v1_example.yaml)。该文件会由核心 Rust 测试实际解析并编译为 Leaf 配置，避免文档字段漂移。

Shadowsocks、标准 UDP、UoT v2 及 mesh chain 示例见
[`leaf_policy_shadowsocks_example.yaml`](leaf_policy_shadowsocks_example.yaml)。

## 6. 自定义域名和 IP 规则

```yaml
rules:
  - DOMAIN,api.example.com,overseas-fallback
  - DOMAIN-SUFFIX,example.com,overseas-fallback
  - DOMAIN-KEYWORD,video,media-exit
  - IP-CIDR,203.0.113.0/24,DIRECT,no-resolve
  - PORT-RANGE,443,overseas-fallback
  - NETWORK,udp,mesh-direct
  - MATCH,default-exit
```

支持的 v1 规则：

| 类型 | operand | 说明 |
| --- | --- | --- |
| `DOMAIN` | 完整域名 | 精确匹配 |
| `DOMAIN-SUFFIX` | 域名后缀 | 子域与根域匹配 |
| `DOMAIN-KEYWORD` | 字符串 | 域名关键字匹配 |
| `IP-CIDR` | IPv4/IPv6 CIDR | IP 规则，可加 `no-resolve` |
| `GEOIP` | GeoX 分类 | `LAN` 内置；其他分类从内置或显式 GeoIP DAT 加载 |
| `GEOSITE` | GeoX 分类 | 默认自动使用内置 GeoSite 快照 |
| `COUNTRY` | MMDB 国家代码 | 需要显式 `mmdb` rule-set |
| `EXTERNAL` | `site:CODE`、`geoip:CODE` 或 `mmdb:CODE` | 显式外部数据源 |
| `PORT-RANGE` | 端口或范围 | 目标端口匹配 |
| `NETWORK` | `tcp` 或 `udp` | 传输类型匹配 |
| `INBOUND-TAG` | tag | Leaf inbound tag |
| `MATCH` / `FINAL` | 无 | 最终匹配 TCP 与 UDP |

规则顺序不会自动重排。未知规则、未知字段、未知 actor、循环 group、空 group 和不支持 UDP 的明确规则目标都会在启动前失败，而不是静默忽略。

## 7. GeoSite、GeoIP 与自定义 rule-set

只写 `GEOSITE` 或非 `LAN` 的 `GEOIP` 时，EasyTier 自动释放并校验随程序打包的 GeoX 快照，不要求用户配置路径。

需要固定自有数据时：

```yaml
rule-sets:
  my-site:
    type: geosite
    path: ./rules/geosite.dat
    update: manual
    sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    source-url: https://example.invalid/geosite.dat
```

v1 只允许 `update: manual`。`source-url` 是来源元数据，不会在启动时下载。相对路径以 policy 文件目录为基准；配置了 `sha256` 时内容不匹配会 fail-closed。

## 8. DNS 行为

```yaml
dns:
  # 前四个地址保留；198.19.0.1 继续作为虚拟 DNS，不会分配给域名。
  fake-ip-range: 198.19.0.0/16
  # 仅在 EasyTier IPv6 开启时使用；与现有 ULA 冲突时应修改。
  fake-ip-range6: fd65:6173:7974::/64
  # 省略 direct 时使用以下内置预置；显式写 direct: [] 才会改用
  # EasyTier 从平台获得的安全 underlay DNS。
  direct:
    - system
    - doh:dns.alidns.com@223.5.5.5
    - 223.5.5.5
    - 119.29.29.29
  # 省略 proxy 时使用以下 bootstrap-pinned DoH 预置，TCP-only actor 也能解析。
  proxy:
    - doh:cloudflare-dns.com@1.1.1.1
    - doh:dns.google@8.8.8.8
    - doh:dns.quad9.net@9.9.9.9
```

当前固定 Leaf 版本不支持 `tls://`/DoT 配置，因此预置中的加密 DNS 使用带 bootstrap IP 的 `doh:<域名>@<IP>` 语法。显式配置会覆盖对应集合的默认值；不会把用户自定义 DNS 与内置预置暗中合并。

`fake-ip-range` 必须是前缀长度不大于 `/22` 的 IPv4 CIDR。默认 `198.19.0.0/16` 仍属于基准测试保留空间，但避开 Mihomo/Clash 常见的 `198.18.0.0/16`；前四个地址保留，因此不会覆盖 `198.19.0.1` 虚拟 DNS。不要改成真实 LAN、CGNAT 或公网范围。

`fake-ip-range6` 必须是至少 `/118` 大小的 IPv6 CIDR。默认值是 EasyTier 专属的低冲突 ULA，不再使用 Mihomo/Clash 示例中常见的 `fdfe:dcba:9876::/64`。Leaf 会规范化网络地址、保留前四个地址，并循环使用固定 1000 个槽位，因此不会因长期出现新域名而无限增长。该字段只在 EasyTier 的 IPv6 开关开启时生效；现有网络使用相同 ULA 时必须改为不重叠的前缀。

域名规则不会先被全局解析成 IP。字面 `DIRECT` 规则走 direct lookup；SOCKS actor 接收原始域名并由实际代理路径处理。fallback/chain 是外层 group，不应被描述成拥有固定 DNS 出口；尤其 fallback 最终选择 `DIRECT` 时，固定 Leaf 版本的 resolver 选择边界与字面 `DIRECT` 不完全相同。需要严格 direct DNS 的域名应写在 group 规则之前并直接指向 `DIRECT`。

不要用全局放行 53 端口绕开策略 DNS，这会破坏 FakeDNS。v1 不承诺 Mihomo/sing-box 完整的 `nameserver-policy` 或 split-DNS。

## 9. 字段说明

### 9.1 EasyTier `[policy_proxy]`

| 字段 | 必需 | 说明 |
| --- | --- | --- |
| `enabled` | 是 | `false` 时不创建 Leaf、策略 TUN 或策略任务 |
| `config_file` | 二选一 | policy YAML 路径 |
| `config_inline` | 二选一 | 内联 policy YAML，最多 4 MiB |
| `outbound_interface` | 启用时是 | underlay 物理出口接口 |
| `leaf_executable` | Linux 可省略 | 默认查找同目录或 `PATH` 中的 `easytier-leaf-worker` |

`config_file` 与 `config_inline` 互斥。当前每个进程只允许一个 policy-enabled 实例。

### 9.2 policy 根字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `version` | integer | v1 必须为 `1` |
| `dns` | mapping | direct/proxy resolver 集合 |
| `rule-sets` | mapping | 可选手工 GeoX/MMDB 文件 |
| `proxies` | mapping | SOCKS5、Shadowsocks、Trojan、VMess 或 VLESS actor |
| `groups` | mapping | chain/fallback group |
| `rules` | sequence | 有序 first-match 规则，不能为空 |

### 9.3 proxy 字段

| 字段 | 说明 |
| --- | --- |
| `type` | `socks5`、`shadowsocks`、`trojan`、`vmess` 或 `vless`；HTTP 会被拒绝 |
| `server` | native 协议使用地址字符串；mesh SOCKS 使用 `virtual-ip`/`instance-id` mapping |
| `port` | native 必需；mesh 省略为托管 HEV，填写则为显式 peer SOCKS 端口 |
| `via` | SOCKS5 可用 `native`/`mesh`；其他协议固定 `native`，经 mesh 时使用 chain |
| `udp` | `false/off`、`true/native` 或 Shadowsocks 专用 `uot-v2` |
| `username` / `password` | SOCKS 凭据必须同时出现；Shadowsocks/Trojan 只使用 `password` |
| `cipher` | Shadowsocks 或 VMess 必需；支持范围见上文 |
| `uuid` | VMess/VLESS 必需 |
| `alter-id` | VMess 可省略或为 `0`；其他值拒绝 |
| `transport` | Trojan/VMess/VLESS 可选 `{ type: websocket, path, headers }`；省略为 TCP |
| `tls` | Trojan 必需，VMess/VLESS 可选；字段为 `server-name`、`insecure` |

### 9.4 group 字段

| 字段 | 说明 |
| --- | --- |
| `type` | `chain` 或 `fallback` |
| `members` | 有序 actor/group 名称；允许 `DIRECT`、`REJECT`；不能为空 |

actor 与 group 名只能使用 ASCII 字母、数字、`_`、`-`、`.`，最长 64 字符；不能使用保留名 `DIRECT` 或 `REJECT`。展开后的 chain 最多 32 个 actor，group 引用总数最多 64。

## 10. v1 兼容与限制

- 不配置 `[policy_proxy]` 或设置 `enabled=false` 时，不创建新运行时，普通 EasyTier mesh 行为保持原路径。
- policy-enabled 配置需要带相应 feature 和 sidecar 的新构建，不能把该 YAML 当作 EasyTier 2.9.10 原生字段使用。
- 当前发布声明仅覆盖 Linux 与 Android；macOS/Windows 等目标保留架构兼容设计，但尚不能引用 Linux/Android 结果代替实机验证。
- 单进程单 policy-enabled 实例；不承诺 netns、多实例、HTTP actor、在线 Geo 更新和完整 split-DNS。
- native SOCKS/SS 的 UDP 完整性由用户服务负责；`udp: true/native` 不是探测结果。
- Trojan/VMess/VLESS 当前只承诺本文列出的 TCP、TLS、WebSocket 子集；fingerprint、Reality、early-data、smux、Brutal、flow/XUDP/XHTTP 不在 schema 中。
- 锁定 Leaf 不支持 Shadowsocks 2022；只支持本文列出的传统 AEAD cipher。
- UoT v2 使用 TCP，避免 UDP 被封锁时无法建立 association，但仍有 TCP
  head-of-line blocking；它不是无条件的性能加速。
- chain/fallback 首版主要用于 TCP。UDP 使用显式、已验证的 mesh actor。

## 11. 验证清单

部署后至少验证：

1. `--check-config` 成功，未知字段能明确失败。
2. mesh 虚拟 IP 的 ICMP/TCP 在策略开启和 Leaf worker 重启期间保持可用。
3. 国内域名/IP 从 DIRECT 出口出现，境外域名从选定 mesh/chain/fallback 出口出现。
4. UDP 测试源地址是策略 TUN 虚拟 IP，而不是物理网卡地址。
5. 停止网络后 Leaf、HEV、策略 TUN、规则和临时配置全部清理。
6. Android 断 Wi-Fi 前先安排设备侧自动重开 Wi-Fi，再继续 wireless ADB；截图只用于最终视觉确认。
7. 使用 Trojan/VMess/VLESS 时，分别验证 direct 与 mesh 前置 chain 的 TLS HTTP 请求；UDP 与性能必须引用同一精确候选和同一服务端对照。

实现语义参考：Mihomo `rules/parser.go::ParseRule` 的 first-match 规则形状、`adapter/outboundgroup/{parser.go::ParseProxyGroup,fallback.go::{findAliveProxy,DialContext,ListenPacketContext,SupportUDP}}` 的有序组边界；固定 Leaf `proxy/chain/outbound/{stream,datagram}.rs`、`proxy/failover/{stream,datagram}.rs` 和 `proxy/socks/outbound/datagram.rs::Handler::handle`。EasyTier 的有意差异是严格 v1 schema、无主动公共 URL 健康检查、托管 mesh bridge，以及明确不宣称 SOCKS UDP 多跳。
