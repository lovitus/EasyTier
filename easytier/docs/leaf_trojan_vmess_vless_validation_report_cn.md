# Leaf Trojan、VMess、VLESS 验证报告

状态：候选实现已完成 `.160` 编译与聚焦测试，精确 GitHub artifact 和远端功能/性能矩阵待完成。未完成的单元格不得解释为通过。

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
| EasyTier SHA | 待候选提交 |
| Linux workflow / artifact | 待完成 |
| Android workflow / artifact | 待完成 |
| BUILD_INFO / SHA256 / symbols | 待完成 |

## 功能矩阵

| 协议与路径 | TCP/TLS HTTP | 域名/SNI/Host | UDP | stop/start | 结果 |
| --- | --- | --- | --- | --- | --- |
| Trojan TLS direct | 待测 | 待测 | 待测 | 待测 | 待完成 |
| Trojan TLS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS direct | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VMess WS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS direct | 待测 | 待测 | 待测 | 待测 | 待完成 |
| VLESS WSS through mesh | 待测 | 待测 | 待测 | 待测 | 待完成 |

测试凭据只写远端临时文件，不进入本报告或仓库。

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
