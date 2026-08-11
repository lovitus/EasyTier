#!/usr/bin/env bash
set -euo pipefail

if [[ -d "${HOME}/.cargo/bin" ]]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
GH_REPO="${GH_REPO:-lovitus/EasyTier}"
CANDIDATE_WORKFLOWS=(profiling-beta.yml android-policy-candidate.yml)
ACTIVE_CANDIDATE_WORKFLOWS=("${CANDIDATE_WORKFLOWS[@]}")
FORMAL_WORKFLOWS=(core.yml gui.yml mobile.yml ohos.yml test.yml)

usage() {
  cat <<'EOF'
Usage: scripts/release-operator.sh COMMAND [ARGUMENTS]

Normal commands:
  version                      Print the current release version.
  init-release                Create compact manifest, matrix, and release-note stubs.
  prepare                     Run local gates and the required remote preflight scopes.
  validate [SHA]              Run candidates, then all five formal workflows.
  retry [SHA]                 Retry failed/canceled exact-SHA validation runs.
  status [SHA]                Show candidate and formal workflow state.
  publish VERSION [SHA]       Audit and dispatch EasyTier Release.
  help-advanced               Show recovery and low-level commands.

The normal successful path is init-release -> prepare -> commit/push -> validate ->
artifact/device approval -> publish. Candidates finish before formal workflows start;
artifact/device validation then overlaps the formal matrix.

Commands reject dirty, detached, unpushed, SHA-mismatched, incomplete, or already
published inputs. Existing successful/active exact-SHA runs are reused. A failed run
requires retry for a diagnosed unchanged-SHA infrastructure/flaky failure, or a new SHA.

Linux Profiling Beta is always required. Android is required by default; set
ANDROID_CANDIDATE_MODE=skip only when the matrix records N/A or maintainer waiver.
EOF
}

advanced_usage() {
  cat <<'EOF'
Advanced commands:
  builder-sync | builder-frontend | builder-frontend-repair | builder-preflight
  init-validation-matrix
  dispatch-candidates [SHA] | dispatch-formal [SHA] | monitor [SHA]
  dispatch-pipeline [SHA] | dispatch-all [SHA]
  dispatch-release VERSION [SHA]
EOF
}

die() {
  printf 'release operator: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

gh_retry() {
  local attempts="${GH_RETRY_ATTEMPTS:-5}"
  local delay="${GH_RETRY_DELAY_SECONDS:-5}"
  local attempt output

  for ((attempt = 1; attempt <= attempts; attempt++)); do
    if output="$(gh "$@" 2>&1)"; then
      printf '%s' "$output"
      return 0
    fi
    if ((attempt == attempts)); then
      printf 'release operator: GitHub command failed after %s attempts: gh' "$attempts" >&2
      printf ' %q' "$@" >&2
      printf '\n%s\n' "$output" >&2
      return 1
    fi
    printf 'release operator: transient GitHub failure (%s/%s), retrying in %ss: %s\n' \
      "$attempt" "$attempts" "$delay" "$output" >&2
    sleep "$delay"
  done
}

head_sha() {
  git -C "$REPO_ROOT" rev-parse HEAD
}

current_branch() {
  git -C "$REPO_ROOT" symbolic-ref --quiet --short HEAD || die "detached HEAD is not releasable"
}

cargo_version() {
  awk '
    /^\[package\]$/ { in_package = 1; package_is_easytier = 0; next }
    /^\[/ { in_package = 0; package_is_easytier = 0 }
    in_package && /^name[[:space:]]*=[[:space:]]*\"easytier\"/ { package_is_easytier = 1 }
    in_package && package_is_easytier && /^version[[:space:]]*=/ {
      gsub(/^[^\"]*\"|\".*$/, "")
      print
      exit
    }
  ' "$REPO_ROOT/easytier/Cargo.toml"
}

validate_cross_platform_version() {
  local version="$1" prerelease
  if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    return 0
  fi
  if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-([0-9]+)$ ]]; then
    prerelease="${BASH_REMATCH[1]}"
    [[ "$prerelease" == "0" || "$prerelease" != 0* ]] || return 1
    ((10#$prerelease <= 65535))
    return
  fi
  return 1
}

configure_candidate_mode() {
  case "${ANDROID_CANDIDATE_MODE:-required}" in
    required) ACTIVE_CANDIDATE_WORKFLOWS=("${CANDIDATE_WORKFLOWS[@]}") ;;
    skip) ACTIVE_CANDIDATE_WORKFLOWS=(profiling-beta.yml) ;;
    *) die "ANDROID_CANDIDATE_MODE must be required or skip" ;;
  esac
}

release_input_path() {
  local kind="$1" version="$2"
  printf '%s/easytier/docs/release/v%s_%s.md' "$REPO_ROOT" "$version" "$kind"
}

require_release_inputs() {
  local version manifest matrix release_notes
  version="$(cargo_version)"
  [[ -n "$version" ]] || die "cannot read workspace package version"
  validate_cross_platform_version "$version" ||
    die "Cargo version $version is not cross-platform safe; use x.y.z or x.y.z-N with N <= 65535"
  manifest="$(release_input_path candidate_manifest "$version")"
  matrix="$(release_input_path validation_matrix "$version")"
  release_notes="$REPO_ROOT/easytier/docs/release_notes/v${version}.md"
  [[ -f "$manifest" ]] || die "missing pre-build candidate manifest: $manifest"
  [[ -f "$matrix" ]] || die "missing pre-build validation matrix: $matrix; run init-validation-matrix before committing"
  [[ -f "$release_notes" ]] || die "missing pre-build user-facing release notes: $release_notes"
}

require_unpublished_version() {
  local version remote_tags
  version="$(cargo_version)"
  remote_tags="$(git -C "$REPO_ROOT" ls-remote --tags origin \
    "refs/tags/v${version}" "refs/tags/v${version}^{}")" ||
    die "failed to verify whether v${version} already exists on origin"
  [[ -z "$remote_tags" ]] ||
    die "v${version} already exists on origin; freeze a new version before candidate validation"
}

locked_leaf_sha() {
  sed -n 's#.*lovitus/leaf.git?rev=\([0-9a-f]\{40\}\).*#\1#p' "$REPO_ROOT/Cargo.lock" | head -1
}

pinned_hev_sha() {
  sed -n 's/^[[:space:]]*HEV_SERVER_COMMIT:[[:space:]]*\([0-9a-f]\{40\}\).*/\1/p' \
    "$REPO_ROOT/.github/workflows/profiling-beta.yml" | head -1
}

init_release_inputs() {
  local version manifest matrix release_notes leaf_sha hev_sha
  version="$(cargo_version)"
  validate_cross_platform_version "$version" ||
    die "Cargo version $version is not cross-platform safe"
  require_unpublished_version
  manifest="$(release_input_path candidate_manifest "$version")"
  matrix="$(release_input_path validation_matrix "$version")"
  release_notes="$REPO_ROOT/easytier/docs/release_notes/v${version}.md"
  leaf_sha="$(locked_leaf_sha)"
  hev_sha="$(pinned_hev_sha)"
  [[ -n "$leaf_sha" && -n "$hev_sha" ]] || die "cannot resolve locked Leaf/HEV pins"

  if [[ ! -e "$manifest" ]]; then
    cat >"$manifest" <<EOF
# v${version} Candidate Manifest

- Status: DRAFT
- Candidate SHA: pending immutable commit
- Scope: TODO
- Leaf SHA: \`$leaf_sha\`
- HEV SHA: \`$hev_sha\`
- Android candidate: required
- Preflight: \`scripts/release-operator.sh prepare\`
- Validation: \`scripts/release-operator.sh validate\`

Post-build run IDs and measurements belong in the validation matrix after publication.
EOF
    printf 'created %s\n' "$manifest"
  else
    printf 'kept existing %s\n' "$manifest"
  fi

  if [[ ! -e "$matrix" ]]; then
    init_validation_matrix
  else
    printf 'kept existing %s\n' "$matrix"
  fi

  if [[ ! -e "$release_notes" ]]; then
    cat >"$release_notes" <<EOF
# v${version}

## Changes

- TODO: describe user-visible changes.

## Compatibility

- TODO: describe compatibility boundaries and known limitations.
EOF
    printf 'created %s\n' "$release_notes"
  else
    printf 'kept existing %s\n' "$release_notes"
  fi
}

require_exact_artifact_attestation() {
  local sha="$1"
  [[ "${EXACT_ARTIFACT_VALIDATED_SHA:-}" == "$sha" ]] ||
    die "exact Linux/applicable Android artifacts must be validated first; rerun with EXACT_ARTIFACT_VALIDATED_SHA=$sha"
}

assert_clean_pushed_sha() {
  local expected_sha="$1"
  local branch remote_sha

  [[ "$(head_sha)" == "$expected_sha" ]] || die "requested SHA is not the current HEAD"
  git -C "$REPO_ROOT" diff --quiet || die "tracked worktree changes are present"
  git -C "$REPO_ROOT" diff --cached --quiet || die "staged changes are present"
  [[ -z "$(git -C "$REPO_ROOT" ls-files --others --exclude-standard)" ]] || die "untracked files are present"

  branch="$(current_branch)"
  remote_sha="$(git -C "$REPO_ROOT" ls-remote --heads origin "refs/heads/$branch" | awk 'NR == 1 {print $1}')"
  [[ "$remote_sha" == "$expected_sha" ]] || die "origin/$branch does not equal $expected_sha"
  require_release_inputs
  require_unpublished_version
}

changed_candidate_files() {
  local base_ref="${PREFLIGHT_BASE:-origin/codex/current}"
  git -C "$REPO_ROOT" rev-parse "$base_ref^{commit}" >/dev/null 2>&1 ||
    die "cannot resolve preflight base $base_ref"
  {
    git -C "$REPO_ROOT" diff --name-only "$base_ref"...HEAD
    git -C "$REPO_ROOT" diff --name-only
    git -C "$REPO_ROOT" diff --cached --name-only
  } | sort -u
}

automatic_preflight_scope() {
  local changed file_name
  local rust=false frontend=false unknown=false
  changed="$(changed_candidate_files)"
  [[ -n "$changed" ]] || { printf 'full\n'; return; }

  while IFS= read -r file_name; do
    case "$file_name" in
      AGENTS.md|*.md|*/docs/*|.github/*|scripts/*)
        ;;
      Cargo.toml|Cargo.lock|.cargo/*|rust-toolchain*|third_party/*)
        rust=true
        frontend=true
        ;;
      easytier-web/*|easytier-gui/*|tauri-plugin-vpnservice/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml)
        frontend=true
        case "$file_name" in
          *.rs|*/Cargo.toml|*/build.rs) rust=true ;;
        esac
        ;;
      *.rs|*/Cargo.toml|*/build.rs|easytier/*|easytier-contrib/*)
        rust=true
        ;;
      *)
        unknown=true
        ;;
    esac
  done <<<"$changed"

  if [[ "$unknown" == true || ( "$rust" == true && "$frontend" == true ) ]]; then
    printf 'full\n'
  elif [[ "$rust" == true ]]; then
    printf 'rust\n'
  elif [[ "$frontend" == true ]]; then
    printf 'frontend\n'
  else
    printf 'none\n'
  fi
}

prepare_release() {
  local scope="${PREFLIGHT_SCOPE:-auto}"
  [[ "$scope" != none ]] || die "PREFLIGHT_SCOPE=none is not an allowed manual override"
  require_release_inputs
  require_unpublished_version
  "$SCRIPT_DIR/pre-commit-check.sh"
  if [[ "$scope" == auto ]]; then
    scope="$(automatic_preflight_scope)"
  fi
  printf 'release preflight scope: %s\n' "$scope"
  case "$scope" in
    none) printf 'remote preflight skipped: candidate changes are documentation/release tooling only\n' ;;
    rust) "$SCRIPT_DIR/leaf-remote-preflight.sh" ;;
    frontend) "$SCRIPT_DIR/remote-frontend-preflight.sh" ;;
    full)
      "$SCRIPT_DIR/leaf-remote-preflight.sh"
      "$SCRIPT_DIR/remote-frontend-preflight.sh"
      ;;
    *) die "PREFLIGHT_SCOPE must be auto, full, rust, or frontend" ;;
  esac
}

latest_run_json() {
  local workflow="$1" sha="$2"
  gh_retry run list --repo "$GH_REPO" --workflow "$workflow" --commit "$sha" --limit 20 \
    --json databaseId,status,conclusion,headSha,url,createdAt \
    --jq 'sort_by(.createdAt) | reverse | .[0] // empty'
}

show_status() {
  local sha="$1" workflow run
  printf 'Exact SHA: %s\n' "$sha"
  for workflow in "${CANDIDATE_WORKFLOWS[@]}" "${FORMAL_WORKFLOWS[@]}"; do
    run="$(latest_run_json "$workflow" "$sha")"
    if [[ -z "$run" ]]; then
      printf '%-30s NOT_STARTED\n' "$workflow"
    else
      printf '%-30s %-12s %-10s %s\n' \
        "$workflow" \
        "$(jq -r '.status' <<<"$run")" \
        "$(jq -r '.conclusion // "-"' <<<"$run")" \
        "$(jq -r '.url' <<<"$run")"
    fi
  done
}

run_is_active() {
  case "$(jq -r '.status' <<<"$1")" in
    queued|in_progress|pending|requested|waiting) return 0 ;;
    *) return 1 ;;
  esac
}

run_is_successful() {
  [[ "$(jq -r '.status' <<<"$1")" == "completed" ]] &&
    [[ "$(jq -r '.conclusion' <<<"$1")" == "success" ]]
}

dispatch_group() {
  local sha="$1" group_name="$2"
  shift 2
  local workflows=("$@")
  local workflow run branch
  branch="$(current_branch)"

  for workflow in "${workflows[@]}"; do
    run="$(latest_run_json "$workflow" "$sha")"
    if [[ -z "$run" ]]; then
      if ! gh_retry workflow run "$workflow" --repo "$GH_REPO" --ref "$branch" >/dev/null; then
        cancel_active_group_runs "$sha" "${workflows[@]}"
        die "failed to dispatch $workflow; active $group_name peers were canceled"
      fi
      printf 'dispatched %s at %s\n' "$workflow" "$sha"
    elif run_is_successful "$run"; then
      printf 'reusing successful %s run %s\n' "$workflow" "$(jq -r '.databaseId' <<<"$run")"
    elif run_is_active "$run"; then
      printf 'reusing active %s run %s\n' "$workflow" "$(jq -r '.databaseId' <<<"$run")"
    else
      cancel_active_group_runs "$sha" "${workflows[@]}"
      die "$group_name workflow $workflow already concluded $(jq -r '.conclusion // "unknown"' <<<"$run") for $sha"
    fi
  done
}

successful_run_id() {
  local workflow="$1" sha="$2" run
  run="$(latest_run_json "$workflow" "$sha")"
  [[ -n "$run" ]] || return 1
  run_is_successful "$run" || return 1
  jq -r '.databaseId' <<<"$run"
}

require_core_cross_targets() {
  local run_id="$1" jobs target count failed
  jobs="$(gh_retry run view "$run_id" --repo "$GH_REPO" --json jobs)"
  for target in mips-unknown-linux-musl mipsel-unknown-linux-musl; do
    count="$(jq --arg target "$target" '[.jobs[] | select(.name | contains($target))] | length' <<<"$jobs")"
    failed="$(jq --arg target "$target" '[.jobs[] | select((.name | contains($target)) and .conclusion != "success")] | length' <<<"$jobs")"
    [[ "$count" -gt 0 && "$failed" -eq 0 ]] || die "Core run $run_id lacks a successful $target job"
  done
}

require_group_success() {
  local sha="$1" group_name="$2"
  shift 2
  local workflow
  for workflow in "$@"; do
    successful_run_id "$workflow" "$sha" >/dev/null || die "$group_name workflow $workflow is not successful for $sha"
  done
}

cancel_active_group_runs() {
  local sha="$1"
  shift
  local workflow run run_id
  for workflow in "$@"; do
    run="$(latest_run_json "$workflow" "$sha")"
    [[ -n "$run" ]] || continue
    if run_is_active "$run"; then
      run_id="$(jq -r '.databaseId' <<<"$run")"
      gh_retry run cancel "$run_id" --repo "$GH_REPO" >/dev/null || true
      printf 'cancel requested for %s run %s\n' "$workflow" "$run_id"
    fi
  done
}

monitor_group() {
  local sha="$1" group_name="$2"
  shift 2
  local workflows=("$@")
  local workflow run status conclusion failed_summary
  local poll_seconds="${WORKFLOW_POLL_SECONDS:-30}"
  local discovery_timeout="${WORKFLOW_DISCOVERY_TIMEOUT:-300}"
  local discovery_started_at all_present all_success

  discovery_started_at="$(date +%s)"
  while true; do
    all_present=true
    all_success=true
    failed_summary=""
    for workflow in "${workflows[@]}"; do
      run="$(latest_run_json "$workflow" "$sha")"
      if [[ -z "$run" ]]; then
        all_present=false
        all_success=false
        continue
      fi
      status="$(jq -r '.status' <<<"$run")"
      conclusion="$(jq -r '.conclusion // ""' <<<"$run")"
      if [[ "$status" != "completed" ]]; then
        all_success=false
      elif [[ "$conclusion" != "success" ]]; then
        failed_summary="$workflow run $(jq -r '.databaseId' <<<"$run") concluded ${conclusion:-unknown}"
        break
      fi
    done

    if [[ -n "$failed_summary" ]]; then
      printf '%s failed: %s\n' "$group_name" "$failed_summary" >&2
      cancel_active_group_runs "$sha" "${workflows[@]}"
      return 1
    fi
    if [[ "$all_present" != true ]] &&
      (( $(date +%s) - discovery_started_at >= discovery_timeout )); then
      printf '%s failed: not all runs appeared within %ss\n' "$group_name" "$discovery_timeout" >&2
      cancel_active_group_runs "$sha" "${workflows[@]}"
      return 1
    fi
    if [[ "$all_present" == true && "$all_success" == true ]]; then
      printf '%s workflows succeeded for %s\n' "$group_name" "$sha"
      return 0
    fi
    sleep "$poll_seconds"
  done
}

monitor_candidates() {
  monitor_group "$1" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
}

monitor_formal() {
  local sha="$1"
  monitor_group "$sha" formal "${FORMAL_WORKFLOWS[@]}"
  require_core_cross_targets "$(successful_run_id core.yml "$sha")"
}

retry_group() {
  local sha="$1" group_name="$2"
  shift 2
  local workflows=("$@")
  local workflow run run_id conclusion branch
  branch="$(current_branch)"
  for workflow in "${workflows[@]}"; do
    run="$(latest_run_json "$workflow" "$sha")"
    if [[ -z "$run" ]]; then
      gh_retry workflow run "$workflow" --repo "$GH_REPO" --ref "$branch" >/dev/null
      printf 'dispatched missing %s at %s\n' "$workflow" "$sha"
    elif run_is_successful "$run" || run_is_active "$run"; then
      printf 'reusing %s run %s\n' "$workflow" "$(jq -r '.databaseId' <<<"$run")"
    else
      run_id="$(jq -r '.databaseId' <<<"$run")"
      conclusion="$(jq -r '.conclusion // "unknown"' <<<"$run")"
      case "$conclusion" in
        failure|timed_out)
          gh_retry run rerun "$run_id" --failed --repo "$GH_REPO" >/dev/null
          printf 'rerunning failed jobs for %s run %s\n' "$workflow" "$run_id"
          ;;
        cancelled|stale|action_required)
          gh_retry run rerun "$run_id" --repo "$GH_REPO" >/dev/null
          printf 'rerunning %s run %s after %s\n' "$workflow" "$run_id" "$conclusion"
          ;;
        *)
          die "$group_name workflow $workflow cannot be retried from conclusion $conclusion"
          ;;
      esac
    fi
  done
}

retry_validation() {
  local sha="$1"
  retry_group "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
  monitor_candidates "$sha"
  retry_group "$sha" formal "${FORMAL_WORKFLOWS[@]}"
  monitor_formal "$sha"
}

dispatch_candidates() {
  local sha="$1"
  dispatch_group "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
  monitor_candidates "$sha"
}

dispatch_formal() {
  local sha="$1"
  require_group_success "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
  dispatch_group "$sha" formal "${FORMAL_WORKFLOWS[@]}"
  monitor_formal "$sha"
}

dispatch_pipeline() {
  local sha="$1"
  dispatch_candidates "$sha"
  printf 'candidate workflows passed; start exact-artifact/device validation while formal workflows run\n'
  dispatch_formal "$sha"
}

init_validation_matrix() {
  local version matrix
  version="$(cargo_version)"
  validate_cross_platform_version "$version" ||
    die "Cargo version $version is not cross-platform safe"
  matrix="$(release_input_path validation_matrix "$version")"
  [[ ! -e "$matrix" ]] || die "validation matrix already exists: $matrix"
  cat >"$matrix" <<EOF
# v${version} Validation Matrix

Candidate SHA: resolved when the immutable candidate is committed.

| Gate | Status | Required evidence |
| --- | --- | --- |
| Complete candidate diff and release inputs | BLOCKED | Record local audit and immutable SHA. |
| \`.160\` locked no-run and focused tests | BLOCKED | Record exact commands and results. |
| Linux profiling candidate | BLOCKED | Record exact-SHA run, hashes, build info and target. |
| Android policy candidate | BLOCKED | Record exact-SHA run, or explicit N/A/waiver when Android validation is outside scope. |
| Linux exact-artifact validation | BLOCKED | Record compatibility, lifecycle, cleanup and applicable network evidence. |
| Android physical-device validation | BLOCKED | Record PASS or explicit WAIVED_BY_MAINTAINER; Mobile compilation is not a substitute. |
| Formal Core including MIPS/MIPSel | BLOCKED | Record exact-SHA run and both cross-target jobs. |
| Formal GUI | BLOCKED | Record exact-SHA run and artifact audit. |
| Formal Mobile | BLOCKED | Record exact-SHA run and all supported ABIs. |
| Formal OHOS | BLOCKED | Record exact-SHA run. |
| Formal Test | BLOCKED | Record exact-SHA run. |
| Release | BLOCKED | Record tag, release target, assets, checksums and install smoke after publication. |
EOF
  printf 'created %s\n' "$matrix"
}

main() {
  local command="${1:-}" sha version core_run workflow make_latest
  configure_candidate_mode
  case "$command" in
    -h|--help|help|'') usage ;;
    help-advanced) advanced_usage ;;
    version) cargo_version ;;
    init-release) init_release_inputs ;;
    prepare) prepare_release ;;
    status)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      show_status "$sha"
      ;;
    builder-sync) "$SCRIPT_DIR/remote-builder-sync.sh" --source ;;
    builder-frontend) "$SCRIPT_DIR/remote-builder-sync.sh" --frontend-deps ;;
    builder-frontend-repair) "$SCRIPT_DIR/remote-builder-sync.sh" --repair-frontend-deps ;;
    builder-preflight)
      "$SCRIPT_DIR/leaf-remote-preflight.sh"
      "$SCRIPT_DIR/remote-frontend-preflight.sh"
      ;;
    init-validation-matrix) init_validation_matrix ;;
    validate|dispatch-candidates|dispatch-formal|dispatch-pipeline|dispatch-all)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      case "$command" in
        dispatch-candidates) dispatch_candidates "$sha" ;;
        dispatch-formal) dispatch_formal "$sha" ;;
        validate|dispatch-pipeline|dispatch-all) dispatch_pipeline "$sha" ;;
      esac
      ;;
    retry)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      retry_validation "$sha"
      ;;
    monitor)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      require_release_inputs
      monitor_candidates "$sha"
      monitor_formal "$sha"
      ;;
    dispatch-core)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      require_group_success "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
      dispatch_group "$sha" diagnostic core.yml
      ;;
    dispatch-rest)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      require_group_success "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
      core_run="$(successful_run_id core.yml "$sha")" || die "Core is not successful for $sha"
      require_core_cross_targets "$core_run"
      dispatch_group "$sha" diagnostic gui.yml mobile.yml ohos.yml test.yml
      ;;
    publish|dispatch-release)
      need gh; need jq
      version="${2:-}"
      [[ "$version" == v* ]] || die "VERSION must start with v"
      validate_cross_platform_version "${version#v}" ||
        die "VERSION must be vX.Y.Z or vX.Y.Z-N with numeric N <= 65535"
      [[ "${version#v}" == "$(cargo_version)" ]] || die "VERSION does not match Cargo version"
      sha="${3:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      require_group_success "$sha" candidate "${ACTIVE_CANDIDATE_WORKFLOWS[@]}"
      require_group_success "$sha" formal "${FORMAL_WORKFLOWS[@]}"
      require_core_cross_targets "$(successful_run_id core.yml "$sha")"
      VALIDATED_SHA="$sha" "$SCRIPT_DIR/release-candidate-audit.sh" --release
      [[ -z "$(latest_run_json release.yml "$sha")" ]] || die "release.yml already has a run for $sha"
      make_latest=true
      [[ "$version" == *-* ]] && make_latest=false
      gh_retry workflow run release.yml --repo "$GH_REPO" --ref "$(current_branch)" \
        -f version="$version" -f make_latest="$make_latest" -f artifact_sha="$sha" >/dev/null
      printf 'dispatched release.yml for %s at %s\n' "$version" "$sha"
      ;;
    *) usage >&2; exit 2 ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
