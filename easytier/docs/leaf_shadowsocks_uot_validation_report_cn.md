# Leaf Shadowsocks 与 UoT v2 功能/性能验证报告

> 状态：候选实现与验证进行中。未完成的项目不能作为发布证据。

## 候选身份

- EasyTier SHA：待候选提交后冻结。
- Leaf SHA：`742ad65c441f9d60279916b82628b810efbd48fb`；EasyTier `Cargo.lock`
  精确 pin 已由 `.160 --locked` 预检验证。
- Gust：`lovitus/gust` `v3.2.9-porty8`，release commit `0f15013`。
- 平台：Linux、Android。
- Linux 主性能路径：两台 10Gbps 双栈公网验证节点之间分别运行 IPv4、IPv6和
  双栈矩阵；内网机器用于大陆 IPv4 对照。`10.20.0.65` 不作为大陆 IPv4 客户端基线。

## 预检证据

- `.160` 标准 `scripts/leaf-remote-preflight.sh` 已对精确 Leaf pin 完成 `--locked`
  no-run；最终增量编译 31.69 秒，新增四项 policy 测试和既有 Leaf/HEV/netstack
  focused suite 全部通过。
- Leaf UoT 测试二进制实际运行 5 项并通过：JSON/protobuf 映射、unconnected request、
  IPv4/IPv6/域名 packet 地址、lazy 首包、帧边界和短缓冲对齐。独立 Leaf 全量 lib
  test 的既有 Tokio-macros/GeoSite lite-protobuf 门槛未伪装为候选失败或成功。
- 两台双栈节点上的 Gust release 文件已校验；`gost` SHA-256 为
  `46ebef5815c6918f1c6e6102cc22a1af5398e92eee4070c05bddd62825c21647`。

## 功能矩阵

| 场景 | Linux | Android | 证据 |
|---|---|---|---|
| Shadowsocks TCP native | pending | pending | pending |
| Shadowsocks UDP native | pending | pending | pending |
| Shadowsocks UoT v2 native | pending | pending | pending |
| mesh SOCKS -> Shadowsocks TCP | pending | pending | pending |
| mesh SOCKS -> Shadowsocks UoT v2 | pending | pending | pending |
| chain/fallback first-match | pending | pending | pending |
| incompatible UoT fail-closed | pending | pending | pending |
| stop/start and resource baseline | pending | pending | pending |

## 性能矩阵

同一候选、同一服务器、同一 cipher、同一 payload 和相同测试时长比较：

| 路径 | TCP throughput | UDP throughput/loss | CPU | RSS | 状态 |
|---|---:|---:|---:|---:|---|
| SS native | pending | pending | pending | pending | pending |
| SS UoT v2 | n/a | pending | pending | pending | pending |
| mesh -> SS native | pending | pending | pending | pending | pending |
| mesh -> SS UoT v2 | n/a | pending | pending | pending | pending |

UoT 的目标是受限网络中的 UDP 可用性，不预设其吞吐优于标准 UDP。报告必须记录
RTT、丢包、并发、payload、cipher、Gust 命令和原始结果位置。

## 发布判断

pending。只有精确 artifact 的 Linux/Android 功能、资源和性能矩阵全部完成后更新。
