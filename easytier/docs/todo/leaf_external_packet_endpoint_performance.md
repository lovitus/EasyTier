# Leaf External Packet Endpoint 通用性能方案 TODO

## 状态

- 状态：设计已收敛，尚未实现，尚未形成候选制品。
- 优先级：Leaf policy 数据面的 P0 性能工作；不阻塞 mesh-only 和 `no_tun` 使用。
- 当前推荐：EasyTier 继续拥有唯一 TUN 和 packet classifier；Leaf 增加窄的、批量化的 external packet endpoint。
- 明确边界：不修改 EasyTier mesh 数据面，不改变 QUIC/KCP 选择，不让 policy 拥有 mesh transport，不把 Leaf-owned TUN 作为跨平台主方案。
- 实施原则：先用同一制品完成 legacy/memory-batch/stream-batch A/B；达到门槛即停止，不提前引入共享内存。

## 1. 已确认的性能根因

精确候选 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` 的分层验证已经排除
DIRECT、VLESS、TLS 和 WebSocket actor 是第一瓶颈：

| 路径 | 吞吐 |
| --- | ---: |
| 完整 EasyTier policy DIRECT | 约 285 Mbit/s |
| 完整 EasyTier policy CDN VLESS | 约 277.4 Mbit/s |
| 同一 Leaf worker auto-TUN DIRECT 中位数 | 约 652.8 Mbit/s |
| 同一 Leaf worker auto-TUN CDN VLESS 中位数 | 约 540.0 Mbit/s |
| sing-box SOCKS CDN VLESS 对照 | 约 580.5 Mbit/s |

192 MiB 传输的 syscall 统计：

| 路径 | syscall 总量 |
| --- | ---: |
| EasyTier DIRECT core + Leaf worker | 1,275,318 |
| EasyTier CDN VLESS core + Leaf worker | 1,070,290 |
| sing-box SOCKS CDN VLESS | 152,781 |

完整路径约有 81 个百分点 system CPU，hot threads 约有 23k 次/秒 involuntary
context switch。根因是：

```text
应用/kernel TCP
  -> EasyTier TUN
  -> packet classifier
  -> bounded mpsc
  -> Unix datagram（逐包 send/recv）
  -> Leaf smoltcp
  -> Unix datagram（逐包 send/recv）
  -> EasyTier TUN writer
```

详细原始证据见 `docs/leaf_policy_dataplane_performance_investigation_cn.md`。本 TODO
只定义解决方案和验收条件，不复制验证日志。

## 2. 设计结论

最终结构采用一个统一的 batch packet endpoint 语义和两个运行后端：

```text
                         EasyTier-owned TUN
                                |
                    existing packet classifier
                     /                       \
                 mesh                         policy
                  |                              |
       existing EasyTier data plane       PacketBatch endpoint
                                                 |
                           +---------------------+--------------------+
                           |                                          |
                  in-process memory batch                  isolated worker stream batch
                  Android / future iOS                     Linux / Windows / macOS
                           |                                          |
                           +---------------- Leaf TUN runner ----------+
                                              |
                                      existing Leaf router/outbounds
```

“通用”指所有平台使用同一 endpoint 语义、批格式、背压和关闭协议；不要求所有平台
强行使用同一种 OS IPC。移动平台不为形式统一增加进程 IPC，桌面平台不为少量代码统一
放弃 worker 崩溃隔离。

## 3. 保持不变的行为

- EasyTier 仍是唯一 TUN owner，继续先区分 mesh 和 policy packet。
- mesh packet 不进入 Leaf endpoint，mesh smoltcp、QUIC、KCP、proxy selector 均保持原逻辑。
- Leaf 继续负责规则、FakeDNS、GeoSite/GeoIP、chain/fallback 和代理协议 outbound。
- `via: mesh` 的有端口、无端口语义不变，endpoint 不参与 mesh transport 选择。
- Linux TUN、veth 都可从 policy endpoint 优化获益。
- `no_tun` 没有透明 packet ingress，本路径不运行，功能和性能必须保持不变。
- Magic DNS、underlay DNS generation、Android VPN ownership 不因本方案改变。
- 不新增用户可见的 batch size、queue size 或 transport 配置。

## 4. Leaf 侧最小扩展

锁定 Leaf SHA：`36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`。

当前 `leaf::StartOptions` 只有配置和 runtime 选项，TUN inbound 最终直接构造
`tun::AsyncDevice`。应保留现有 `leaf::start()`，新增资源入口：

```rust
pub fn start_with_resources(
    runtime_id: RuntimeId,
    options: StartOptions,
    resources: ExternalResources,
) -> Result<(), Error>;

pub struct ExternalResources {
    pub packet_endpoints: HashMap<String, ExternalPacketEndpoint>,
}

pub struct PacketBatch {
    pub packets: Vec<bytes::Bytes>,
    pub total_bytes: usize,
}
```

实现约束：

- `leaf::start()` 调用 `start_with_resources(..., ExternalResources::default())`，保持标准 Leaf 行为。
- endpoint 按 TUN inbound tag 一次性 `take()`，不能使用进程全局 registry。
- 有 external endpoint 时，TUN runner 使用该 endpoint；否则继续原有 `auto/fd` 路径。
- endpoint 只替换 packet I/O，不进入 Router、Dispatcher、DNS、outbound actor 或 stat manager。
- Leaf config 不增加跨平台 IPC 地址、密钥或 EasyTier 专属 transport 字段。
- 公共资源 API 不引用 `ZCPacket`、EasyTier 类型或 mesh 类型。

预期 Leaf patch 边界：

- `leaf/src/lib.rs`：兼容入口和 `ExternalResources` ownership。
- `leaf/src/app/inbound/manager.rs`：按 inbound tag 传递并消费 endpoint。
- `leaf/src/app/inbound/tun_listener.rs`：把 endpoint 交给 TUN inbound。
- `leaf/src/proxy/tun/inbound.rs`：抽象 OS TUN 与 external packet I/O。

修改前必须再次以锁定 SHA 对照以上文件；不能根据 Leaf 默认分支或 Cargo cache 推断接口。

## 5. PacketBatch 与内存预算

初始硬边界：

| 项目 | 上限 |
| --- | ---: |
| 单批 packet 数 | 32 |
| 单批 payload | 64 KiB |
| 每方向待处理 batch | 8 |
| 每方向 channel payload | 512 KiB |
| 双向 channel payload | 1 MiB |

实际 RSS 还包含当前 active batch、`Bytes` 元数据和 allocator 开销，因此 `1 MiB` 是
channel payload 硬上限，不得误写成总 RSS 增量承诺。

批处理规则：

- 收到第一个 packet 后立即开始发送，不设置固定聚合 sleep 或 timer。
- 只用非阻塞 drain 合并当前已经就绪的 packet。
- 达到 32 packets、64 KiB 或当前没有更多就绪 packet 时立即 flush。
- 单 packet 不允许超过现有最大 packet 限制。
- packet 顺序必须保持；每方向只能有一个实际 transport writer。
- 第一版允许安全的一次 buffer ownership 转移或一次复制，不为理论 zero-copy 引入 unsafe ring。
- 只有同一制品证明确有必要，才允许调整 32/64 KiB/8；调整必须记录吞吐、延迟、RSS 和唤醒数据。

## 6. 后端一：MemoryBatchEndpoint

目标平台：Android，未来 iOS；Linux 可作为同制品诊断模式验证性能上界。

- 使用有界 async channel 直接移动 `PacketBatch` ownership。
- 不创建 Unix datagram、loopback socket、shared-memory object 或 packet FD。
- EasyTier 到 Leaf 满载时不能阻塞共享 TUN；只丢 policy packet并增加有界计数。
- Leaf 到 EasyTier 可以等待有界容量，使 TCP 自然产生背压。
- sender/receiver close 是明确退出协议，不能依赖 detached runner 自行消失。
- 不增加 polling task、固定 heartbeat 或空闲 wakeup。

当前 `InProcessLeafRuntime` 虽然与 core 同进程，但仍创建 `LeafPacketBridge::pair()` 并把
Unix datagram FD 交给 Leaf，因此不能把“已经 in-process”当成该优化已经存在。

风险边界：workspace release profile 使用 `panic = "abort"`，移动端 in-process Leaf
无法提供进程级故障隔离。本方案不能宣称修复该边界，也不能为了性能把桌面默认 worker
直接改成 in-process。Leaf panic 策略和 runtime ownership 如需加强，应单列 lifecycle
候选，不与 packet transport 候选混合。

## 7. 后端二：FramedBatchEndpoint

目标平台：Linux、Windows、macOS 的隔离 Leaf worker。

- 使用本地双向 byte stream，一帧承载一个 `PacketBatch`。
- Unix 可映射 Unix domain socket，Windows 可映射 named pipe。
- 可评估 `interprocess` 作为平台封装；其官方说明覆盖 Windows 和通用 Unix，Linux、
  macOS、Windows 有完整支持，Android 属于显式支持但 CI 不完整：
  <https://github.com/kotauskas/interprocess>。
- 数据面协议必须自行保持极小：magic、version、packet count、payload bytes、逐包 `u16`
  length 和 payload。
- control handshake 使用随机 nonce 和协议版本；连接、握手和关闭均有界超时。
- 接收端验证 frame 总长、packet count、逐包长度、IP version 和最大 packet size。
- EOF、worker exit 或畸形 frame 立即撤销 active bridge，policy fail-closed，mesh 保持运行。
- 先实现安全 framed stream，不先追求共享内存或平台专用 `sendmmsg/recvmmsg`。

stream backend 的目标是把 bridge 的 syscall 和 wakeup 从 O(packet) 降到 O(batch)，同时
保留 worker 崩溃隔离。它不是 zero-copy 承诺。

## 8. 背压、故障和关闭语义

EasyTier 到 Leaf：

- classifier 的 mesh 分支永远不等待 policy transport。
- policy ingress 队列或 transport 满时只丢 policy packet。
- TCP 由端点重传恢复，UDP 明确丢包；不得绕过规则回退 DIRECT。
- dropped packets 和 full batches 使用聚合计数，不逐包记录日志。

Leaf 到 EasyTier：

- 可等待有界 output capacity，不得消费后丢弃无法提交的 packet。
- output receiver 关闭必须唤醒并终止 Leaf TUN runner。
- runtime generation 更换后，旧 endpoint 不能向新 runtime 注入 packet。

生命周期：

- 每个 endpoint 带不可复用 generation/nonce。
- restart 必须先撤销 active bridge，再关闭旧 endpoint，再发布新 endpoint。
- worker crash、Leaf runtime error、config reload、DNS generation rebuild 和 stopVpn 使用同一关闭协议。
- shutdown 后不得残留 task、thread、FD、socket path、named pipe 或 pending batch。

## 9. 明确排除的方案

### 9.1 Leaf-owned TUN 作为主方案

它是有价值的 Linux 性能上界和可选 fast path，但不能覆盖 Android 单 VPN TUN、未来 iOS、
veth 共享分类和 `no_tun`，因此不能作为通用答案。

### 9.2 Flow-level 直接接 Leaf Dispatcher

Leaf Dispatcher 可以接受 TCP stream 和 UDP session，但 EasyTier 当前透明策略边界是 packet
classifier。改成 flow ownership 需要重新实现任意目标 TCP 接管、UDP NAT、FakeIP 反查、
timeout 和回包注入，并会侵入现有 mesh data plane。该方向复杂度和回归面明显大于 packet
endpoint，不进入当前计划。

### 9.3 HEV 代替 Leaf netstack

仍需要从共享 TUN 把 policy packet 交给 HEV，还会增加 SOCKS hop，不能消除 packet bridge
根因。HEV 保持现有 mesh SOCKS/UoT 职责。

### 9.4 自研共享内存 lock-free ring

暂不实现。主要风险包括 memory ordering、lost wakeup、worker crash 后 stale owner、generation
重用、句柄权限、异常长度和跨平台通知。现有候选也不能提供完整平台答案：

- iceoryx2 的 Android 当前只有 local inter-thread PoC，iOS 仍是 planned：
  <https://github.com/eclipse-iceoryx/iceoryx2>。
- ipc-channel 始终序列化消息且 channel 无界：
  <https://github.com/servo/ipc-channel>。
- shared_memory 官方范围主要是 Linux/Windows：
  <https://github.com/elast0ny/shared_memory>。

只有 framed batch 已通过正确性验证但仍无法达到性能门槛，并且 profiling 明确把剩余瓶颈
定位在 stream IPC 后，才允许另开 TODO 评估 desktop-only shared-memory backend。不能在本
TODO 中顺手加入。

## 10. 分阶段实现

### Phase 0：同制品 A/B 能力

- 在非用户公开的 profiling 入口提供 `legacy`、`memory-batch`、`stream-batch` 模式。
- 三种模式必须来自同一 commit、同一优化级别和同一 workflow artifact。
- 默认生产模式在验证完成前保持 `legacy`。
- 记录 exact SHA、Leaf locked SHA、BUILD_INFO、artifact hash 和实际 backend。

### Phase 1：Leaf external endpoint 与 memory batch

- 实现 `start_with_resources` 和 external TUN endpoint。
- EasyTier in-process runtime 不再把 Unix datagram FD 作为唯一 packet transport。
- 保留 legacy backend 以便同制品回退和 A/B。
- 在 Linux 强制 in-process 诊断与 Android 实际 host 上验证。
- 未完成 Phase 1 性能数据前，不开始 shared memory 或 flow-level 设计。

### Phase 2：隔离 worker stream batch

- 实现最小 versioned frame 和私有握手。
- Linux 先完成 process worker A/B。
- API 和 wire format 设计时覆盖 Windows/macOS，但第一版仍按当前范围只发布 Linux/Android。
- stream batch 达到门槛后，桌面默认继续使用 process worker，不改为 in-process。

### Phase 3：剩余成本定位

- 只在 Phase 1/2 未达到门槛时执行。
- 分别测 classifier、batch build/copy、Leaf smoltcp、TUN read/write 和 frame stream。
- 优先减少 allocation/copy 或调整已有 batch，不直接增加新的 transport 层。
- 任何 shared memory、GSO 或 flow-level 候选必须有独立根因证据和独立用户决策。

## 11. 候选与验证安排

该实现属于非平凡 Leaf/policy candidate，必须遵守：

```text
本地仅格式化
  -> 更新 leaf_parallel_workboard 和 candidate manifest
  -> 192.168.2.160 完整 snapshot 的 --locked no-run + focused tests
  -> 检查 Cargo.lock / cfg / workflow pin / generated files / complete diff
  -> 一次 candidate commit/push
  -> 一组 Linux/Android profiling workflows
  -> 同一精确 artifact 并行完成多场景验证
```

两台 10 Gbps 双栈公网验证机的真实地址从本地 `.envrc.local` 读取；仓库文档不记录其
域名、凭据或代理节点秘密。测试必须覆盖独立 IPv4、独立 IPv6 和双栈，不能只使用移动
网络到单个境外节点的高噪音结果。

同一候选至少覆盖：

- DIRECT、CDN VLESS 和一个非 WebSocket actor 对照。
- IPv4、IPv6、双栈及 DNS/FakeIP。
- Linux TUN、veth；`no_tun` 作为不经过 endpoint 的回归对照。
- Android policy 流量、mesh 流量、前后台和 stop/start。
- worker/runtime crash、reload、DNS generation rebuild、outage/recovery。
- overload 时 policy fail-closed、mesh 不受影响。
- idle CPU、RSS、FD、thread/task 和日志增长。

## 12. 必须补的测试

Leaf vendored tests：

- external endpoint 按 inbound tag 只消费一次。
- endpoint 不存在时原 `auto/fd` 行为完全保留。
- batch packet 边界、顺序、IPv4/IPv6 bytes 保持。
- 畸形长度、超大 packet、超大 batch 和未知 version fail-closed。
- output receiver close 能终止 TUN runner。
- runtime generation 替换后旧 endpoint 不能注入。

EasyTier tests：

- batch 上限和双向 payload budget。
- 第一包无固定聚合延迟。
- 高负载会形成 batch，低负载立即发送单 packet batch。
- policy transport 满不阻塞 mesh writer。
- Leaf output 背压不消费后丢包。
- worker EOF、runtime shutdown 和 restart 清理完整。
- legacy/memory/stream backend 对相同 packet sequence 输出一致。

process frame tests：

- partial read/write、frame 合并和拆分。
- 错误 magic/version/count/length。
- 不增加 listener handshake/nonce：worker 只继承父进程创建的已连接 FD 3，没有
  accept/connect 身份边界；启动超时继续复用现有 worker ready pipe。
- peer crash、半关闭和 stale generation。
- 有界队列在长期阻塞下不增长。

## 13. 性能与发布门槛

相同网络、相同远端、相同制品各运行至少三次，使用中位数：

| 指标 | 门槛 |
| --- | --- |
| Linux DIRECT | 不低于同制品 Leaf-owned TUN 的 90% |
| Linux CDN VLESS | 不低于同制品 Leaf-owned TUN 的 90% |
| bridge 相关 syscall | 相比 legacy 至少下降 80% |
| disabled/no_tun 吞吐 | 相比父候选回退不超过 5% |
| Linux/Android idle runner CPU | 单核占用低于 5%，且无持续 wake storm |
| channel payload | 双向硬上限 1 MiB |
| 生命周期 | 连续 10 次 start/stop、crash/recovery 后回到 RSS/FD/task 基线 |
| 正确性 | TCP、UDP、FakeDNS、chain/fallback、mesh 共存全部通过 |

如果 batch endpoint 达到以上门槛，本性能工作结束，不继续引入 shared memory、io_uring、
自定义 lock-free queue 或 flow-level 重构。

如果未达到门槛，必须先提交同制品 profiling 证据说明剩余比例位于哪里，再决定下一步；
不能依据理论吞吐直接扩大架构。

## 14. 当前置信度与未验证声明

- 根据现有 syscall、system CPU、context switch 和 auto-TUN 对照，external batch endpoint
  移除第一瓶颈的置信度约为 85%。
- 仅靠 batch endpoint 达到 Leaf-owned TUN 吞吐 90% 的先验置信度约为 60%，不是发布承诺。
- memory batch 是否还受 packet allocation/classifier 限制，必须由 Phase 1 同制品 A/B 回答。
- framed local stream 是否已足够接近 memory batch，必须由 Phase 2 回答。
- Windows、macOS、iOS 当前仅是设计兼容目标；在获得对应 artifact 和实机证据前不得声明已支持。
## 2026-07-19 implementation candidate: opt-in packet batch endpoint

Status: **implementation in progress; experimental and default-off**.

The implementation is gated by the canonical experimental feature name
`leaf-packet-batch`. The same list is exposed through `--exp-feature`,
`ET_EXP_FEATURES`, TOML/protobuf configuration, the management RPC model, and a
GUI experimental switch. Backend selection occurs once before a policy runtime
is published:

- Android in-process Leaf: bounded in-memory `PacketBatch` channels.
- Linux and non-NetworkExtension macOS worker Leaf: a bounded framed stream
  carrying `PacketBatch` values.
- A platform/runtime combination without a compiled backend: retain the exact
  legacy packet bridge and report the unsupported reason.
- Batch initialization failure before publication: retry once with the legacy
  bridge. A failure after publication is fail-closed and follows the existing
  full-runtime restart path; it never changes backend inside a generation.

The feature does not change TUN ownership, first-match policy, FakeDNS, HEV,
mesh routing, QUIC/KCP selection, proxy protocols, or the legacy bridge. A
disabled or absent feature therefore follows the existing calls and packet
format unchanged. The batch limits remain 32 packets, 64 KiB payload per batch,
and eight pending batches in each direction.

### Locked dependency and reference semantics

- The inspected pre-change Leaf baseline is
  `lovitus/leaf@36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`. EasyTier `Cargo.lock` and
  `easytier-policy/Cargo.toml` now lock the narrow external-endpoint fork commit
  `lovitus/leaf@2153f126c4841fc7f74d2da4f9e61d622882795f`; the inspected and tested
  Leaf worktree matches that exact SHA.
- Mihomo `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::New`
  passes the owned TUN directly to `tun.NewStack`, starts that stack, and stores
  both objects on the listener. `Listener.Close` closes the stack and TUN in the
  same owner lifecycle.
- sing-box
  `/Users/fanli/Documents/singbox-withfallback/protocol/tun/inbound.go::Start`
  similarly creates one interface, passes it directly to `tun.NewStack`, and
  starts stack/interface in order; `Inbound.Close` closes both together.
- Clash Meta Android
  `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt::run`
  detaches the single `VpnService` FD into `TunModule.TunDevice`;
  `core/src/main/golang/native/tun.go::startTun` passes that FD directly to the
  in-process core. There is no per-packet sidecar IPC on Android.

Observable compatibility followed here: the host remains the sole TUN owner;
the selected packet endpoint is immutable for one runtime generation; close of
either endpoint terminates that generation; packet order and boundaries are
preserved. EasyTier intentionally differs only because desktop policy Leaf is
already isolated in a worker, so that backend uses framed batches rather than
moving TUN ownership into Leaf.

### Candidate evidence contract

Record every implementation/preflight/artifact step below before enabling the
feature by default. The first candidate requires `.160` locked no-run and exact
codec/lifecycle/fallback tests, one immutable Linux/Android workflow set, and
same-artifact enabled/disabled functional, syscall, throughput, CPU, RSS, FD,
task, restart, and cleanup evidence. Unsupported platforms may claim only
legacy fallback, not batch acceleration.

### 2026-07-19 development and preflight evidence

- Leaf fork commit: `2153f126c4841fc7f74d2da4f9e61d622882795f` on
  `codex/shadowsocks-uot-v2`. Exact `.160` integration binary ran the three
  public endpoint tests: bounded/order contract, channel ownership, and
  endpoint-without-TUN rejection; all passed.
- EasyTier clean builder gate: historical `/workspace/target` (37 GiB) and
  `/workspace/.artifacts` (18 GiB) were removed while Cargo registry and pnpm
  store were retained. Disk use fell from 96% to 69% before rebuilding.
- `scripts/leaf-remote-preflight.sh` synchronized the complete snapshot and
  rebuilt from an empty target with `--locked` in 5m43s. Exact binaries were
  `easytier-55ca2b60007c94fa`, `easytier_policy-79765f131fcf832f`, and
  `netstack_smoltcp-7d3834b0fcf73e6b`.
- All 28 configured focused filters matched at least one test and passed. The
  script now fails with exit 97 before execution if a filter matches zero
  tests. PacketBatch evidence includes legacy selection, unsupported fallback
  selection, memory and stream boundary/order, stream close, and worker channel
  ownership.
- Clean Node 22/pnpm 9.12.1 frontend preflight regenerated protobuf TypeScript,
  passed 11 Vitest files / 82 tests including the GUI experiment round-trip,
  passed `vue-tsc`, and built 344 Vite modules in 15.21s.
- Post-build `.160` state is one current generation only: target 2.8 GiB,
  Node modules 434 MiB, Cargo registry 1.1 GiB, disk 71% with 61 GiB free.
- This is compiler/unit/UI evidence, not performance acceptance. Exact
  Linux/Android workflow artifacts and same-artifact legacy-vs-batch functional,
  syscall, throughput, CPU, RSS, FD, task, restart, and cleanup results remain
  mandatory before the feature can be recommended or enabled by default.

### 2026-07-19 first immutable workflow result and follow-up gate

- First EasyTier candidate: `aae707ca9236565cdcc31adbeccc2814ff0918b4`.
- Android workflow `29675426117` completed successfully in 20m0s. It built the
  debug APK and captured-UID policy probe, passed the VPN lifecycle and
  persisted-selection tests, packaged the exact candidate, and uploaded the
  artifact. This is Android compile/package evidence only; real-device behavior
  and performance evidence remain open.
- Linux workflow `29675426118` failed in job `88161897043` at its exact
  `x86_64-unknown-linux-musl`, `easytier-policy --no-default-features` no-run
  build. `packet.rs` uses Tokio `AsyncReadExt`/`AsyncWriteExt`, but the policy
  crate did not declare Tokio's `io-util` feature. Cargo feature unification in
  the wider `.160` build had hidden this dependency declaration error.
- The follow-up adds only Tokio `io-util` and makes both GNU and musl policy
  `--no-default-features --locked --no-run` builds mandatory in
  `scripts/leaf-remote-preflight.sh`. It does not change PacketBatch framing,
  selection, fallback, lifecycle, mesh, HEV, DNS, rules, or proxy behavior.
- On `.160`, the follow-up passed the exact GNU minimal gate (13.33s from the
  clean state), exact musl minimal gate (15.25s), the unified no-run build, and
  all 28 nonzero focused filters. Incremental reruns took 0.45s, 0.44s, and
  0.73s respectively. `Cargo.lock`, platform cfg, generated files, and workflow
  pins are unchanged by this follow-up.

### 2026-07-19 exact-artifact Linux A/B and decoder finding

- Exact candidate `61e9852fd18b83bb96cd5c8c8af69e79dd2e43c4` came from Linux
  workflow `29676030695` and Android workflow `29676030682`; both completed
  successfully. The downloaded archives, nested Linux tar, all SHA256 manifests,
  target metadata, BuildIDs, and debug-info markers matched the exact SHA.
- On isolated CentOS 7 / Linux 3.10 namespaces, three untraced six-second
  `iperf3` runs per mode preserved the policy TUN path, bounded traffic, low idle
  CPU, process survival, clean shutdown, and unchanged host state. Legacy
  medians were 711.7 Mbit/s upload and 880.9 Mbit/s download. StreamBatch medians
  were 851.4 Mbit/s upload (+19.6%) and 759.7 Mbit/s download (-13.8%). The
  feature therefore remains default-off and is not yet performance-accepted.
- A separate traced run, excluded from throughput medians, counted 173,867 core
  and 195,117 worker syscalls for legacy versus 156,389 core and 297,935 worker
  syscalls for StreamBatch. Batch worker `recvfrom` calls increased to 157,407
  and `futex` calls to 30,757. This is not an idle loop; untraced load CPU was
  lower in batch mode, while idle core/worker CPU remained below 0.34%/0.01%.
- Root cause is the EasyTier framed-stream decoder in
  `easytier-policy/src/packet.rs::read_batch_async`: although the frame header
  already carries packet count and total payload bytes, it performs one
  `read_u16` and one `read_exact` per packet. This turns one encoded batch into
  `1 + 2N` stream reads and explains the worker syscall amplification.
- Mihomo reference remains
  `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::{New,Listener.Close}`;
  sing-box reference remains
  `/Users/fanli/Documents/singbox-withfallback/protocol/tun/inbound.go::{Start,Close}`.
  Both keep a direct in-process TUN lifetime and do not have this worker framing
  layer. EasyTier intentionally retains worker isolation; the compatibility fix
  changes only decoder read granularity by reading the already-bounded complete
  frame body once, then validating and splitting it in memory. Frame bytes,
  ordering, failure behavior, queue bounds, Leaf API, mesh, HEV, DNS, rules, and
  lifecycle semantics remain unchanged.
- The decoder follow-up passed `.160` GNU policy no-default no-run in 2.07s,
  musl policy no-default no-run in 2.27s, and the unified no-run build in
  47.92s. The standard incremental rerun completed those gates in 0.45s,
  0.44s, and 0.74s and passed every configured focused test, including the new
  contiguous-body corruption test. An initial invocation redundantly supplied
  that policy filter as an extra EasyTier filter and correctly stopped with the
  script's zero-match exit 97; the standard no-argument candidate gate passed.

## 2026-07-19 `39dd4d2f` exact-artifact follow-up and reusable-buffer candidate

### Exact artifact and platform gates

- Linux profiling workflow `29677226981` and Android policy workflow `29677226991` both completed successfully for exact commit `39dd4d2f989a459fb44d9cc8c9aab708338e4f83`.
- The Linux workflow ZIP, nested tarball, outer/inner SHA256 manifests, `BUILD_INFO.txt`, musl target, Build IDs, and unstripped debug information were verified before deployment.
- The Android ZIP and all three APK SHA256 values were verified. The candidate APK v2 signing certificate matched `BUILD_INFO.txt`.
- Android was upgraded with package-data retention. `firstInstallTime` stayed unchanged and all 10 persisted `shared_prefs`/WebView Local Storage files were byte-identical before first post-upgrade start.
- The GUI exposed `leaf-packet-batch` disabled for the old configuration. Semantic WebView control saved exactly `experimental_features: ["leaf-packet-batch"]` without changing the other configuration.
- Android runtime validation remains pending because starting the restored configuration captured the remote ADB return path. This is a validation-access failure, not positive or negative PacketBatch runtime evidence; do not count it as a functional pass.

### Isolated Linux A/B and fallback evidence

The exact artifact was run on `.37` through the real policy TUN path in isolated namespaces. Three untraced runs per mode were used for medians; trace/profile runs were excluded.

| mode | upload runs (bit/s) | upload median | download runs (bit/s) | download median |
| --- | --- | ---: | --- | ---: |
| legacy | 729031106, 733145403, 778824424 | 733145403 | 836391794, 905082656, 934033807 | 905082656 |
| PacketBatch | 786089140, 816441071, 733401467 | 786089140 | 818438813, 830728107, 825730573 | 825730573 |

- PacketBatch upload improved by about 7.2%, but download remained about 8.8% below legacy and therefore failed the 95% acceptance floor.
- All six A/B runs had bounded logs, idle CPU below 1%, stable FD/thread counts, clean core/worker shutdown, correct TUN byte bounds, and byte-identical host state before/after.
- PacketBatch added one worker thread and three FDs as designed. Peak RSS was about 18.1 MiB core / 8.5 MiB worker versus 17.3 MiB / 8.4 MiB legacy.
- A real old worker from `61c6f313` rejected `--packet-batch`; the parent retried once without the flag, traffic passed, and both processes cleaned up. This proves initialization-time unsupported fallback rather than a fake capability test.

### Root-cause evidence and bounded follow-up

A separate `strace -c` pair showed that the contiguous-body decoder reduced core syscalls, but the PacketBatch worker still made `245242` total syscalls versus legacy `199524`. PacketBatch data I/O calls were no longer the dominant excess; `futex`, `mmap`, and `munmap` remained elevated.

A separate symbolized `perf` pair confirmed the allocation/wakeup cost:

| metric | legacy worker | PacketBatch worker |
| --- | ---: | ---: |
| task clock over 12 s | 8525 ms | 8445 ms |
| context switches | 76707 | 88315 |
| CPU migrations | 585 | 1011 |
| page faults | 201602 | 559275 |

PacketBatch-only samples concentrated in the dedicated framing thread at allocator, page-fault, page-clear, and TLB-shootdown paths. The exact code path allocated a new stream body and encoded frame for every batch on both sides of the Unix stream.

The next candidate therefore makes only the following bounded change:

- retain one grow-on-demand body buffer and one grow-on-demand encoded-frame buffer per stream direction;
- clear/resize and reuse capacity across frames;
- preserve the exact frame bytes, 32-packet/64-KiB limits, channel capacity, dedicated worker thread, Leaf API, legacy backend, MemoryBatch backend, policy semantics, and failure behavior;
- extend the existing stream test to prove smaller subsequent frames preserve bytes/boundaries while reusing capacity.

The complete snapshot passed `scripts/leaf-remote-preflight.sh` on `.160`: GNU and musl minimal locked no-run gates, the unified library no-run gate, and all configured focused tests. No dependency, lockfile, platform `cfg`, generated file, or workflow change is part of this follow-up.
