# Leaf v1 Release Gates

> This is the only execution board for the Leaf v1 candidate. Keep it short and current.
> Architecture and compatibility notes remain in `leaf_optional_policy_proxy.md`; detailed evidence belongs in `leaf_validation_journal.md`; deferred work belongs in `leaf_post_v1_backlog.md`.

## Candidate state

- Candidate base: `afceaab282b92c61c8c8b1e216358fe810d82395`.
- `afceaab2` is **not** a validated release candidate: its Linux and Android workflows were intentionally cancelled to stop excessive candidate pushes.
- Pending candidate batch: Android native command mapping, authoritative native VPN shutdown with failure-only WebView fallback, dependency-free lifecycle/command contracts, HEV platform-host audit, and release-document split. Record the exact commit SHA locally immediately after the single candidate commit; do not create a second documentation-only workflow commit.
- Last exact Linux/Android build baseline: `61c6f313559cedce3453970e2729c6eb7035e48a`.
- `61c6f313` passed Linux lifecycle and Android HEV traffic validation, but Android cycle 10 exposed a WebView-owned VPN-stop race that left the TUN alive.
- `e8f7e74549f83791ed43a6f692ff7a034bab070d` proved the direct native stop path was reached, but used the wrong native plugin command name and is rejected.
- Local branch, working tree base, and `origin/codex/profiling-beta` were aligned to `afceaab2` before continuing. The remaining tracked local modification to `AGENTS.md` is maintainer-owned and outside the candidate.

## P0 gates

- [ ] Android native VPN stop is independent of WebView readiness and JavaScript queue progress.
- [ ] Native success does not schedule a redundant second stop through the frontend.
- [ ] Native failure preserves the existing frontend fallback and reports the native failure.
- [ ] Stop/start, process death, Wi-Fi loss/recovery, and repeated cycles return TUN, HEV, Leaf, FD, thread, and task ownership to baseline.
- [x] HEV hosting and shutdown boundaries are audited for Windows, macOS, Linux, Android, iOS, and constrained targets; v1 claims only evidence actually obtained.
- [ ] The v1 capability boundary is frozen: unsupported advanced transports or rule/DNS fields are rejected, hidden, or explicitly experimental.
- [ ] Default configuration remains simple: DIRECT and mesh work without HEV-specific tuning; optional chain/fallback examples do not silently imply UoT or KCP.

## One-push preflight

- [x] Format changed Rust files locally with Rust 1.95 and edition 2024.
- [x] Run remote minimal `cargo test --no-run` or `cargo check` for the smallest affected target after confirming no cargo/rustc process is active.
- [x] Run the exact focused test binary separately.
- [x] Inspect `Cargo.lock`, platform `cfg` boundaries, workflow commit pins, generated bindings, and the complete candidate diff.
- [ ] Confirm documentation evidence names the exact candidate SHA.
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
