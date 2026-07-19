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


### 2026-07-19 Phase 0 locked-source and ownership audit

- Clean EasyTier worktree: `codex/leaf-owned-policy-tun` at exact parent `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`.
- `Cargo.lock` source: `git+https://github.com/lovitus/leaf.git?rev=36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb#36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`.
- Inspected Leaf worktree: detached exact `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`; inspected `leaf/src/proxy/tun/inbound.rs::{new,new_smoltcp}`, `leaf/src/app/inbound/manager.rs::InboundManager::new`, `leaf/src/lib.rs` TUN setup boundary, `leaf/src/sys.rs::post_tun_creation_setup`, and `leaf/src/config/common.rs` TUN mapping.
- Leaf semantics: `fd >= 0` wraps inherited packet I/O; `auto=true` uses default TUN settings and enables Leaf global route setup; `fd=-1`, `auto=false`, and explicit name/address/gateway/netmask creates a Leaf-owned TUN without invoking `sys::post_tun_creation_setup` because `InboundManager::tun_auto()` remains false.
- EasyTier semantics: `PolicyRoutingGuard` owns table `52000`, rule priority `10900`, protocol `99`, physical-route bypass, source rules, fwmark bypass, `/1` capture and terminal fail-closed routes. `POLICY_SOCKET_MARK=0x45545001`; worker validates and exports `OUTBOUND_INTERFACE`, while EasyTier underlay sockets retain SO_MARK.
- Existing lifecycle: routing currently captures to the EasyTier TUN before Leaf starts; a missing bridge drops policy packets while mesh-classified packets continue. Runtime replacement is transactional because the candidate is built before the active worker is stopped.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::{New,Close}` creates the TUN, gives it to the stack, starts the stack, and closes stack/TUN/redirect state under one listener owner.
- sing-box reference: `/Users/fanli/Documents/singbox-withfallback/protocol/tun/inbound.go::{Start,Close}` opens the interface, constructs and starts the stack/interface in stages, and closes stack/interface/redirect together.
- Intentional EasyTier difference: the Leaf worker owns only the policy TUN and Leaf stack; EasyTier retains the mesh TUN, mesh classifier, route transaction and fallback route. This is required by mesh/Magic DNS ownership and does not change actor semantics.
- Route decision: keep the EasyTier TUN `/1` as a lower-priority fail-closed fallback while an explicit Leaf-TUN `/1` is primary. Route replacement uses the existing key/replace primitive; if the Leaf interface disappears, the kernel removes its primary route and immediately selects the existing EasyTier fallback instead of the physical `/0`.
- Reload decision: each candidate uses a bounded unique interface/address slot so a new worker can become ready before route replacement; the old worker remains active until the existing transactional activation step.
- Leaf patch decision: **not required** for the spike. Any later discovery that requires a Leaf fork change is a new Go/No-Go decision and stops the current candidate.
- Phase 0 result: **GO** for a Linux-only, default-off internal candidate. Android, macOS, Windows, veth and `no_tun` remain legacy.

### 2026-07-19 Phase 1 candidate manifest (planned snapshot)

- Parent SHA: `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`.
- Intended branch: `codex/leaf-owned-policy-tun`; exact candidate SHA pending implementation and `.160` gate.
- Configuration: `policy_proxy.leaf_tun_fast_path=false` by default; CLI/env and management round-trip use the same boolean. Unsupported runtime/mode logs one bounded fallback reason and uses legacy.
- Core scope: policy config/CLI/protobuf round-trip; explicit Leaf-owned TUN config; worker FD optionality/readiness; Linux route primary/fallback selection; transactional candidate activation. No dependency, Leaf pin, mesh, DNS, rules, HEV, QUIC/KCP, or actor change.
- Focused tests: config default/round-trip; Leaf JSON fd versus owned TUN; interface slot bounds; worker no-FD command path; route primary/fallback ordering and owner cleanup; unsupported backend selection; runtime fallback and generation replacement.
- `.160` gate: standard `scripts/leaf-remote-preflight.sh` after its filter list includes every new focused test; GNU/musl minimal locked no-run, unified no-run, direct test binaries, lockfile/cfg/workflow/generated-file/complete-diff audit.
- Workflow gate: query exact SHA first, then one automatic Linux/Android pair only after `.160` passes. No documentation-only dispatch.
- Linux artifact evidence: `.37/.38` old-kernel correctness/failure/10-cycle cleanup and `lv1g2/lv1g3` same-artifact feature-off/on IPv4/IPv6 DIRECT/VLESS profiling.
- Android evidence: exact workflow build and legacy-selection tests; no real-device claim while the device is unavailable.
- Tasks during waits: reference/diff audit, bounded route-conflict fixtures, host cleanup/preflight, exact artifact verification commands, performance matrix and abort scripts.

### 2026-07-19 Phase 1 implementation snapshot before remote preflight

- State: code complete locally; formatted with Rust 1.95/edition 2024; not yet compiled or tested.
- Configuration surface: `[policy_proxy].leaf_tun_fast_path`, direct CLI `--policy-leaf-tun-fast-path`, `ET_POLICY_LEAF_TUN_FAST_PATH`, management protobuf field 80, launcher round-trip, and Linux-only GUI checkbox. Every surface defaults to `false`.
- Legacy invariant: `false`, unsupported platform, Linux veth, and non-TUN selection retain the original fd-backed `LeafPacketBridge` configuration and original capture routes.
- Fast-path ownership: Leaf receives `fd=-1`, `auto=false`, and an explicit bounded TUN identity. EasyTier retains policy table 52000, mark/source bypass rules, mesh classification, DNS inputs, worker lifecycle, and transactional reload ownership.
- Transactional switch: a new worker must create its unique TUN before EasyTier replaces the primary `/1` capture routes. The original EasyTier TUN remains installed at metric 65536 while the active Leaf TUN uses metric 65535. If the Leaf interface disappears, the kernel immediately falls back to the original TUN, where the absent/disconnected bridge keeps non-mesh traffic fail-closed while specific mesh routes remain usable.
- Fallback: worker creation or route selection failure destroys the unpublished fast candidate and retries the unchanged legacy candidate. A failed reload does not switch away from the previously active generation.
- Allocation bound: per-generation `/30` addresses come from `198.18.0.0/16`; this does not overlap the default `198.19.0.0/16` FakeIP range. Interface names stay within Linux `IFNAMSIZ`. Custom user ranges still require real-host conflict validation.
- Added focused evidence targets: exact legacy-vs-owned Leaf JSON, bounded TUN identity, config propagation, primary/fallback route metrics, launcher round-trip, frontend projection, and standard existing Leaf/policy/netstack regressions.
- No changes: Leaf dependency SHA, packet/rule semantics, DNS/FakeDNS logic, mesh routes or protocols, HEV, QUIC/KCP, proxy actors, Android runtime, macOS runtime, Windows, or veth data path.
- Next hard gate: `scripts/leaf-remote-preflight.sh` on `192.168.2.160`. Any compile/type/generated-code failure is repaired in this local candidate before a workflow is permitted.

### 2026-07-19 Phase 1 remote preflight result

- Builder: `root@192.168.2.160`, container `easytier-debug-builder`, exact synchronized workspace `/workspace`.
- First complete Rust preflight: `--locked` no-run succeeded in 4m32s; every configured EasyTier, policy, and netstack focused test passed.
- Candidate audit found one platform-boundary ambiguity: Linux-targeted OHOS could satisfy `target_os=linux`. The selector was tightened to require `target_os=linux && target_env!=ohos`; default-off, veth rejection, and supported-TUN selection received a pure regression test.
- Final Rust preflight after that source change: incremental `--locked` no-run succeeded in 53.41s. All configured focused tests passed, including the new backend-boundary test. The only warning is the pre-existing test helper `parse_system_dns_servers` being unused in the non-test library build.
- Frontend preflight on the same synchronized candidate: protobuf TypeScript codegen succeeded; `config-ui.spec.ts` and `policy-editor.spec.ts` passed 22/22 tests; `vue-tsc -b` and Vite production build succeeded in 15.26s.
- Generated-file boundary: frontend protobuf output is build-generated under `src/generated/proto` and is not tracked in this checkout. Remote codegen produced the expected full generated directory; no generated artifact is being silently committed.
- Lock/workflow audit: no `Cargo.lock` or `.github/workflows` diff. Locked Leaf remains `lovitus/leaf@36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`.
- Diff hygiene: `git diff --check` passed. Rust formatting used Rust 1.95 with edition 2024. No local compile or test was run.
- Dispatch status: `.160` hard lock is satisfied. One commit/push and the automatic Linux/Android profiling pair are now permitted; no manual duplicate dispatch is permitted.

### 2026-07-19 PacketBatch performance harness recovery

The PacketBatch code reverts also removed `scripts/leaf-packet-batch-validation.sh`. The failed implementation must stay reverted, but deleting its reusable validation harness was too broad. The harness is restored in generic form without reintroducing any PacketBatch product code:

- `scripts/leaf-policy-dataplane-validation.sh`: Linux/root exact-artifact runner for `legacy` and `leaf-owned-tun` modes.
- `scripts/leaf-policy-dataplane-compare.py`: cross-platform, dependency-free repeated-run evaluator; it can aggregate evidence on macOS without building EasyTier or modifying host networking.
- Preserved checks: BUILD_INFO SHA, isolated namespaces, host state before/after, upload/download iperf, idle CPU, CPU/RSS/FD/thread samples, optional core/worker strace, capture-TUN byte amplification, log growth, child ownership and bounded cleanup.
- New fast-path checks: `etp*` is present; both primary IPv4 `/1` routes use the Leaf TUN at metric 65535; both fallback `/1` routes use the EasyTier TUN at metric 65536. Legacy mode requires no `etp*`, the original metric-65535 routes, and no metric-65536 fallback.
- Storm guard: either core or worker exceeding 20% idle CPU over the three-second pre-load sample aborts the run before throughput load. The threshold is configurable but must be recorded.
- A/B evaluator defaults: at least three complete runs per mode; upload and download median at least 95% of legacy; combined peak RSS growth at most 65536 KiB; when `--require-strace` is used, median syscall-per-byte ratio at most 0.50.
- Local tooling tests: Bash syntax passed; Python compile passed; a synthetic three-run positive matrix passed; a synthetic 80%-upload regression was rejected. No product binary was built locally.

Planned exact-artifact sequence on a validation host:

```bash
CANDIDATE_SHA=<corrected-commit-sha>
CANDIDATE_SHORT=${CANDIDATE_SHA:0:8}
ARTIFACT_ROOT=/slab2/leaf-owned-policy-tun/$CANDIDATE_SHORT

for mode in legacy leaf-owned-tun; do
  for run in 1 2 3; do
    sudo scripts/leaf-policy-dataplane-validation.sh \
      --bundle "$ARTIFACT_ROOT/bundle" \
      --output-root "$ARTIFACT_ROOT/evidence" \
      --candidate-sha "$CANDIDATE_SHA" \
      --mode "$mode" --run "$run" --trace
  done
done
python3 scripts/leaf-policy-dataplane-compare.py \
  --output-root "$ARTIFACT_ROOT/evidence" \
  --candidate-sha "$CANDIDATE_SHA" \
  --require-strace \
  --output "$ARTIFACT_ROOT/evidence/comparison.json"
```

The runner is a full Linux validation harness, not a replacement for old-kernel lifecycle/failure injection, real dual-stack tests, proxy interoperability, or Android workflow evidence.

## `fe9b68bc` workflow failure and corrected preflight

- Linux run `29681107125` and Android run `29681107121` both failed and are not artifact evidence.
- The Linux compiler error was confined to `InProcessLeafFactory::start`: adding the non-`Copy` owned-TUN option made `LeafConfigOptions` non-`Copy`, while the existing `&self` method still moved it.
- The correction clones this small startup configuration at the existing runtime-construction boundary. It does not change packet, route, DNS, mesh, HEV, proxy, or lifecycle semantics.
- The standard `.160` preflight previously compiled `easytier/leaf-policy-proxy` without the separate `easytier-policy/leaf-inprocess` feature combination. It now enables both in the same Cargo feature-unified no-run build.
- Corrected `.160` preflight: `cargo test --locked --no-run --package easytier --package easytier-policy --package netstack-smoltcp --lib --features easytier/leaf-policy-proxy,easytier-policy/leaf-inprocess` passed in 4m34s.
- All configured focused EasyTier, policy, and netstack tests passed from the exact generated test binaries.
- Frontend sources are unchanged from the previously successful `.160` frontend preflight, so no duplicate frontend build was run for this mechanical correction.

## Corrected candidate manifest

- Parent candidate: `fe9b68bcebaf4915a77c3943a3c08942f52b71a6`.
- Build-affecting correction: clone `LeafConfigOptions` at the in-process runtime start boundary.
- Validation-tooling changes: require `leaf-inprocess` in `.160` no-run coverage; restore a generalized legacy-versus-Leaf-owned-TUN Linux data-plane runner and offline comparator.
- Lockfile, dependency pins, generated protocol code, platform routing semantics, Leaf pin, and GitHub workflow files: unchanged.
- Required workflows after the batched commit: one automatic Linux profiling beta run and one automatic Android policy candidate run.
- Linux artifact evidence: legacy/feature-on functional routing, DIRECT throughput, idle CPU, RSS/FD/tasks, optional syscall-per-byte, traffic amplification, lifecycle cleanup, fallback, and IPv4/IPv6 real-host checks.
- Android evidence for this candidate: workflow build and in-process compile boundary only while the dedicated device is unavailable; no real-device claim.
- Build wait work: prepare isolated Linux hosts, exact-artifact checksum/build-info checks, host-specific `/slab2` names, and bounded abort thresholds.
