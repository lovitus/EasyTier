#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/leaf-candidate-artifacts.sh COMMIT_SHA [DESTINATION]

Download and verify the exact Linux profiling release and Android policy
candidate produced for COMMIT_SHA. This script does not build or deploy.

Environment:
  GH_REPO  GitHub repository (default: lovitus/EasyTier)
EOF
}

if [[ ${1:-} == "-h" || ${1:-} == "--help" ]]; then
  usage
  exit 0
fi

sha=${1:-}
if [[ ! $sha =~ ^[0-9a-f]{40}$ ]]; then
  usage >&2
  exit 2
fi

repo=${GH_REPO:-lovitus/EasyTier}
destination=${2:-.artifacts/leaf-candidates/$sha}
branch=codex/profiling-beta

for command_name in gh jq tar; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'required command not found: %s\n' "$command_name" >&2
    exit 1
  fi
done
if ! command -v sha256sum >/dev/null 2>&1 \
  && ! command -v shasum >/dev/null 2>&1; then
  echo 'required command not found: sha256sum or shasum' >&2
  exit 1
fi

if [[ -e $destination ]]; then
  printf 'destination already exists; refusing to mix candidates: %s\n' \
    "$destination" >&2
  exit 1
fi

parent=$(dirname "$destination")
mkdir -p "$parent"
stage="${destination}.partial.$$"
rm -rf "$stage"
mkdir -p "$stage/linux" "$stage/android"
cleanup() {
  if [[ -n ${stage:-} && -d $stage ]]; then
    rm -rf "$stage"
  fi
}
trap cleanup EXIT

find_successful_run() {
  local workflow=$1
  local result count
  result=$(
    gh run list \
      --repo "$repo" \
      --workflow "$workflow" \
      --branch "$branch" \
      --commit "$sha" \
      --limit 20 \
      --json databaseId,headSha,status,conclusion,url \
      | jq --arg sha "$sha" \
        '[.[] | select(.headSha == $sha and .status == "completed" and .conclusion == "success")]'
  )
  count=$(jq 'length' <<<"$result")
  if [[ $count -ne 1 ]]; then
    printf '%s has %s successful runs for %s; expected exactly one\n' \
      "$workflow" "$count" "$sha" >&2
    return 1
  fi
  jq -r '.[0] | [.databaseId, .url] | @tsv' <<<"$result"
}

verify_checksums() {
  local directory=$1
  (
    cd "$directory"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c SHA256SUMS.txt
    else
      shasum -a 256 -c SHA256SUMS.txt
    fi
  )
}

IFS=$'\t' read -r linux_run_id linux_run_url < <(
  find_successful_run profiling-beta.yml
)
IFS=$'\t' read -r android_run_id android_run_url < <(
  find_successful_run android-policy-candidate.yml
)

tag_sha_before=$(gh api "repos/$repo/git/ref/tags/profiling-beta" --jq '.object.sha')
if [[ $tag_sha_before != "$sha" ]]; then
  printf 'profiling-beta tag points to %s, not requested %s\n' \
    "$tag_sha_before" "$sha" >&2
  exit 1
fi

gh release download profiling-beta \
  --repo "$repo" \
  --dir "$stage/linux" \
  --pattern 'easytier-profiling-beta-linux-x86_64-musl.tar.gz' \
  --pattern 'SHA256SUMS.txt' \
  --pattern 'RELEASE_NOTES.txt'
verify_checksums "$stage/linux"

linux_bundle=easytier-profiling-beta-linux-x86_64-musl
mkdir -p "$stage/linux/extracted"
tar -xzf "$stage/linux/$linux_bundle.tar.gz" -C "$stage/linux/extracted"
linux_info="$stage/linux/extracted/$linux_bundle/BUILD_INFO.txt"
grep -Fxq "commit=$sha" "$linux_info"
grep -Fxq "run_id=$linux_run_id" "$linux_info"
grep -Fxq 'target=x86_64-unknown-linux-musl' "$linux_info"
verify_checksums "$stage/linux/extracted/$linux_bundle"

tag_sha_after=$(gh api "repos/$repo/git/ref/tags/profiling-beta" --jq '.object.sha')
if [[ $tag_sha_after != "$tag_sha_before" ]]; then
  echo 'profiling-beta tag changed while downloading; refusing mixed evidence' >&2
  exit 1
fi

android_artifact="easytier-android-policy-candidate-aarch64-$sha"
gh run download "$android_run_id" \
  --repo "$repo" \
  --name "$android_artifact" \
  --dir "$stage/android"
verify_checksums "$stage/android"
android_info="$stage/android/BUILD_INFO.txt"
grep -Fxq "commit_sha=$sha" "$android_info"
grep -Fxq "workflow_run_id=$android_run_id" "$android_info"
grep -Fxq 'target=aarch64-linux-android' "$android_info"

linux_hev_commit=$(sed -n 's/^hev_server_commit=//p' "$linux_info")
android_hev_commit=$(sed -n 's/^hev_server_commit=//p' "$android_info")
if [[ -z $linux_hev_commit || $linux_hev_commit != "$android_hev_commit" ]]; then
  printf 'HEV pin mismatch: linux=%s android=%s\n' \
    "$linux_hev_commit" "$android_hev_commit" >&2
  exit 1
fi

cat > "$stage/VALIDATION_INFO.txt" <<EOF
commit_sha=$sha
repository=$repo
branch=$branch
linux_run_id=$linux_run_id
linux_run_url=$linux_run_url
android_run_id=$android_run_id
android_run_url=$android_run_url
hev_server_commit=$linux_hev_commit
retrieved_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF

mv "$stage" "$destination"
stage=
printf 'verified candidate artifacts: %s\n' "$destination"
