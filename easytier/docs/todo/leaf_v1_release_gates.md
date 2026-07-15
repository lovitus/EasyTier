# Leaf v1 Release Gates

> This is the only execution board for the Leaf v1 candidate. Keep it short and current.
> Architecture and compatibility notes remain in `leaf_optional_policy_proxy.md`; detailed evidence belongs in `leaf_validation_journal.md`; deferred work belongs in `leaf_post_v1_backlog.md`.

## Candidate state

- Exact validated artifact baseline: `00b62e65b9b52bdd2546c0d436e8ffc8acea6d2c`; unique Linux/Android workflows, hashes/signature, built-in HEV TCP/UDP in both directions, captured-UID policy TLS semantics, and semantic Android stop/start passed. It is not releasable because HEV TCP throughput remained about 5-6% of DIRECT and a three-peer Wi-Fi outage exposed stale OSPF peer-info recovery.
- Local batched successor: policy-only reuse of the existing KCP endpoint without enabling user SOCKS/KCP behavior, bounded smoltcp fallback, and OSPF session-generation restart after peer-info removal. The mandatory `.160` gate passed; no candidate SHA or artifact claim exists until this snapshot is frozen.
- Historical `afceaab282b92c61c8c8b1e216358fe810d82395` workflows were intentionally cancelled to stop excessive candidate pushes and provide no artifact evidence.
- `61c6f313` passed Linux lifecycle and Android HEV traffic validation, but Android cycle 10 exposed a WebView-owned VPN-stop race that left the TUN alive.
- `e8f7e74549f83791ed43a6f692ff7a034bab070d` proved the direct native stop path was reached, but used the wrong native plugin command name and is rejected.
- Local branch, working tree base, and `origin/codex/profiling-beta` were aligned to `afceaab2` before continuing. The remaining tracked local modification to `AGENTS.md` is maintainer-owned and outside the candidate.

## P0 gates

- [ ] Android native VPN stop is independent of WebView readiness and JavaScript queue progress.
- [ ] Native success does not schedule a redundant second stop through the frontend.
- [ ] Native failure preserves the existing frontend fallback and reports the native failure.
- [ ] Stop/start, process death, Wi-Fi loss/recovery, and repeated cycles return TUN, HEV, Leaf, FD, thread, and task ownership to baseline.
- [ ] Built-in HEV TCP approaches the proven existing KCP path without changing explicit user SOCKS/KCP configuration, and KCP-disabled destinations fail over to mesh smoltcp without kernel/direct escape.
- [ ] A third peer relearns an Android peer through the hub after Wi-Fi loss/recovery without waiting for a new direct peer connection.
- [x] HEV hosting and shutdown boundaries are audited for Windows, macOS, Linux, Android, iOS, and constrained targets; v1 claims only evidence actually obtained.
- [ ] The v1 capability boundary is frozen: unsupported advanced transports or rule/DNS fields are rejected, hidden, or explicitly experimental.
- [ ] Default configuration remains simple: DIRECT and mesh work without HEV-specific tuning; optional chain/fallback examples do not silently imply UoT or KCP.

## One-push preflight

- [x] Format changed Rust files locally with Rust 1.95 and edition 2024.
- [x] Run remote minimal `cargo test --locked --no-run` for the complete KCP/policy/OSPF batch after confirming no cargo/rustc process is active.
- [x] Run KCP endpoint isolation 1/1, OSPF generation/cache invalidation 1/1, and mesh relay 8/8 directly from the built test binary.
- [x] Inspect `Cargo.lock`, platform `cfg` boundaries, workflow pins, generated bindings, and the complete candidate diff; no sensitive/generated file changed and `git diff --check` passed.
- [ ] Record the new exact candidate SHA in the local journal immediately after the single commit.
- [ ] Commit and push one complete candidate snapshot to `codex/profiling-beta`.
- [ ] Run one Linux and one Android workflow pair for that exact snapshot.

## Exact-candidate acceptance

- [ ] Verify workflow commit SHA, `BUILD_INFO.txt`, build ID, symbols, target, signer, and `SHA256SUMS.txt`.
- [ ] Linux: normal stop, SIGTERM, Leaf/HEV crash, route/network replacement, fail-closed, repeated lifecycle, and resource baseline.
- [ ] Android: cold start, stop/start, Leaf/HEV failure, Wi-Fi loss with Wi-Fi restored before wireless ADB continuation, network recovery, repeated lifecycle, and resource baseline.
- [ ] Linux and Android: real TCP and UDP through DIRECT, mesh, chain, and fallback configurations within the frozen v1 boundary.
- [ ] No screenshots or simulated taps are used for Android control; screenshots are reserved for final visual evidence.

## Workflow rule

The rolling beta validates a complete candidate; it is not the compiler feedback loop. Do not push again for a single mechanical fix. Accumulate related fixes, run the remote minimal preflight and exact tests, inspect the full diff, then create one candidate.
