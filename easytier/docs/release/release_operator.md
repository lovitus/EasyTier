# Release operator

This is the short operational entry point for remote preflight and formal release
workflow dispatch. The scripts enforce the detailed gates in `AGENTS.md`; do not
reconstruct `rsync`, dependency cleanup, or `gh workflow run` commands manually.

## Remote builder

```bash
# Source-only synchronization. Cargo target, registry, Corepack and node_modules
# are protected. The command refuses to sync while cargo/rustc is active.
scripts/release-operator.sh builder-sync

# Incremental frozen-lockfile dependency preparation for frontend work.
scripts/release-operator.sh builder-frontend

# Explicit recovery only after an actual dependency failure. This removes only
# the documented workspace node_modules directories, never Cargo artifacts.
scripts/release-operator.sh builder-frontend-repair

# Standard Leaf/policy Rust preflight, including sync, no-run and focused tests.
scripts/leaf-remote-preflight.sh

# Complete standard gate: Rust preflight followed by the ordered frontend gate.
scripts/release-operator.sh builder-preflight
```

Source synchronization uses `rsync --delete-delay`: stale source is removed only
after transfer succeeds. It never treats a lockfile change as permission to erase
dependency directories. `builder-frontend` updates dependencies incrementally;
the destructive but bounded repair path is always explicit. Corepack uses a
lockfile-hash-specific home, so a stale tool cache cannot poison a new candidate.

## Candidate-first release pipeline

All commands below operate on the current pushed SHA. They reject dirty,
detached, unpushed or SHA-mismatched worktrees and refuse duplicate dispatches.

```bash
scripts/release-operator.sh status
scripts/release-operator.sh init-validation-matrix  # before the candidate commit
scripts/release-operator.sh dispatch-pipeline
scripts/release-operator.sh dispatch-release v3.0.15-1
```

`dispatch-pipeline` first starts Linux Profiling Beta and Android Policy Candidate
in parallel. Only after both succeed does it start Core, GUI, Mobile, OHOS and Test
in parallel. Each group cancels its remaining queued or running workflows on the
first failure. Existing successful or active exact-SHA runs are reused, so an
interrupted operator can safely run the same command again without duplicating
work. `dispatch-all` is retained as an alias for this complete pipeline; it no
longer bypasses candidate validation.

Linux Profiling Beta is the mandatory optimized/debug artifact and automatic
validation gate after the complete code batch and `.160` preflight. Android Policy
Candidate runs in parallel by default. It may be omitted only when Android evidence
is outside the candidate scope, using
`ANDROID_CANDIDATE_MODE=skip scripts/release-operator.sh dispatch-pipeline`; the
validation matrix must record the reason as `N/A` or an explicit maintainer waiver.
This option never skips Linux Beta and never permits formal workflows before the
selected candidate gates pass.

The validation matrix and candidate manifest must exist before dispatch. Create
the matrix before the candidate commit, then fill its evidence without moving the
frozen release ref. GitHub API reads, dispatches and cancellation use bounded
retries for transient TLS/API failures. A persistent failure remains fail-closed.

`dispatch-release` requires all five formal workflows and the existing
release-candidate audit. Artifact inspection and real-device approval remain
required gates; this wrapper does not weaken or replace them. A SemVer
prerelease is automatically published with `prerelease=true` and
`make_latest=false`; a stable version remains the default latest release.
Cross-platform prereleases use one numeric identifier such as `3.0.15-1` because
the Windows MSI bundler rejects textual or dotted prerelease identifiers.

An unsuccessful workflow for the exact SHA is never silently replaced by a new
run. Diagnose and rerun that run explicitly only when the source tree is unchanged,
or create a new SHA for a source fix. Documentation-only evidence updates use
`[skip ci]` and never create a new artifact candidate.
