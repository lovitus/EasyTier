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

## 11. 2026-07-19 跨主机复核

dual-TUN 的单机否决被后续跨主机证据推翻后，重新审计 PacketBatch 是否也被 `.160` 的主机特异行为误判。复核没有重新构建：直接使用 `39dd4d2f989a459fb44d9cc8c9aab708338e4f83` 的未过期精确 Linux workflow artifact。

- workflow：[run 29677226981](https://github.com/lovitus/EasyTier/actions/runs/29677226981)；artifact ID `8439501207`；
- workflow ZIP SHA-256：`32f8f7d7fd3007258ab75aec1091a2a02668855701c546aa68cbdf647967b818`；
- 内部 musl bundle SHA-256：`fafab7ab2c41a4b199147b8800c7a60df55ca91cfd1b62e57dfc294b06fd0c25`；
- `BUILD_INFO`、内部四个二进制 SHA-256、commit、target 和 symbols 均通过核对；
- 每台主机使用候选自带的 `leaf-packet-batch-validation.sh`，交错运行三次 legacy 和三次 batch；
- 每轮使用独立 netns、专用端口、同一 artifact、同一 DIRECT policy，并检查 TUN 字节、流量放大、日志、空闲 CPU、进程关闭和 host-state 回基线；
- 只对证据快照中的 DHCP lifetime 做归一化；KR 另归一化易变的 IPv6 RA `expires`，不修改产品路径。

### 跨主机中位数

| Host | Kernel | Legacy up/down Mbit/s | Batch up/down Mbit/s | Upload ratio | Download ratio | RSS ratio | Download gate |
|---|---|---:|---:|---:|---:|---:|---|
| `.160`（原记录） | 3.10 | `733.1 / 905.1` | `786.1 / 825.7` | `1.072` | `0.912` | not recorded here | fail |
| `.37` | 3.10 | `726.5 / 841.6` | `678.9 / 796.2` | `0.934` | `0.946` | `1.035` | fail |
| `.38` | 3.10 | `721.7 / 886.7` | `697.5 / 817.4` | `0.966` | `0.922` | `1.028` | fail |
| lv1g2 | 4.19 | `765.3 / 600.0` | `817.9 / 428.5` | `1.069` | `0.714` | `1.007` | fail |
| lv1g3 | 5.4 | `516.1 / 415.0` | `72.1 / 287.4` | `0.140` | `0.693` | `1.016` | fail |
| KR | 5.10 | `584.6 / 389.4` | `653.7 / 322.9` | `1.118` | `0.829` | `0.880` | fail |

五台新增主机的下载都低于 95% 门槛，加上原 `.160` 后为六台全部失败。`.37` 的 `0.946` 接近门槛，但 `.38`、lv1g2、lv1g3 和 KR 分别回退约 7.8%、28.6%、30.7% 和 17.1%，不能由单一主机、单一内核或一次网络波动解释。

lv1g3 的 batch 上传还出现明显不稳定：三次约为 `72.1 / 515.5 / 71.7 Mbit/s`，相邻 legacy 三次稳定在约 `507-529 Mbit/s`。这不是接受依据，而是 framed StreamBatch 在该主机上的额外风险信号。早期 perf 中 batch worker 的 context switch、CPU migration、page fault 和 syscall 同时增加，与跨主机下载回退方向一致。

KR 原第三轮在历史采样脚本中命中一次 `SIGPIPE`，前四轮已有完整 summary 且 host state clean。清理后在独立证据目录只重跑第三组 legacy/batch并通过；未把失败的半成品目录计入中位数。生产 EasyTier PID `44990`、生产 `tun0` 和 iperf PID `52372` 全程保留。

最终清理确认五台均无 `etpb-*` namespace、测试进程或测试 TUN；`.37` 原有 `etns_scale` 保留。证据归档 SHA-256：

| Host | Evidence archive SHA-256 |
|---|---|
| `.37` | `ffb79f3d4513c0f9a3a536d90ae5f20153726f25453b704b058512f77171002c` |
| `.38` | `88f4e51d773fecc902427cfad714b85e7db308bdb40b54e8094a0ba9d916171b` |
| lv1g2 | `cd447d6ae0f3208ef71eacdbbf183cb740d685b6cebf1f3514fe10e8144cd293` |
| lv1g3 | `e27dd0cb632c2d9b4a229ae6596368ccabbb2764831227888c1a213df3d6cf4b` |
| KR | `3738d366f40656c309a1ae4db493599db323b962a57dc392de0d5ce6e57b3862` |

### 修正后的判断

原始 `.160` 单机性能证据不足以单独证明跨平台或跨主机失败，这一点确实是验证设计缺口；但新增五主机证据重复了同一下载回退，因此 PacketBatch 的回退结论不是 dual-TUN 那样的误报。Android panic 仍是独立、确定性的首版阻塞，即使修复初始化顺序也不能改变 Linux StreamBatch 未达到性能门槛的事实。

本结论只否决 `39dd4d2f` 的 framed StreamBatch/MemoryBatch external endpoint 实现及其公共配置扩张，不否决 GSO/GRO、Leaf-owned TUN或未来以不同机制减少逐包开销的方案。
