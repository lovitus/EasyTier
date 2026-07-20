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

## Generic dual-TUN candidate `87301ee0` cross-host audit

This audit is deliberately separate from the `8b48153a` fast-GSO matrix above. Commit `87301ee0831629e5d86c3392b69b126aae9bb6d2` is the earlier generic Leaf-owned dual-TUN implementation, before the Leaf GSO/offload patch. Its owned TUN therefore must report `tun_flags=0x1001`, not `0x5001`.

### Exact artifact and method

- Linux workflow: [run 29682690040](https://github.com/lovitus/EasyTier/actions/runs/29682690040), successful for exact SHA `87301ee0831629e5d86c3392b69b126aae9bb6d2`.
- Android workflow: [run 29682690075](https://github.com/lovitus/EasyTier/actions/runs/29682690075), successful for the same SHA. Android was not rerun in this Linux performance audit.
- Target: `x86_64-unknown-linux-musl`; build ID `efd4e765d99335a85ff5473b22f86f5b602fcde0`.
- Exact archive SHA-256: `32127dfe4b82c0cbff8ac9f8423698051addd218c750d2360526e0b4c2e49860`. The outer archive and all four package binaries passed their recorded SHA-256 checks before deployment.
- Each accepted host result uses three interleaved untraced `legacy` and three `leaf-owned-tun` runs from the candidate's own harness. The comparator requires at least three runs, download and upload ratios at least `0.95`, bounded RSS, idle CPU at most 20%, unchanged host state, and clean core/worker shutdown.
- The main medians use only runs 1-3. Later short runs were used only to capture `tun_flags`; they are not mixed into performance medians.
- Evidence snapshot comparison normalized DHCP `valid_lft/preferred_lft` on all hosts and the volatile IPv6 RA `expires Nsec` field on KR. These changes affect evidence comparison only, not the product or traffic path.

### Raw accepted runs

Values in the upload/download columns are bit/s. Each bracket contains runs 1, 2, and 3 in order. RSS is total core plus Leaf-worker peak KiB.

| Host | Kernel | Mode | Upload runs | Download runs | RSS runs KiB |
|---|---|---|---|---|---|
| `.160` | 3.10.0-1160.el7 | legacy | `[694274065, 699224975, 874056459]` | `[1409586000, 1402458000, 1407676000]` | `[26200, 26092, 25932]` |
| `.160` | 3.10.0-1160.el7 | generic-owned-TUN | `[1181905000, 644165935, 676439286]` | `[909245462, 905313166, 933923401]` | `[23392, 23332, 23272]` |
| `.37` | 3.10.0-1160.el7 | legacy | `[688699124, 686638043, 732049801]` | `[882818728, 881879236, 889545101]` | `[26208, 26416, 26200]` |
| `.37` | 3.10.0-1160.el7 | generic-owned-TUN | `[872717092, 1126177000, 1089252000]` | `[1109644000, 1142914000, 1158085000]` | `[23148, 23460, 23280]` |
| `.38` | 3.10.0-1160.el7 | legacy | `[725027110, 714307096, 714739182]` | `[902077879, 881725578, 844931526]` | `[26208, 26192, 26284]` |
| `.38` | 3.10.0-1160.el7 | generic-owned-TUN | `[854326447, 1169878000, 840734966]` | `[1348698000, 1128417000, 1152165000]` | `[23092, 23068, 23032]` |
| lv1g2 | 4.19.0 | legacy | `[740217380, 768960659, 670592249]` | `[540741168, 615938183, 539615716]` | `[33804, 34436, 34716]` |
| lv1g2 | 4.19.0 | generic-owned-TUN | `[853052102, 944693549, 929932036]` | `[941489081, 967291404, 931255345]` | `[33852, 33284, 32732]` |
| lv1g3 | 5.4.0 | legacy | `[577875836, 262920318, 548843930]` | `[391108257, 439291654, 402860901]` | `[34924, 35232, 35060]` |
| lv1g3 | 5.4.0 | generic-owned-TUN | `[683332525, 518483343, 707931540]` | `[695693469, 758538170, 621032344]` | `[34112, 33676, 32944]` |
| KR | 5.10.0 | legacy | `[596626636, 608443303, 609033341]` | `[428811624, 446281743, 491515549]` | `[36048, 31112, 33644]` |
| KR | 5.10.0 | generic-owned-TUN | `[702643653, 760350409, 747455632]` | `[789142186, 815483897, 658644594]` | `[31864, 27632, 34788]` |

### Comparator medians

| Host | Legacy up/down bit/s | Generic up/down bit/s | Download ratio | Upload change | RSS change | Gate |
|---|---:|---:|---:|---:|---:|---|
| `.160` | `699224975 / 1407676000` | `676439286 / 909245462` | `0.6459` | -3.3% | -10.6% | fail |
| `.37` | `688699124 / 882818728` | `1089252000 / 1142914000` | `1.2946` | +58.2% | -11.2% | pass |
| `.38` | `714739182 / 881725578` | `854326447 / 1152165000` | `1.3067` | +19.5% | -12.0% | pass |
| lv1g2 | `740217380 / 540741168` | `929932036 / 941489081` | `1.7411` | +25.6% | -3.3% | pass |
| lv1g3 | `548843930 / 402860901` | `683332525 / 695693469` | `1.7269` | +24.5% | -3.9% | pass |
| KR | `608443303 / 446281743` | `747455632 / 789142186` | `1.7683` | +22.8% | -5.3% | pass |

All 18 accepted generic runs captured traffic on a unique candidate-owned `etp*` interface; all 18 legacy runs captured `tun0`. A separate netns-internal observer run on each of `.37`, `.38`, lv1g2, lv1g3, and KR read `tun_flags=0x1001`, proving that the positive results came from the old generic dual-TUN path and not the later GSO path. Two preliminary flag observers were excluded: the first inspected the host namespace, and the second raced netns removal while traversing sysfs. The final observer uses a read-only helper inside the active test netns; this probe correction did not change candidate code or the three-run performance matrix.

Every accepted run recorded `host_state_unchanged=true`, `core_shutdown_clean=true`, zero idle worker CPU, and no 20% idle-CPU abort. Final cleanup found no candidate process, `etpd-*` namespace, or `etp*` TUN on any host. `.37` retained its pre-existing `etns_scale` namespace. KR retained production EasyTier PID `44990`, production `tun0`, and iperf PID `52372`.

lv1g3 did not see the newly created lv1g2 `/slab2/easytier-87301ee-audit` directory despite its `/slab2` mount reporting the lv1g2 export. To avoid assuming propagation, lv1g3 received the same verified archive into the separately named `/slab2/easytier-87301ee-audit-lv1g3`; its scripts and evidence names were also separate.

Evidence archives under local `.artifacts/87301ee-crosshost/`:

| Host | Main evidence SHA-256 | Flag evidence SHA-256 |
|---|---|---|
| `.37` | `fb13186f692a257b77c219c65aac330860178df30a0cc9b1248be2c1441da10e` | `bb5ee68c159f2006cd431ad02460fc070b7347762f492a6a67f96b40ac5da552` |
| `.38` | `4183aa2a148bb91a6069bd173fc2340a56aa697f0f2848251f00ed515d2f6e38` | `aef3201ec50b2a76ff2c1e973429938e69f11ef845dc3940de6585b4630b0e12` |
| lv1g2 | `37db2ca731acd134b819830abe1360fc3c5ca92a218f7e2d6a5e07c831e6da3d` | `cf6e7fdfb17c25bba17ad02a00929115384e57025de658efadd874ff86319379` |
| lv1g3 | `bb3d95c47fec49e5000f90338e7bcb0da39ec13dec4b253eed53af9b7cf701cb` | `d63b294c57ea7fa67cc734b218c5dec52e23d2afaabef030bd56b5b2f4dfed56` |
| KR | `d924b0e2e87b39e9f33b9bf262b59d4755d143562e642e2087e42e9933598b47` | `07e05ca4d7b5ebec14997a63cbeb263469dad89ab0944cabc59663814ab3c14e` |

### Corrected conclusion

The original `.160` measurement remains valid for that host: generic dual-TUN consistently reduced download throughput there. The mistake was treating that single-host result as evidence that the generic dual-TUN design failed generally. Five additional hosts spanning Linux 3.10, 4.19, 5.4, and 5.10 all pass, improve download by 29.5%-76.8%, improve upload by 19.5%-58.2%, and reduce median RSS by 3.3%-12.0%.

Therefore the previous broad rejection of `87301ee0` was a cross-host false rejection. It does not justify restoring that historical commit separately: the user-approved `f6617c51` candidate already contains the cleaner fast-path hierarchy `fast-GSO -> fast-generic -> legacy`, so the validated generic path remains the capability fallback when GSO is unavailable or initialization fails. `.160` remains a host-specific experimental-feature caveat, not a kernel-version gate.
