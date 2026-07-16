#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  sudo scripts/leaf-linux-validate.sh snapshot NETNS OUTPUT_DIR LABEL
  sudo scripts/leaf-linux-validate.sh worker-restart NETNS OUTPUT_DIR
  sudo scripts/leaf-linux-validate.sh route-recovery NETNS OUTPUT_DIR [OUTAGE_SECONDS]

This script operates only inside an existing Linux network namespace. It does
not create a topology, start EasyTier, build binaries, or modify host routes.

Optional executable probe hooks receive NETNS and PHASE as arguments:
  LEAF_LINUX_MESH_PROBE         must succeed before/during/after faults
  LEAF_LINUX_POLICY_PROBE       must succeed before/after recovery
  LEAF_LINUX_FAIL_CLOSED_PROBE  must prove no policy leak during route outage

Environment:
  WORKER_RECOVERY_SECONDS  default: 30
  ROUTE_SETTLE_SECONDS     default: 8
EOF
}

action=${1:-}
if [[ -z $action || $action == -h || $action == --help ]]; then
  usage
  exit 0
fi
shift

namespace=${1:-}
output_dir=${2:-}
if [[ -z $namespace || -z $output_dir ]]; then
  usage >&2
  exit 2
fi
shift 2

if [[ $EUID -ne 0 ]]; then
  echo 'root is required for namespace and /proc evidence' >&2
  exit 1
fi
if [[ ! $namespace =~ ^[A-Za-z0-9._-]+$ \
  || ! -e /var/run/netns/$namespace ]]; then
  printf 'existing network namespace required: %s\n' "$namespace" >&2
  exit 1
fi
for command_name in ip readlink basename awk wc date; do
  command -v "$command_name" >/dev/null 2>&1 || {
    printf 'required command not found: %s\n' "$command_name" >&2
    exit 1
  }
done
mkdir -p "$output_dir"

run_hook() {
  local variable_name=$1
  local phase=$2
  local hook=${!variable_name:-}
  if [[ -z $hook ]]; then
    printf 'probe hook not configured: %s\n' "$variable_name" >&2
    return 2
  fi
  if [[ ! -x $hook ]]; then
    printf 'probe hook is not executable: %s=%s\n' "$variable_name" "$hook" >&2
    return 2
  fi
  "$hook" "$namespace" "$phase"
}

snapshot() {
  local label=$1
  local state_file="$output_dir/${label}-state.txt"
  local resource_file="$output_dir/${label}-resources.tsv"
  (
    set +e
    printf 'captured_at=%s\nnamespace=%s\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$namespace"
    echo '===== address ====='
    ip -n "$namespace" -details address show
    echo '===== rule ====='
    ip -n "$namespace" -4 rule show
    echo '===== main routes ====='
    ip -n "$namespace" -4 route show table main
    echo '===== policy bypass routes ====='
    ip -n "$namespace" -4 route show table 52000
    echo '===== sockets ====='
    ip netns exec "$namespace" ss -H -tunap
  ) >"$state_file" 2>&1

  printf 'pid\texecutable\trss_kib\tfd_count\tthread_count\n' >"$resource_file"
  local pid executable rss fd_count thread_count
  for pid in $(ip netns pids "$namespace"); do
    [[ -r /proc/$pid/status ]] || continue
    executable=$(basename "$(readlink -f "/proc/$pid/exe" 2>/dev/null || echo unknown)")
    rss=$(awk '$1 == "VmRSS:" { print $2 }' "/proc/$pid/status" 2>/dev/null || true)
    fd_count=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    thread_count=$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
    printf '%s\t%s\t%s\t%s\t%s\n' \
      "$pid" "$executable" "${rss:-0}" "$fd_count" "$thread_count" \
      >>"$resource_file"
  done
  printf 'snapshot: %s %s\n' "$state_file" "$resource_file"
}

find_leaf_worker() {
  local pid executable matches=
  for pid in $(ip netns pids "$namespace"); do
    executable=$(basename "$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)")
    if [[ $executable == easytier-leaf-worker ]]; then
      matches+="${matches:+ }$pid"
    fi
  done
  if [[ $matches == *' '* || -z $matches ]]; then
    printf 'expected exactly one easytier-leaf-worker, found: %s\n' \
      "${matches:-none}" >&2
    return 1
  fi
  printf '%s\n' "$matches"
}

case "$action" in
  snapshot)
    label=${1:-}
    [[ -n $label && $label =~ ^[A-Za-z0-9._-]+$ ]] || {
      usage >&2
      exit 2
    }
    snapshot "$label"
    ;;
  worker-restart)
    recovery_seconds=${WORKER_RECOVERY_SECONDS:-30}
    [[ $recovery_seconds =~ ^[1-9][0-9]*$ ]] || {
      echo 'WORKER_RECOVERY_SECONDS must be a positive integer' >&2
      exit 2
    }
    run_hook LEAF_LINUX_MESH_PROBE before-worker-kill
    run_hook LEAF_LINUX_POLICY_PROBE before-worker-kill
    snapshot before-worker-kill
    old_worker=$(find_leaf_worker)
    kill -KILL "$old_worker"

    deadline=$((SECONDS + recovery_seconds))
    new_worker=
    while (( SECONDS < deadline )); do
      candidate=$(find_leaf_worker 2>/dev/null || true)
      if [[ -n $candidate && $candidate != "$old_worker" ]]; then
        new_worker=$candidate
        break
      fi
      sleep 1
    done
    if [[ -z $new_worker ]]; then
      printf 'Leaf worker did not restart within %ss\n' "$recovery_seconds" >&2
      exit 1
    fi
    printf 'Leaf worker restarted: old=%s new=%s\n' "$old_worker" "$new_worker"
    run_hook LEAF_LINUX_MESH_PROBE after-worker-restart
    run_hook LEAF_LINUX_POLICY_PROBE after-worker-restart
    snapshot after-worker-restart
    ;;
  route-recovery)
    outage_seconds=${1:-12}
    settle_seconds=${ROUTE_SETTLE_SECONDS:-8}
    [[ $outage_seconds =~ ^[1-9][0-9]*$ \
      && $settle_seconds =~ ^[0-9]+$ ]] || {
      echo 'route timing values must be non-negative integers' >&2
      exit 2
    }
    default_routes=$(ip -n "$namespace" -4 route show default)
    route_count=$(sed '/^[[:space:]]*$/d' <<<"$default_routes" | wc -l)
    if [[ $route_count -ne 1 ]]; then
      printf 'expected exactly one namespace default route, found %s\n' \
        "$route_count" >&2
      exit 1
    fi
    read -r -a default_route_args <<<"$default_routes"
    restored=false
    restore_default() {
      if [[ $restored == false ]]; then
        ip -n "$namespace" -4 route add "${default_route_args[@]}" \
          >/dev/null 2>&1 || true
        restored=true
      fi
    }
    trap restore_default EXIT INT TERM

    run_hook LEAF_LINUX_MESH_PROBE before-route-outage
    run_hook LEAF_LINUX_POLICY_PROBE before-route-outage
    snapshot before-route-outage
    ip -n "$namespace" -4 route del "${default_route_args[@]}"
    sleep "$outage_seconds"
    snapshot during-route-outage
    run_hook LEAF_LINUX_MESH_PROBE during-route-outage
    run_hook LEAF_LINUX_FAIL_CLOSED_PROBE during-route-outage
    restore_default
    trap - EXIT INT TERM
    sleep "$settle_seconds"
    run_hook LEAF_LINUX_MESH_PROBE after-route-recovery
    run_hook LEAF_LINUX_POLICY_PROBE after-route-recovery
    snapshot after-route-recovery
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
