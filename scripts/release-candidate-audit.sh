#!/usr/bin/env bash
set -euo pipefail

phase="${1:---source}"
case "$phase" in
  --source|--candidate|--release) ;;
  *) echo "usage: $0 [--source|--candidate|--release]" >&2; exit 2 ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ledger="$repo_root/easytier/docs/release/v3.0.0_recovery_ledger.md"
manifest="$repo_root/easytier/docs/release/v3.0.0_candidate_manifest.md"
matrix="$repo_root/easytier/docs/release/v3.0.0_validation_matrix.md"
failures=0

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}

pass() {
  printf 'PASS: %s\n' "$*"
}

cd "$repo_root"

if [[ -n "$(git status --porcelain)" ]]; then
  fail "current candidate worktree is dirty"
else
  pass "current candidate worktree is clean"
fi

while IFS= read -r worktree_dir; do
  if [[ -n "$(git -C "$worktree_dir" status --porcelain)" ]]; then
    fail "dirty linked worktree: $worktree_dir"
  fi
done < <(git worktree list --porcelain | sed -n 's/^worktree //p')

archive_refs=(
  codex/archive-invalid-v3-defaa442
  codex/archive-gui-geo-20260720
  codex/archive-pollsender-089d-20260720
  codex/archive-agents-rejected-20260720
  codex/archive-stash-merge-20260719
  codex/archive-stash-index-20260719
  codex/archive-stash-untracked-20260719
  codex/archive-stash-combined-20260719
)
for archive_ref in "${archive_refs[@]}"; do
  local_sha="$(git rev-parse "$archive_ref" 2>/dev/null || true)"
  remote_sha="$(git rev-parse "refs/remotes/origin/$archive_ref" 2>/dev/null || true)"
  if [[ -z "$local_sha" || "$local_sha" != "$remote_sha" ]]; then
    fail "archive ref is absent or differs from origin: $archive_ref"
  fi
done

if [[ "$(git rev-parse stash@{0})" != "$(git rev-parse codex/archive-stash-merge-20260719)" ]]; then
  fail "stash merge commit is not the archived merge ref"
else
  pass "three-parent stash merge is archived"
fi

locked_leaf="$(sed -n 's#.*lovitus/leaf.git?rev=\([0-9a-f]\{40\}\).*#\1#p' Cargo.lock | head -1)"
expected_leaf="$(sed -n 's/^- Leaf SHA: `\([^`]*\)`.*/\1/p' "$manifest")"
if [[ "$locked_leaf" != "$expected_leaf" ]]; then
  fail "Leaf lock $locked_leaf differs from manifest $expected_leaf"
else
  pass "Leaf lock matches manifest"
fi

expected_hev="$(sed -n 's/^- HEV SHA: `\([^`]*\)`.*/\1/p' "$manifest")"
for workflow_file in .github/workflows/profiling-beta.yml .github/workflows/mobile.yml; do
  if ! rg -q "HEV_SERVER_COMMIT:?[= ]+$expected_hev" "$workflow_file"; then
    fail "HEV pin missing from $workflow_file"
  fi
done

read_cargo_version() {
  awk '/^version = "/ { gsub(/version = |"/, ""); print; exit }' "$1"
}

versions=(
  "$(read_cargo_version easytier/Cargo.toml)"
  "$(read_cargo_version easytier-web/Cargo.toml)"
  "$(read_cargo_version easytier-gui/src-tauri/Cargo.toml)"
  "$(jq -r .version easytier-gui/package.json)"
  "$(jq -r .version easytier-gui/src-tauri/tauri.conf.json)"
)
expected_version=2.6.10
if [[ "$phase" != "--source" ]]; then expected_version=3.0.0; fi
for version in "${versions[@]}"; do
  if [[ "$version" != "$expected_version" ]]; then
    fail "version $version differs from expected $expected_version for $phase"
  fi
done

if [[ "$phase" != "--source" ]] && rg -q '\| NEEDS_REVIEW \|' "$ledger"; then
  fail "recovery ledger still contains NEEDS_REVIEW"
fi

if [[ "$phase" != "--source" ]]; then
  recovery_ref="${RECOVERY_REF:-codex/v3.0.0-recovery}"
  current_sha="$(git rev-parse HEAD)"
  current_tree="$(git rev-parse HEAD^{tree})"
  recovery_tree="$(git rev-parse "$recovery_ref^{tree}" 2>/dev/null || true)"
  if [[ -z "$recovery_tree" ]]; then
    fail "recovery reference cannot be resolved: $recovery_ref"
  elif [[ "$recovery_tree" != "$current_tree" ]]; then
    fail "current tree $current_tree differs from recovery tree $recovery_tree"
  else
    pass "candidate $current_sha has recovery-matching tree $current_tree"
  fi
fi

workflow_success() {
  workflow_name=$1
  candidate_sha=$2
  conclusion="$(gh run list -R lovitus/EasyTier --commit "$candidate_sha" --limit 30 \
    --json name,status,conclusion \
    --jq ".[] | select(.name == \"$workflow_name\" and .status == \"completed\") | .conclusion" \
    | head -1)"
  [[ "$conclusion" == success ]]
}

if [[ "$phase" == "--release" ]]; then
  candidate_sha="$(git rev-parse HEAD)"
  for workflow_name in "EasyTier Core" "EasyTier GUI" "EasyTier Mobile" "EasyTier OHOS" "EasyTier Test"; do
    workflow_success "$workflow_name" "$candidate_sha" || fail "$workflow_name is not successful for $candidate_sha"
  done
  if ! rg -q '\| Android physical device \| (PASS|WAIVED_BY_MAINTAINER) \|' "$matrix"; then
    fail "Android physical gate is neither PASS nor explicitly waived"
  fi
  if git show-ref --verify --quiet refs/tags/v3.0.0; then
    fail "v3.0.0 tag already exists"
  fi
fi

if ((failures > 0)); then
  printf '%d release-candidate audit failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'release-candidate audit passed for %s\n' "$phase"
