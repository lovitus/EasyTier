# Linux Leaf-owned Policy TUN 验证日志

> **状态：等待 Phase 0；本文件按时间和候选 SHA 追加，不覆盖历史失败。**
>
> 主计划：`docs/todo/leaf_linux_owned_policy_tun_performance.md`。
> 本日志只记录开发、构建、测试、artifact、实机、性能、资源和清理事实，不把未运行项目
> 标记为通过。

## 1. 固定基线

- EasyTier baseline：`0cf368072aad4882309e6f6d450e45f5f4e1a9ac`
- Linux workflow：`29651991456`
- Android workflow：`29651991435`
- Linux artifact：`easytier-profiling-beta-linux-x86_64-musl`
- Baseline locked Leaf：`36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`
- Baseline report：`docs/leaf_policy_dataplane_performance_investigation_cn.md`
- Failed predecessor：
  `docs/failed_attempts/FAILED_leaf_external_packet_endpoint_performance.md`

基线 workflow artifact 已存在，不为重复 profile 重新构建 `0cf368`。

## 2. 候选 manifest 模板

每个 build-affecting候选开始前复制并填写：

```text
Candidate label:
Intended SHA / immutable snapshot:
Parent SHA:
Locked Leaf URL + SHA:
Current phase:
User-visible config change: yes/no
Build-affecting files:
Exact functions changed:
Reference files/functions inspected:
Intentional reference differences:
Feature-off call path:
Supported modes:
Unsupported fallback:
Prepare failure behavior:
Active failure behavior:
Owner-scoped cleanup objects:
Known risks:

.160 busy check:
.160 smallest-feature --locked no-run command:
.160 focused test filters:
.160 expected test binaries:
Lockfile/cfg/workflow/generated-file audit:

Automatic workflows expected:
Existing same-SHA runs checked:
Linux evidence matrix:
Android/non-Linux evidence matrix:
Tasks during .160 wait:
Tasks during GitHub wait:
Abort commands and cleanup owner:
```

manifest未填写完整、`.160`未通过或相同区域还有可安全合并的已知工作时，不得push。

## 3. Phase 0 源码审计记录模板

```text
Date/time:
EasyTier worktree/commit:
Cargo.lock Leaf URL/SHA:
Leaf inspected HEAD:

Leaf auto-TUN creation:
Leaf ready/error propagation:
Leaf close ownership:
EasyTier mesh TUN ownership:
EasyTier policy route/rule ownership:
EasyTier underlay bind/mark behavior:
Magic DNS route behavior:
Multi-instance naming/table behavior:

Mihomo files/functions/observable semantics:
sing-box files/functions/observable semantics:
Required intentional differences:
Can spike avoid Leaf changes: yes/no + evidence
Phase 0 Go/No-Go:
```

## 4. `.160` 开发预检记录模板

```text
Date/time:
Snapshot hash:
Sync source/result:
Builder cargo/rustc pre-check:
Smallest-feature --locked no-run result/duration:
Focused test binary/hash:
Focused tests/result/duration:
Cargo.lock changed: yes/no + reason
Platform cfg audit:
Workflow pin audit:
Generated file audit:
Complete diff audit:
Disk before/after:
Candidate dispatch lock: PASS/FAIL
```

所有Cargo命令使用timeout、`CARGO_BUILD_JOBS=$(nproc)`和本地`7890`反向转发；不在维护者
Mac编译，不在`.160`生成release/profile制品。

## 5. Workflow 与 artifact 记录模板

```text
Candidate SHA:
Push time:
Existing exact-SHA run query:
Linux workflow ID/status/headSha:
Android workflow ID/status/headSha:
Duplicate dispatch: none / cancelled ID + cause

Artifact download source:
Fresh destination:
ZIP integrity:
Outer SHA256:
Nested SHA256:
BUILD_INFO commit/ref/run/target:
Build ID:
Debug symbols:
Android APK SHA/signature:
Artifact gate: PASS/FAIL
```

同一 SHA 已有等价run时不重复dispatch。下载失败必须从字节零重试并重新验证ZIP。

## 6. Linux 功能与故障矩阵模板

| 场景 | host/kernel | feature | expected | actual | raw evidence | cleanup |
| --- | --- | --- | --- | --- | --- | --- |
| feature off | | off | exact legacy | | | |
| unsupported veth | | on | legacy | | | |
| unsupported `no_tun` | | on | legacy | | | |
| capability missing | | on | legacy, zero host mutation | | | |
| DIRECT TCP/UDP | | on | fast path | | | |
| REJECT | | on | fail closed | | | |
| FakeDNS/domain rule | | on | domain preserved | | | |
| GeoSite/GeoIP | | on | first match | | | |
| chain/fallback | | on | actor semantics unchanged | | | |
| mesh CIDR | | on | EasyTier mesh TUN | | | |
| Magic DNS | | on | EasyTier mesh TUN | | | |
| underlay/DNS | | on | bypass Leaf policy TUN | | | |
| worker kill | | on | policy recovery, mesh retained | | | |
| route loss/recovery | | on | bounded recovery | | | |
| interface/table conflict | | on | legacy | | | |
| 10x stop/start | | on | baseline | | | |

每个场景记录测试前后进程、TUN、route、rule、FD、线程、RSS和临时文件，不把其他实例对象
计入本候选cleanup失败。

## 7. 性能记录模板

固定条件：同一artifact、相邻时间、相同目标、相同网络族、相同方向、相同并发和传输量。

| scenario | AF | direction | streams | bytes/time | legacy runs | fast runs | medians | delta | gate |
| --- | --- | --- | ---: | --- | --- | --- | --- | ---: | --- |
| physical control | IPv4 | forward | | | | n/a | | | |
| physical control | IPv6 | forward | | | | n/a | | | |
| DIRECT | IPv4 | forward | | | | | | | |
| DIRECT | IPv4 | reverse | | | | | | | | |
| DIRECT | IPv6 | forward | | | | | | | | |
| DIRECT | IPv6 | reverse | | | | | | | | |
| CDN VLESS | IPv4 | forward | | | | | | | |
| CDN VLESS | IPv6 | forward | | | | | | | |

资源记录：

| scenario | mode | core CPU | worker CPU | RSS | FD | threads | ctx switch | faults | syscalls |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| | legacy | | | | | | | | |
| | fast | | | | | | | | |

无探针吞吐与strace/perf分开运行。原始数据留在`/slab2`的host独立目录；仓库只写摘要和
不含秘密的证据索引。

## 8. Android/未支持平台记录模板

```text
Candidate SHA/APK SHA/signature:
Feature selection result:
Fast-path Linux code linked but unreachable evidence:
Existing config migration:
VPN startup:
Mesh traffic:
Policy traffic:
DNS/network generation:
Stop/start cleanup:
CPU/RSS/FD/tasks:
Device unavailable boundary:
Conclusion:
```

没有实机时明确写“未验证”，不得以workflow编译成功替代Android功能证据。

## 9. 最终验收记录模板

```text
Exact accepted/rejected SHA:
All hard gates:
Failed or waived gates (waiver requires user decision):
Feature-off compatibility:
Linux old-kernel compatibility:
IPv4/IPv6 performance:
Failure/recovery:
Resource baseline:
Android/non-Linux legacy evidence:
Remaining known issues:
Raw evidence locations:
Final decision: ACCEPT / REJECT / NEEDS NEW CANDIDATE
Rollback/revert commit if rejected:
```

## 10. 执行日志

### 2026-07-19 Plan initialization

- 主方案和独立journal已建立；尚未修改实现代码。
- 复用`0cf368`已存在的精确Linux/Android workflow artifact和profile，不重复构建基线。
- 当前下一步是Phase 0锁定源码与route ownership审计；完成前不创建候选workflow。

