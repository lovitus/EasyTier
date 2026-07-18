# Leaf Trojan、VMess、VLESS 验证报告

状态：首个候选 `bfbe4de5` 已完成精确 artifact 与部分远端矩阵，但因 VLESS WSS ALPN 互操作失败被否决；最小修复候选待重新预检和构建。未完成的单元格不得解释为通过。

## 实现边界

- 锁定 Leaf：`lovitus/leaf@742ad65c441f9d60279916b82628b810efbd48fb`。
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

## 功能矩阵

| 协议与路径 | TCP/TLS HTTP | 域名/SNI/Host | UDP | stop/start | 结果 |
| --- | --- | --- | --- | --- | --- |
| Trojan TLS direct | HTTPS 204、64 MiB 完整 | 服务端确认远端代理来源 | 待测 | 一次清理通过 | TCP 通过；给定节点需 `insecure`，fingerprint pin 未支持 |
| Trojan TLS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS direct | HTTPS 3/3、64 MiB 完整 | WS Host 有效，服务端确认代理来源 | 待测 | 一次清理通过 | TCP 通过 |
| VMess WS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS direct | `bfbe4de5` 超时/空响应 | sing-box 有/无 early-data 均通过 | 待测 | 一次清理通过 | 否决：WSS TLS 缺少 `http/1.1` ALPN |
| VLESS WSS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |

测试凭据只写远端临时文件，不进入本报告或仓库。

### `bfbe4de5` VLESS WSS 否决原因

- sing-box `1.13.14` 在相同 lv1g3、相同节点、相同目标下，有 early-data 与移除 early-data 都返回 HTTPS 204，并完整传输 64 MiB，因此不是节点失效或 early-data 必需。
- EasyTier 的 VLESS worker、TUN 与 TLS actor 均成功启动，但 WSS 请求超时或被空响应关闭。
- 锁定 Leaf `742ad65c` 的 WS actor 编译语义和 Mihomo `0a87b948` 的 `adapter/outbound/vless.go::StreamConnContext` 都把 WSS ALPN 限定为 `http/1.1`。原 EasyTier 通用 TLS actor没有设置 ALPN，Cloudflare 可以协商 h2，而后续 Leaf WS actor仍发送 HTTP/1.1 Upgrade。
- 替换修复仅在 TLS+WebSocket 组合设置 `alpn: [http/1.1]`；普通 Trojan TLS、明文 VMess WS、协议 actor、mesh/HEV/DNS/rules 均不改变。

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
