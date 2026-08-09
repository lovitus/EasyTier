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
the destructive but bounded repair path is always explicit.

## Formal workflows

All commands below operate on the current pushed SHA. They reject dirty,
detached, unpushed or SHA-mismatched worktrees and refuse duplicate dispatches.

```bash
scripts/release-operator.sh status
scripts/release-operator.sh dispatch-core
scripts/release-operator.sh dispatch-rest
scripts/release-operator.sh dispatch-release v3.0.15
```

`dispatch-rest` requires successful Core MIPS and MIPSel jobs before it starts
GUI, Mobile, OHOS and Test. `dispatch-release` requires all five formal workflows
and the existing release-candidate audit. Artifact inspection and real-device
approval remain required gates; this wrapper does not weaken or replace them.

If a workflow already exists for the exact SHA, rerun that run explicitly rather
than dispatching a duplicate. Documentation-only evidence updates use `[skip ci]`
and never create a new artifact candidate.
