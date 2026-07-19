# Linux Leaf-owned Policy TUN 性能优化 TODO

> **状态：已批准进入设计与最小 spike，尚未批准合并或发布。**
>
> 本方案从精确基线 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` 开始，只评估
> Linux 普通 TUN policy 模式。Android、Windows、macOS、iOS、veth 和 `no_tun`
> 保持 legacy。任何平台未通过支持检查或 fast path 初始化失败，都必须在切换 policy
> route 前清理并继续 legacy；不得修改原有 mesh 数据面来补偿 policy 性能。

日期：2026-07-19

## 1. 决策背景

PacketBatch/external endpoint 实验已经失败并归档：
`docs/failed_attempts/FAILED_leaf_external_packet_endpoint_performance.md`。

失败不推翻 `0cf368` 的分层 profile：同一 Leaf worker、同一远端、同一负载中，仅让
Leaf 直接拥有 Linux TUN，就取得以下结果：

| 路径 | 完整 EasyTier policy | Leaf auto-TUN | 提升 |
| --- | ---: | ---: | ---: |
| DIRECT | 约 285 Mbit/s | 约 652.8 Mbit/s | 约 129% |
| CDN VLESS | 约 277.4 Mbit/s | 约 540.0 Mbit/s | 约 95% |

完整 EasyTier policy 路径约有 sing-box SOCKS 的 7.0 至 8.3 倍 syscall，并以 TUN、
Unix datagram、复制和跨进程唤醒为主要 system CPU 成本。Leaf auto-TUN VLESS 已达到
sing-box SOCKS 对照约 93%。因此 Linux Leaf-owned TUN 是已有因果证据支持的最小方向，
不是根据 API 形态推测的新架构。

基线报告：`docs/leaf_policy_dataplane_performance_investigation_cn.md`。

## 2. 目标与非目标

目标：

- Linux 普通 TUN policy 流量直接进入 Leaf-owned TUN，移除 EasyTier/Leaf 逐包桥接；
- EasyTier mesh TUN 继续负责 mesh CIDR、Magic DNS 和原 mesh 数据面；
- Leaf 继续负责 FakeDNS、规则、DIRECT、chain/fallback 和所有代理 outbound；
- 使用锁定 Leaf 已有 auto-TUN 能力，第一选择是不修改 Leaf fork；
- fast path default-off，并通过单一实验特性显式启用；
- 支持检查或准备阶段失败时，原 legacy 路径保持原样；
- feature off、未支持平台和未支持运行模式不得产生可测功能或性能回退。

非目标：

- 不优化 Android/iOS 单 VpnService TUN；
- 不在第一候选支持 Linux veth 或 `no_tun`；
- 不修改 EasyTier mesh、QUIC/KCP、smoltcp fallback 或 endpoint selector；
- 不实现 PacketBatch、共享内存、io_uring、GSO/GRO 或 flow-level dispatcher；
- 不修改 Trojan、VMess、VLESS、Shadowsocks/UoT 等 actor；
- 不借本候选重构通用路由、DNS 或平台生命周期；
- spike 通过前不增加 GUI、RPC、protobuf 或公开配置字段。

## 3. 平台与模式边界

| 平台/模式 | 第一候选行为 |
| --- | --- |
| Linux + 普通 TUN + policy | 可尝试 Leaf-owned TUN fast path |
| Linux + veth | legacy |
| Linux + `no_tun` | legacy |
| Android/iOS | legacy，单系统 VPN ownership 不变 |
| Windows/macOS | legacy，未获得平台路由与生命周期证据前不启用 |
| policy disabled | 原 EasyTier 路径，不创建 Leaf policy TUN |
| feature disabled/absent | 精确 legacy 路径 |

平台未启用 fast path 不等于“不支持 EasyTier/Leaf”，只表示该性能实验不适用。

## 4. 目标数据面

```text
                         host applications
                                |
                       Linux policy routing
                         /              \
              mesh/Magic DNS          policy default
                    |                       |
          EasyTier-owned mesh TUN     Leaf-owned policy TUN
                    |                       |
          existing mesh data plane    existing Leaf smoltcp
                                            |
                              DIRECT / proxy / mesh actor
```

关键不变量：

- mesh IP、Magic DNS 和 EasyTier 控制流量不得进入 Leaf policy TUN；
- Leaf underlay、DNS upstream 和 native proxy socket 不得被 policy default route 回捕；
- `via: mesh` 仍通过现有 mesh actor/bridge，不由 policy TUN 选择 QUIC/KCP；
- route 切换前 Leaf TUN、worker、FakeDNS 和必要 bypass 必须已经 ready；
- route 切换失败必须回滚本候选创建的全部对象；
- shutdown 时先撤销捕获，再停止 Leaf，再删除本候选 TUN/route/rule；
- 任意 cleanup 可重复执行，不删除用户或其他实例拥有的对象。

## 5. 参考实现与锁定源码

实现前必须在 journal 记录实际锁定 SHA 和下列精确语义，不能根据默认分支推断：

- Leaf `leaf/src/proxy/tun/inbound.rs::{new,new_smoltcp}`：auto-TUN 创建、地址、MTU、
  runner 与 shutdown ownership；
- Leaf `leaf/src/app/inbound/{manager,tun_listener}.rs`：inbound ready/error 传播；
- Mihomo
  `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::{New,Listener.Close}`：
  TUN 与 stack 同 owner 生命周期；
- sing-box
  `/Users/fanli/Documents/singbox-withfallback/protocol/tun/inbound.go::{Start,Close}`：
  interface/stack 启动顺序与关闭；
- EasyTier `easytier/src/instance/virtual_nic.rs`、Linux policy route/rule 管理和
  `easytier-policy/src/leaf_process.rs`：现有 ownership、fail-closed 和清理语义。

EasyTier 与参考实现的预期差异：EasyTier 仍保留独立 mesh TUN 和 mesh actor，因此不能
直接复制“单 TUN 负责全部流量”的 route 结构。差异必须在候选 manifest 中记录理由、
失败行为和实测证据。

## 6. 支持检查与原子切换

第一版 support probe 必须在任何 host route 变更前完成，并至少确认：

- 编译目标为 Linux；
- 当前实例使用普通 TUN，policy 已启用，且不是 veth/`no_tun`；
- `/dev/net/tun`、所需 capability 和现有 Linux route API 可用；
- 锁定 Leaf worker包含计划使用的 auto-TUN 配置；
- 确定性接口名、table、rule priority、地址和临时路径无 owner 冲突；
- mesh CIDR、Magic DNS、peer/underlay、DNS upstream 和控制端点可建立明确 bypass；
- 多实例边界已明确；第一候选可显式限制为单 policy-enabled 实例。

建议状态机：

```text
LegacyActive
    -> FastPreparing
    -> FastReady
    -> FastActive
    -> FastStopping
    -> LegacyActive / Stopped
```

切换事务：

1. 生成隔离的 Leaf auto-TUN 配置和 owner identity；
2. 启动 worker，但不安装 policy capture route；
3. 等待 worker ready、接口存在、地址/MTU正确和本地探针通过；
4. 安装并核验 mesh、Magic DNS、underlay、DNS 和控制流量 bypass；
5. 最后提交 policy capture route；
6. 发布 `FastActive`；
7. 任一步失败，按逆序撤销 candidate-owned状态，然后保持/恢复 legacy。

运行中 worker crash 时，policy 可以在恢复事务期间 fail-closed，但 mesh 必须继续。只有旧
route 和 Leaf TUN 已完整撤销后，supervisor 才能重新激活 legacy；不得让两个 policy TUN
同时成为 active owner。

## 7. 实施阶段与 Go/No-Go

### Phase 0：源码和路由审计

- 从 `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` 建立独立干净工作树；
- 记录锁定 Leaf SHA、精确函数和现有 route ownership；
- 用纯 route plan/unit fixture证明 mesh、Magic DNS、underlay 和 policy 的分类；
- 确认无需修改 Leaf；如果必须修改 Leaf，停止并提交新的用户决策点。

Go：可以使用现有 Leaf auto-TUN 且 bypass/cleanup 能用现有 Linux primitives表达。

No-Go：需要修改 mesh dataplane、Leaf actor、Android ownership或引入新的跨平台 IPC。

### Phase 1：内部 Linux spike

- 只实现内部开关，不接 GUI/RPC/protobuf；
- 只覆盖 Linux普通 TUN、单 policy实例；
- feature off调用链保持与 `0cf368` 一致；
- 加入 support probe、prepare/commit/rollback 和 owner-scoped cleanup；
- 在 `.160` 完成最小 `--locked` no-run 和 focused tests。

Go：同一 debug候选在隔离 Linux namespace通过 DIRECT、VLESS、mesh/Magic DNS、
unsupported fallback、启动失败回退和清理。

No-Go：任何启动 panic、回环、mesh劣化、非 owner对象删除或 legacy语义变化。

### Phase 2：精确 profiling candidate

- 冻结一个完整候选和 candidate manifest；
- `.160` 完整 preflight通过后只 push一次；
- 复用自动 Linux/Android workflow，先查询相同 SHA，禁止重复 dispatch；
- 下载、校验并部署精确 artifact；
- 用同一 artifact feature off/on完成所有 Linux功能与性能矩阵。

Go：达到第 10 节全部门槛。

No-Go：任一主场景性能不足、功能异常或资源不能回基线；使用 `git revert`撤销候选。

### Phase 3：公开实验开关

只有 Phase 2通过后才允许加入：

- `--exp-feature leaf-owned-policy-tun`；
- `ET_EXP_FEATURES` 和配置文件字段；
- RPC/protobuf 与 GUI实验选项；
- 用户文档中的 Linux-only、default-off、fallback和已知限制。

公共配置候选属于新的完整 snapshot，必须重新执行 `.160`、workflow和实机回归，不能复用
Phase 2源码 SHA的结论。

## 8. 开发与构建工作流

每个 build-affecting候选严格执行：

```text
更新 workboard + journal candidate manifest
  -> 本地只做 rustfmt
  -> 同步完整 snapshot 到 192.168.2.160
  -> smallest-feature --locked no-run
  -> focused tests
  -> 检查 Cargo.lock / cfg / workflow pins / generated files / complete diff
  -> 一个 candidate commit/push
  -> 自动 Linux + Android workflow 各一次
  -> 校验 exact artifact
  -> 并行 Linux功能/性能与 Android legacy回归证据
  -> journal记录原始数据、清理和结论
```

`.160` 是机械反馈和 focused test gate，不构建发布优化制品。GitHub workflow只构建已经
preflight的不可变候选。Markdown、TODO、journal和报告修改不单独触发 workflow。

等待 `.160` 时完成参考源码审计、route fixture、cfg/lockfile/diff检查和验证脚本准备；
等待 GitHub时完成验证机清理、端口分配、基线快照、失败注入矩阵和 artifact校验脚本，
不得启动第二个Cargo任务或修改在途 snapshot。

## 9. 测试矩阵

单元/组件测试：

- support probe对 Linux TUN、veth、`no_tun`、非 Linux 和 capability缺失的判定；
- route plan中 mesh CIDR、Magic DNS、underlay、DNS upstream优先于 policy default；
- prepare失败零host变更；
- commit中途失败完整回滚；
- cleanup幂等且只删除owner匹配对象；
- worker crash后先撤capture再恢复legacy；
- feature off不构造fast-path类型、不新增task/thread/FD；
- 多实例或接口名冲突明确拒绝fast path并保持legacy。

`.37/.38` CentOS 7 / Linux 3.10功能验证：

- TUN/capability支持与不支持分支；
- DIRECT、REJECT、FakeDNS、GeoSite/GeoIP、chain/fallback；
- mesh CIDR、Magic DNS、peer连接和QUIC/KCP不受影响；
- worker kill、默认路由丢失/恢复、网关替换和重复stop/start；
- 预占接口名、table、rule priority和地址冲突；
- 连续10次生命周期后RSS、FD、线程、route、rule、TUN和临时文件回基线。

`lv1g2/lv1g3` 10Gbps双栈性能验证：

- 物理IPv4/IPv6容量控制；
- 同一artifact feature off/on；
- DIRECT与CDN VLESS；
- IPv4、IPv6、双栈选择；
- 正向、反向、至少三次中位数；
- 吞吐窗口同步采集core/worker CPU、RSS、FD、线程、context switch、page fault和syscall；
- probe/strace/perf运行与无探针吞吐分开，探针结果不进入吞吐中位数；
- 测试文件、配置、PID和日志分别位于共享 `/slab2` 的 `lv1g2/`、`lv1g3/` 子目录。

Android与其他未支持平台：

- Android workflow必须构建成功；
- unit/config测试证明该feature选择legacy；
- 有设备时补精确APK保留数据升级、VPN启动、mesh/policy、stop/start和资源基线；
- 未有实机证据时只能声明“未启用fast path”，不得声明平台性能通过。

## 10. 验收门槛

| 项目 | 硬门槛 |
| --- | --- |
| Linux DIRECT | fast path中位吞吐至少为同制品legacy的150% |
| Linux CDN VLESS | fast path中位吞吐至少为同制品legacy的150% |
| IPv4/IPv6任一方向 | 不得低于同制品legacy的95% |
| Leaf auto-TUN上界 | fast path达到既有同条件上界的90% |
| syscall | core+worker合计相比legacy至少下降50% |
| context switch | 热线程合计相比legacy至少下降40% |
| feature off | 吞吐/CPU相比父候选回退不超过5% |
| idle CPU | 无流量时core+worker不持续超过单核5% |
| steady RSS | 无持续增长，fast path增量必须记录且不超过32 MiB |
| 生命周期 | 10次start/stop/crash/recovery后FD、线程、route、rule、TUN回基线 |
| 正确性 | TCP、UDP、FakeDNS、规则、chain/fallback、mesh/Magic DNS全部通过 |
| fallback | 不支持或prepare失败保持legacy；active失败期间policy fail-closed、mesh保留 |

任一硬门槛失败即 No-Go，不通过继续增加buffer、线程、内存或平台特殊分支来追逐指标。

## 11. 风险与立即终止条件

主要风险：

- policy default route回捕 Leaf underlay形成回环；
- mesh/Magic DNS被错误导入Leaf TUN；
- route切换窗口产生泄漏或错误DIRECT；
- worker crash后双 TUN ownership或残留route；
- 多实例命名/table冲突；
- Linux旧内核route/rule行为差异。

立即终止：

- panic、进程重启风暴、单核持续忙循环；
- 流量回环、日志/网络流量暴增、RSS/FD/线程持续增长；
- 测试机默认路由、生产实例或现有防火墙受到未隔离影响；
- mesh、Magic DNS或feature-off路径出现功能回归；
- 任何不明状态不能由owner-scoped cleanup恢复。

出现终止条件时立即停止负载、保存有界日志和状态、清理本候选对象并报告，不继续采样。

## 12. 文档与证据规则

- 逐候选原始过程只写
  `docs/todo/leaf_linux_owned_policy_tun_validation_journal.md`；
- 本 TODO只更新当前阶段、Go/No-Go、边界和最终决策；
- 最终通过后再建立用户文档和正式验证报告；
- 失败则移动到 `docs/failed_attempts/`，文件名和开头明确标记 FAILED；
- 每条性能结论必须包含 exact SHA、artifact、网络族、方向、并发、传输量、原始值和中位数；
- 私有域名、IP、UUID、密码和节点配置只放NAS私有目录，不进入仓库。

## 13. 当前执行状态

| 阶段 | 状态 | 证据 |
| --- | --- | --- |
| `0cf368`根因与上界 | 完成 | 已有精确artifact和分层profile |
| Phase 0锁定源码/route审计 | 完成 | 锁定 Leaf 36ba707f；无需 Leaf patch，见 journal |
| Phase 1内部spike | 未开始 | 无代码候选 |
| Phase 2 exact artifact验证 | 未开始 | 禁止提前dispatch |
| Phase 3公开实验开关 | 未批准 | 仅在Phase 2通过后讨论 |


## 2026-07-19 implementation checkpoint

The default-off Phase 1 candidate is implemented on top of exact parent `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`. It has not passed the remote compiler/test gate and must not yet be described as functional or performant. The detailed implementation inventory, fallback semantics, and pending evidence are recorded in `leaf_linux_owned_policy_tun_validation_journal.md`.
