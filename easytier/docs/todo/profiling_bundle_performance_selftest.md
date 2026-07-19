# Profiling bundle 隔离性能自检 TODO

## 状态

- 状态：实现已准备，远程 `.160` 预检和精确 profiling artifact 实测待完成。
- 范围：Linux profiling bundle；不进入生产 binary、APK、正式 release archive 或 Cargo feature graph。
- 目标：一次命令在本机隔离构造 policy DIRECT 流量，自动输出吞吐、CPU、RSS、FD、线程、TUN 流量和清理证据。

## 设计

profiling bundle 独占包含：

- `leaf-perf-selftest.sh`：root namespace orchestration、资源采样、门槛和 JSON 汇总。
- `easytier-perf-probe`：std-only TCP upload/download fixture 和 client。
- 原有 `easytier-core`、`easytier-leaf-worker`、CLI 和 HEV sidecar。

probe 源码位于 `tools/easytier-perf-probe.rs`，故意不加入 Cargo workspace。workflow
直接使用 `rustc` 构建 musl 测试工具，因此生产 Cargo manifest、Cargo.lock、feature 和链接
依赖均不变化。只有 `.github/workflows/profiling-beta.yml` 复制该工具；正式 release workflow
不得引用它。

## 隔离拓扑

```text
client namespace                 router namespace               fixture namespace
192.0.2.2/30 -- default --> 192.0.2.1/30   198.51.100.1/30 --> 198.51.100.2/30
EasyTier core + Leaf                                           TCP fixture
```

- 三个 namespace 均为当前 PID 唯一命名。
- router 只在自己的 namespace 开启 IPv4 forwarding。
- 不添加 host route、host address、iptables/nftables rule、NAT 或 host sysctl。
- fixture 没有公网接口，不能访问外网，也不能被宿主/LAN 直接访问。
- fixture 目标不在 client connected subnet，应用连接必须匹配 policy TUN；Leaf DIRECT 的
  marked outbound socket再经 client 默认路由到 fixture。
- cleanup 只按精确 namespace PID 终止进程，不使用 `killall`、`pkill` 或模糊进程匹配。
- 退出后删除三个 namespace，并对账宿主 address、route、rule 和原有 namespace 列表。

## 自动证据

输出目录包含：

- `summary.json`：机器可读总结果和 gate。
- `upload.json`、`download.json`：字节数、client/server elapsed 和 bit/s。
- `resources.tsv`：100 ms 周期的 PID、user/system ticks、RSS、FD、线程原始数据。
- `resources-summary.tsv`：每个 executable 的平均 CPU、最大 RSS/FD/线程。
- `host-before.txt`、`host-after.txt`：宿主状态对账。
- `easytier-core.log`、`fixture.jsonl`：启动和 fixture 生命周期。
- `warmup.json`：正式采样前的小流量可达性证据。

TUN gate 要求上传和下载阶段至少一个方向的 TUN counter 增量达到 payload 的 80%，避免
误把 underlay bypass 当作 policy 性能。每阶段 TUN RX+TX 不得超过 payload 的四倍加 16 MiB，
只用于发现明显回环/重复劫持，不把正常 TCP ACK 开销误判为故障。

自检不设置固定吞吐通过值，因为开发机、虚拟机和 ARM/x86 性能不同。它保证路径真实、
结果可重复采集；候选间回退判断由相同主机和相同参数的历史 `summary.json` 完成。

## 使用

```bash
./leaf-perf-selftest.sh --check-only

sudo ./leaf-perf-selftest.sh \
  --bytes 134217728 \
  --output /var/tmp/easytier-perf-candidate-SHA
```

默认每方向 128 MiB。probe 固定只分配 64 KiB 数据 buffer，不会按测试字节数分配内存。

## 待验证门槛

- `.160`：`bash -n`、probe unit tests、musl probe build、完整 snapshot 的 Leaf remote preflight。
- 精确 artifact：在隔离 Linux host 完成 upload/download，`summary.json` 所有 gate 为 true。
- 资源：连续运行三次后无 namespace、进程、TUN、FD 或临时路由残留。
- 可重复性：相同 host 三次吞吐中位数可计算，raw samples 非空且包含 core 和 Leaf worker。
- 生产排除：正式 release archive、Android APK 内容和生产 binary symbols 均不出现 self-test probe/harness。

## 后续边界

- 第一版只测 IPv4 literal `MATCH,DIRECT`，用于稳定捕获已确认的 packet bridge 性能瓶颈。
- IPv6、SOCKS/SS/VLESS fixture 可在相同拓扑上增加独立 profile，但不得让第一版依赖公网节点。
- 不把 sing-box 打进第一版：std-only fixture 更小、确定、无下载供应链和配置版本漂移。
- harness 不自动修改生产配置，不读取用户订阅、节点域名、UUID 或密码。

## 2026-07-19 implementation and `.160` evidence

Status: implemented locally and preflighted; exact profiling-workflow artifact packaging is still pending.

Implementation boundary:

- The fixture is a standalone, std-only Rust TCP probe compiled directly by `profiling-beta.yml`; it is not a Cargo workspace member and adds no production dependency.
- The harness creates separate client, router, and fixture network namespaces. The client reaches the fixture only through its policy TUN and a namespace-local router; it does not add host routes, alter host firewall rules, enable host forwarding, or use the external network.
- The harness records byte-exact upload/download throughput plus `/proc` CPU ticks, RSS, FD count, and thread count for EasyTier, Leaf, and the probe.
- TUN byte counters prove policy capture. A gross byte-amplification bound detects an obvious forwarding loop. Process survival, host-state stability, and namespace cleanup are hard gates.
- Production exclusion is structural: neither `Cargo.toml` nor any workflow other than `profiling-beta.yml` references `easytier-perf-probe` or `leaf-perf-selftest.sh`.

Remote preflight on `192.168.2.160`:

- Standard `scripts/leaf-remote-preflight.sh` no-run build and focused Leaf/HEV/netstack tests passed.
- `bash -n scripts/leaf-perf-selftest.sh` passed.
- Probe unit tests passed: byte-exact upload and download, including a non-buffer-aligned payload.
- The probe compiled as a static x86_64 musl PIE and its CLI smoke check passed.

Isolated 128 MiB-per-direction run, using the previously validated product artifact `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` with the current standalone harness and probe:

- Upload: `1,227,058,983.684 bit/s`.
- Download: `1,396,717,006.734 bit/s`.
- EasyTier core: average CPU `106.485%`, max RSS `17,580 KiB`, max FD `36`, max threads `9`.
- Leaf worker: average CPU `196.854%`, max RSS `7,900 KiB`, max FD `12`, max threads `6`.
- Policy TUN counters changed in both directions.
- Policy-path, loop-bound, process-survival, host-state, and cleanup gates all passed.
- CPU percentages are process totals across cores and may exceed 100%; they are not normalized to one core.

A first smoke run exposed a harness-only false failure: `ip -details address show` included volatile Linux bridge timers in the host-state snapshot. The snapshot now uses stable one-line address state, after which both the 8 MiB smoke run and the 128 MiB run passed without changing product code.

Remaining acceptance boundary:

- Build one exact `profiling-beta` candidate and verify its bundle checksums, metadata, executable mode, probe unit-test result, and end-to-end self-test from the downloaded artifact.
- Do not trigger a workflow for this documentation or harness-only checkpoint. Batch it with the next build-affecting profiling candidate unless an exact harness artifact is explicitly required earlier.

## Packaged-bundle acceptance evidence

A workflow-equivalent archive was assembled on `192.168.2.160`, extracted into a fresh directory, checksum-verified, and executed only from that clean extraction.

Archive evidence:

- Archive: `easytier-profiling-beta-linux-x86_64-musl-selftest.tar.gz`.
- SHA-256: `200066a99cea65ee5fb649365806fcd609895d0fbec987701cd3631999043d49`.
- Size: `99,512,052` bytes.
- Members are limited to `easytier-core`, `easytier-leaf-worker`, `easytier-perf-probe`, `leaf-perf-selftest.sh`, `SELFTEST_INFO.txt`, and `SHA256SUMS.txt` under one bundle directory.
- Every member checksum passed after clean extraction.
- `--check-only` passed from the extracted bundle.
- A 32 MiB-per-direction run from the extracted bundle passed at approximately `1.095 Gbit/s` upload and `1.428 Gbit/s` download.
- The run reported unchanged host state, no host firewall or forwarding changes, observed policy-TUN traffic, acceptable gross byte amplification, surviving core/Leaf processes, and complete namespace cleanup.

Production-package exclusion audit:

- `actionlint .github/workflows/profiling-beta.yml` passed.
- No Cargo manifest references either self-test file, so the probe cannot affect production dependency resolution, compilation, binary size, initialization, or hot paths.
- No workflow other than `profiling-beta.yml` references either self-test file.
- Formal upload paths remain limited to generated web output, `./artifacts/*`, the existing Magisk package directory, or the OHOS HAR. `release.yml` only aggregates those formal artifacts.
- Therefore compiled Core, GUI, Android, OHOS, Magisk, and formal release assets cannot acquire the self-test probe or harness through the current packaging graph. GitHub-generated source archives naturally contain repository source files; they are not executable production packages and add no installed/runtime overhead.

The profiling artifact should still be checked after its next normal GitHub build to preserve exact-SHA release evidence, but that is release provenance verification rather than an implementation or isolation gap in the self-test facility.

## Mesh matrix extension

Status: implemented locally; exact profiling artifact validation is pending the next normally required candidate workflow.

The profiling bundle now has one combined entry point, `profiling-perf-selftest.sh`, which runs the existing isolated Leaf `MATCH,DIRECT` baseline and then the following EasyTier mesh modes:

- `native`: two directly connected underlay namespaces, two EasyTier TUN peers, KCP and QUIC proxy acceleration disabled.
- `kcp`: the same direct topology with `--enable-kcp-proxy true`; an active long connection must appear in `easytier-cli proxy` with `transport_type: Kcp` before throughput is accepted.
- `quic`: the same direct topology with `--enable-quic-proxy true`; the RPC evidence must report `transport_type: Quic`.
- `relay`: client and destination use disjoint underlay `/30` networks and connect only to a third no-TUN EasyTier relay. Neither endpoint has an underlay route to the other. The accepted route must have `path_len: 2` and `next_hop_hostname: perf-r`.

Every mode transfers byte-exact upload and download payloads, captures both EasyTier cores' CPU, RSS, FD and thread counts, records TUN and underlay byte deltas, and preserves route/transport evidence with the result. It does not use the public network, DNS, NAT, host forwarding, or host firewall rules.

### Immediate safety aborts

The harness treats validation-host availability as a harder gate than completing a sample. A watchdog polls every 250 ms and immediately terminates the exact test process groups when any of these conditions occurs:

- one EasyTier process exceeds `512 MiB` RSS or all test cores exceed `1 GiB` RSS;
- one process exceeds `512` FDs or `128` threads;
- one core log exceeds `16 MiB`;
- an idle process remains above `80%` of one CPU for approximately two seconds;
- active transfer consumes at least `180%` aggregate CPU while underlay traffic makes less than `64 KiB` progress for approximately two seconds;
- TUN or isolated-underlay bytes exceed the phase-specific amplification budget;
- a monitored core exits, a mode exceeds its wall timeout, or route/transport evidence does not match the requested mode.

On abort it writes a structured `abort.json`, sends TERM and then KILL only to the recorded test sessions, deletes only the namespaces created by that run, and preserves the final 200 lines of each test log. The combined wrapper applies equivalent descendant-process, namespace-byte, log, resource, no-progress and timeout guards around the older Leaf DIRECT harness.

The limits can be lowered on embedded validation hosts through documented `ET_PERF_*` environment variables. Raising them is not a way to make a failing candidate pass; a raised limit requires separate evidence that the observed use is expected.

### Platform boundary

- Linux with root and network namespaces: full Leaf DIRECT plus native/KCP/QUIC/forced-relay matrix. The scripts are architecture-neutral; the profiling workflow currently supplies an x86_64-musl bundle.
- Android: not included in the on-device production APK. Android VPN ownership prevents a self-contained second peer and relay from being created safely inside the same application process; existing captured-UID instrumentation and exact remote artifacts remain the Android evidence path.
- Windows and macOS: the Rust probe itself is portable, but neither platform has a network-namespace equivalent already integrated into EasyTier's test package. A loopback-only approximation would not prove TUN/relay isolation and is intentionally not reported as equivalent coverage.
- Production Core, GUI, Android, OHOS, Magisk and release assets remain unchanged and do not include any self-test executable or script.

Initial disposable `.160` topology proof before script integration:

- native upload, 16 MiB: approximately `1.238 Gbit/s`;
- KCP upload, 16 MiB: approximately `0.828 Gbit/s`, with live RPC `transport_type: Kcp`;
- QUIC upload, 16 MiB: approximately `0.447 Gbit/s`, with live RPC `transport_type: Quic`;
- forced relay upload, 16 MiB: approximately `1.076 Gbit/s`, with destination route `path_len: 2` through `perf-r`.

These are topology feasibility numbers from an older validated product artifact, not performance acceptance thresholds for the new harness.

## 2026-07-19 integrated mesh-matrix evidence

The current scripts were copied to `192.168.2.160` and run with the previously validated product binaries at `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`. This proves the harness and topology; it is not current-candidate product performance evidence.

A 16 MiB-per-direction mesh-only run passed all modes:

| Mode | Upload | Download | Required path evidence |
| --- | ---: | ---: | --- |
| native | `1.295 Gbit/s` | `1.201 Gbit/s` | direct one-hop route; no KCP/QUIC proxy flag |
| KCP | `0.882 Gbit/s` | `0.884 Gbit/s` | live RPC reported `transport_type: Kcp` |
| QUIC | `0.463 Gbit/s` | `0.860 Gbit/s` | live RPC reported `transport_type: Quic` |
| forced relay | `1.122 Gbit/s` | `1.021 Gbit/s` | no endpoint underlay route; `path_len: 2` through `perf-r` |

Across that run the largest EasyTier RSS was `31,392 KiB`, the largest FD count was `26`, and the largest thread count was `13`. Every mode passed byte exactness, route/transport proof, TUN and underlay amplification bounds, process survival, namespace cleanup, and stable host-state gates.

The combined entry point then passed with 8 MiB per direction:

- Leaf DIRECT: approximately `1.037 Gbit/s` upload and `1.387 Gbit/s` download.
- Mesh native, KCP, QUIC, and forced relay all passed in the same invocation.
- The top-level report set `all_requested_families_passed`, `safety_watchdogs_passed`, and `production_network_untouched` to true.

Safety fault injection lowered the per-process RSS limit to `1 KiB`. On the first sample the watchdog observed `perf-b` at `16,120 KiB`, wrote `abort.json` with `reason: rss_limit`, terminated the exact process sessions, and left no matching process or namespace. The normal thresholds were not relaxed afterward.

A clean archive was assembled under `/data/easytier-builder`, checksum-verified, extracted into a new directory, and run from that extraction with 4 MiB per direction. Evidence:

- Archive SHA-256: `d001b682b5e0dc3255d07b178ca49773efed0c86171573c732a77c45d43366e3`.
- Archive size: `122,649,639` bytes.
- Members: the four existing profiling binaries, `easytier-perf-probe`, the Leaf and mesh harnesses, the combined entry point, `BUILD_INFO.txt`, and `SHA256SUMS.txt`.
- All member checksums, both `--check-only` paths, the combined run, final host-state gate, and cleanup passed.
- The temporary archive, extraction, and result directory were removed after recording evidence.

The first packaging attempt correctly stopped on `/tmp` exhaustion before starting a test. Exact self-test temporary directories were removed, recovering root free space from effectively zero to approximately `1.2 GiB`; large packaging was moved to `/data/easytier-builder`. This host currently has only about `8.5 GiB` free on `/data`, so future workflow-equivalent packaging must preflight disk space and clean exact staging paths.

Final static exclusion evidence:

- `actionlint` passes for `profiling-beta.yml`.
- No Cargo manifest references any self-test source or script.
- No non-profiling workflow references any self-test source or script.
- Production Core, GUI, Android, OHOS, Magisk and formal release packaging therefore remain unaffected.

Remaining provenance step: when the next build-affecting candidate legitimately runs `profiling-beta.yml`, verify the downloaded exact-SHA artifact contains these members and rerun the combined entry point. Do not trigger a workflow solely for this harness/documentation change.

The combined wrapper's Leaf-specific guard was also fault-injected with a `1 KiB` total RSS limit and `--skip-mesh`. It observed `1,576 KiB`, emitted `reason: total_rss_limit`, terminated the Leaf harness process group with exit status 1, and left no matching process or namespace. The temporary report was removed after its fields and cleanup were verified.
