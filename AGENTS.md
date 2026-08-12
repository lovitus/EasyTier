# Repository Agent Instructions

These are project invariants. Detailed commands belong in maintained scripts and
runbooks, not in this file.

## Start and ownership

- The canonical checkout is /Volumes/micron512g/code/easytier on codex/current.
  Start each task with: scripts/ensure-codex-worktree-current.sh --update
- Develop in a named branch/worktree created from the guarded codex/current. Never
  continue from an old release worktree.
- Stop on unexpected changes. Never reset, clean, force-push, drop stashes, or remove
  unpreserved work. Unrelated parallel worktrees do not block a release.
- After publication, record evidence with [skip ci], fast-forward codex/current, and
  remove obsolete clean worktrees.

## Development

- Batch related code, tests, platform configuration, generated files, dependency pins,
  examples, and pre-build documentation into one candidate.
- Do not compile EasyTier on the maintainer's Mac. Local formatting is required; run
  scripts/pre-commit-check.sh before every commit.
- Use .160 for compiler and focused-test feedback. GitHub Actions is not an
  edit-by-edit compiler.
- The Rust release preflight must run the workflow-shared GUI sidecar setup and
  formal formatting, Clippy, each-feature and lockfile checks before focused tests.
  Never recreate those gates with ad-hoc SSH or `docker exec` commands.
- Treat the first non-zero exit as evidence. Preserve its command, exit code, and stderr,
  diagnose it, then retry. Never hide required failures.
- Documentation/evidence-only commits use [skip ci] and never invalidate an already
  built SHA.

## Normal release

The complete runbook is easytier/docs/release/release_operator.md. The normal path has
four commands:

    scripts/release-operator.sh init-release
    scripts/release-operator.sh prepare
    scripts/release-operator.sh validate
    EXACT_ARTIFACT_VALIDATED_SHA="$(git rev-parse HEAD)" \
      scripts/release-operator.sh publish "v$(scripts/release-operator.sh version)"

- Freeze version, scope, release notes, manifest, matrix, and dependency pins before
  prepare. Any later build-affecting change creates a new candidate.
- prepare runs local gates and only the required .160 Rust/frontend scopes.
- validate reuses exact-SHA runs, waits for Linux/required Android candidates, then
  starts Core, GUI, Mobile, OHOS, and Test together.
- As soon as candidate artifacts exist, validate artifacts and Linux/Android devices
  while the five formal workflows run. Publication remains blocked until both lanes pass.
- A formal failure cancels active formal peers. Use retry only for a diagnosed
  infrastructure/flaky failure on the unchanged SHA; a source fix requires a new SHA.
- Never push evidence or wording changes to the frozen release ref between validation
  and Release. Append them after publication with [skip ci].
- Cross-platform prereleases use X.Y.Z-N with numeric N <= 65535.

## Builder and workflow safety

- scripts/release-operator.sh is the only normal entry point for builder sync,
  preflight, workflow dispatch, retry, and Release.
- Source sync preserves Cargo, Node, and Corepack caches. Dependency replacement is an
  explicit repair operation, never a normal sync step.
- Builder Cargo commands use all cores, a bounded timeout, the maintained proxy forward,
  and no competing Cargo/rustc process. Deployable optimized artifacts come only from CI.
- The formal feature matrix uses `CARGO_INCREMENTAL=0`. Low-space recovery may prune only
  `target/debug/incremental` under the builder lock; never delete dependency or registry caches.
- Existing successful or active exact-SHA runs are always reused. Failed runs fail closed
  until retry or a new source SHA is chosen.
- Core, GUI, Mobile, OHOS, and Test are dispatched together after candidate success; do
  not serialize Core first.

## Validation safety

- Host roles are fixed: .160 builds; .37/.38 cover CentOS 7 and internal networking;
  the private public pair covers 10 Gbps dual-stack/performance; the KR host is
  10.20.0.65. Never put private hostnames in the repository.
- Use exact artifacts, explicit non-production ports, bounded processes, and scoped
  cleanup. Stop immediately on storms, unbounded growth, lost connectivity, or watchdog
  failures.
- Android validation prefers ADB/CDP/API automation. Screenshots and coordinate clicks
  are final evidence only. Wireless outage tests must schedule device-side Wi-Fi recovery
  before disconnecting ADB.
- Policy traffic evidence must come from a VPN-captured application UID, not the EasyTier
  app, ADB shell, or run-as.
- Performance reports identify both endpoints, SHA, transport/path, IP family, samples,
  CPU, and resource baseline. Do not infer WAN performance from .160 or noisy mobile links.

## Domain-specific gates

- Before changing Leaf/policy behavior, inspect and cite the corresponding Mihomo source
  and tests under /Users/fanli/Documents/mihomo-rev; use sing-box where needed. Document
  intentional semantic differences and add parity tests.
- Before auditing a Cargo Git dependency, read its exact locked URL/SHA and inspect that
  revision, not a cache or default branch.
- Experimental hot-path changes require a focused Rust benchmark on .160 before production
  edits. A microbenchmark never replaces exact-artifact validation.
- Every release requires successful exact-SHA MIPS and MIPSel Core jobs and the atomic
  portability gate in easytier/docs/release/atomic_u64_portability_gate.md.
- Workflow success alone is not artifact proof. Verify checksums/build info, required
  sidecars, architecture, permissions/signatures, and one installed-artifact smoke before
  Release.
- `release.yml` owns the full formal-artifact download, extraction, compliance audit and
  release assembly on GitHub runners. Never download the complete Core/GUI/Mobile/OHOS
  matrix locally. Locally download only exact Linux/Android candidate artifacts required
  for device tests and one representative formal asset for the installed-artifact smoke.
