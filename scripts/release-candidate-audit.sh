#!/usr/bin/env bash
set -euo pipefail

phase="${1:---source}"
case "$phase" in
  --source|--candidate|--release) ;;
  *) echo "usage: $0 [--source|--candidate|--release]" >&2; exit 2 ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$repo_root/easytier/docs/release/v3.0.5_candidate_manifest.md"
matrix="$repo_root/easytier/docs/release/v3.0.5_validation_matrix.md"
failures=0

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}

pass() {
  printf 'PASS: %s\n' "$*"
}

cd "$repo_root"

evidence_sha="$(git rev-parse HEAD)"
validated_sha="${VALIDATED_SHA:-$evidence_sha}"
if ! git rev-parse "$validated_sha^{commit}" >/dev/null 2>&1; then
  fail "validated SHA cannot be resolved: $validated_sha"
else
  validated_sha="$(git rev-parse "$validated_sha^{commit}")"
fi

if [[ "$phase" == "--candidate" && "$validated_sha" != "$evidence_sha" ]]; then
  fail "candidate audit requires HEAD to be the build candidate; VALIDATED_SHA is only for post-build release evidence"
fi

if [[ "$validated_sha" != "$evidence_sha" ]]; then
  if ! git merge-base --is-ancestor "$validated_sha" "$evidence_sha"; then
    fail "evidence HEAD $evidence_sha does not descend from validated SHA $validated_sha"
  fi
  non_documentation_changes="$(
    while IFS= read -r file_name; do
      case "$file_name" in
        *.md|AGENTS.md|*/AGENTS.md|docs/*|*/docs/*) ;;
        *) printf '%s\n' "$file_name" ;;
      esac
    done < <(git diff --name-only "$validated_sha..$evidence_sha")
  )"
  if [[ -n "$non_documentation_changes" ]]; then
    fail "post-build evidence contains non-documentation changes: $non_documentation_changes"
  else
    pass "documentation evidence HEAD $evidence_sha reuses validated build SHA $validated_sha"
  fi
fi

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
  remote_sha="$(git ls-remote --heads origin "refs/heads/$archive_ref" | awk '{print $1}')"
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
for workflow_file in \
  .github/workflows/android-policy-candidate.yml \
  .github/workflows/core.yml \
  .github/workflows/gui.yml \
  .github/workflows/gui-macos-aarch64-test.yml \
  .github/workflows/mobile.yml \
  .github/workflows/profiling-beta.yml
do
  if ! rg -q "HEV_SERVER_COMMIT:?[= ]+$expected_hev" "$workflow_file"; then
    fail "HEV pin missing from $workflow_file"
  fi
done

if ! rg -q 'FEATURES="\$FEATURES,leaf-policy-proxy"' .github/workflows/core.yml; then
  fail "formal Core workflow does not enable leaf-policy-proxy for policy targets"
fi
for sidecar in easytier-leaf-worker easytier-hev-socks-egress; do
  if ! rg -q "$sidecar" .github/workflows/core.yml; then
    fail "formal Core workflow does not package $sidecar"
  fi
  if ! rg -q "$sidecar" .github/workflows/gui.yml; then
    fail "formal GUI workflow does not package $sidecar"
  fi
  for tauri_config in \
    easytier-gui/src-tauri/tauri.linux.conf.json \
    easytier-gui/src-tauri/tauri.macos.conf.json
  do
    if ! jq -e --arg sidecar "binaries/$sidecar" \
      '.bundle.externalBin | index($sidecar) != null' "$tauri_config" >/dev/null
    then
      fail "$tauri_config does not bundle $sidecar"
    fi
  done
done
if ! rg -q "cfg\(any\(target_os = \"linux\", target_os = \"macos\"\)\)" \
  easytier-gui/src-tauri/Cargo.toml
then
  fail "GUI does not enable the shared Linux/macOS policy dependency boundary"
fi

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
if [[ "$phase" != "--source" ]]; then expected_version=3.0.5; fi
for version in "${versions[@]}"; do
  if [[ "$version" != "$expected_version" ]]; then
    fail "version $version differs from expected $expected_version for $phase"
  fi
done

if [[ "$phase" != "--source" ]]; then
  release_base="${RELEASE_BASE:-v3.0.0}"
  current_sha="$validated_sha"
  current_tree="$(git rev-parse "$validated_sha^{tree}")"
  tracked_files="$(git ls-tree -r --name-only "$validated_sha" | wc -l | tr -d ' ')"
  if ! git rev-parse "$release_base^{commit}" >/dev/null 2>&1; then
    fail "release base cannot be resolved: $release_base"
  elif ! git merge-base --is-ancestor "$release_base" "$validated_sha"; then
    fail "release base $release_base is not an ancestor of $current_sha"
  else
    pass "candidate $current_sha descends from $release_base with tree $current_tree ($tracked_files tracked files)"
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

if [[ "$phase" != "--source" ]]; then
  candidate_sha="$validated_sha"
  if [[ "$phase" == "--candidate" ]]; then
    published_sha="$(git ls-remote --heads origin refs/heads/codex/profiling-beta | awk '{print $1}')"
    if [[ "$published_sha" != "$candidate_sha" ]]; then
      fail "origin/codex/profiling-beta $published_sha differs from candidate $candidate_sha"
    else
      pass "candidate SHA is the published profiling-beta SHA"
    fi
  fi
  for workflow_name in "EasyTier Linux Profiling Beta" "EasyTier Android Policy Candidate"; do
    workflow_success "$workflow_name" "$candidate_sha" || fail "$workflow_name is not successful for $candidate_sha"
  done
fi

if [[ "$phase" == "--release" ]]; then
  candidate_sha="$validated_sha"
  for workflow_name in "EasyTier Core" "EasyTier GUI" "EasyTier Mobile" "EasyTier OHOS" "EasyTier Test"; do
    workflow_success "$workflow_name" "$candidate_sha" || fail "$workflow_name is not successful for $candidate_sha"
  done
  if ! rg -q '\| Android physical device \| (PASS|WAIVED_BY_MAINTAINER) \|' "$matrix"; then
    fail "Android physical gate is neither PASS nor explicitly waived"
  fi
  if rg -q '\| FAIL \||\| N/A \|' "$matrix"; then
    fail "validation matrix contains FAIL or N/A"
  fi
  unresolved_external_gates="$(
    awk -F'|' '
      $3 ~ /^[[:space:]]*BLOCKED[[:space:]]*$/ {
        gate=$2
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", gate)
        if (gate !~ /^(Core formal workflow|GUI formal workflow|Mobile formal workflow|OHOS formal workflow|Test formal workflow|Tag and GitHub Release)$/) print gate
      }
    ' "$matrix"
  )"
  if [[ -n "$unresolved_external_gates" ]]; then
    fail "validation matrix contains unresolved external gates: $unresolved_external_gates"
  fi
  if git show-ref --verify --quiet refs/tags/v3.0.5; then
    fail "v3.0.5 tag already exists"
  fi
  if [[ -n "$(git ls-remote --tags origin refs/tags/v3.0.5 refs/tags/v3.0.5^{} 2>/dev/null)" ]]; then
    fail "origin already contains v3.0.5"
  fi
fi

if ((failures > 0)); then
  printf '%d release-candidate audit failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'release-candidate audit passed for %s\n' "$phase"
