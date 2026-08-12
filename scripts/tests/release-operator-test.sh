#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../release-operator.sh"

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

assert_eq() {
  [[ "$1" == "$2" ]] || fail "expected [$2], got [$1]"
}

assert_true() {
  "$@" || fail "command failed: $*"
}

assert_false() {
  if "$@"; then
    fail "command unexpectedly succeeded: $*"
  fi
}

test_versions() {
  assert_true validate_cross_platform_version "$(cargo_version)"
  assert_true validate_cross_platform_version 3.0.15
  assert_true validate_cross_platform_version 3.0.15-1
  assert_true validate_cross_platform_version 3.0.15-65535
  assert_false validate_cross_platform_version 3.0.15-beta.1
  assert_false validate_cross_platform_version 3.0.15-rc.1
  assert_false validate_cross_platform_version 3.0.15-01
  assert_false validate_cross_platform_version 3.0.15-65536
  require_release_inputs
}

test_rust_preflight_includes_formal_checks_first() (
  local log
  log="$(mktemp)"
  run_formal_static_preflight() { printf 'static\n' >>"$log"; }
  run_leaf_preflight() { printf 'leaf\n' >>"$log"; }
  run_frontend_preflight() { printf 'frontend\n' >>"$log"; }

  run_preflight_scope rust
  assert_eq "$(cat "$log")" $'static\nleaf'
  : >"$log"
  run_preflight_scope full
  assert_eq "$(cat "$log")" $'static\nleaf\nfrontend'
  : >"$log"
  run_preflight_scope frontend
  assert_eq "$(cat "$log")" frontend
  rm -f "$log"
)

test_workflow_sets() {
  assert_eq "${CANDIDATE_WORKFLOWS[*]}" "profiling-beta.yml android-policy-candidate.yml"
  assert_eq "${FORMAL_WORKFLOWS[*]}" "core.yml gui.yml mobile.yml ohos.yml test.yml"
  unset ANDROID_CANDIDATE_MODE
  configure_candidate_mode
  assert_eq "${ACTIVE_CANDIDATE_WORKFLOWS[*]}" "profiling-beta.yml android-policy-candidate.yml"
  ANDROID_CANDIDATE_MODE=skip configure_candidate_mode
  assert_eq "${ACTIVE_CANDIDATE_WORKFLOWS[*]}" "profiling-beta.yml"
  unset ANDROID_CANDIDATE_MODE
  configure_candidate_mode
}

test_mips_gate() (
  gh_retry() {
    cat <<'EOF'
{"jobs":[{"name":"build (mips-unknown-linux-musl)","conclusion":"success"},{"name":"build (mipsel-unknown-linux-musl)","conclusion":"success"}]}
EOF
  }
  require_core_cross_targets 1

  (
    gh_retry() {
      cat <<'EOF'
{"jobs":[{"name":"build (mips-unknown-linux-musl)","conclusion":"failure"},{"name":"build (mipsel-unknown-linux-musl)","conclusion":"success"}]}
EOF
    }
    require_core_cross_targets 2
  ) && fail "MIPS gate accepted a failed target"
  return 0
)

test_gh_retry() {
  local state
  state="$(mktemp)"
  printf '0\n' >"$state"
  gh() {
    local count
    count="$(cat "$state")"
    count=$((count + 1))
    printf '%s\n' "$count" >"$state"
    if ((count < 3)); then
      printf 'temporary TLS failure\n' >&2
      return 1
    fi
    printf 'ok'
  }
  export GH_RETRY_ATTEMPTS=3 GH_RETRY_DELAY_SECONDS=0
  assert_eq "$(gh_retry run list)" ok
  unset GH_RETRY_ATTEMPTS GH_RETRY_DELAY_SECONDS
  assert_eq "$(cat "$state")" 3
  rm -f "$state"
}

test_formal_requires_candidates() {
  if (
    latest_run_json() { printf ''; }
    dispatch_formal deadbeef
  ); then
    fail "formal workflows started without successful candidates"
  fi
}

test_resumable_dispatch() {
  local log
  log="$(mktemp)"
  current_branch() { printf 'codex/test'; }
  latest_run_json() {
    case "$1" in
      profiling-beta.yml)
        printf '{"databaseId":1,"status":"completed","conclusion":"success"}'
        ;;
      android-policy-candidate.yml) printf '' ;;
    esac
  }
  gh_retry() { printf '%s\n' "$*" >>"$log"; }
  dispatch_group deadbeef candidate "${CANDIDATE_WORKFLOWS[@]}" >/dev/null
  assert_eq "$(wc -l <"$log" | tr -d ' ')" 1
  grep -q 'workflow run android-policy-candidate.yml' "$log" || fail "missing candidate dispatch"
  rm -f "$log"
}

test_failed_group_cancels_peer() {
  local log
  log="$(mktemp)"
  latest_run_json() {
    case "$1" in
      profiling-beta.yml)
        printf '{"databaseId":10,"status":"completed","conclusion":"failure"}'
        ;;
      android-policy-candidate.yml)
        printf '{"databaseId":11,"status":"in_progress","conclusion":null}'
        ;;
    esac
  }
  gh_retry() { printf '%s\n' "$*" >>"$log"; }
  WORKFLOW_POLL_SECONDS=0 WORKFLOW_DISCOVERY_TIMEOUT=0 \
    monitor_group deadbeef candidate "${CANDIDATE_WORKFLOWS[@]}" >/dev/null 2>&1 &&
    fail "failed group was accepted"
  grep -q 'run cancel 11' "$log" || fail "active peer was not canceled"
  rm -f "$log"
}

test_versions
test_rust_preflight_includes_formal_checks_first
test_workflow_sets
test_mips_gate
test_gh_retry
test_formal_requires_candidates
test_resumable_dispatch
test_failed_group_cancels_peer
printf 'release-operator tests: PASS\n'
