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
- Android evidence: exact workflow build plus the WADB-forwarded real device; preserve the candidate package data, use the captured-UID probe, and prove unchanged legacy selection, TLS policy traffic, stop/start cleanup, and bounded idle resource use.
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
- Android evidence for this candidate: workflow build, in-process compile boundary, and the WADB-forwarded real device. The earlier unavailable-device assumption was stale and must not be reused.
- Build wait work: prepare isolated Linux hosts, exact-artifact checksum/build-info checks, host-specific `/slab2` names, and bounded abort thresholds.

## Runtime evidence: owned-TUN readiness race

- Exact artifact: `02f65d0c5e6b309876269fdffd2d2971404a2df6`, Linux run `29681632123`, Build ID `ecd5c26012001924cb9b84017f245d7e93f682d4`.
- `lv1g2` legacy run 1 and Leaf-owned run 1 both completed with clean process/TUN/namespace teardown and zero measured idle CPU.
- Leaf-owned run 2 failed before load: the Leaf interface name existed, but the route switch returned Linux `ENETDOWN` (`Network is down`). Existing transactional fallback correctly shut down that candidate and restored the unchanged legacy bridge, so traffic did not escape policy and no resource remained.
- Reference boundary already audited in Phase 0: Mihomo `listener/sing_tun/server.go::{New,Close}` and sing-box `protocol/tun/inbound.go::{Start,Close}` do not publish a successfully started TUN owner before its interface startup completes. EasyTier intentionally delegates device creation to the locked Leaf child, but its process factory must provide the equivalent observable contract before returning an owned-TUN runtime.
- Minimal correction: on Linux and only for `leaf_owned_tun`, readiness requires both `if_nametoindex` success and `IFF_UP`. The route selector remains synchronous and unchanged; permanent startup failures retain the existing three-second bound and legacy fallback.
- Explicit non-goals: no fixed post-start sleep, no route-layer blind retry, no mesh/QUIC/KCP/HEV change, no platform behavior change, and no hot-path polling.

### Readiness correction preflight and candidate manifest

- Parent: `02f65d0c5e6b309876269fdffd2d2971404a2df6`.
- Product delta: Linux-only owned-TUN startup waits for `IFF_UP`; legacy fd mode, in-process mode, non-Linux platforms, routing, and all packet paths are unchanged.
- Validation-tooling delta: old-awk-compatible `/1` suffix parsing and one focused readiness flag test.
- `lv1g2` isolated feasibility probe observed flags `0x1090` before `ip link set up` and `0x1091` afterward; the temporary namespace was removed by trap.
- `.160` feature-unified `--locked` no-run passed in 47.29s; all configured EasyTier, policy, and netstack focused tests passed from exact binaries.
- No `Cargo.lock`, dependency pin, generated protocol, platform workflow, or GUI source change. The prior frontend preflight remains applicable.
- Required dispatch: one automatic Linux and one automatic Android workflow for the new source SHA. Do not dispatch duplicates.
- Required artifact evidence: at least three successful Leaf-owned starts without `ENETDOWN`, three legacy/feature-on resource/performance runs, old-kernel lifecycle/fallback, and real-host IPv4/IPv6 checks.

## 2026-07-19 Exact candidate `87301ee0` validation

### Immutable artifacts

- Exact SHA: `87301ee0831629e5d86c3392b69b126aae9bb6d2`.
- Linux workflow `29682690040`: passed. The outer and inner SHA-256 manifests, `BUILD_INFO.txt`, exact commit, `x86_64-unknown-linux-musl` target, symbols, and Build ID `efd4e765d99335a85ff5473b22f86f5b602fcde0` were verified before deployment.
- Android workflow `29682690075`: passed. APK hashes, `BUILD_INFO.txt`, exact commit, package identity, and signing certificates were verified before `adb install -r`.
- The product correction is limited to Linux Leaf-owned-TUN startup readiness: `if_nametoindex` plus `IFF_UP` within the existing bounded startup window. Android, legacy fd mode, mesh, HEV, DNS, rules, QUIC/KCP, and proxy actors are unchanged.

### Linux repeated A/B evidence

- On `lv1g2`, the fast path started successfully 3/3 with no `ENETDOWN`; every run removed its process, TUN, namespace, and policy state.
- Fast-path medians: upload `332.807092 Mbit/s`, download `290.230926 Mbit/s`, combined RSS `35324 KiB`, idle core `0.333%`, idle worker `0%`, syscall/byte `2.2893e-7`.
- Legacy medians: upload `221.787284 Mbit/s`, download `76.639036 Mbit/s`, combined RSS `37428 KiB`, idle core/worker `0%`, syscall/byte `2.32812e-4`.
- Relative traced result: fast upload is about 50% higher, download about 3.79 times legacy, RSS about 2.1 MiB lower, and syscall/byte about 0.10% of legacy. The generalized PacketBatch self-check harness accepted that traced matrix, but traced throughput is diagnostic only because `strace` penalizes the syscall-heavy legacy path disproportionately. The later untraced `.160` matrix below supersedes this result for throughput acceptance.
- Candidate-independent bare-link baselines on the two 10 Gbps dual-stack hosts were about 7.63/5.89 Gbit/s from g2 to g3 over IPv4/IPv6 and 7.49/7.62 Gbit/s in reverse. These values prevent the policy result from being misrepresented as a physical-link limit.

### Linux 3.10 compatibility evidence

- `.37` ran the exact fast-path artifact: upload `84.920 Mbit/s`, download `78.598 Mbit/s`, idle core/worker `0%`, clean core/worker shutdown, and unchanged host state.
- `.38` ran the exact legacy artifact: upload `60.603 Mbit/s`, download `46.833 Mbit/s`, idle core/worker `0%`, clean core/worker shutdown, and unchanged host state.
- Both hosts had no remaining EasyTier process, test TUN, table-52000 rule, or `0x45545001` rule after the run. A stale `.37` policy rule from an older test was identified before this matrix and precisely removed rather than hidden by broad firewall cleanup.

### Android WADB real-device evidence

- Correction: the device was not unavailable. It was mounted through WADB at local endpoint `127.0.0.1:4111`; the prior unavailable-device note was stale.
- Device: Android 13, model `23013PC75G`. Only `com.kkrainbow.easytier.policycandidate` was upgraded; the separate baseline package was not modified. `adb install -r` preserved `firstInstallTime=2026-07-17 11:09:29` and the saved network/policy configuration.
- The saved candidate retained virtual IP `10.44.0.90`, policy proxy, IPv4/IPv6 VPN routes, and user-enabled KCP/QUIC. No test disabled or bypassed those mesh transports.
- VPN network evidence: `tun0`, addresses `10.44.0.90/24` and `fd00::1/128`, DNS `198.19.0.1`, routes `0.0.0.0/0` and `::/0`, owner UID `10019`, and captured UID ranges excluding only the owner.
- Captured probe UID `10020` reached `www.wikipedia.org:443` through the saved `MATCH` actor and completed a real TLS handshake. First start: `probe_valid=true`, TCP and TLS true, `2773 ms`; after stop/start: the same evidence passed in `1880 ms`.
- Stop 1 removed the candidate VPN and Leaf thread and settled at 65 tasks/277 FDs. Stop 2 settled at 65 tasks/279 FDs; the two-FD difference is the active CDP inspection connection. There was no growing Leaf/TUN/task residue across the observed restart.
- Stable foreground sampling showed Leaf at about `1.2%` of one core; WebView/rendering and generic Tokio workers dominated foreground activity. With the Activity backgrounded and VPN retained, a 20-second thread sum was `9.10%` of one core, Leaf `0.45%`, with 70 threads before and after.
- Residual risk: the same background window logged 37 SELinux denials (`15` proc-net route probes and `22` sysfs-net probes), about 1.85 events/s. This is not the former Leaf netstack full-core loop and is outside this Linux-only candidate, but it remains an Android mesh polling/battery follow-up rather than a closed release claim.

### Candidate decision

- The Linux fast path passes its default-off functional, startup-readiness, cleanup, and old-kernel gates for the exercised matrix. It does **not** pass the cross-host untraced performance gate; the `.160` matrix below prevents this candidate from being marked performance-complete.
- Android confirms unchanged legacy behavior on the exact artifact, including real captured-UID TLS and stop/start cleanup. It does not claim that the Linux-only optimization accelerates Android.
- Remaining work is broader protocol/dual-stack/failure-injection acceptance and the independent Android mesh polling/SELinux battery investigation; neither justifies reintroducing the failed PacketBatch implementation.

## 2026-07-19 `.160` CPU-path A/B correction

The exact `87301ee0831629e5d86c3392b69b126aae9bb6d2` musl artifact was copied to `192.168.2.160`, verified again with the package `SHA256SUMS.txt`, and exercised in isolated namespaces. The host was idle, its builder had no Cargo/rustc process, and it had no EasyTier, TUN, namespace, table-52000, or fwmark-rule residue before the run.

### Traced diagnostic matrix

- Three runs per mode, all with clean shutdown and unchanged host state.
- Fast medians: upload `86.386 Mbit/s`, download `88.987 Mbit/s`, combined RSS `23100 KiB`, syscall/byte `1.706778745e-4`, idle core/worker `0%`.
- Legacy medians: upload `79.702 Mbit/s`, download `89.256 Mbit/s`, combined RSS `25508 KiB`, syscall/byte `2.645683350e-4`, idle core/worker `0%`.
- Result: upload `+8.4%`, download `-0.3%`, RSS `-2408 KiB` (`-9.4%`), syscall/byte `-35.5%`.
- Comparator result: **failed** because syscall/byte ratio `0.6451` exceeded the required `0.5000`. The large core-syscall reduction remains real, but worker syscalls dominate the total gate on this host.

### Untraced throughput/resource matrix

- A fresh evidence root ran three runs per mode without `strace`; the same idle-CPU abort guard remained active.
- Fast medians: upload `676.439 Mbit/s`, download `909.245 Mbit/s`, combined RSS `23332 KiB`, idle core `0%`, idle worker `0.333%`.
- Legacy medians: upload `699.225 Mbit/s`, download `1407.676 Mbit/s`, combined RSS `26092 KiB`, idle core/worker `0%`.
- Result: upload `-3.3%` and still above the 95% gate; download `-35.4%` and clearly below the 95% gate; RSS `-2760 KiB` (`-10.6%`).
- All three download samples were consistent: fast `905/909/934 Mbit/s`; legacy `1402/1408/1410 Mbit/s`. This is not a one-run outlier.
- FD/thread maxima were unchanged: core/worker `31+13=44` FDs and `9+6=15` threads for both modes. Resource benefit on this host is RSS, not descriptor or task count.
- Comparator result: **failed**, solely on download ratio `0.6459`.

### Corrected decision

- The earlier traced `lv1g2` throughput must not be used as proof of a general speedup. `strace` adds cost in proportion to syscall count and therefore can make removal of the legacy bridge look like a much larger throughput gain than an untraced deployment sees.
- Leaf-owned TUN still has demonstrated benefits: lower RSS on both hosts, drastically fewer core syscalls, bounded ownership, clean lifecycle, and no idle-CPU storm.
- It also has a demonstrated high-throughput download regression on `.160`. The TODO remains open and the feature must stay default-off/experimental until an untraced `lv1g2/lv1g3` matrix and a profile explain or eliminate this direction-specific ceiling.

## 2026-07-19 Dual-TUN accounting and download root cause

### Original evidence gap

- The generic harness validated both route sets but recorded bytes only for `capture_tun`: the Leaf-owned TUN in fast mode and the EasyTier TUN in legacy mode.
- It did not independently record the EasyTier fallback TUN and Leaf primary TUN in the same fast run. Therefore the earlier summary proved route metrics but did not by itself prove that fallback `tun0` stayed out of the normal data path.

### Supplemental exact-artifact capture

- One additional untraced fast run used an external namespace watcher without modifying the product artifact.
- Leaf primary `etp2b870001`: RX delta `717243110`, TX delta `1037947450` bytes.
- EasyTier fallback `tun0`: RX delta `0`, TX delta `96` bytes.
- Result: the primary Leaf TUN carried the load. The fallback TUN saw only negligible control traffic and did not cause the throughput regression.
- Future harness output must expose `easytier_tun_bytes` and `leaf_owned_tun_bytes` separately while retaining the existing `tun_bytes` compatibility field.

### TUN capability difference

- Leaf primary: `tun_flags=0x1001` (`IFF_TUN|IFF_NO_PI`), TX checksum off, TCP segmentation off.
- EasyTier TUN: `tun_flags=0x5001` (`IFF_TUN|IFF_NO_PI|IFF_VNET_HDR`), TX checksum on, TCP segmentation on.
- Neither observed flag set contained `IFF_MULTI_QUEUE`; the material difference is virtio-net header/offload support, not queue count.
- EasyTier's Linux path explicitly uses `tun_rs::DeviceBuilder::offload(true)`, `recv_multiple`, software GSO splitting, GRO aggregation, and `send_multiple` in `easytier/src/instance/linux_tun_offload.rs`.
- Locked Leaf `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb` uses `tun 0.7` and sends each stack packet with `tun_sink.send(pkt).await`; the owned-TUN configuration does not request or decode virtio-net headers and does not batch stack-to-TUN writes.

### Load CPU evidence

- Fast download: Leaf worker `165.4-169.5%` of one core equivalent, EasyTier core approximately `0%`, throughput `905-934 Mbit/s`.
- Legacy download: Leaf worker `195.0-201.1%`, EasyTier core `162.1-170.4%`, throughput `1402-1410 Mbit/s`.
- The fast path therefore removes about half of product CPU consumption and is more CPU-efficient per delivered bit, but loses the parallel offload stage and hits a lower absolute download ceiling.

### Kernel conclusion and recovery boundary

- `.160` runs CentOS 7 kernel `3.10.0-1160.el7.x86_64`. Kernel age may affect the magnitude and still requires an untraced newer-kernel comparison.
- Kernel 3.10 is not the primary missing capability: on the same host and kernel, EasyTier `tun0` successfully enabled VNET_HDR/checksum/TSO and delivered about 1.41 Gbit/s. The Leaf-owned TUN did not request those features.
- The candidate can plausibly be recovered with a narrow, generic Linux Leaf TUN offload backend: VNET_HDR-aware framing, GSO split, GRO batching, and a capability ladder that preserves the non-offloaded Leaf-owned fast path when offload is unavailable.
- Do not attempt to fix this with route changes, multi-TUN ownership changes, KCP/QUIC changes, or an `ethtool` command alone. `IFF_VNET_HDR` must be selected when the TUN file descriptor is opened and Leaf must understand the virtio header.
- Acceptance gate for any spike: on `.160`, untraced fast download must reach at least 95% of same-artifact legacy while retaining the measured RSS/CPU benefit and dual-TUN/fail-closed cleanup. Then repeat untraced IPv4/IPv6 on `lv1g2/lv1g3` before keeping the feature.

### Correct fast-path capability ladder

The fallback order is generation-scoped and must be:

1. `fast-gso`: Leaf-owned primary TUN with VNET_HDR, checksum, GSO split, GRO aggregation, and batched I/O.
2. `fast-generic`: the current Leaf-owned primary TUN without VNET_HDR/offload.
3. `legacy`: the original EasyTier TUN plus packet bridge.

- GSO capability-probe or initialization failure retries the unpublished candidate as `fast-generic`; it must not skip directly to legacy.
- Only failure to create, start, publish, route, or sustain the generic Leaf-owned TUN permits fallback to legacy.
- A fatal error after publication restarts the same ordered candidate ladder transactionally. It must not switch implementations per packet or return a partially initialized GSO stream to the generic codec.
- The existing public `leaf_tun_fast_path` request remains sufficient. The selected internal mode must be observable as `fast-gso`, `fast-generic`, or `legacy`; no additional user-facing tuning switch is required.
- Required failure-injection tests: GSO available selects `fast-gso`; GSO unavailable selects `fast-generic`; generic fast failure selects `legacy`; failed replacement retains the previously active generation.

## 2026-07-19 `fast-gso` bounded implementation spike

### Go/no-go decision

- Continue only as one bounded recovery spike. The generic owned-TUN candidate remains default-off and is not releasable because `.160` untraced download reached only `64.59%` of same-artifact legacy.
- The failed PacketBatch implementation remains fully reverted. This spike does not restore its endpoint API, framing, Android memory channel, public feature, or packet-batch state machine.
- Hard stop: if `.160` untraced download does not reach at least `95%` of same-artifact legacy while retaining the RSS/CPU benefit, reject the complete Leaf-owned TUN candidate and return the product boundary to `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`.

### Exact dependency and source boundary

- Previous locked Leaf: `lovitus/leaf@36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`.
- Spike Leaf: `lovitus/leaf@a5bb6a31df2c62200be052b61ca01b01ea5e3c25`, branch `codex/easytier-linux-tun-offload`.
- Locked-source files inspected before implementation: `leaf/src/proxy/tun/inbound.rs::{new,new_smoltcp}`, `leaf/src/app/inbound/manager.rs::{new,tun_auto}`, and `leaf/src/lib.rs` runtime setup. EasyTier still owns table `52000`, capture routes, marks, candidate publication, mesh classification and legacy fallback.
- Reference primitives: EasyTier `easytier/src/instance/linux_tun_offload.rs` and resolved Apache-2.0 `tun-rs 2.8.7`. The spike reuses `DeviceBuilder::offload`, `recv_multiple`, `send_multiple`, `GROTable`, GSO splitting and the existing bounded `IDEAL_BATCH_SIZE`; it does not invent another virtio-net implementation.

### Implementation boundary

- Leaf-only product changes: `leaf/Cargo.toml`, `leaf/src/proxy/tun/mod.rs`, `leaf/src/proxy/tun/inbound.rs`, and new `leaf/src/proxy/tun/linux_tun_offload.rs`.
- `fast-gso`: explicit Linux owned-TUN plus smoltcp first attempts a `tun-rs` VNET_HDR/TCP-offload device. Both packet directions use a fixed 128-packet maximum and no unbounded queue or extra thread.
- `fast-generic`: any GSO capability or initialization error drops the unpublished offload device and continues through Leaf's original `tun 0.7.22` device and codec.
- `legacy`: failure of that generic worker candidate or EasyTier's route transaction continues through the existing EasyTier candidate fallback. No per-packet or per-flow implementation mixing is introduced.
- Observable mode is emitted at startup as `fast-gso`, `fast-generic`, or the existing EasyTier legacy fallback warning. No second public tuning switch was added.
- No changes to EasyTier mesh, HEV, QUIC/KCP, policy rules, DNS/FakeDNS, proxy actors, veth, `no_tun`, Android TUN ownership, macOS or Windows paths.

### Pre-integration compiler evidence

- Builder: `192.168.2.160`, isolated source `/workspace/.leaf-offload-spike-1`, all available CPU cores, bounded Cargo timeout and forwarded `127.0.0.1:7890`.
- Leaf has no repository lockfile, so the standalone spike check is mechanical feedback rather than candidate evidence. After the isolated copy generated its lock resolution, `cargo check --locked --lib --no-default-features --features inbound-tun,config-json,outbound-direct` passed.
- A standalone `cargo test --no-run` exposed the locked Leaf repository's pre-existing minimal-feature `SiteGroup: MessageFull` test-target mismatch; it is not counted as a pass or as an adapter regression. The authoritative gate is the EasyTier workspace `Cargo.lock` plus standard `.160 --locked` preflight after recording the exact Leaf rev.

### Next dispatch lock

- Update the EasyTier workspace lockfile to exact Leaf `a5bb6a31df2c62200be052b61ca01b01ea5e3c25` on `.160`.
- Run the complete standard Leaf/HEV no-run and focused suite with `scripts/leaf-remote-preflight.sh`; inspect lockfile, platform cfg, workflow pins and complete candidate diff.
- Only after that gate may one batched Linux/Android profiling workflow pair be generated. The first artifact action is untraced `.160` legacy/fast A/B plus explicit TUN flags and dual-TUN byte accounting, not a broad protocol matrix.

### EasyTier integration preflight result

- EasyTier dependency manifest and `Cargo.lock` now both select exact Leaf `a5bb6a31df2c62200be052b61ca01b01ea5e3c25`; Cargo removed the old source identity and added the new exact `?rev=a5bb6a31...#a5bb6a31...` package.
- `scripts/leaf-remote-preflight.sh` synchronized the complete snapshot and passed the feature-unified `--locked` no-run in `4m37s` on `.160`.
- The script executed the exact EasyTier, policy and netstack test binaries serially. All configured focused tests passed, including owned-TUN routing/readiness, mesh TCP/UDP/UoT, HEV ownership, protocol validation, DNS/FakeDNS and netstack backpressure/cooperation coverage.
- The only compiler warning remains the pre-existing non-test `parse_system_dns_servers` dead-code warning. No GitHub workflow or release artifact has been started from this spike yet.
