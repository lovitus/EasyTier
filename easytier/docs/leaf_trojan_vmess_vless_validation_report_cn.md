# Leaf Trojan、VMess、VLESS 验证报告

状态：`bfbe4de5`、`a3634330` 与 `de3e0388` 均已被真实互操作否决。`de3e0388` 证明无-flow VLESS 修复本身正确，但暴露了代理端点域名经 `direct:system` 回到 FakeDNS 的独立自举回环；最小编译边界修复已通过 `.160`，待新 artifact。未完成的单元格不得解释为通过。

## 实现边界

- 锁定 Leaf：`lovitus/leaf@36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`。该提交仅把不可配置的 VLESS TCP 默认 Vision 改为标准无-flow请求/响应，UDP及其他协议未改。
- EasyTier 只增加三个窄协议编译器和一个 crate-private TLS/WebSocket 编译层。
- EasyTier mesh、HEV、DNS、规则、chain/fallback 选择器和生命周期未修改。
- direct 使用 native 协议 actor；mesh 前置使用现有 SOCKS actor 加普通 chain。
- 不支持 Shadowsocks 2022、fingerprint/uTLS、Reality、early-data、smux、Brutal、VLESS flow/XUDP/XHTTP。

## 预检证据

| 项目 | 结果 |
| --- | --- |
| Rust 1.95 / edition 2024 rustfmt | 通过 |
| `.160` `--locked` no-run | 通过；EasyTier、easytier-policy、netstack-smoltcp 三个 lib test binary |
| 既有 Leaf/HEV/netstack focused suite | 通过 |
| 新 protocol schema test | 通过 |
| 新 Leaf actor compiler test | 通过 |
| policy-document / policy-editor Vitest | 29/29 通过 |
| frontend `vue-tsc -b` | 通过 |
| frontend Vite production build | 通过，344 modules transformed |

新增 feature 只使 lockfile 增加 `cfb-mode`、`keccak`、`lz_fnv`、`sha3`、`tokio-tungstenite`、`tungstenite` 六个包。Leaf pin 未改变。

## 精确候选

| 项目 | 值 |
| --- | --- |
| EasyTier SHA | `bfbe4de5129298b1c15ea3a7e1132e376bfcc811`，已否决 |
| Linux workflow / artifact | `29646685998` 成功；精确 musl artifact 已部署 lv1g2/lv1g3 |
| Android workflow / artifact | `29646686016` 成功；当前无设备，不声称实包通过 |
| BUILD_INFO / SHA256 / symbols | commit/run/target、外层和包内 SHA256、Build ID、debug info 均通过 |
| VLESS 无-flow 替换 SHA | `de3e03887917dea4765dc83bb5f21db6b266df19`，Linux/Android `29649710067/29649710096` 成功，artifact 校验通过但域名端点回环，已否决 |

## 功能矩阵

| 协议与路径 | TCP/TLS HTTP | 域名/SNI/Host | UDP | stop/start | 结果 |
| --- | --- | --- | --- | --- | --- |
| Trojan TLS direct | HTTPS 204、64 MiB 完整 | 服务端确认远端代理来源 | 待测 | 一次清理通过 | TCP 通过；给定节点需 `insecure`，fingerprint pin 未支持 |
| Trojan TLS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS direct | HTTPS 3/3、64 MiB 完整 | WS Host 有效，服务端确认代理来源 | 待测 | 一次清理通过 | TCP 通过 |
| VMess WS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS direct | `bfbe4de5`、`a3634330` 均失败 | sing-box 有/无 early-data 均通过 | 待测 | 一次清理通过 | 否决：锁定 Leaf TCP 强制 Vision；无-flow 修复待 artifact |
| VLESS WSS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |

测试凭据只写远端临时文件，不进入本报告或仓库。

### VLESS WSS 候选否决与真实根因

- sing-box `1.13.14` 在相同 lv1g3、相同节点、相同目标下，有 early-data 与移除 early-data 都返回 HTTPS 204，并完整传输 64 MiB，因此不是节点失效或 early-data 必需。
- EasyTier 的 VLESS worker、TUN 与 TLS actor 均成功启动，但 WSS 请求超时或被空响应关闭。
- `bfbe4de5` 确有 WSS ALPN 缺口；`a3634330` 已按 Mihomo `0a87b948` 的 `adapter/outbound/vless.go::StreamConnContext` 增加 `http/1.1`，精确生成配置也确认 TLS、WS Host/path、UUID和 actor 顺序正确，但真实节点仍失败，因此 ALPN 不是充分根因。
- 锁定 Leaf `742ad65c` 的 `proxy/vless/stream.rs::build_vless_tcp_header` 无条件写入 `xtls-rprx-vision` addon，并始终使用 Vision 响应解析器；公共配置却没有 flow 字段。Mihomo `transport/vless/conn.go::{sendRequest,recvResponse,newConn}` 只在显式 flow 时编码 addon/启用 Vision，无 flow 时发送 addon 长度 `0`，校验响应版本并跳过服务端声明的 addon。
- fork `36ba707f` 只把 TCP 改为上述标准无-flow语义，并在响应头完成后直接读取底层流，避免持续复制。独立 integration test 覆盖精确请求字节、分片响应、响应 addon 和非法版本；`.160` 三项通过。EasyTier 仍明确拒绝 flow，不新增配置表面。

## 性能矩阵

`lv1g2`、`lv1g3` 使用 `/slab2`。每组至少三次，报告中位数、CPU、RSS、失败率；direct 与 mesh 前置分别测试，并使用同条件 sing-box 客户端作为对照。IPv4、IPv6、双栈必须分开记录，不能用移动网络或中国大陆受限出口替代 10 Gbps 双栈基线。

| 协议 | 地址族 | sing-box direct | EasyTier direct | EasyTier mesh | 结论 |
| --- | --- | ---: | ---: | ---: | --- |
| Trojan TLS | IPv4 | 待测 | 待测 | 待测 | 待完成 |
| Trojan TLS | IPv6 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS | IPv4 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS | IPv6 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS | IPv4 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS | IPv6 | 待测 | 待测 | 待测 | 待完成 |

## 发布判断

当前只能判断配置编译与前端集成通过，不能判断真实节点互操作、mesh chain UDP 或性能合格。上述功能、生命周期、资源和性能矩阵闭环后再给出发布结论。

## `de3e0388` 的分层隔离与代理端点 DNS 根因

- 同主机、同节点、同目标、相邻时间的 sing-box 下载 64 MiB 成功，约 512 Mbit/s；`de3e0388` 域名配置 12 秒零字节，因此服务端故障已排除。
- 临时标准 sing-box 服务端分别提供 plain VLESS、VLESS+WS、VLESS+WSS。`de3e0388` 三组均完整下载 64 MiB，约 275-295 Mbit/s，证明无-flow VLESS、WebSocket、TLS/ALPN 和组合顺序可用。
- 同一 CDN 节点的强制 IPv4 与强制 IPv6 配置也都完整下载 64 MiB，约 272/291 Mbit/s；地址族和公网链路不是根因。
- 域名配置运行时，Leaf worker 没有连接真实 CDN 地址，而是并发连接 FakeIP `198.19.0.4:443` 与 `fd65:6173:7974::4:443`，持续停在 `SYN-SENT`。默认 `dns.direct` 的 `system` 被原样编译为 Leaf `direct:system`，TUN 接管后该系统入口已指向 FakeDNS，代理端点 bootstrap 查询因此回到自身。
- Mihomo `hub/executor/executor.go::updateDNS` 为代理端点单独设置 `ProxyServerHostResolver`，`component/dialer/dialer.go::parseAddr` 用该 resolver 解析节点地址；sing-box `common/dialer/dialer.go::NewWithOptions` 也为 domain server address 构造独立 resolve dialer。EasyTier 的最小等价修复是在配置编译时把 `system` 展开为 TUN 接管前捕获的底层 DNS IP，不修改 Leaf 或任何协议 actor。
