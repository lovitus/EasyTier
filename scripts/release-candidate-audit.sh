#!/usr/bin/env bash
set -euo pipefail

phase="${1:---source}"
case "$phase" in
  --source|--candidate|--release) ;;
  *) echo "usage: $0 [--source|--candidate|--release]" >&2; exit 2 ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
expected_version="$(
  awk '/^version = "/ { gsub(/version = |"/, ""); print; exit }' \
    "$repo_root/easytier/Cargo.toml"
)"
release_tag="v${expected_version}"
manifest="$repo_root/easytier/docs/release/${release_tag}_candidate_manifest.md"
matrix="$repo_root/easytier/docs/release/${release_tag}_validation_matrix.md"
failures=0

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}

pass() {
  printf 'PASS: %s\n' "$*"
}

cd "$repo_root"

if "$repo_root/scripts/pre-commit-check.sh"; then
  pass "repository syntax and formatting gate"
else
  fail "repository syntax and formatting gate"
fi

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
  [[ "$worktree_dir" == "$repo_root" ]] && continue
  dirty_paths="$({
    git -C "$worktree_dir" diff --name-only
    git -C "$worktree_dir" diff --cached --name-only
    git -C "$worktree_dir" ls-files --others --exclude-standard
  } | sort -u)"
  [[ -n "$dirty_paths" ]] || continue

  non_documentation_changes="$(printf '%s\n' "$dirty_paths" | grep -Ev '(^|/)docs/|\.md$' || true)"
  if [[ -n "$non_documentation_changes" ]]; then
    fail "dirty linked worktree contains non-documentation changes: $worktree_dir"
  else
    pass "linked worktree contains documentation-only WIP: $worktree_dir"
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

package_overrides="$(
  jq -r '
    .pnpm.overrides // {} |
    to_entries |
    sort_by(.key)[] |
    "\(.key)=\(.value)"
  ' package.json
)"
lock_overrides="$(
  awk '
    $0 == "overrides:" {
      in_overrides = 1
      next
    }
    in_overrides && /^[^ ]/ {
      exit
    }
    in_overrides && /^  [^ ]/ {
      line = substr($0, 3)
      separator = index(line, ": ")
      if (separator > 0) {
        key = substr(line, 1, separator - 1)
        value = substr(line, separator + 2)
        gsub(/^['\''"]|['\''"]$/, "", value)
        print key "=" value
      }
    }
  ' pnpm-lock.yaml | sort
)"
if [[ "$package_overrides" != "$lock_overrides" ]]; then
  fail "pnpm overrides differ between package.json and pnpm-lock.yaml"
else
  pass "pnpm overrides match lockfile"
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

mihomo_manifest="easytier/resources/mihomo/manifest.json"
if ! jq -e '
  .schema_version == 2 and
  .release_policy.channel == "latest-stable" and
  .release_policy.reject_draft == true and
  .release_policy.reject_prerelease == true and
  (has("version") | not) and
  (has("tag") | not) and
  (has("source_distribution") | not) and
  [.targets[] | select(.status == "supported")] as $supported |
  ($supported | length) == 9 and
  all($supported[];
    (.asset_template | contains("{version}")) and
    (has("asset") | not) and
    (has("asset_sha256") | not) and
    (.archive == "gzip" or .archive == "zip") and
    (.binary_format == "elf" or .binary_format == "macho" or .binary_format == "pe"))
' "$mihomo_manifest" >/dev/null; then
  fail "Mihomo latest-stable mapping template is incomplete or contains a fixed release"
fi
for unresolved_target in \
  loongarch64-unknown-linux-musl \
  armv7-unknown-linux-musleabihf \
  armv7-unknown-linux-musleabi \
  arm-unknown-linux-musleabihf \
  arm-unknown-linux-musleabi \
  mips-unknown-linux-musl \
  mipsel-unknown-linux-musl
do
  if [[ "$(jq -r --arg target "$unresolved_target" '.targets[$target].status' "$mihomo_manifest")" != "needs_review" ]]; then
    fail "unproven Mihomo target is not marked needs_review: $unresolved_target"
  fi
done
for packaging_file in \
  .github/workflows/core.yml \
  .github/workflows/gui.yml \
  .github/workflows/gui-macos-aarch64-test.yml \
  .github/workflows/profiling-beta.yml
do
  if ! rg -q 'fetch-mihomo-release\.sh resolve-latest' "$packaging_file"; then
    fail "latest-stable Mihomo resolution is missing from $packaging_file"
  fi
done
if ! rg -q 'easytier-mihomo' .github/workflows/profiling-beta.yml ||
   ! rg -q 'MIHOMO_RELEASE_MANIFEST\.json' .github/workflows/profiling-beta.yml ||
   ! rg -q 'MIHOMO_SHA256SUMS\.txt' .github/workflows/profiling-beta.yml ||
   ! rg -q 'MIHOMO_BUILD_INFO\.txt' .github/workflows/profiling-beta.yml; then
  fail "profiling bundle does not contain the pinned Mihomo runtime and metadata"
fi
if ! rg -q 'Mihomo version drift' .github/workflows/release.yml ||
   ! rg -q 'MIHOMO_RELEASE_MANIFEST-\$\{gui_target\}\.json' \
     .github/workflows/release.yml; then
  fail "Release does not compare exact Core and GUI Mihomo manifests"
fi
if ! rg -q 'easytier-mihomo' .github/workflows/test.yml ||
   ! rg -q 'easytier-gost' .github/workflows/test.yml; then
  fail "Test workflow does not provide the Mihomo/GOST externalBin fixtures"
fi
for tauri_config in \
  easytier-gui/src-tauri/tauri.linux.conf.json \
  easytier-gui/src-tauri/tauri.macos.conf.json \
  easytier-gui/src-tauri/tauri.windows.conf.json
do
  if ! jq -e '.bundle.externalBin | index("binaries/easytier-mihomo") != null' \
    "$tauri_config" >/dev/null
  then
    fail "$tauri_config does not bundle pinned Mihomo"
  fi
  if ! jq -e '.bundle.externalBin | index("binaries/easytier-gost") != null' \
    "$tauri_config" >/dev/null
  then
    fail "$tauri_config does not bundle pinned GOST"
  fi
  for required_resource in \
    binaries/easytier-mihomo-manifest.json \
    binaries/MIHOMO_LICENSE.txt \
    binaries/MIHOMO_SOURCE.md \
    binaries/MIHOMO_SHA256SUMS.txt \
    binaries/MIHOMO_BUILD_INFO.txt \
    binaries/easytier-gost.manifest.json \
    binaries/GOST_LICENSE.txt \
    binaries/GOST_SOURCE.md
  do
    if ! jq -e --arg resource "$required_resource" \
      '.bundle.resources | index($resource) != null' "$tauri_config" >/dev/null
    then
      fail "$tauri_config does not bundle $required_resource"
    fi
  done
done
for required_gost_file in \
  easytier/resources/gost/manifest.json \
  easytier/resources/gost/LICENSE \
  easytier/resources/gost/SOURCE.md \
  scripts/fetch-gost-release.sh
do
  test -f "$required_gost_file" || fail "missing pinned GOST input: $required_gost_file"
done
for packaging_file in \
  .github/workflows/core.yml \
  .github/workflows/gui.yml \
  .github/workflows/gui-macos-aarch64-test.yml \
  .github/workflows/release.yml
do
  if ! rg -q 'easytier-gost' "$packaging_file"; then
    fail "pinned GOST packaging is missing from $packaging_file"
  fi
done
for required_release_source in \
  fetch-source \
  'mihomo-\{tag\}-source\.tar\.gz' \
  'mihomo-\{tag\}-vendor\.tar\.gz' \
  MIHOMO_SOURCE_SHA256SUMS.txt \
  MIHOMO_UNSUPPORTED.txt
do
  if ! rg -q "$required_release_source" .github/workflows/release.yml \
    scripts/fetch-mihomo-release.sh easytier/resources/mihomo
  then
    fail "Mihomo release/source audit marker is missing: $required_release_source"
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
    easytier-gui/src-tauri/tauri.macos.conf.json \
    easytier-gui/src-tauri/tauri.windows.conf.json
  do
    if ! jq -e --arg sidecar "binaries/$sidecar" \
      '.bundle.externalBin | index($sidecar) != null' "$tauri_config" >/dev/null
    then
      fail "$tauri_config does not bundle $sidecar"
    fi
  done
done
for packaging_file in \
  .github/workflows/core.yml \
  .github/workflows/gui.yml \
  .github/workflows/release.yml
do
  if ! rg -q 'SOCKS_EGRESS_BUILD_INFO\.txt' "$packaging_file"; then
    fail "SOCKS egress backend metadata is missing from $packaging_file"
  fi
done
if ! rg -q 'backend="leaf-portable"' .github/workflows/core.yml ||
   ! rg -q 'backend="leaf-portable"' .github/workflows/gui.yml; then
  fail "Windows portable SOCKS backend is not explicitly identified"
fi
if ! rg -q 'easytier-hev-socks-egress\.exe' .github/workflows/gui.yml ||
   ! rg -q 'socks-egress machine mismatch' .github/workflows/release.yml; then
  fail "Windows portable SOCKS egress target or architecture verification is incomplete"
fi
if ! rg -q 'easytier-gost-guardian' .github/workflows/core.yml ||
   ! rg -q 'easytier-gost-guardian' .github/workflows/gui.yml ||
   ! rg -q 'easytier-gost-guardian' .github/workflows/gui-macos-aarch64-test.yml ||
   ! rg -q 'easytier-gost-guardian' .github/workflows/release.yml ||
   ! jq -e '.bundle.externalBin | index("binaries/easytier-gost-guardian") != null' \
     easytier-gui/src-tauri/tauri.macos.conf.json >/dev/null
then
  fail "macOS/FreeBSD GOST parent guardian is not packaged consistently"
fi
if ! rg -q 'Sign and verify macOS release executables' .github/workflows/core.yml ||
   ! rg -q 'Verify bundled policy sidecars' .github/workflows/gui.yml ||
   ! rg -q 'Verify signed policy sidecars' .github/workflows/gui-macos-aarch64-test.yml; then
  fail "macOS SOCKS egress signature verification is not enforced"
fi
for tauri_config in \
  easytier-gui/src-tauri/tauri.linux.conf.json \
  easytier-gui/src-tauri/tauri.macos.conf.json \
  easytier-gui/src-tauri/tauri.windows.conf.json
do
  if ! jq -e '.bundle.resources | index("binaries/SOCKS_EGRESS_BUILD_INFO.txt") != null' \
    "$tauri_config" >/dev/null
  then
    fail "$tauri_config does not bundle SOCKS egress backend metadata"
  fi
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
  if [[ "${EXACT_ARTIFACT_VALIDATED_SHA:-}" != "$candidate_sha" ]]; then
    fail "exact-artifact validation is not attested for $candidate_sha"
  else
    pass "exact-artifact validation attestation matches $candidate_sha"
  fi
  if rg -q '\| FAIL \|' "$matrix"; then
    fail "pre-build validation matrix contains an explicit FAIL"
  fi
  if git show-ref --verify --quiet "refs/tags/$release_tag"; then
    fail "$release_tag tag already exists"
  fi
  if [[ -n "$(git ls-remote --tags origin "refs/tags/$release_tag" "refs/tags/$release_tag^{}" 2>/dev/null)" ]]; then
    fail "origin already contains $release_tag"
  fi
fi

if ((failures > 0)); then
  printf '%d release-candidate audit failure(s)\n' "$failures" >&2
  exit 1
fi
printf 'release-candidate audit passed for %s\n' "$phase"
