# [FAILED / 失败] Leaf External Packet Endpoint / PacketBatch 性能实验总结

> **最终状态：失败，不进入发布，不继续在当前实现上叠加修复。**
>
> `leaf-packet-batch` 没有达到双向性能门槛，并在 Android 实机触发确定性启动
> panic。正确回退基线是
> `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`；该边界保留当时已经存在的
> Leaf/HEV、Shadowsocks/UoT、Trojan、VMess、VLESS、规则和 DNS 等功能，只否决其后
> 的 PacketBatch 性能实验。

日期：2026-07-19

## 1. 原目标

在不改变 EasyTier mesh、规则、DNS、HEV、QUIC/KCP 和代理协议语义的前提下，减少
EasyTier TUN 与 Leaf 之间逐包 IPC、复制和唤醒成本，并满足：

- Android 使用同进程有界 MemoryBatch endpoint；
- Linux 和桌面 worker 使用隔离的 framed StreamBatch endpoint；
- 通过 `leaf-packet-batch` 显式启用，默认关闭；
- 不支持或初始化失败时回退原 legacy packet FD；
- 所有平台使用相同有界 batch、背压、顺序和关闭语义；
- Linux 双向吞吐不得低于 legacy 的 95%，并显著降低 syscall；
- Android 启动、stop/start、DNS、规则、mesh 共存和资源回基线必须通过。

这个目标被过早描述为“通用且较高概率获得大幅收益”。后续实测不支持该判断。

## 2. 实验前已知基线

精确基线：`0cf368072aad4882309e6f6d450e45f5f4e1a9ac`，Linux workflow
`29651991456`，锁定 Leaf `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`。

同一制品的分层 profile 已证明完整 EasyTier policy 路径存在明显逐包开销：

| 路径 | 中位吞吐 |
| --- | ---: |
| 完整 EasyTier policy DIRECT | 约 285 Mbit/s |
| 完整 EasyTier CDN VLESS | 约 277.4 Mbit/s |
| Leaf auto-TUN DIRECT | 约 652.8 Mbit/s |
| Leaf auto-TUN CDN VLESS | 约 540.0 Mbit/s |
| sing-box SOCKS CDN VLESS | 约 580.5 Mbit/s |

完整 EasyTier 路径约有 sing-box SOCKS 的 7.0 至 8.3 倍 syscall，system CPU 和跨进程
切换占比很高。这个根因判断仍然有效；失败的是“用当前 PacketBatch 设计即可安全、通用地
消除该差距”的解决方案判断。

完整基线与原始证据索引见
`docs/leaf_policy_dataplane_performance_investigation_cn.md`。

## 3. 实际实现范围

EasyTier 候选序列：

| 候选 | 内容 |
| --- | --- |
| `aae707ca9236565cdcc31adbeccc2814ff0918b4` | PacketBatch API、MemoryBatch/StreamBatch、配置/CLI/RPC/GUI feature gate |
| `61e9852fd18b83bb96cd5c8c8af69e79dd2e43c4` | 补齐 Tokio `io-util` 最小 feature 构建 |
| `39dd4d2f989a459fb44d9cc8c9aab708338e4f83` | StreamBatch decoder 改为一次读取连续 frame body |
| `ca5751fb7f78dc549624fc855ad611d4ccbb42e8` | 尝试复用 stream frame/body buffer，未形成可接受候选 |

Leaf external endpoint fork：
`2153f126c4841fc7f74d2da4f9e61d622882795f`。

实现保持 feature 默认关闭，并保留 legacy backend；但这只能限制未启用时的运行时影响，
不能抵消依赖/API、平台初始化和验证维护面的扩大。

## 4. Linux 实测结果

### `61e9852f` StreamBatch

| 模式 | 上传中位数 | 下载中位数 |
| --- | ---: | ---: |
| legacy | 711.7 Mbit/s | 880.9 Mbit/s |
| StreamBatch | 851.4 Mbit/s | 759.7 Mbit/s |
| 变化 | +19.6% | -13.8% |

上传改善，但下载明显低于 95% 门槛。最初 decoder 对每个 packet 执行独立长度和 body read，
导致 worker `recvfrom` 和 `futex` 增加；这促成了 `39dd4d2f` 的连续 body 修复。

### `39dd4d2f` 连续 body decoder

| 模式 | 上传中位数 | 下载中位数 |
| --- | ---: | ---: |
| legacy | 733.1 Mbit/s | 905.1 Mbit/s |
| PacketBatch | 786.1 Mbit/s | 825.7 Mbit/s |
| 变化 | +7.2% | -8.8% |

下载仍未达到 95% 门槛，说明问题不只是 decoder read 次数。

独立 profile：

| 指标 | legacy worker | PacketBatch worker |
| --- | ---: | ---: |
| 12 秒 task clock | 8525 ms | 8445 ms |
| context switches | 76,707 | 88,315 |
| CPU migrations | 585 | 1,011 |
| page faults | 201,602 | 559,275 |
| syscall 总量 | 199,524 | 245,242 |

热点转移到 framing thread 的分配、清页、page fault、TLB shootdown 和唤醒。`ca5751fb`
尝试复用 buffer，但在它形成可信制品前，前两轮数据已经证明该架构没有稳定的双向收益；
继续修补会扩大实现，而不再符合“小改动面”的前提。

## 5. Android 确定性失败

精确 `39dd4d2f` APK 在保留数据升级后启用 `leaf-packet-batch`，启动 policy runtime 时
发生 panic：

```text
tun-0.7.22/src/configuration.rs:97
called Result::unwrap() on Err InvalidAddress
```

因果链：

1. Android MemoryBatch external endpoint 正确传入 `fd = -1`，且不需要 OS-TUN 的
   address/gateway/netmask；
2. 锁定 Leaf 在选择 external endpoint 前先构造 `tun::Configuration`；
3. 空 address 进入依赖内部 `unwrap()`，runtime 在返回错误前直接 panic；
4. EasyTier 的“初始化失败回退 legacy”只能处理 `Result::Err`，无法捕获 abort/panic，因而
   fallback 没有机会执行；
5. 前台服务重启后会重复进入同一失败路径。

该问题可以通过调整 Leaf 初始化顺序单独修复，但修复 panic 不能证明 PacketBatch 有性能价值；
在 Linux 已失败的情况下继续合并该修复只会延长失败实验。

## 6. 已通过但不足以接受方案的证据

以下证据仍可复用，不应被误写成整个方案通过：

- batch packet 数、payload、channel 容量和 frame 长度均有硬上限；
- Linux packet 顺序、边界、TCP 数据完整性和正常清理通过；
- Linux idle CPU、FD、线程、TUN 和临时文件回基线通过；
- 旧 worker 不识别 `--packet-batch` 时，父进程可以在启动阶段回退 legacy；
- feature 默认关闭，未启用时走原 legacy backend；
- workflow artifact 的 SHA、BUILD_INFO、musl target、Build ID 和 debug symbols经过核验；
- `.160` 曾通过相关最小构建和 focused tests。

这些结果只能证明部分正确性和验证工具有效，不能覆盖双向性能失败与 Android 启动 panic。

## 7. 失败原因总结

- 把“逐包桥接是瓶颈”错误推导为“batch endpoint 必然接近 Leaf-owned TUN”。
- 忽略 framed stream 自身的编码、分配、调度和跨线程成本。
- 在核心 A/B 和 Android 启动门槛通过前，过早加入公共配置、CLI、RPC 和 GUI。
- capability/fallback 只验证 API 与可返回错误，没有覆盖初始化期间 panic。
- 首轮下载方向低于门槛后仍继续修 decoder 和 buffer，偏离了“达到门槛，否则停止”的原计划。
- 将跨平台设计兼容性表述成了未经实测的平台性能置信度。

## 8. 最终处置与回退边界

- 本方案标记为 **FAILED**，不得作为推荐功能、发布能力或后续实现基础。
- 不继续调试、默认启用或为其增加更多 transport、buffer pool、shared memory、io_uring、
  flow adapter 或平台分支。
- 正确代码回退基线是 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`。
- 回退不删除该基线已有的 Trojan、VMess、VLESS、Shadowsocks/UoT、HEV、规则、DNS 或其他
  Leaf 功能。
- 当前实验分支、workflow run、原始 profile 和本文保留为失败证据，不通过 destructive reset
  隐藏失败历史。
- 如果执行正式回退，应使用可审计 revert 或从该基线建立干净候选；不得把本实验之后的
  无关功能错误归入回退范围。

## 9. 后续优化的准入条件

下一个性能方案必须独立立项，不能从本文继续追加实现。最低准入条件：

- 直接使用 `0cf368` 已有精确 workflow artifact 和 profile，不重复构建同一基线；
- 先证明改动只覆盖 profile 中的主要热点，再写用户配置和 GUI；
- 第一个 spike 不改变公共配置、Leaf actor、mesh、DNS、规则或代理协议；
- 同一精确制品三次中位数至少提升 20%，任何方向不得低于 legacy 95%；
- 功能、异常恢复和资源门槛通过前，不扩展到第二个平台；
- 平台未证明时只保持 legacy，不宣称全平台加速；
- 出现 panic、wake storm、持续 CPU/RSS/FD 增长或测试机可用性风险时立即终止。

## 10. 原始证据

- Linux `0cf368` 分层 profile：NAS 私有验证目录中的
  `layer-profile-v2-0cf36807-20260719`、`core-profile-0cf36807-20260719`、
  `leaf-auto-tun-plain-0cf36807-20260719`。
- PacketBatch exact artifact A/B、strace、perf 与 Android panic 证据记录在原工作板和验证日志。
- 私有代理域名、UUID、密码和节点配置不进入仓库。
