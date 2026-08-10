# Repository Agent Instructions

These rules are mandatory. Prefer the maintained scripts and recorded evidence over
reconstructing commands from memory.

## 1. Canonical checkout and worktrees

- The canonical checkout is `/Volumes/micron512g/code/easytier` and must remain on
  `codex/current`. Start every task there with
  `scripts/ensure-codex-worktree-current.sh --update` before reading or editing project files.
- `codex/current` is a clean integration pointer, not a development branch. Create each
  feature or release branch in its own clearly named worktree from the guarded current HEAD.
  Never continue from a historical version-named worktree merely because it is open.
- The guard may only fast-forward a clean `codex/current`. It must never reset, clean,
  rebase, switch, or discard a dirty/feature worktree. If a worktree is stale, divergent,
  unexpectedly dirty, or contains changes not made in the current task, stop and resolve
  ownership before proceeding.
- Never use destructive Git operations, force-push, `stash pop/drop`, or remove a worktree
  that contains unpreserved work. Preserve concurrent work by branch/ref before integration.
- After release evidence is recorded, fast-forward `codex/current` to the accepted release
  branch, remove obsolete version worktrees, and run `git worktree prune`.

## 2. Editing and commit gates

- Batch all known related implementation, tests, platform `cfg`, generated files, dependency
  pins, examples, and pre-build user documentation into one candidate. GitHub Actions is not
  an edit-by-edit compiler.
- For a non-trivial policy/Leaf or multi-module batch, maintain the existing parallel workboard
  with each lane's objective, build-affecting state, evidence target, status, and shared SHA.
  Workboard-only updates stay local and never trigger workflows.
- Do not compile EasyTier on the maintainer's Mac. Local formatting is required and allowed;
  for Rust 2024 files use the project toolchain explicitly when needed, for example
  `rustup run 1.95 rustfmt --edition 2024 FILE...`.
- Run `scripts/pre-commit-check.sh` before every commit. It is the hard syntax/format gate for
  whitespace, Rust, shell, JSON, and GitHub Actions. Never commit first and let `.160` or CI
  discover mechanical errors.
- Run `scripts/check-command-environment.sh` before local release or artifact commands. Fix
  invalid `LANG`, `LC_CTYPE`, or `LC_ALL` at the maintained environment source; do not hide
  locale faults with one-off command prefixes. In zsh, never use lowercase `path` as a local
  or loop variable because it mutates `PATH`.
- Treat every unexpected non-zero exit as evidence: preserve the first command, exit code,
  and unfiltered stderr; classify product, environment, dependency, transport, or timeout
  before retrying. Never mask required failures with `|| true` or an unconditional success
  message. Use `set -o pipefail` when producer status matters.
- When the maintainer authorizes a workspace commit, include all safe tracked and untracked
  non-code changes already present. Exclude only credentials/private host metadata, generated
  output, caches, machine-local files, or items explicitly excluded by the maintainer. Never
  sweep unrelated source changes into a candidate.
- Documentation-only and post-build evidence changes must not trigger candidates or formal
  workflows. Accumulate them locally, then publish with `[skip ci]` after the immutable build
  or release SHA exists. If a docs-only push starts a workflow, cancel it immediately.

## 3. Immutable candidate and release sequence

The normal sequence is exactly:

1. Freeze scope and release version.
2. Complete the whole code batch and its tests/docs.
3. Run local formatting and `scripts/pre-commit-check.sh`.
4. Run the complete `.160` preflight.
5. Commit and push one immutable candidate.
6. Run Linux Profiling Beta and Android Policy Candidate in parallel.
7. Verify those exact artifacts and run Linux/Android real-device validation in parallel.
8. Run Core, GUI, Mobile, OHOS, and Test together against the same SHA.
9. Audit formal artifacts and run installed-artifact smoke tests.
10. Dispatch EasyTier Release for that SHA.
11. Append evidence with `[skip ci]`, update the Release body, and advance `codex/current`.

Hard constraints for this sequence:

- Before the first candidate build, choose a version whose `vVERSION` tag does not exist.
  Use only `X.Y.Z` or the cross-platform numeric prerelease form `X.Y.Z-N` (`N <= 65535`).
  Update every Cargo/Tauri/Android version, user-facing release notes, candidate manifest, and
  validation matrix in the same pre-build snapshot. Never validate under an already-published
  version and bump the version afterward.
- The pre-build manifest records scope, intended evidence, `.160` commands/tests, dependency
  pins, and workflow set. It must not claim future run IDs, measurements, PASS results, assets,
  or publication facts.
- Any tracked build-affecting change after candidate dispatch creates a new SHA and invalidates
  all earlier candidate, artifact, real-device, and formal evidence. Documentation-only
  evidence never creates a new candidate and must not move the release ref before publication.
- Reuse every successful or active exact-SHA workflow. Before dispatch, query existing runs;
  never duplicate a run, and remember that a duplicate Android dispatch can cancel the
  authoritative run through concurrency rules. A failed run is fail-closed. Repeat the same
  SHA only for a demonstrated infrastructure/flaky failure, with the diagnosis recorded;
  source fixes require a new complete candidate.
- A push to an auto-trigger branch such as `codex/profiling-beta` may already have started both
  candidate workflows. Query by exact SHA before any manual dispatch; the operator must reuse
  those runs rather than creating a second pair.
- Candidate workflow success does not replace real-device validation. Formal workflows begin
  only after exact Linux and applicable Android artifacts have passed their planned functional,
  lifecycle, recovery, resource, and cleanup matrix.
- Start all five formal workflows together. Do not serialize Core first in the normal release
  path. On the first formal failure, stop remaining peers, diagnose once, and avoid partial
  repeated matrices.
- Freeze the release branch from formal dispatch through Release. Do not push run IDs, evidence,
  wording fixes, or any other tracked change to that ref in between.
- Use build wait time for independent diff/pin review, fixture preparation, host cleanup,
  baseline capture, and validation scripting. Do not mutate the in-flight snapshot or start a
  competing build.

## 4. Release operator

- `scripts/release-operator.sh` is the sole entry point for `.160` preparation and release
  workflow dispatch. Do not reconstruct `rsync` or `gh workflow run` commands from memory.
- Use `builder-preflight` once for the complete candidate. Use `dispatch-candidates SHA`, then
  validate the exact artifacts, then set `EXACT_ARTIFACT_VALIDATED_SHA=SHA` and use
  `dispatch-formal SHA`. Use `dispatch-release vVERSION SHA` only after formal and artifact
  audits pass. `dispatch-pipeline` is resumable but must stop between candidate and formal
  stages unless that exact validation attestation is present.
- Mutation commands must reject dirty, detached, unpushed, SHA-mismatched, incomplete, or
  already-published version inputs. Existing active/successful exact-SHA runs are reused.
- Linux Profiling Beta is always required. Android Policy Candidate is required by default;
  use `ANDROID_CANDIDATE_MODE=skip` only when Android is unrelated and record explicit `N/A`
  or maintainer waiver. Mobile compilation never substitutes for Android physical evidence.
- The formal Release workflow resolves artifacts by its own exact `GITHUB_SHA`. Do not move the
  dispatch ref between formal runs and Release.
- Download GitHub artifacts through the configured local proxy when direct transfer is slow.
  Use a fresh destination, total timeout, checksum/build-info verification, and `unzip -tq`.
  Never append or resume a partial Actions ZIP because redirect handling does not preserve a
  trustworthy byte range.

## 5. `.160` builder and dependency handling

- `192.168.2.160` is the dedicated fast compiler/focused-test builder, not a performance or
  old-system validation host. Source sync must use `scripts/remote-builder-sync.sh`; it checks
  Cargo/rustc idleness, preserves Cargo/Node/Corepack caches, and deletes old source only after
  transfer succeeds.
- Standard preflight is `scripts/release-operator.sh builder-preflight`. For Leaf/HEV Rust use
  `scripts/leaf-remote-preflight.sh`; it builds the relevant library test binary once and runs
  focused tests serially. Extend its filter list instead of using a narrow target that may run
  zero relevant tests.
- Container: `easytier-debug-builder`; workspace `/workspace`; host source
  `/data/easytier-builder/workspace`; Cargo registry
  `/data/easytier-builder/cargo-registry`. `mold` is required. Use
  `CARGO_BUILD_JOBS=$(nproc)`, debug/smallest-feature builds, and bounded `timeout` for every
  Cargo command. Never produce manual release/profile artifacts.
- Forward the maintainer proxy on every builder SSH operation with
  `-R 7890:127.0.0.1:7890` plus keepalive and `ExitOnForwardFailure=yes`. The local proxy is
  known available; do not probe for or report it as missing.
- Before Cargo, confirm no competing Cargo/rustc process. Redirect long `docker exec` output to
  a file, read it in a separate SSH call, and never pipe it directly through `head`, `tail`, or
  `grep`. Prefer `cargo test --no-run`, then execute the exact test binary with a timeout.
- Frontend preflight uses Node 22 at `/opt/node22` and must run in this order: frontend-lib
  Vitest, frontend-lib build, frontend build, VPN plugin build, GUI build. Normal dependency
  preparation is incremental. Only `builder-frontend-repair` may remove the fixed approved
  `node_modules` set; never broadly erase caches/dependencies during sync.
- `/tmp` on `.160` is small. Put large archives, extracted bundles, and profiling results under
  `/data/easytier-builder/` and remove only the exact staging directory after recording evidence.
- GNU builder binaries do not run on CentOS 7. Manual binaries for `.37`/`.38` must be non-release
  `x86_64-unknown-linux-musl`; deployable optimized artifacts come only from GitHub workflows.

## 6. Validation hosts and remote safety

- Fixed roles: `.160` builds/tests; `192.168.1.37` and `.38` provide CentOS 7/internal-network
  compatibility and functional validation; the two private public hosts stored in local
  metadata provide shared-NAS 10 Gbps dual-stack, interoperability, and performance evidence;
  `10.20.0.65` is the KR host. Never substitute roles or put private hostnames in the repository.
- The two public hosts share `/slab2`; do not copy artifacts between them. Use separate
  host-named subdirectories for configs, scripts, PIDs, logs, and results. Never stage sustained
  validation data under their `/tmp` or root filesystems.
- Internal hosts have slow GitHub access. Download once through a fast host, verify hashes/build
  info there, then SCP the exact files to `.37` and `.38`.
- Every SSH command uses `ServerAliveInterval=30`, `ServerAliveCountMax=3`, and
  `ConnectTimeout=10`. Start background services with `setsid ... < /dev/null` and verify in a
  separate SSH call; never use a combined `nohup ... & sleep` command.
- Before each remote network test, clean only the intended EasyTier processes/TUNs and verify
  no residue. Use explicit, unique ports for every protocol: UDP=`base`, TCP=`base+1`,
  QUIC=`base+2`, WG=`base+3`, WS=`base+4`; never rely on production default ports.
- Inspect host routes/bridges before allocating namespace CIDRs. Scope temporary-file, PID, FD,
  route, and cleanup assertions to the exact validation instance. Stop immediately on storms,
  unbounded growth, lost host connectivity, or watchdog violations and preserve failure evidence.
- On Linux, EasyTier-owned IPv6 terminal routes use metric `4294967294`; `4294967295` collides
  with the kernel sentinel on old validation kernels.

## 7. Android validation

- Routine product validation is Linux and Android unless the maintainer changes scope. When an
  Android device is declared unavailable, do not reconnect or mutate it until it is explicitly
  made available again.
- Prefer ADB shell/package/network commands, WebView CDP, and direct Tauri/plugin invocations.
  Use screenshots or coordinate clicks only for final visual evidence when semantic control is
  unavailable. Preserve candidate app data across upgrades with `adb install -r`; do not use
  uninstall/clear-data as a shortcut unless the test explicitly requires a clean install.
- Wireless-ADB outage tests must schedule and verify one detached device-side script that logs
  disable and re-enable return codes before Wi-Fi is disabled. Never issue a standalone host-side
  disable that can strand ADB.
- Policy traffic evidence must come from an application UID captured by the VPN. The candidate
  app, ADB shell, and `run-as` are not valid traffic sources. Use the packaged policy probe target
  and instrumentation runner, confirm its UID is in VPN ranges, require application/TLS evidence
  rather than TCP handshake alone, record the controlled VPN-down baseline, and uninstall probe
  packages after validation.
- When counting Android FDs/tasks, use entry-wise output such as `ls -1`; column-formatted `ls`
  piped to `wc -l` is invalid evidence.

## 8. Policy, Leaf, and dependency semantics

- Before changing Leaf or policy behavior, inspect the corresponding Mihomo implementation and
  tests under `/Users/fanli/Documents/mihomo-rev`. Use sing-box when Mihomo lacks the behavior or
  platform integration. Record exact files/functions and observable semantics before editing,
  then add parity/compatibility tests.
- Do not invent policy, DNS/FakeDNS, Geo, group/fallback, lifecycle, loop-prevention, update, or
  hot-path semantics when reference behavior is unknown. Document every intentional EasyTier
  difference, compatibility boundary, failure behavior, and validation evidence.
- Before auditing a Cargo Git dependency, read its exact URL/SHA from `Cargo.lock` and inspect a
  worktree checked out at that SHA. A cache directory or dependency default branch is not proof.
- Keep Leaf/policy integration decoupled from the mesh data plane. Do not modify established mesh
  routing/transport ownership merely to work around a policy-component defect.

## 9. Performance experiments and evidence

- Before changing a production hot path for an experimental optimization, build a focused Rust
  tool/benchmark under `tools/` and run it on `.160` against the unchanged baseline. Measure
  realistic throughput plus CPU, wakeups, syscalls, allocations, queueing, or batching. Reject
  low-value mechanisms before touching production code.
- A microbenchmark authorizes only a small production experiment; it never replaces exact-artifact
  functional, cross-host, resource, recovery, interoperability, and cleanup validation.
- Profiling-only tools and self-tests must remain absent from formal production archives and
  dependency graphs. Their watchdogs are mandatory; any liveness, resource, amplification,
  no-progress, or timeout violation terminates the exact process group and preserves abort data.
- Performance conclusions must use appropriate hosts and identify both endpoints, IP family,
  transport, relay/direct path, build SHA, sample count, CPU, and resource baseline. Do not infer
  10 Gbps WAN behavior from `.160`, `.37`, `.38`, or a noisy mobile link.

## 10. Portability and final artifact gates

- Every release includes the non-waivable 64-bit atomic audit. Record every `AtomicU64`/`fetch_max`
  hit as shim, test-only, or explicitly unreachable on MIPS/MIPSel. MIPS-reachable code may not use
  `std::sync::atomic::AtomicU64`; shim operations must exist in the locked fallback implementation.
  Implement max via load/compare-exchange when `fetch_max` is unavailable.
- Formal Core must contain successful exact-SHA `mips-unknown-linux-musl` and
  `mipsel-unknown-linux-musl` jobs. Neither x86, one endianness, grep, shim imports, nor an older
  SHA substitutes for them.
- Workflow success alone is insufficient. Before Release, inspect exact Core/GUI artifacts for
  feature flags, sidecars/native executables, architecture, endianness, executable permissions,
  checksums/build info, and macOS signatures, then run installed-artifact smoke tests for supported
  delivery paths. A missing feature or sidecar invalidates the candidate even when CI is green.
