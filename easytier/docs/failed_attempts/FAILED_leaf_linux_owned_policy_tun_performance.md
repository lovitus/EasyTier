# FAILED: Leaf-owned Linux policy TUN performance candidate

Status: **rejected; user has not approved another architecture attempt.**

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
