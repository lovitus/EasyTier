#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
GH_REPO="${GH_REPO:-lovitus/EasyTier}"
FORMAL_WORKFLOWS=(core.yml gui.yml mobile.yml ohos.yml test.yml)

usage() {
  cat <<'EOF'
Usage: scripts/release-operator.sh COMMAND [ARGUMENTS]

Commands:
  status [SHA]                 Show formal workflow state for one exact SHA.
  builder-sync                Safely synchronize source to 192.168.2.160.
  builder-frontend            Incrementally prepare frozen frontend dependencies.
  builder-frontend-repair     Explicitly replace only known frontend dependencies.
  builder-preflight           Run the standard Rust and ordered frontend gates.
  dispatch-all [SHA]          Dispatch all five formal workflows in parallel, then
                              monitor them and cancel the remainder on first failure.
  monitor [SHA]               Monitor existing formal runs with the same fail-fast rule.
  dispatch-core [SHA]         Optional diagnostic: dispatch only Core.
  dispatch-rest [SHA]         Optional diagnostic: dispatch the other four after Core.
  dispatch-release VERSION [SHA]
                               Audit and dispatch EasyTier Release.

Mutation commands refuse dirty, detached, unpushed, or SHA-mismatched trees.
Repeated workflow dispatch is refused; rerun the existing run explicitly.
EOF
}

die() {
  printf 'release operator: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

head_sha() {
  git -C "$REPO_ROOT" rev-parse HEAD
}

current_branch() {
  git -C "$REPO_ROOT" symbolic-ref --quiet --short HEAD || die "detached HEAD is not releasable"
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
}

latest_run_json() {
  local workflow="$1" sha="$2"
  gh run list --repo "$GH_REPO" --workflow "$workflow" --commit "$sha" --limit 20 \
    --json databaseId,status,conclusion,headSha,url,createdAt \
    --jq 'sort_by(.createdAt) | reverse | .[0] // empty'
}

show_status() {
  local sha="$1" workflow run
  printf 'Exact SHA: %s\n' "$sha"
  for workflow in "${FORMAL_WORKFLOWS[@]}"; do
    run="$(latest_run_json "$workflow" "$sha")"
    if [[ -z "$run" ]]; then
      printf '%-12s NOT_STARTED\n' "$workflow"
    else
      printf '%-12s %-12s %-10s %s\n' \
        "$workflow" \
        "$(jq -r '.status' <<<"$run")" \
        "$(jq -r '.conclusion // "-"' <<<"$run")" \
        "$(jq -r '.url' <<<"$run")"
    fi
  done
}

ensure_not_dispatched() {
  local workflow="$1" sha="$2" existing
  existing="$(latest_run_json "$workflow" "$sha")"
  [[ -z "$existing" ]] || die "$workflow already has run $(jq -r '.databaseId' <<<"$existing") for $sha; rerun that run instead"
}

dispatch_once() {
  local workflow="$1" sha="$2"
  ensure_not_dispatched "$workflow" "$sha"
  gh workflow run "$workflow" --repo "$GH_REPO" --ref "$(current_branch)"
  printf 'dispatched %s at %s\n' "$workflow" "$sha"
}

successful_run_id() {
  local workflow="$1" sha="$2" run
  run="$(latest_run_json "$workflow" "$sha")"
  [[ -n "$run" ]] || return 1
  [[ "$(jq -r '.status' <<<"$run")" == "completed" ]] || return 1
  [[ "$(jq -r '.conclusion' <<<"$run")" == "success" ]] || return 1
  jq -r '.databaseId' <<<"$run"
}

require_core_cross_targets() {
  local run_id="$1" jobs target count failed
  jobs="$(gh run view "$run_id" --repo "$GH_REPO" --json jobs)"
  for target in mips-unknown-linux-musl mipsel-unknown-linux-musl; do
    count="$(jq --arg target "$target" '[.jobs[] | select(.name | contains($target))] | length' <<<"$jobs")"
    failed="$(jq --arg target "$target" '[.jobs[] | select(.name | contains($target) and .conclusion != "success")] | length' <<<"$jobs")"
    [[ "$count" -gt 0 && "$failed" -eq 0 ]] || die "Core run $run_id lacks a successful $target job"
  done
}

require_formal_success() {
  local sha="$1" workflow
  for workflow in "${FORMAL_WORKFLOWS[@]}"; do
    successful_run_id "$workflow" "$sha" >/dev/null || die "$workflow is not successful for $sha"
  done
}

cancel_active_formal_runs() {
  local sha="$1" workflow run status run_id
  for workflow in "${FORMAL_WORKFLOWS[@]}"; do
    run="$(latest_run_json "$workflow" "$sha")"
    [[ -n "$run" ]] || continue
    status="$(jq -r '.status' <<<"$run")"
    case "$status" in
      queued|in_progress|pending|requested|waiting)
        run_id="$(jq -r '.databaseId' <<<"$run")"
        gh run cancel "$run_id" --repo "$GH_REPO" || true
        printf 'cancel requested for %s run %s\n' "$workflow" "$run_id"
        ;;
    esac
  done
}

monitor_formal_runs() {
  local sha="$1" workflow run status conclusion
  local poll_seconds="${FORMAL_POLL_SECONDS:-30}"
  local discovery_timeout="${FORMAL_DISCOVERY_TIMEOUT:-300}"
  local discovery_started_at
  local all_present all_success failed_summary

  discovery_started_at="$(date +%s)"

  while true; do
    all_present=true
    all_success=true
    failed_summary=""

    for workflow in "${FORMAL_WORKFLOWS[@]}"; do
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
        failed_summary="${workflow} run $(jq -r '.databaseId' <<<"$run") concluded ${conclusion:-unknown}"
        break
      fi
    done

    if [[ -n "$failed_summary" ]]; then
      printf 'formal candidate failed: %s\n' "$failed_summary" >&2
      cancel_active_formal_runs "$sha"
      return 1
    fi

    if [[ "$all_present" != true ]] &&
      (( $(date +%s) - discovery_started_at >= discovery_timeout )); then
      printf 'formal candidate failed: not all five runs appeared within %ss\n' \
        "$discovery_timeout" >&2
      cancel_active_formal_runs "$sha"
      return 1
    fi

    if [[ "$all_present" == true && "$all_success" == true ]]; then
      require_core_cross_targets "$(successful_run_id core.yml "$sha")"
      printf 'all five formal workflows succeeded for %s\n' "$sha"
      return 0
    fi

    sleep "$poll_seconds"
  done
}

main() {
  local command="${1:-}" sha version core_run workflow make_latest
  case "$command" in
    -h|--help|help|'') usage ;;
    status)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      show_status "$sha"
      ;;
    builder-sync)
      "$SCRIPT_DIR/remote-builder-sync.sh" --source
      ;;
    builder-frontend)
      "$SCRIPT_DIR/remote-builder-sync.sh" --frontend-deps
      ;;
    builder-frontend-repair)
      "$SCRIPT_DIR/remote-builder-sync.sh" --repair-frontend-deps
      ;;
    builder-preflight)
      "$SCRIPT_DIR/leaf-remote-preflight.sh"
      "$SCRIPT_DIR/remote-frontend-preflight.sh"
      ;;
    dispatch-all)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      for workflow in "${FORMAL_WORKFLOWS[@]}"; do
        ensure_not_dispatched "$workflow" "$sha"
      done
      for workflow in "${FORMAL_WORKFLOWS[@]}"; do
        if ! gh workflow run "$workflow" --repo "$GH_REPO" --ref "$(current_branch)"; then
          sleep 5
          cancel_active_formal_runs "$sha"
          die "failed to dispatch $workflow; remaining formal runs were canceled"
        fi
        printf 'dispatched %s at %s\n' "$workflow" "$sha"
      done
      monitor_formal_runs "$sha"
      ;;
    monitor)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      monitor_formal_runs "$sha"
      ;;
    dispatch-core)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      dispatch_once core.yml "$sha"
      ;;
    dispatch-rest)
      need gh; need jq
      sha="${2:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      core_run="$(successful_run_id core.yml "$sha")" || die "Core is not successful for $sha"
      require_core_cross_targets "$core_run"
      for workflow in gui.yml mobile.yml ohos.yml test.yml; do
        ensure_not_dispatched "$workflow" "$sha"
      done
      for workflow in gui.yml mobile.yml ohos.yml test.yml; do
        gh workflow run "$workflow" --repo "$GH_REPO" --ref "$(current_branch)"
        printf 'dispatched %s at %s\n' "$workflow" "$sha"
      done
      ;;
    dispatch-release)
      need gh; need jq
      version="${2:-}"
      [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] || die "VERSION must look like v3.0.15"
      sha="${3:-$(head_sha)}"
      assert_clean_pushed_sha "$sha"
      require_formal_success "$sha"
      VALIDATED_SHA="$sha" "$SCRIPT_DIR/release-candidate-audit.sh" --release
      ensure_not_dispatched release.yml "$sha"
      make_latest=true
      [[ "$version" == *-* ]] && make_latest=false
      gh workflow run release.yml --repo "$GH_REPO" --ref "$(current_branch)" \
        -f version="$version" -f make_latest="$make_latest" -f artifact_sha="$sha"
      printf 'dispatched release.yml for %s at %s\n' "$version" "$sha"
      ;;
    *) usage >&2; exit 2 ;;
  esac
}

main "$@"
