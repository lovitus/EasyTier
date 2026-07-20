# Leaf Trojan、VMess、VLESS 开发与验证报告

状态：Linux 精确候选 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` 已完成协议、native、mesh chain、fallback、TCP/UDP、IPv4/IPv6、性能和生命周期验证。Android 已完成同 SHA 的构建、签名和制品完整性验证，但本轮没有可用设备执行三协议实机流量，因此 Android 运行时仍是明确缺口。

本文不是只记录最终成功结果。它同时记录开发期间被否决的候选、失败现象、交叉对照、错误假设、测试工具修正、最终修复和残余风险。任何标为“未验证”或“无效证据”的项目均不得解释为通过。

## 1. 结论摘要

- Trojan TLS、VMess WebSocket、VLESS WebSocket/TLS 已按窄插件模式接入，没有重构现有 EasyTier mesh、HEV、规则选择器或代理组。
- direct 使用协议 native actor；`via: mesh` 复用现有 mesh SOCKS actor作为 chain 第一跳，再进入协议 actor。native 与 mesh 两种路径均完成 Linux TCP/UDP 实测。
- 初始 VLESS 失败不是 EasyTier mesh 问题。先后定位并修复了两个独立问题：锁定 Leaf 强制发送 Vision flow，以及代理端点域名经 `direct:system` 回到 FakeDNS 的 bootstrap 回环。
- 最终 Linux 候选在原始节点、CDN 节点、双栈、强制 IPv4、强制 IPv6、受控 IPv6 literal、native、mesh chain 和 fallback 上均通过。
- 原始节点上 EasyTier 与 sing-box 吞吐同量级。CDN Trojan/VLESS 上 EasyTier 约为 sing-box SOCKS 的一半，但 `MATCH,DIRECT` 控制组也只有约 `285-290 Mbit/s`，证据指向通用 policy TUN/Leaf 数据面上限，而不是新协议 actor 的独立性能故障。
- VMess CDN 路径在 EasyTier 和 sing-box 中都高度波动，出现过一次 60 秒未传完和一次远端断流；相邻重试通过，当前没有稳定证据证明协议实现错误。
- Android 不能沿用 Linux 结果，也不能沿用其他 Leaf 候选的历史实机结果。本报告只声明本候选 APK 构建、签名和完整性通过。

## 2. 实现与兼容边界

- 最终锁定 Leaf fork：`lovitus/leaf@36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`。
- Leaf fork 的唯一协议行为修改是：VLESS 未配置 flow 时发送标准无-flow 请求，按标准无-flow响应处理；没有引入 Vision、XUDP、XHTTP 或新的公共配置。
- EasyTier 增加三个窄协议配置编译器，以及一个 crate-private TLS/WebSocket 编译层。
- direct 使用协议 native actor；mesh 前置使用既有 SOCKS actor和普通 chain，不让协议插件拥有 mesh transport。
- EasyTier mesh、HEV、KCP/QUIC选择、规则首匹配、chain/fallback选择器和生命周期没有因三协议接入而重构。
- 当前不接受或不宣称：Shadowsocks 2022、Trojan fingerprint/uTLS、smux、Brutal、VMess legacy alter-id、VLESS flow/XTLS/XUDP/XHTTP、Reality 和 WebSocket early-data。
- 未知或不支持字段必须 fail-closed；不能把 Mihomo/sing-box 配置原样粘贴后假定所有字段生效。

## 3. 开发候选与否决时间线

| 阶段 | EasyTier / Leaf 候选 | 构建证据 | 运行时证据 | 判断 |
| --- | --- | --- | --- | --- |
| 初始三协议插件 | EasyTier `bfbe4de5129298b1c15ea3a7e1132e376bfcc811` | Linux [29646685998](https://github.com/lovitus/EasyTier/actions/runs/29646685998)、Android [29646686016](https://github.com/lovitus/EasyTier/actions/runs/29646686016) 成功；Linux制品元数据、SHA256、musl target、Build ID通过 | Trojan direct、VMess WS direct通过；VLESS WSS超时/空响应；同节点 sing-box有/无 early-data均通过 | 否决。VLESS存在真实互操作缺陷 |
| WSS ALPN 假设 | EasyTier `a36343304a34f1510a63a0d66002012ed0ec6fa2` | 生成配置确认含 `http/1.1` ALPN | VLESS仍失败 | 否决。ALPN缺口真实，但不是充分根因 |
| VLESS no-flow修复 | Leaf `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb` | `.160` 独立 integration tests `3/3`，主仓库 `--locked` no-run 与 focused suite通过 | 标准请求字节、分片响应、响应 addon、非法版本均覆盖 | Leaf修复本身通过，进入主仓库候选 |
| no-flow精确制品 | EasyTier `de3e03887917dea4765dc83bb5f21db6b266df19` | Linux [29649710067](https://github.com/lovitus/EasyTier/actions/runs/29649710067)、Android [29649710096](https://github.com/lovitus/EasyTier/actions/runs/29649710096) 成功，Linux制品校验通过 | plain VLESS、VLESS+WS、VLESS+WSS均通过；强制v4/v6通过；域名形式CDN VLESS仍失败 | 否决。协议已正常，出现独立DNS bootstrap回环 |
| DNS bootstrap修复 | EasyTier `1059c21d88d06d10c9c965750269484dbc7dcbcf` | Linux [29651804523](https://github.com/lovitus/EasyTier/actions/runs/29651804523) 为 `87/88`；新增行为测试通过，唯一失败是旧快照仍期待 `direct:system` | 运行时代码和精确测试通过 | 代码方向正确，候选因机械快照错误不作为最终制品 |
| 最终精确候选 | EasyTier `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` | Linux [29651991456](https://github.com/lovitus/EasyTier/actions/runs/29651991456)、Android [29651991435](https://github.com/lovitus/EasyTier/actions/runs/29651991435) 成功 | Linux完整功能、对照、性能、IPv6、fallback、生命周期矩阵通过 | Linux验收候选；Android实机流量待补 |

这条时间线说明构建成功从未被当作功能成功：`bfbe4de5` 和 `de3e0388` 的 Linux/Android workflow 都成功，但仍因真实互操作失败被否决。

## 4. 预检、配置与前端证据

最终批次进入 workflow 前完成以下远程预检：

| 项目 | 结果 |
| --- | --- |
| Rust 1.95 / edition 2024 rustfmt | 通过 |
| `.160` 最小 feature `--locked` no-run | 通过；EasyTier、easytier-policy、netstack-smoltcp lib test binary均生成 |
| 既有 Leaf/HEV/netstack focused suite | 通过 |
| 新协议 schema 严格校验 | 通过 |
| Leaf actor编译及 actor顺序测试 | 通过 |
| VLESS no-flow Leaf integration tests | `3/3` 通过 |
| DNS `system` 展开测试 | `expands_system_dns_to_captured_platform_servers_for_proxy_bootstrap` 通过 |
| policy document / policy editor Vitest | `29/29` 通过 |
| frontend `vue-tsc -b` | 通过 |
| frontend production build | 通过，344 modules transformed |

配置、前端和后端共同验证了：协议类型、UUID/password、cipher、TLS、SNI、WebSocket Host/path、UDP capability、YAML往返，以及未知字段拒绝。真实节点凭据只存在于远端私有配置，没有进入测试 fixture、仓库文档或默认模板。

## 5. VLESS 两个独立根因的隔离过程

### 5.1 根因一：锁定 Leaf 强制 Vision

初始症状：

- Trojan TLS 和 VMess WS 使用同一 EasyTier 数据面可以工作。
- VLESS WSS 在 EasyTier 中超时或被空响应关闭。
- 同主机、同节点、同目标、相邻时间的 sing-box成功。
- 去掉 sing-box early-data 后仍成功，排除 early-data依赖。
- 补 `http/1.1` ALPN 后 EasyTier仍失败，排除“只有ALPN”这一假设。

源码对照：

- 锁定 Leaf `742ad65c` 的 `proxy/vless/stream.rs::build_vless_tcp_header` 无条件写入 `xtls-rprx-vision` addon，并始终启用 Vision响应解析；公共配置却没有 flow字段。
- Mihomo `transport/vless/conn.go::{sendRequest,recvResponse,newConn}` 只在显式配置 flow 时编码 addon并启用 Vision；未配置 flow时发送长度 `0`。

修复：Leaf fork `36ba707f` 只恢复标准无-flow语义。独立测试覆盖精确请求字节、分片响应、服务端响应 addon 和非法版本，不改变UDP或其他协议。

### 5.2 根因二：代理端点域名回到 FakeDNS

`de3e0388` 加入 no-flow修复后，域名节点仍失败。为避免再次直接改协议层，执行了以下分层隔离：

| 对照 | 结果 | 排除项 |
| --- | --- | --- |
| 同主机/同节点/同目标 sing-box | 64 MiB完成，约 `512 Mbit/s`；EasyTier 12秒零字节 | 服务端失效、目标失效、当时公网故障 |
| 临时 plain VLESS服务端 | 64 MiB完成，约 `294 Mbit/s` | VLESS无-flow编码/解码 |
| 临时 VLESS+WS服务端 | 64 MiB完成，约 `295 Mbit/s` | WebSocket层与 actor顺序 |
| 临时 VLESS+WSS服务端 | 64 MiB完成，约 `275 Mbit/s` | TLS、ALPN、WS组合 |
| CDN节点强制IPv4 | 64 MiB完成，约 `272 Mbit/s` | IPv4公网路径 |
| CDN节点强制IPv6 | 64 MiB完成，约 `291 Mbit/s` | IPv6公网路径 |

最终进程/socket证据显示，域名配置下 Leaf worker没有连接真实代理地址，而是持续向 IPv4 FakeIP `198.19.0.4:443` 和 IPv6 FakeIP `fd65:6173:7974::4:443` 发起 SYN。根因链为：

1. 默认 `dns.direct` 包含 `system`。
2. 编译结果保留为 Leaf `direct:system`。
3. TUN 接管后，system resolver已经指向 Leaf virtual DNS。
4. 代理服务器自身的域名解析返回 FakeIP。
5. Leaf把 FakeIP当作真实代理端点连接，形成 bootstrap回环和重试。

参考语义：

- Mihomo `hub/executor/executor.go::updateDNS` 为代理端点单独设置 `ProxyServerHostResolver`，`component/dialer/dialer.go::parseAddr` 使用该 resolver。
- sing-box `common/dialer/dialer.go::NewWithOptions` 也为 domain server address构造独立 resolve dialer。

最小修复只位于 `easytier-policy/src/leaf_config.rs::compile_dns_servers`：在TUN接管前把 `system` 展开为宿主捕获的底层DNS IP，去重并生成 `direct:<IP>`；没有平台DNS时不退回 `direct:system`，保持 fail-closed。没有修改 VLESS、TLS/WS、FakeDNS、mesh、HEV、路由或代理组。

最终 `0cf36807` 的域名节点连续三次64 MiB通过，UDP `3/3`，worker对IPv4/IPv6 FakeIP的代理端点 socket均为 `0`，关闭该回环。

## 6. Linux 最终功能矩阵

每个64 MiB场景均检查传输字节完整性。UDP结果记录为请求/响应 `3/3`，不能用TCP成功代替UDP证据。

| 场景 | Trojan | VMess | VLESS | 路径证据 | 结果 |
| --- | --- | --- | --- | --- | --- |
| 原始节点 native | TCP 64 MiB + UDP `3/3` | TCP 64 MiB + UDP `3/3` | TCP 64 MiB + UDP `3/3` | 协议 actor直接出站 | 通过 |
| 原始节点 mesh chain | TCP 64 MiB + UDP `3/3` | TCP 64 MiB + UDP `3/3` | TCP 64 MiB + UDP `3/3` | 观察到精确 egress peer的 mesh transport | 通过 |
| CDN双栈 native/mesh | 两路径均通过 | 两路径均通过 | 两路径均通过 | 域名端点；FakeIP socket为0 | 通过 |
| CDN强制IPv4 native/mesh | 两路径均通过 | 重试批次两路径均完成 | 两路径均通过 | 代理服务器连接族为IPv4 | 通过，VMess高波动 |
| CDN强制IPv6 native/mesh | 两路径均通过 | 两路径均通过 | 两路径均通过 | 代理服务器连接族为IPv6 | 通过 |
| fallback | 不单独重复 | 不单独重复 | `failed native SOCKS -> mesh VLESS chain` | 首项固定拒绝，实际连接mesh egress | TCP/UDP通过 |
| 受控IPv6 literal目标 | native/mesh均通过 | native/mesh均通过 | native/mesh均通过 | 目标端tcpdump确认代理出口来源 | 六场景通过 |

## 7. chain 与 fallback 验证

- chain：原始和CDN的Trojan、VMess、VLESS均验证 `mesh-hop -> protocol-native`。TCP、UDP和实际mesh transport观察均通过。
- fallback：首项使用固定拒绝的私有 native SOCKS `127.0.0.1:1`，第二项使用已通过的 `mesh-hop -> VLESS` chain。
- fallback连续三次64 MiB完成，UDP `3/3`，中位约 `202 Mbit/s`。
- 该测试证明的是“首项建立失败后切换到chain”，不是主动健康检查、首包后故障迁移或无损会话迁移。不能扩大为Mihomo完整fallback语义。

## 8. IPv4、IPv6与双栈证据

这里区分两类地址族验证：

- 代理端点地址族：CDN域名分别以双栈、强制IPv4和强制IPv6运行，三协议native/mesh均完成。
- 最终目标地址族：另用不在本地直连前缀内的受控IPv6 literal目标，目标端抓包确认流量来自代理出口。

### 8.1 被作废的同机房IPv6结果

lv1g2与lv1g3的公网IPv6位于同一物理 `/48`。该直连 `/48` 比policy auto-route的 `::/1` 和 `8000::/1` 更具体，因此最初访问lv1g3 IPv6 literal得到的多Gbit结果实际旁路了policy。tcpdump确认后，此结果被明确作废，未计入通过矩阵。

这不是产品回归，而是路由优先级导致的测试拓扑错误；行为与Mihomo/sing-box同类auto-route语义一致。

### 8.2 有效的受控IPv6目标

改用不在直连前缀内的受控KR IPv6 literal目标后：

| 路径 | Trojan | VMess | VLESS |
| --- | ---: | ---: | ---: |
| native单次吞吐 | 108 Mbit/s | 54 Mbit/s | 77 Mbit/s |
| mesh单次吞吐 | 114 Mbit/s | 52 Mbit/s | 85 Mbit/s |

六个场景均完成64 MiB。目标端tcpdump观察到代理出口来源而不是lv1g2，证明没有本地旁路。临时HTTP服务及唯一带注释的IPv6防火墙规则在测试后删除。

## 9. 性能与同时间对照

除受控IPv6 literal表明确标记为单次外，以下EasyTier表使用三次64 MiB传输中位数，单位为Mbit/s。

### 9.1 EasyTier native 与 mesh

| 节点/地址族 | Trojan native | Trojan mesh | VMess native | VMess mesh | VLESS native | VLESS mesh |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 原始节点 | 254 | 216 | 186 | 187 | 167 | 174 |
| CDN双栈 | 305 | 224 | 72 | 45 | 288 | 207 |
| CDN强制IPv4 | 285 | 67 | 98 | 69 | 278 | 207 |
| CDN强制IPv6 | 268 | 219 | 56 | 56 | 281 | 240 |

原始VLESS在同时间补充复测为 `3/3`、中位 `176 Mbit/s`；表中的 `167 Mbit/s` 保留首次完整批次结果，不用后测覆盖原始数据。

### 9.2 CDN 节点 sing-box direct 对照

同一lv1g2客户端、相同私有CDN节点、相邻时间、相同64 MiB IPv4目标：

| 代理端点地址族 | Trojan | VMess | VLESS |
| --- | ---: | ---: | ---: |
| 双栈 | 581 | 45 | 589 |
| 强制IPv4 | 609 | 41 | 572 |
| 强制IPv6 | 587 | 92 | 596 |

### 9.3 原始节点 sing-box 对照

| 原始节点 | EasyTier native | EasyTier mesh chain | sing-box direct |
| --- | ---: | ---: | ---: |
| Trojan | 254 | 216 | 233 |
| VMess WS | 186 | 187 | 154 |
| VLESS WSS | 176（同时间复测） | 174 | 188（五次复测中位） |

Trojan原始节点使用自签名证书。用户提供的 `fingerprint` 经Mihomo `component/ca/fingerprint.go::NewFingerprintVerifier` 走读确认是整张DER证书的SHA-256 pin，而不是ClientHello指纹。sing-box对照先验证DER摘要匹配，再以私有 `certificate_path` 建立trust anchor，没有使用 `insecure: true`。EasyTier当前不支持该fingerprint字段，这一安全兼容缺口不能因功能测试通过而隐藏。

### 9.4 数据面控制组

- 裸IPv4网络三次中位约 `5.55 Gbit/s`。
- EasyTier policy `MATCH,DIRECT` 三次中位约 `285 Mbit/s`。
- core `--multi-thread --multi-thread-count 4` 后约 `290 Mbit/s`，没有实质改善。
- CDN Trojan/VLESS native与policy DIRECT接近，而sing-box SOCKS约为其两倍。因此当前上限归类为通用policy TUN/Leaf数据面问题，不归类为Trojan/VLESS actor缺陷。
- sing-box使用内核SOCKS inbound，EasyTier使用透明policy TUN，二者入口不等价；对照用于用户可感知的现实上限和故障定位，不是微基准等价性声明。
- 本轮没有对每个场景统一采集可比较的CPU曲线，因此不在报告中伪造协议级CPU结论。

## 10. 波动、失败与重试的分类

| 现象 | 相邻对照 | 分类 |
| --- | --- | --- |
| CDN强制IPv4 VMess首次第三次60秒仅完成约49 MiB | 紧邻sing-box中位约54 Mbit/s；EasyTier重跑三次全部完成，中位约98 Mbit/s | 节点/CDN高波动，保留风险，不判定固定Leaf错误 |
| 原始VLESS sing-box一次只剩约32 KiB时远端关闭，curl 18 | sing-box紧邻重试 `5/5`；EasyTier `3/3`、UDP `3/3`、FakeIP socket 0 | 节点瞬态，不构成Leaf回归 |
| `bfbe4de5` VLESS失败 | sing-box同条件成功，ALPN修复仍失败，源码发现强制Vision | 产品/依赖实现缺陷，已修复 |
| `de3e0388` 域名VLESS失败 | plain/WS/WSS及强制v4/v6均成功；worker连接FakeIP | 产品DNS bootstrap缺陷，已修复 |
| 同机房IPv6 literal出现多Gbit结果 | tcpdump显示直连 `/48` 旁路 | 无效测试证据，已作废并更换目标 |
| `1059c21d` Linux suite 87/88 | 新行为和精确测试通过；唯一失败为旧快照期待 `direct:system` | 机械测试快照错误，不是运行时回归 |

## 11. 生命周期与资源回基线

- native与fallback-mesh交替执行10轮stop/start，十轮TCP/UDP均通过。
- 最终lv1g2残留：core `0`、Leaf worker `0`、TUN `0`、本候选Leaf临时文件增量 `0`。
- 常驻lv1g3 egress peer基线约 `25 MiB RSS / 7 threads / 27 FDs`。
- 流量高峰约 `36 MiB / 19 threads / 28 FDs`。
- 空闲90秒后约 `25 MiB / 7 threads / 26 FDs`，QUIC/KCP会话资源回落，没有观察到持续增长。
- 这是一轮10次循环和90秒有界观察，不等价于数小时或数日soak；长期稳定性仍不能从该数据外推。

## 12. Android 制品证据与明确缺口

最终Android workflow [29651991435](https://github.com/lovitus/EasyTier/actions/runs/29651991435) 已确认：

- workflow head SHA为 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`；
- target为 `aarch64-linux-android`；
- HEV pin与候选一致；
- 三个APK的SHA256、ZIP完整性和签名证书摘要通过。

本轮三协议开发结束时Android设备不可用，因此以下项目没有被验证，不能引用其他候选的Android Leaf结果代替：

- Trojan、VMess、VLESS native实机流量；
- 三协议经mesh chain和fallback；
- 三协议UDP；
- Wi-Fi/蜂窝切换后的协议重建；
- 本批协议负载下的FD、线程、RSS和耗电回基线。

## 13. 测试工具与环境纠正记录

开发期间也修正了多项测试编排问题。它们必须记录，否则重复测试看起来会像产品反复回归：

| 问题 | 影响 | 处理 |
| --- | --- | --- |
| UDP采集脚本在严格shell模式引用未绑定变量 | UDP批次提前退出 | 修正collector，再重跑UDP；旧结果不计 |
| mesh runner在 `pipefail` 下因进程查询无匹配提前退出 | 误报mesh场景未启动 | 改为显式容忍空查询并验证真实mesh transport |
| 初始脚本假定错误的mesh peer地址 | 无法命中实际私有配置的egress | 按实际私有配置修正；错误批次作废 |
| fallback YAML生成器曾遗漏或重复 `rules:` | check-config/启动失败 | 修正生成器并以最终成功配置重跑，不归因于fallback实现 |
| SLL2上的tcpdump过滤器不接受 `tcp[13]` 表达式 | 首次抓包命令失败 | 改用兼容过滤器，重新收集连接族和来源证据 |
| lv1g2/lv1g3 `/slab2` 的NFS视图未及时显示新文件 | 两端看到的制品目录不一致 | 一次性传输精确artifact并校验SHA；不视为产品问题 |

所有这些纠正都没有通过修改被测候选来“修好测试”；只有重新获得有效证据后才更新矩阵。

## 14. 制品、清理与敏感信息

- Linux制品核对了workflow SHA、外层ZIP、tarball、内层二进制SHA256、`BUILD_INFO.txt`、`x86_64-unknown-linux-musl` target、静态PIE、Build ID、debug info和HEV pin。
- Android制品核对了workflow SHA、target、HEV pin、APK SHA256、签名元数据和APK ZIP完整性。
- 私有节点地址、域名、UUID、密码、证书和WebSocket路径只存于远端权限为 `0600` 的临时配置，不写入仓库。
- 协议验证的临时服务、进程、TUN、专用端口和精确防火墙规则均已清理；既有生产sing-box服务未被停止或覆盖。
- 仓库源文件和文档执行了针对节点秘密的扫描，没有发现私有端点或认证信息。

## 15. 目标逐项审计

| 用户目标 | 证据 | 状态 |
| --- | --- | --- |
| 以插件方式接入Trojan、VMess、VLESS | schema、compiler、TLS/WS层、`.160`测试、精确workflow | 完成 |
| 不重构mesh/HEV/规则和代理组 | 实现边界审计；native actor与现有mesh chain组合 | 完成 |
| 三协议direct可访问 | 原始与CDN的64 MiB TCP、UDP `3/3` | Linux完成 |
| 三协议经过mesh再出站 | 原始与CDN mesh chain、真实mesh transport、TCP/UDP | Linux完成 |
| chain与fallback | 六类mesh chain；固定失败首项切换到VLESS mesh chain | Linux完成 |
| IPv4、IPv6、双栈 | 代理端点双栈/强制v4/v6；外部前缀IPv6 literal及目标端抓包 | Linux完成 |
| 与sing-box交叉验证 | 原始与CDN节点、相同主机/节点/目标/时间窗口 | 完成 |
| 性能报告 | EasyTier native/mesh、sing-box、raw、policy DIRECT、多线程控制组 | 完成；通用数据面上限待优化 |
| 生命周期与资源 | 10轮stop/start、进程/TUN/临时文件归零、peer资源回落 | Linux完成 |
| Android构建 | 同SHA APK、签名、哈希、target | 完成 |
| Android三协议实机 | 无设备，不借用其他候选结果 | 未完成 |
| 不泄露节点秘密 | 匿名标签、私有远端配置、仓库扫描 | 完成 |

## 16. 发布边界与残余风险

当前可以声明：

- Linux上的Trojan TLS、VMess WS、VLESS WS/WSS窄子集可用；
- 三协议可作为native出站，也可位于现有mesh hop之后；
- 本报告覆盖的TCP、UDP、chain、fallback和地址族组合通过；
- VLESS no-flow和代理端点DNS bootstrap两个已知阻塞已闭环。

当前不能声明：

- Android三协议运行时已验收；
- fingerprint/uTLS、Reality、early-data、smux、Brutal、flow/XUDP/XHTTP或SS2022兼容；
- EasyTier policy TUN性能达到sing-box内核SOCKS水平；
- VMess CDN吞吐稳定；
- 10轮生命周期等价于长期soak。

首版残余风险按优先级为：

1. Android精确APK仍需完成三协议native/mesh/fallback、UDP、网络切换和资源回基线。
2. policy TUN/Leaf通用单流上限约 `285-290 Mbit/s`，CDN Trojan/VLESS可明显慢于sing-box SOCKS。
3. VMess CDN路径存在高波动，发布说明应避免承诺稳定吞吐。
4. unsupported字段必须继续严格拒绝，不能为了“兼容配置”静默忽略。

## 17. 历史精确制品逐次数据

前述性能表只保留中位数，不足以反映波动和资源代价。本节直接从 lv1g2 保留的 `0cf36807` 原始 `.result`/`.tsv` 复算，不引用人工摘要。

### 17.1 EasyTier native 与 mesh 原始批次

每行包含三次 64 MiB TCP、TCP 返回码、UDP 请求/响应、FakeIP socket 计数，以及 TCP+UDP 完成后采集的 core+Leaf worker 资源快照。这里的 RSS/FD/线程是“测试后单点快照”，不是 idle/peak；第 18 节另有统一 200 ms 采样的 idle/transfer-peak/post 对照。

| 场景 | 三次TCP Mbit/s | 中位 | rc | UDP | FakeIP | 总RSS MiB | 总FD | 总线程 |
| --- | --- | ---: | --- | --- | ---: | ---: | ---: | ---: |
| original-trojan-native | 243.1/290.9/253.6 | 253.6 | 0/0/0 | 3/3 | 0 | 37.1 | 44 | 8 |
| original-vmess-native | 175.7/185.7/193.8 | 185.7 | 0/0/0 | 3/3 | 0 | 39.2 | 48 | 8 |
| original-vless-native | 189.3/165.7/166.9 | 166.9 | 0/0/0 | 3/3 | 0 | 40.6 | 56 | 8 |
| cdn-trojan-native | 278.9/304.6/305.7 | 304.6 | 0/0/0 | 3/3 | 0 | 38.9 | 44 | 8 |
| cdn-vmess-native | 44.0/92.8/72.2 | 72.2 | 0/0/0 | 3/3 | 0 | 36.9 | 50 | 8 |
| cdn-vless-native | 260.2/288.0/300.9 | 288.0 | 0/0/0 | 3/3 | 0 | 38.5 | 45 | 8 |
| cdn-trojan-native-v4 | 336.8/284.8/278.6 | 284.8 | 0/0/0 | 3/3 | 0 | 38.4 | 44 | 8 |
| cdn-vmess-native-v4 | 44.5/27.5/0.0 | 27.5 | 0/0/28 | 3/3 | 0 | 36.2 | 45 | 8 |
| cdn-vmess-native-v4-repeat | 98.7/98.0/33.4 | 98.0 | 0/0/0 | 3/3 | 0 | 38.2 | 49 | 8 |
| cdn-vless-native-v4 | 266.2/284.7/277.7 | 277.7 | 0/0/0 | 3/3 | 0 | 36.6 | 48 | 8 |
| cdn-trojan-native-v6 | 267.8/276.6/252.2 | 267.8 | 0/0/0 | 3/3 | 0 | 38.1 | 44 | 8 |
| cdn-vmess-native-v6 | 44.3/89.8/56.4 | 56.4 | 0/0/0 | 3/3 | 0 | 38.1 | 44 | 8 |
| cdn-vless-native-v6 | 281.4/280.6/267.9 | 280.6 | 0/0/0 | 3/3 | 0 | 38.4 | 43 | 8 |
| original-trojan-mesh | 188.3/216.5/229.6 | 216.5 | 0/0/0 | 3/3 | 0 | 40.0 | 49 | 8 |
| original-vmess-mesh | 186.9/192.0/148.7 | 186.9 | 0/0/0 | 3/3 | 0 | 41.0 | 51 | 16 |
| original-vless-mesh | 169.1/174.4/186.5 | 174.4 | 0/0/0 | 3/3 | 0 | 40.7 | 54 | 8 |
| cdn-trojan-mesh | 188.6/223.7/238.2 | 223.7 | 0/0/0 | 3/3 | 0 | 43.4 | 59 | 8 |
| cdn-vmess-mesh | 41.2/97.6/45.4 | 45.4 | 0/0/0 | 3/3 | 0 | 41.8 | 64 | 8 |
| cdn-vless-mesh | 193.1/207.3/217.9 | 207.3 | 0/0/0 | 3/3 | 0 | 56.4 | 52 | 8 |
| cdn-trojan-mesh-v4 | 184.9/40.7/66.7 | 66.7 | 0/0/0 | 3/3 | 0 | 42.6 | 121 | 9 |
| cdn-vmess-mesh-v4 | 66.2/68.5/79.2 | 68.5 | 0/0/0 | 3/3 | 0 | 42.1 | 52 | 8 |
| cdn-vless-mesh-v4 | 185.2/207.8/207.0 | 207.0 | 0/0/0 | 3/3 | 0 | 42.5 | 48 | 8 |
| cdn-trojan-mesh-v6 | 197.0/226.3/219.5 | 219.5 | 0/0/0 | 3/3 | 0 | 42.9 | 56 | 8 |
| cdn-vmess-mesh-v6 | 74.3/56.2/39.8 | 56.2 | 0/0/0 | 3/3 | 0 | 41.2 | 61 | 8 |
| cdn-vless-mesh-v6 | 225.6/240.0/243.8 | 240.0 | 0/0/0 | 3/3 | 0 | 42.7 | 55 | 16 |

`cdn-vmess-native-v4` 的第三次 `rc=28` 是 60 秒内未传完，表中的 `0.0` 是 curl 未完成结果，不代表链路真实吞吐为零。相邻完整重跑为 `98.7/98.0/33.4 Mbit/s`。`cdn-trojan-mesh-v4` 的 121 FD 和 `cdn-vless-mesh` 的 56.4 MiB RSS 是活跃 mesh transport 后的单点值；第 11 节的 90 秒观察确认 peer 回到约 `25 MiB/7线程/26 FD`，因此不能把单点峰值误报为泄漏。

### 17.2 原始 sing-box 逐次数据

这些是开发阶段保存的 sing-box SOCKS 客户端数据。当时没有同步记录 sing-box 资源，因此只作为吞吐/波动证据；缺失的资源维度已在第 18 节用相同配置重新采集。

| 场景 | 逐次 Mbit/s | 中位 | 完整性 |
| --- | --- | ---: | --- |
| original-trojan | 232.7/257.4/232.1 | 232.7 | 3/3 HTTP 200、64 MiB |
| original-vmess | 153.6/125.5/158.3 | 153.6 | 3/3 HTTP 200、64 MiB |
| original-vless initial | 163.7 | 单次 | 首次完成；第二次在剩约 32 KiB 时远端关闭 |
| original-vless retry | 192.4/96.4/189.0/186.0/188.3 | 188.3 | 5/5 HTTP 200、64 MiB |
| cdn-trojan-dual | 535.9/586.7/580.8 | 580.8 | 3/3 |
| cdn-trojan-v4 | 577.8/609.4/612.4 | 609.4 | 3/3 |
| cdn-trojan-v6 | 599.4/586.7/574.7 | 586.7 | 3/3 |
| cdn-vmess-dual | 44.6/36.2/52.6 | 44.6 | 3/3 |
| cdn-vmess-v4 | 63.2/40.7/30.0 | 40.7 | 3/3 |
| cdn-vmess-v6 | 28.6/93.7/92.2 | 92.2 | 3/3 |
| cdn-vless-dual | 571.9/595.9/589.4 | 589.4 | 3/3 |
| cdn-vless-v4 | 571.7/589.2/530.2 | 571.7 | 3/3 |
| cdn-vless-v6 | 523.6/595.7/612.5 | 595.7 | 3/3 |

## 18. 同时间 EasyTier / sing-box 资源交叉验证

由于历史 sing-box 结果缺少资源记录，2026-07-19 使用同一 `0cf36807` Linux artifact 和原有私有节点配置补跑。没有修改代码、配置语义或节点，没有构建新二进制，也没有触发 workflow。

方法：

- 每个实现启动后等待稳定，再采集 idle。
- 每个场景连续传输三次 64 MiB，总计 192 MiB。
- 每 200 ms采集RSS、FD和线程；CPU使用 `/proc/<pid>/stat` 的 user+system ticks计算。
- EasyTier计入 core和Leaf worker之和；sing-box计入单一客户端进程。curl自身不计入两边。
- EasyTier使用透明policy TUN，sing-box使用内核SOCKS inbound，入口并不等价；这是端到端用户成本对照，不是纯协议actor微基准。
- 每组结束后采集post，再正常停止并检查进程/TUN清理。

### 18.1 相邻吞吐与CPU

CPU占比受传输持续时间影响；“CPU秒比”是在两边都传输192 MiB后比较CPU总时间，比瞬时百分比更适合判断单位数据成本。

| 场景 | EasyTier三次/中位 Mbit/s | sing-box三次/中位 Mbit/s | EasyTier CPU秒/占比 | sing-box CPU秒/占比 | CPU秒比 ET/SB |
| --- | --- | --- | ---: | ---: | ---: |
| original-trojan | 195.1/257.3/263.3 / 257.3 | 648.2/429.7/729.6 / 648.2 | 4.600/66.5% | 0.910/32.0% | 5.05 |
| original-vmess | 136.1/111.9/187.7 / 136.1 | 70.3/191.4/197.7 / 191.4 | 4.900/42.1% | 1.340/10.2% | 3.66 |
| original-vless | 198.1/196.8/171.5 / 196.8 | 195.3/211.1/195.6 / 195.6 | 4.830/56.2% | 1.370/17.0% | 3.53 |
| cdn-trojan | 264.4/280.2/291.5 / 280.2 | 513.4/542.3/581.2 / 542.3 | 4.290/73.7% | 1.550/51.8% | 2.77 |
| cdn-vmess | 36.0/38.3/42.0 / 38.3 | 56.5/46.8/54.7 / 54.7 | 6.040/14.5% | 2.020/6.6% | 2.99 |
| cdn-vless | 262.6/282.9/277.4 / 277.4 | 580.5/546.4/597.2 / 580.5 | 4.380/74.0% | 1.460/51.4% | 3.00 |

### 18.2 RSS、FD、线程与清理

`peak` 是传输窗口内200 ms采样峰值；`post` 是传输完成后3秒快照。短于200 ms的瞬态可能未命中，因此post偶尔高于transfer peak。MiB按KiB/1024换算。

| 场景/实现 | RSS idle/peak/post MiB | FD idle/peak/post | 线程 idle/peak/post | 清理 |
| --- | --- | --- | --- | --- |
| original-trojan/easytier | 33.7/36.9/36.6 | 42/43/41 | 8/8/8 | core=0, worker=0, TUN=0 |
| original-trojan/sing-box | 36.9/41.4/41.4 | 8/10/8 | 5/7/7 | process=0 |
| original-vmess/easytier | 34.0/37.1/36.3 | 43/48/47 | 8/8/8 | core=0, worker=0, TUN=0 |
| original-vmess/sing-box | 36.5/40.2/40.2 | 8/11/8 | 7/7/7 | process=0 |
| original-vless/easytier | 34.5/37.4/37.1 | 43/46/44 | 8/8/8 | core=0, worker=0, TUN=0 |
| original-vless/sing-box | 36.8/44.1/43.9 | 8/10/8 | 7/7/7 | process=0 |
| cdn-trojan/easytier | 34.6/37.9/36.0 | 47/48/49 | 8/8/8 | core=0, worker=0, TUN=0 |
| cdn-trojan/sing-box | 34.9/41.4/41.2 | 8/10/8 | 5/7/7 | process=0 |
| cdn-vmess/easytier | 34.0/37.1/35.7 | 42/60/46 | 8/8/8 | core=0, worker=0, TUN=0 |
| cdn-vmess/sing-box | 36.7/42.6/42.6 | 8/10/8 | 6/7/7 | process=0 |
| cdn-vless/easytier | 34.2/36.5/35.7 | 42/47/50 | 8/8/8 | core=0, worker=0, TUN=0 |
| cdn-vless/sing-box | 36.9/44.2/44.0 | 8/10/8 | 5/7/7 | process=0 |

EasyTier分项范围：core idle约 `24.6-25.2 MiB`、transfer peak约 `25.6-26.6 MiB`；Leaf worker idle约 `8.8-9.8 MiB`、transfer peak约 `10.5-11.8 MiB`。因此本轮没有发现Leaf协议插件造成异常RSS膨胀。

资源结论：

- EasyTier core+worker的idle总RSS为 `33.7-34.6 MiB`，sing-box为 `34.9-36.9 MiB`；EasyTier略低。
- EasyTier transfer peak为 `36.5-37.9 MiB`，sing-box为 `40.2-44.2 MiB`；EasyTier仍较低。
- EasyTier固定为8线程，sing-box为5-7线程；差异不大。
- EasyTier因core、worker、TUN、控制面和多个listener，idle为42-47 FD，显著高于sing-box的8 FD。最慢的CDN VMess传输期间EasyTier达到60 FD，post回到46。
- sing-box的Go heap在3秒post窗口仍保留到peak附近，但每组停止后process为0；这不是进程泄漏。
- EasyTier每组停止后core、worker和TUN均为0；既有生产sing-box进程没有被停止或替换。

### 18.3 性能判断修正

新一轮相邻对照比历史“中位数拼表”更可信，但也再次显示节点和时间波动：历史original Trojan sing-box约233 Mbit/s，本轮中位648 Mbit/s；两份数据都保留，不能择优删除。

- RSS不是当前主要问题。EasyTier总RSS没有高于sing-box，Leaf worker本身约9-12 MiB。
- CPU成本是明确问题。传输相同192 MiB时，EasyTier core+worker消耗的CPU秒为sing-box的 `2.77-5.05` 倍。
- 吞吐差距并非每个协议一致：original VLESS本轮两者约197/196 Mbit/s；CDN Trojan和VLESS的EasyTier中位约为sing-box的52%和48%；VMess两边都波动且EasyTier约为70%。
- 结合 `MATCH,DIRECT` 约285-290 Mbit/s控制组，Trojan/VLESS的上限仍首先指向透明policy TUN/Leaf公共数据路径，而不是某个新协议插件独有的编码错误。
- “协议功能可用”和“性能合格”必须分开表述。当前没有协议级断流阻塞，但CPU效率和部分路径吞吐仍是明确的首版风险。

### 18.4 本轮采样脚本纠正

首轮六个sing-box被采样脚本误判为startup失败。实际listener可正常建立；原因是脚本在 `set -o pipefail` 下使用 `ss | grep -q`，`grep -q`命中后提前关闭管道，`ss`收到SIGPIPE，使整个条件返回失败。修正为完整消费输出后只重跑sing-box六组，EasyTier没有重复运行。manifest同时保留初次失败和修正后pass，避免隐藏编排错误。

## 19. 证据索引

- 候选执行板：[leaf_parallel_workboard.md](./todo/leaf_parallel_workboard.md)
- Leaf/policy验证日志：[leaf_validation_journal.md](./todo/leaf_validation_journal.md)
- 用户配置与字段说明：[leaf_policy_proxy_cn.md](./leaf_policy_proxy_cn.md)
- 协议实现计划与边界：[leaf_trojan_vmess_vless_plugins_undecided.md](./todo/leaf_trojan_vmess_vless_plugins_undecided.md)
- 远端原始结果位于私有 `/slab2/easytier-validation/` 子目录；该目录包含节点秘密，不复制进仓库。本文只保留匿名统计、候选SHA和可公开workflow ID。
- 本次资源交叉验证原始数据：私有 `trojan-vmess-vless-bfbe4de5/lv1g2/resource-crosscheck-0cf36807-20260719/`，包含manifest、12组result和200 ms原始samples；配置与凭据仍位于独立私有目录。

## 2026-07-19 policy 数据面性能根因补充

同一精确候选的分层复测已确认，VLESS actor 本身不是完整 EasyTier 路径约 2 倍性能差距的第一根因。Leaf worker 改用自身 auto-TUN 后，CDN VLESS 中位约 540.0 Mbit/s，接近 sing-box SOCKS 对照 580.5 Mbit/s；完整 EasyTier policy 路径约 277.4 Mbit/s。主要损失来自 EasyTier TUN/classifier、逐包 Unix-datagram bridge、额外内核复制与跨进程唤醒。可信/废弃证据、原始目录和优化边界见 `docs/leaf_policy_dataplane_performance_investigation_cn.md`。
