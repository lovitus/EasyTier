# Leaf-owned Linux policy TUN cross-kernel validation and restoration decision

Status: **restoration approved by the user on 2026-07-19; restore the exact validated implementation without a kernel gate or adaptive performance logic.**

This document is the audit record for the bounded Leaf-owned TUN/GSO candidate. Do not reintroduce the candidate merely because Linux TUN offload is available: the exact optimized artifact enabled TSO/GSO/GRO correctly but failed the frozen same-host download-throughput gate.

## Frozen boundary

- Product baseline: `0cf368072aad4882309e6f6d450e45f5f4e1a9ac` plus later non-performance Leaf protocol work.
- Rejected EasyTier candidate: `8b48153acc286c70c70faf8a2e4d1cb3c015be05`.
- Rejected Leaf fork revision: `a5bb6a31df2c62200be052b61ca01b01ea5e3c25`.
- Linux workflow: run `29685940754`.
- Android workflow: run `29685940756`.
- Artifact target: `x86_64-unknown-linux-musl`.
- Artifact SHA-256: `7cea7e78cf7fcfd93cb249bb728cd8955374206ba1df8aa0f6b3d50b55715571`.
- EasyTier build ID: `a4bd266d20a29c9f307c4d0daa761b62b9f066cf`.

The experiment changed only the experimental Linux policy data-plane boundary. It did not modify EasyTier mesh, HEV, KCP/QUIC, DNS, rules, or proxy protocol behavior. Its intended generation-scoped fallback was `fast-gso -> fast-generic -> legacy`.

## Build and artifact evidence

- The standard `scripts/leaf-remote-preflight.sh` gate passed on `192.168.2.160`: one feature-unified `--locked` no-run build and all configured focused tests passed.
- Both exact-SHA Linux and Android workflows completed successfully.
- The release manifest, outer archive checksum, package-internal checksums, `BUILD_INFO.txt`, musl target, build ID, debug sections, and unstripped symbols were verified before deployment.
- No Android real-device performance run was started after the Linux hard gate failed.

## Same-artifact `.160` A/B

All acceptance runs were untraced, used the exact `8b48153a` artifact, and ran three times per mode on the same host.

| Mode | Run | Upload bit/s | Download bit/s | Max RSS total KiB |
|---|---:|---:|---:|---:|
| legacy | 1 | 737,063,135 | 1,377,480,000 | 25,832 |
| legacy | 2 | 933,772,976 | 1,416,531,000 | 25,988 |
| legacy | 3 | 715,051,186 | 1,410,275,000 | 26,140 |
| leaf-owned-tun | 1 | 1,281,511,000 | 1,179,041,000 | 25,412 |
| leaf-owned-tun | 2 | 729,834,544 | 1,192,125,000 | 25,492 |
| leaf-owned-tun | 3 | 1,314,642,000 | 1,187,949,000 | 25,620 |

Comparator medians:

- legacy download: `1,410,275,000 bit/s`;
- leaf-owned download: `1,187,949,000 bit/s`;
- download ratio: `0.8424`, below the frozen `0.9500` gate;
- legacy upload: `737,063,135 bit/s`;
- leaf-owned upload: `1,281,511,000 bit/s`;
- legacy RSS: `25,988 KiB`;
- leaf-owned RSS: `25,492 KiB`, only `496 KiB` lower (about 1.9%);
- all six runs reported zero idle CPU, clean core/worker shutdown, and unchanged host state.

The fixed comparator returned failure solely for download throughput: `download_bps: ratio 0.8424 is below 0.9500`.

## GSO and dual-TUN proof

A separate diagnostic run, excluded from the three-run acceptance medians, observed the namespace while traffic was active:

- Leaf-owned interface: `etp4a830001`, `tun_flags=0x5001`;
- `tx-checksumming`, scatter-gather, TCP segmentation offload, generic segmentation offload, and generic receive offload were all enabled;
- fallback EasyTier `tun0`: RX remained `0`, TX increased only from `48` to `144` bytes;
- Leaf-owned TUN: final RX `908,559,370`, TX `663,776,427` bytes.

Therefore the failure was not caused by silently falling back to generic TUN, by measuring the wrong TUN, or by the old dual-TUN capture bug. GSO was active and the owned TUN carried the workload.

## Decision

Reject the full Leaf-owned policy TUN candidate. The 15.8% median download regression violates the explicit hard gate, while the RSS improvement is negligible. Do not spend another workflow on newer-kernel, Android, macOS, Windows, or failure-injection validation for this architecture.

The product keeps the pre-experiment policy data plane and all unrelated Leaf protocol functionality. Any future performance work requires a newly approved, narrower hypothesis and must not revive PacketBatch or full Leaf-owned TUN by incremental patching.

## Auditable revert commits
- `2cd94a4082834a491d6b12f105fb562aab4d5403` Revert "feat(policy): add experimental Leaf-owned TUN fast path"
- `872e9442699aac27c9afacebed3a3a97863fe07d` Revert "fix(policy): cover in-process leaf candidate"
- `fcf72700c4a2acce44b3521b7ba59e5279bd0c1c` Revert "fix(policy): wait for leaf owned tun readiness"
- `95f32790345d5faddcf4be0da84eca054053553d` Revert "perf(policy): add Leaf Linux TUN offload candidate"

## Superseding cross-host evidence

The earlier `.160`-only rejection above is retained as an audit trail but is not the current conclusion. The same exact `8b48153a` artifact was subsequently run on two independent 10 Gbps VPS hosts, with three untraced same-host namespace runs per mode.

| Host boundary | Kernel | Legacy upload/download median | Fast-GSO upload/download median | Fast/legacy download | RSS delta |
|---|---|---:|---:|---:|---:|
| `.160` | CentOS 7 / 3.10 | 737.1 / 1,410.3 Mbit/s | 1,281.5 / 1,187.9 Mbit/s | `0.8424` (-15.8%) | -496 KiB (-1.9%) |
| `lv1g2` | Debian / 4.19 | 697.3 / 586.6 Mbit/s | 947.1 / 898.7 Mbit/s | `1.5320` (+53.2%) | +856 KiB (+2.3%) |
| `lv1g3` | Ubuntu / 5.4 | 423.1 / 363.5 Mbit/s | 746.2 / 683.1 Mbit/s | `1.8791` (+87.9%) | +460 KiB (+1.3%) |

On lv1g2, `tun_flags=0x5001`, TCP segmentation offload, generic segmentation offload, and generic receive offload were active. On lv1g3, `tun_flags=0x5001` was observed. On both hosts the Leaf-owned `etp*` interface carried the workload while fallback `tun0` remained at zero RX and only control-byte TX. Every accepted run reported bounded idle CPU, clean core/worker shutdown, and unchanged host state. The initial lv1g3 attempt was excluded because its DHCP lease lifetime decreased normally and the unmodified harness treated that timestamp drift as a host-state failure; a temporary harness normalized only `valid_lft/preferred_lft` before the clean three-run matrix.

Current interpretation:

- full rejection based on `.160` was incorrect;
- five of six tested Linux hosts pass the frozen throughput gate, including two of the three CentOS 7 / Linux 3.10 hosts;
- kernel version does not explain the `.160` regression, so a `<4.19 -> legacy` rule would be unsupported and would discard proven gains on `.37` and `.38`;
- the current `c5051e1fe6fe1bb18800fdd6aacaa7478b9751dc` revert is a safe temporary product state, not the final performance decision;
- the minimal recommended restoration is the already-audited candidate behind its existing explicit experimental feature, disabled by default, with `.160` recorded as a host-specific negative result rather than inventing an automatic kernel gate.

## Expanded six-host matrix

The exact `8b48153acc286c70c70faf8a2e4d1cb3c015be05` artifact was additionally deployed to `.37`, `.38`, and the KR validation host. The outer archive SHA-256, all four package-internal binary hashes, `BUILD_INFO.txt`, run `29685940754`, and target `x86_64-unknown-linux-musl` passed independently on every host.

All accepted samples are untraced. Each host ran three interleaved legacy/Leaf-owned-TUN samples with the same artifact and harness. Volatile DHCP `valid_lft/preferred_lft` values were normalized on every new host. KR also normalized only IPv6 RA route `expires Nsec`; no address, route target, rule, interface, namespace, or process difference was ignored.

### `.37` raw samples

Kernel: CentOS 7, `3.10.0-1160.el7.x86_64`.

| Mode | Run | Upload bit/s | Download bit/s | TUN bytes | RSS total KiB | Idle core/worker CPU | Clean |
|---|---:|---:|---:|---:|---:|---:|---|
| legacy | 1 | 663,898,450 | 824,781,735 | 1,141,671,072 | 26,452 | 0.000 / 0.000 | yes |
| legacy | 2 | 654,709,632 | 848,357,931 | 1,153,100,004 | 26,432 | 0.000 / 0.000 | yes |
| legacy | 3 | 739,485,553 | 880,433,264 | 1,241,676,133 | 26,296 | 0.333 / 0.000 | yes |
| fast-GSO | 1 | 1,203,938,000 | 1,139,526,000 | 1,788,695,126 | 25,124 | 0.000 / 0.000 | yes |
| fast-GSO | 2 | 920,925,969 | 1,131,905,000 | 1,571,944,334 | 25,204 | 0.000 / 0.000 | yes |
| fast-GSO | 3 | 1,283,576,000 | 1,280,857,000 | 1,958,642,478 | 25,152 | 0.333 / 0.000 | yes |

Comparator medians: legacy `663,898,450 / 848,357,931 bit/s` upload/download and `26,432 KiB` RSS; fast-GSO `1,203,938,000 / 1,139,526,000 bit/s` and `25,152 KiB`. Fast-GSO improves upload by 81.3%, download by 34.3%, and lowers RSS by 1,280 KiB (4.8%). Comparator result: pass.

The owned interface reported `tun_flags=0x5001`; checksum, scatter-gather, TSO, GSO, and GRO were enabled. The final watcher sample recorded fallback `tun0` RX/TX `0/144` and owned `etp30dc0001` RX/TX `879,350,853/1,016,425,030`. The pre-existing `etns_scale` namespace remained present and unchanged after validation; no candidate process, candidate namespace, TUN, or iperf listener remained.

Evidence root: `/data/easytier-validation/owned-tun-8b48153a-dot37`.

### `.38` raw samples

Kernel: CentOS 7, `3.10.0-1160.el7.x86_64`.

| Mode | Run | Upload bit/s | Download bit/s | TUN bytes | RSS total KiB | Idle core/worker CPU | Clean |
|---|---:|---:|---:|---:|---:|---:|---|
| legacy | 1 | 701,884,761 | 874,894,180 | 1,208,911,213 | 26,300 | 0.333 / 0.000 | yes |
| legacy | 2 | 713,279,605 | 879,785,489 | 1,221,862,825 | 26,396 | 0.000 / 0.000 | yes |
| legacy | 3 | 725,457,813 | 906,100,456 | 1,250,658,849 | 26,356 | 0.000 / 0.000 | yes |
| fast-GSO | 1 | 986,133,508 | 1,146,305,000 | 1,630,986,974 | 25,088 | 0.000 / 0.000 | yes |
| fast-GSO | 2 | 931,439,547 | 1,213,707,000 | 1,643,943,858 | 25,116 | 0.000 / 0.000 | yes |
| fast-GSO | 3 | 917,912,501 | 1,238,781,000 | 1,653,381,450 | 25,084 | 0.333 / 0.000 | yes |

Comparator medians: legacy `713,279,605 / 879,785,489 bit/s` upload/download and `26,356 KiB` RSS; fast-GSO `931,439,547 / 1,213,707,000 bit/s` and `25,088 KiB`. Fast-GSO improves upload by 30.6%, download by 38.0%, and lowers RSS by 1,268 KiB (4.8%). Comparator result: pass.

The owned interface reported `tun_flags=0x5001`; checksum, scatter-gather, TSO, GSO, and GRO were enabled. This kernel did not expose usable per-interface `statistics/*_bytes` values to the external watcher, so the watcher recorded `missing`; the harness-owned `tun_bytes` values above and `capture_tun=etp5ab90001` provide the accepted byte-accounting evidence. No candidate process, namespace, TUN, or listener remained.

Evidence root: `/data/easytier-validation/owned-tun-8b48153a-dot38`.

### KR raw samples

Kernel: Debian, `5.10.0-45-amd64`.

| Mode | Run | Upload bit/s | Download bit/s | TUN bytes | RSS total KiB | Idle core/worker CPU | Clean |
|---|---:|---:|---:|---:|---:|---:|---|
| legacy | 1 | 579,847,560 | 425,204,162 | 774,212,447 | 36,476 | 0.000 / 0.000 | yes |
| legacy | 2 | 618,349,799 | 467,670,206 | 836,386,151 | 32,544 | 0.333 / 0.000 | yes |
| legacy | 3 | 541,911,223 | 456,785,606 | 769,849,048 | 25,508 | 0.000 / 0.000 | yes |
| fast-GSO | 1 | 767,307,214 | 793,195,128 | 1,201,676,502 | 36,204 | 0.000 / 0.000 | yes |
| fast-GSO | 2 | 852,043,469 | 850,163,695 | 1,310,102,968 | 38,132 | 0.333 / 0.000 | yes |
| fast-GSO | 3 | 806,151,256 | 825,631,131 | 1,256,631,478 | 26,732 | 0.333 / 0.000 | yes |

Comparator medians: legacy `579,847,560 / 456,785,606 bit/s` upload/download and `32,544 KiB` RSS; fast-GSO `806,151,256 / 825,631,131 bit/s` and `36,204 KiB`. Fast-GSO improves upload by 39.0%, download by 80.7%, and increases RSS by 3,660 KiB (11.2%). Comparator result: pass.

The owned interface reported `tun_flags=0x5001`; checksum, scatter-gather, TSO, GSO, and GRO were enabled. The final watcher sample recorded fallback `tun0` RX/TX `0/144` and owned `etp3b480001` RX/TX `585,615,682/690,487,889`.

The first KR legacy attempt completed throughput but was excluded because the production IPv6 RA route expiry refreshed from `8972sec` to `8994sec`, which the original strict host-state comparison treated as a change. Only that volatile expiry was normalized before the fresh six-run matrix. Production EasyTier PID `44990`, production `tun0`, and existing iperf PID `52372` remained alive; no candidate process, namespace, TUN, or listener remained.

Evidence root: `/root/easytier-validation/owned-tun-8b48153a-kr`. The complete local evidence archive was verified as SHA-256 `e017563600f803577970942b22ed15ecf4e06d096e6a03416e60fdac0bbd65d0`.

## Six-host conclusion

| Host | Kernel | Download fast/legacy | Upload change | RSS change | Gate |
|---|---|---:|---:|---:|---|
| `.160` | 3.10 | `0.8424` | +73.9% | -1.9% | fail |
| `.37` | 3.10 | `1.3432` | +81.3% | -4.8% | pass |
| `.38` | 3.10 | `1.3795` | +30.6% | -4.8% | pass |
| lv1g2 | 4.19 | `1.5320` | +35.8% | +2.3% | pass |
| lv1g3 | 5.4 | `1.8791` | +76.3% | +1.3% | pass |
| KR | 5.10 | `1.8074` | +39.0% | +11.2% | pass |

The candidate passes on five of six independent Linux hosts. `.160` is a real negative result but is not correlated with kernel version: `.37` and `.38` run the same CentOS 7 / Linux 3.10 kernel and pass by wide margins. No automatic kernel-version gate is justified by this evidence.

The narrow recommendation is to restore the already-audited implementation only behind its existing explicit experimental feature, keep it disabled by default, preserve `fast-gso -> fast-generic -> legacy` capability fallback, and document that experimental enablement requires host-local measurement. Do not add runtime micro-benchmarks, adaptive switching, or host-specific heuristics.

## User-approved restoration

The user approved restoration after reviewing the complete six-host matrix. The restoration candidate must satisfy these constraints:

- restore the exact non-document implementation tree from `8b48153acc286c70c70faf8a2e4d1cb3c015be05`;
- keep the feature explicitly experimental and disabled by default;
- preserve capability fallback `fast-gso -> fast-generic -> legacy`;
- do not add a kernel-version gate, runtime micro-benchmark, adaptive switching, host allowlist, or EasyTier mesh change;
- retain `.160` as a real host-specific negative result and retain all five positive results;
- batch the restored code and corrected evidence into one `.160` preflight and one automatic Linux/Android workflow set.

The safe revert `c5051e1fe6fe1bb18800fdd6aacaa7478b9751dc` remains in history as an auditable decision point. Restoration is a new forward commit; no destructive reset or history rewrite is permitted.
