# Repository Agent Instructions

- Do not compile this repository on the maintainer's local machine.
- Local formatting is allowed and expected before a validation push. Because this workspace uses Rust 2024 syntax, format changed Rust files with the project toolchain and an explicit edition (for example `rustup run 1.95 rustfmt --edition 2024 FILE...`) when `cargo fmt` resolves modules with an older edition. Formatting is not a build and must not be deferred to CI.
- Every Leaf or policy-proxy implementation MUST first inspect the corresponding behavior and tests in the local Mihomo source tree at `/Users/fanli/Documents/mihomo-rev`. Inspect sing-box as an additional reference when Mihomo does not cover the feature, when the two projects intentionally differ, or when platform integration differs. This requirement applies to rule ordering and fallthrough, DNS/FakeDNS, GeoIP/GeoSite resources, caches, hot paths, proxy groups and failover, network-change recovery, loop prevention, resource updates, lifecycle, and error handling.
- Before editing Leaf/policy behavior, record the exact Mihomo and/or sing-box source files, functions, and externally observable semantics being followed in the working notes, TODO, test name, or code comment. Add parity or compatibility tests for the relevant behavior. If the reference behavior cannot be established, do not invent a new semantic or performance-sensitive mechanism; continue investigation first.
- EasyTier may intentionally differ only where its mesh architecture, pinned Leaf API, or platform constraints require it. Every intentional difference MUST document its reason, compatibility boundary, failure behavior, and validation evidence before merge. Do not claim Mihomo/sing-box compatibility for an unimplemented subset, and do not silently accept unsupported fields or change first-match rule semantics.
- The primary build-and-validation path is the rolling profiling beta workflow:
  1. Commit the exact code and documentation snapshot to `codex/profiling-beta`.
  2. Push the branch and let `.github/workflows/profiling-beta.yml` build the optimized, symbolized x86_64-musl bundle.
  3. Download the `profiling-beta` release assets, verify `SHA256SUMS.txt`, `BUILD_INFO.txt`, commit SHA, build ID, symbols, and target.
  4. Deploy that exact artifact to isolated validation hosts and run functional, performance, resource, and interoperability checks.
  5. If validation fails, use `git revert` for the offending validation commit(s), push the revert, and let the rolling beta rebuild. Never use destructive reset to hide a failed snapshot.
- Documentation-only changes must remain local and must not be pushed immediately. Accumulate them without triggering GitHub workflows. Push documentation only when it accompanies a code snapshot that actually needs build/real-device validation, when a release is being prepared, or when the maintainer explicitly requests a documentation push.
- Do not trigger the profiling beta workflow merely to validate Markdown, plans, reports, TODO files, comments, or other non-build-affecting changes.
- Use the configured remote builder at `root@192.168.2.160` for rapid, targeted dev/debug compilation and exact-test diagnostics whenever that shortens the feedback loop, including while GitHub Actions is producing the final candidate. Do not duplicate a full optimized candidate build there: GitHub remains authoritative for deployable release/profile artifacts.
- Builder container: `easytier-debug-builder` (image: `rust:1.95-bookworm`). Mounts: `/data/easytier-builder/workspace` → `/workspace`, `/data/easytier-builder/cargo-registry` → `/usr/local/cargo/registry`. The repository root is `/workspace`; sync source to `/data/easytier-builder/workspace/` on the host and run Cargo from `/workspace` (the crate itself is `/workspace/easytier`). Build artifacts remain at `/workspace/target` on the 205G disk. The container must have `mold` installed because `.cargo/config.toml` selects it for GNU Linux targets. Cargo network access uses the maintainer's local proxy on `127.0.0.1:7890`: include `-o ExitOnForwardFailure=yes -R 7890:127.0.0.1:7890` on the same keepalive SSH invocation that runs `docker exec`. Do not infer that the proxy is absent from agent-side listener inspection; the required operation is forwarding it to the remote host.
- When compiling on the remote builder, explicitly use all available CPU cores by exporting `CARGO_BUILD_JOBS=$(nproc)` before `cargo build`, `cargo check`, `cargo test`, or `cargo nextest`.
- For rapid, targeted logic preflight on the remote builder, prefer the GNU debug target with the smallest feature set that still contains the code under test. Set `CARGO_PROFILE_TEST_OPT_LEVEL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_INCREMENTAL=1`; then use `cargo test --no-run` followed by the exact test binary. This path is diagnostic only: deployable validation artifacts must still come from the profiling beta workflow, and old CentOS validation hosts must still receive musl binaries.
- Do not increase test threads for tests that share ports, namespaces, UPnP state, routes, or other host-global network state. Parallelize independent test binaries or nextest hash partitions across runners instead, while keeping each network-sensitive partition at `--test-threads 1`.
- ALWAYS wrap long-running cargo commands with `timeout` to prevent indefinite hangs from deadlocked tests or stalled builds. Use: `timeout 600 cargo check ...`, `timeout 1800 cargo build ...`, `timeout 600 cargo test ...` (per test binary), `timeout 1800 cargo test ...` (full suite). Adjust upward if needed but NEVER run cargo without a timeout on the remote builder.
- Before starting any cargo command in the remote builder container, check that no other cargo/rustc process is already running: `ssh root@192.168.2.160 'docker exec easytier-debug-builder bash -c "if pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; then pgrep -a -x cargo; pgrep -a -x rustc; echo BLOCKED; else echo CLEAR; fi"'`. If BLOCKED, investigate before killing only confirmed stale process IDs; broad `pkill -f` patterns can match the inspection shell itself.
- Manual validation builds on the remote builder must not use `--release` or release/profile optimized builds. Use dev/debug builds for all manual testing; release/profile optimized artifacts may only be produced by GitHub workflows.
- The remote validation hosts `192.168.2.160`, `192.168.1.37`, and `192.168.1.38` run old CentOS 7 / Linux 3.10 userspace. GNU debug binaries from the builder require newer glibc and must not be used there; use non-release `--target x86_64-unknown-linux-musl` binaries for manual validation on these hosts.
- Use `10.20.0.65` for the KR validation host. Do not write the host's public domain name in repository docs, scripts, logs, or reports.
- For a pre-release fix on `releases/**`, include `[skip ci]` in the commit message, push the commit, and manually trigger only `.github/workflows/gui-macos-aarch64-test.yml` (`EasyTier GUI macOS ARM64 Test`).
- Do not trigger Core, the full GUI matrix, Mobile, OHOS, the full Test workflow, tags, or Release until the maintainer explicitly confirms real-device validation.
- After that confirmation, run the formal release workflows against the exact validated commit before starting `EasyTier Release`.
- When starting test/validation services on remote hosts, ALWAYS specify explicit ports for ALL protocols in the `-l` listener list, not just UDP. Default ports (11010 TCP, 11011 WG/WS, 11012 QUIC/WSS, 11013 FakeTCP) will conflict with production instances. Use a port base (e.g. 21030) and allocate: UDP=base, TCP=base+1, QUIC=base+2, WG=base+3, WS=base+4. Example: `-l "udp://0.0.0.0:21030,tcp://0.0.0.0:21031,quic://0.0.0.0:21032,wg://0.0.0.0:21033,ws://0.0.0.0:21034/"`. Increment the base by 10 for each new test round.
- When starting test/validation services on remote hosts, ALWAYS clean up old processes and TUN devices BEFORE starting new ones: `killall -9 easytier-core 2>/dev/null; ip link delete tun0 2>/dev/null; ip link delete tun1 2>/dev/null; sleep 1`. Verify no residual processes with `ps aux | grep easytier-core | grep -v grep` before starting.
- When starting background processes via SSH, use `setsid` with `< /dev/null` to detach: `ssh root@HOST 'setsid /path/to/binary ARGS > /tmp/log 2>&1 < /dev/null &'`. NEVER use `nohup ... & sleep N` in a single SSH command — it hangs. Split start and verify into separate SSH calls.
- ALL SSH commands to remote hosts must include keepalive options to prevent firewall/NAT idle disconnection: `ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@HOST '...'`. This is especially critical for commands expected to run >2 minutes (cargo build, cargo test, large file transfers).
- NEVER pipe `docker exec` output through `tail`/`head`/`grep` in the same SSH command. Docker's stdout buffering with non-TTY pipes can block the remote process when the pipe reader closes early. Instead, redirect to a temp file first, then read it in a separate SSH call: Step 1: `ssh ... 'docker exec ... bash -c "cargo test ... > /tmp/result.txt 2>&1"'`, Step 2: `ssh ... 'docker exec ... tail -30 /tmp/result.txt'`.
- Prefer `cargo test --no-run` followed by directly executing the test binary with `timeout`, over running `cargo test` directly. This separates compilation from execution: `ssh ... 'docker exec ... bash -c "cd /workspace && CARGO_BUILD_JOBS=\$(nproc) timeout 600 cargo test --no-run --package easytier --lib -- TEST_NAME 2>&1 | tee /tmp/build.log"'` then `ssh ... 'docker exec ... bash -c "timeout 120 /workspace/target/debug/deps/easytier-HASH TEST_NAME --nocapture 2>&1 | tee /tmp/test.log"'`.
- Validation hosts: `192.168.1.37`, `192.168.1.38`, `192.168.2.160`, `10.20.0.65` (KR), plus two additional hosts whose names are stored locally in `.envrc.local` (excluded from git via `.git/info/exclude`). Do NOT use short names or 198.18.x.x addresses for those hosts.
- Local development and the Android validation device are on the downstream LAN `192.168.234.0/24`; Android ADB is `192.168.234.227:5555`. The downstream hosts can initiate connections to the upstream validation hosts `192.168.1.37`, `192.168.1.38`, and `192.168.2.160`, but those upstream hosts have no route back to `192.168.234.0/24` by default and must not be expected to initiate connections to the local/Android hosts.
- Android policy traffic MUST be generated by an application UID included in the active VPN UID ranges. The candidate app excludes its own UID and both ADB shell and `run-as` execute outside the captured application domain, so none is valid evidence. Use the workflow-built target/runner pair `easytier-android-policy-probe-debug.apk` (`com.kkrainbow.easytier.policyprobe`) and `easytier-android-policy-probe-runner-debug.apk` (`com.kkrainbow.easytier.policyprobe.test`): uninstall stale copies, install both exact artifacts, confirm the target UID is present in `dumpsys connectivity` VPN ranges, then run `adb shell am instrument -w -e host HOST -e port PORT -e timeout_ms 3000 com.kkrainbow.easytier.policyprobe.test/com.kkrainbow.easytier.policyprobe.PolicyProbeInstrumentation`. Require `probe_valid=true`, record `probe_uid`, `probe_selinux_context`, `probe_connected`, elapsed time, error, target, expected first-match rule, and controlled baseline. The target is code-capable only so Android instrumentation can attach and has no runtime components or business classes; the runner contains only on-demand instrumentation. Uninstall both packages after validation.
- The current routine validation scope is Linux and Android only. Do not start macOS workflows or validation unless the maintainer explicitly changes that scope.
- For Android automation, prefer ADB shell/package/network commands, WebView CDP, and direct Tauri/plugin calls. Use screenshots and simulated clicks only for final visual or interaction verification when semantic automation cannot provide the required evidence.

## Remote Cargo Diagnostic / Fallback — Golden Pattern

Whenever the remote builder is used for targeted diagnostics or as a GitHub fallback, ALL remote `cargo build` / `cargo check` / `cargo test` / `cargo nextest` commands MUST follow this pattern. Every line is mandatory — no shortcuts.

```bash
# ===== STEP 0: Pre-flight — check for stale cargo locks =====
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@192.168.2.160 \
  'docker exec easytier-debug-builder bash -c "if pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null; then pgrep -a -x cargo; pgrep -a -x rustc; echo BLOCKED; else echo CLEAR; fi"'
# If BLOCKED → investigate and clean up FIRST. Do NOT proceed.

# ===== STEP 1: Build (separate from execution) =====
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@192.168.2.160 \
  'docker exec easytier-debug-builder bash -c "cd /workspace && CARGO_BUILD_JOBS=\$(nproc) timeout 1800 cargo CMD ARGS > /tmp/easytier_build.log 2>&1; echo EXIT_CODE=\$?"'
# CMD = build | check | test --no-run | nextest archive
# timeout: 600 for check, 1800 for build/full-test-suite

# ===== STEP 2: Read build result =====
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@192.168.2.160 \
  'docker exec easytier-debug-builder tail -50 /tmp/easytier_build.log'
# Check EXIT_CODE. If non-zero → fix errors and retry from Step 0.

# ===== STEP 3: Run test binary directly (only if Step 1 was test --no-run) =====
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@192.168.2.160 \
  'docker exec easytier-debug-builder bash -c "timeout 300 /workspace/target/debug/deps/easytier-* TEST_FILTER --nocapture > /tmp/easytier_test.log 2>&1; echo EXIT_CODE=\$?"'
# Adjust timeout: 120 for unit tests, 300 for integration/network tests, 600 for benchmarks

# ===== STEP 4: Read test result =====
ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ConnectTimeout=10 root@192.168.2.160 \
  'docker exec easytier-debug-builder tail -50 /tmp/easytier_test.log'
```

Key invariants:
- NEVER combine steps into one SSH call — each step is a separate ssh invocation
- NEVER pipe docker exec to tail/head/grep — redirect to file, then read the file
- NEVER omit timeout — every cargo/docker exec command has a timeout
- NEVER use `--release` for manual validation — always dev/debug builds
- NEVER skip the pre-flight check — stale cargo processes cause silent hangs

## Musl Cross-Compilation (for CentOS 7 validation hosts)

When building `x86_64-unknown-linux-musl` binaries on the remote builder, three environment variables are required:

```bash
docker exec easytier-debug-builder bash -c "
export BINDGEN_EXTRA_CLANG_ARGS=\"-I/usr/include/x86_64-linux-musl\"
export CC_x86_64_unknown_linux_musl=musl-gcc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cd /workspace
CARGO_BUILD_JOBS=\$(nproc) timeout 1800 cargo build --target x86_64-unknown-linux-musl --package easytier --bin easytier-core
"
```

These env vars are needed for any `--target x86_64-unknown-linux-musl` build (debug or release) in the current Debian container. The `musl-dev` package installs headers at `/usr/include/x86_64-linux-musl` and does not provide `/usr/x86_64-linux-musl`; do not pass that nonexistent path as a sysroot. Without the include, C compiler, and linker settings, bindgen or final linking fails.
