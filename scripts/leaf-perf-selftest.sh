#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  sudo ./leaf-perf-selftest.sh [--bundle DIR] [--output DIR] [--bytes N]
  ./leaf-perf-selftest.sh --check-only [--bundle DIR]

Runs an IPv4 upload/download policy DIRECT performance self-test in three
temporary Linux network namespaces. It never changes host routes, forwarding,
firewall rules, or production EasyTier processes.

Defaults:
  --bundle  directory containing this installed script
  --output  ./easytier-perf-selftest-UTC-PID
  --bytes   134217728 (128 MiB per direction)
EOF
}

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bundle_dir=$script_dir
output_dir=
transfer_bytes=$((128 * 1024 * 1024))
check_only=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --bundle)
      bundle_dir=${2:-}
      shift 2
      ;;
    --output)
      output_dir=${2:-}
      shift 2
      ;;
    --bytes)
      transfer_bytes=${2:-}
      shift 2
      ;;
    --check-only)
      check_only=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown option: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! $transfer_bytes =~ ^[1-9][0-9]*$ ]] \
  || (( transfer_bytes > 17179869184 )); then
  echo '--bytes must be an integer in 1..=17179869184' >&2
  exit 2
fi

core="$bundle_dir/easytier-core"
worker="$bundle_dir/easytier-leaf-worker"
probe="$bundle_dir/easytier-perf-probe"
for binary in "$core" "$worker" "$probe"; do
  if [[ ! -f $binary || ! -x $binary ]]; then
    printf 'required profiling-bundle executable is missing: %s\n' "$binary" >&2
    exit 1
  fi
done

if [[ $check_only == true ]]; then
  "$probe" --help >/dev/null
  printf 'profiling self-test bundle is complete: %s\n' "$bundle_dir"
  exit 0
fi

if [[ $(uname -s) != Linux ]]; then
  echo 'the isolated performance self-test currently requires Linux' >&2
  exit 1
fi
if [[ $EUID -ne 0 ]]; then
  echo 'root is required to create isolated network namespaces' >&2
  exit 1
fi
for command_name in ip sysctl setsid timeout awk find readlink basename date getconf cmp sort; do
  command -v "$command_name" >/dev/null 2>&1 || {
    printf 'required command not found: %s\n' "$command_name" >&2
    exit 1
  }
done

if [[ -z $output_dir ]]; then
  output_dir="$(pwd)/easytier-perf-selftest-$(date -u +%Y%m%dT%H%M%SZ)-$$"
fi
if [[ -d $output_dir && -n $(find "$output_dir" -mindepth 1 -maxdepth 1 -print -quit) ]]; then
  printf 'output directory is not empty; refusing mixed evidence: %s\n' "$output_dir" >&2
  exit 1
fi
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)

run_id=$$
client_ns="et-perf-client-$run_id"
router_ns="et-perf-router-$run_id"
fixture_ns="et-perf-fixture-$run_id"
suffix=$((run_id % 100000))
client_veth="epc${suffix}"
router_client_veth="epr${suffix}"
router_fixture_veth="eps${suffix}"
fixture_veth="epf${suffix}"
sample_marker="$output_dir/.sample"
sample_pid=
cleanup_complete=false

namespace_exists() {
  ip netns list | awk '{ print $1 }' | grep -Fxq "$1"
}

cleanup_topology() {
  if [[ $cleanup_complete == true ]]; then
    return
  fi
  cleanup_complete=true
  rm -f "$sample_marker"
  if [[ -n ${sample_pid:-} ]]; then
    kill "$sample_pid" 2>/dev/null || true
    wait "$sample_pid" 2>/dev/null || true
  fi
  local namespace pid
  for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
    if namespace_exists "$namespace"; then
      for pid in $(ip netns pids "$namespace" 2>/dev/null); do
        kill -TERM "$pid" 2>/dev/null || true
      done
    fi
  done
  sleep 1
  for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
    if namespace_exists "$namespace"; then
      for pid in $(ip netns pids "$namespace" 2>/dev/null); do
        kill -KILL "$pid" 2>/dev/null || true
      done
      ip netns delete "$namespace" 2>/dev/null || true
    fi
  done
}
trap cleanup_topology EXIT
trap 'exit 130' INT TERM

capture_host_state() {
  {
    echo '===== address ====='
      ip -o address show
    echo '===== ipv4 rules ====='
    ip -4 rule show
    echo '===== ipv6 rules ====='
    ip -6 rule show
    echo '===== ipv4 routes ====='
    ip -4 route show table all
    echo '===== ipv6 routes ====='
    ip -6 route show table all
    echo '===== namespaces ====='
    ip netns list | sort
  } >"$1"
}

capture_host_state "$output_dir/host-before.txt"
for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
  if namespace_exists "$namespace"; then
    printf 'self-test namespace already exists: %s\n' "$namespace" >&2
    exit 1
  fi
  ip netns add "$namespace"
done

ip link add "$client_veth" type veth peer name "$router_client_veth"
ip link set "$client_veth" netns "$client_ns"
ip link set "$router_client_veth" netns "$router_ns"
ip link add "$router_fixture_veth" type veth peer name "$fixture_veth"
ip link set "$router_fixture_veth" netns "$router_ns"
ip link set "$fixture_veth" netns "$fixture_ns"

ip -n "$client_ns" link set "$client_veth" name eth0
ip -n "$router_ns" link set "$router_client_veth" name lan0
ip -n "$router_ns" link set "$router_fixture_veth" name lan1
ip -n "$fixture_ns" link set "$fixture_veth" name eth0
for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
  ip -n "$namespace" link set lo up
done
ip -n "$client_ns" address add 192.0.2.2/30 dev eth0
ip -n "$router_ns" address add 192.0.2.1/30 dev lan0
ip -n "$router_ns" address add 198.51.100.1/30 dev lan1
ip -n "$fixture_ns" address add 198.51.100.2/30 dev eth0
ip -n "$client_ns" link set eth0 up
ip -n "$router_ns" link set lan0 up
ip -n "$router_ns" link set lan1 up
ip -n "$fixture_ns" link set eth0 up
ip -n "$client_ns" route add default via 192.0.2.1
ip -n "$fixture_ns" route add default via 198.51.100.1
ip netns exec "$router_ns" sysctl -q -w net.ipv4.ip_forward=1 >/dev/null

cat >"$output_dir/policy.yaml" <<'EOF'
version: 1
rules:
  - MATCH,DIRECT
EOF

fixture_log="$output_dir/fixture.jsonl"
ip netns exec "$fixture_ns" setsid "$probe" server \
  --listen 198.51.100.2:25000 \
  --sessions 3 \
  --timeout-seconds 180 \
  >"$fixture_log" 2>&1 < /dev/null &
fixture_launcher_pid=$!

deadline=$((SECONDS + 10))
while ! grep -Fq '"event":"ready"' "$fixture_log" 2>/dev/null; do
  if ! kill -0 "$fixture_launcher_pid" 2>/dev/null; then
    echo 'performance fixture exited before readiness' >&2
    cat "$fixture_log" >&2
    exit 1
  fi
  if (( SECONDS >= deadline )); then
    echo 'performance fixture readiness timed out' >&2
    exit 1
  fi
  sleep 0.1
done

core_log="$output_dir/easytier-core.log"
ip netns exec "$client_ns" setsid env RUST_LOG=off "$core" \
  --instance-name "perf-$run_id" \
  --network-name "perf-$run_id" \
  --network-secret "isolated-$run_id" \
  -i 10.90.0.1 \
  -l 'udp://0.0.0.0:21030,tcp://0.0.0.0:21031,quic://0.0.0.0:21032,wg://0.0.0.0:21033,ws://0.0.0.0:21034/' \
  --policy-config "$output_dir/policy.yaml" \
  --policy-outbound-interface eth0 \
  --policy-leaf-executable "$worker" \
  >"$core_log" 2>&1 < /dev/null &
core_launcher_pid=$!

find_namespace_executable() {
  local namespace=$1
  local wanted=$2
  local pid executable matches=
  for pid in $(ip netns pids "$namespace" 2>/dev/null); do
    executable=$(basename "$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)")
    if [[ $executable == "$wanted" ]]; then
      matches+="${matches:+ }$pid"
    fi
  done
  [[ -n $matches && $matches != *' '* ]] || return 1
  printf '%s\n' "$matches"
}

deadline=$((SECONDS + 30))
tun_name=
while :; do
  if ! kill -0 "$core_launcher_pid" 2>/dev/null; then
    echo 'easytier-core exited before readiness' >&2
    cat "$core_log" >&2
    exit 1
  fi
  tun_name=$(ip -n "$client_ns" -o -4 address show 2>/dev/null \
    | awk '$4 ~ /^10[.]90[.]0[.]1\// { print $2; exit }')
  if [[ -n $tun_name ]] \
    && find_namespace_executable "$client_ns" easytier-core >/dev/null \
    && find_namespace_executable "$client_ns" easytier-leaf-worker >/dev/null; then
    break
  fi
  if (( SECONDS >= deadline )); then
    echo 'easytier-core/Leaf/TUN readiness timed out' >&2
    cat "$core_log" >&2
    exit 1
  fi
  sleep 0.2
done

timeout 30 ip netns exec "$client_ns" "$probe" client \
  --target 198.51.100.2:25000 \
  --direction upload \
  --bytes 1048576 \
  --timeout-seconds 20 \
  >"$output_dir/warmup.json"

read_tun_counter() {
  ip netns exec "$client_ns" cat "/sys/class/net/$tun_name/statistics/$1"
}

resource_samples="$output_dir/resources.tsv"
printf 'time_ns\tpid\texecutable\tuser_ticks\tsystem_ticks\trss_kib\tfd_count\tthread_count\n' \
  >"$resource_samples"
touch "$sample_marker"
(
  while [[ -e $sample_marker ]]; do
    now=$(date +%s%N)
    for pid in $(ip netns pids "$client_ns" 2>/dev/null); do
      [[ -r /proc/$pid/stat && -r /proc/$pid/status ]] || continue
      executable=$(basename "$(readlink -f "/proc/$pid/exe" 2>/dev/null || echo unknown)")
      read -r user_ticks system_ticks < <(awk '{ print $14, $15 }' "/proc/$pid/stat")
      rss_kib=$(awk '$1 == "VmRSS:" { print $2; exit }' "/proc/$pid/status")
      fd_count=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
      thread_count=$(find "/proc/$pid/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$now" "$pid" "$executable" "$user_ticks" "$system_ticks" \
        "${rss_kib:-0}" "$fd_count" "$thread_count" >>"$resource_samples"
    done
    sleep 0.1
  done
) &
sample_pid=$!

rx0=$(read_tun_counter rx_bytes)
tx0=$(read_tun_counter tx_bytes)
timeout 240 ip netns exec "$client_ns" "$probe" client \
  --target 198.51.100.2:25000 \
  --direction upload \
  --bytes "$transfer_bytes" \
  --timeout-seconds 180 \
  >"$output_dir/upload.json"
rx1=$(read_tun_counter rx_bytes)
tx1=$(read_tun_counter tx_bytes)
timeout 240 ip netns exec "$client_ns" "$probe" client \
  --target 198.51.100.2:25000 \
  --direction download \
  --bytes "$transfer_bytes" \
  --timeout-seconds 180 \
  >"$output_dir/download.json"
rx2=$(read_tun_counter rx_bytes)
tx2=$(read_tun_counter tx_bytes)

rm -f "$sample_marker"
wait "$sample_pid"
sample_pid=
wait "$fixture_launcher_pid"

core_pid=$(find_namespace_executable "$client_ns" easytier-core)
worker_pid=$(find_namespace_executable "$client_ns" easytier-leaf-worker)
if ! kill -0 "$core_pid" 2>/dev/null || ! kill -0 "$worker_pid" 2>/dev/null; then
  echo 'EasyTier or Leaf exited during the transfer' >&2
  exit 1
fi

clock_ticks=$(getconf CLK_TCK)
resource_summary="$output_dir/resources-summary.tsv"
awk -F '\t' -v hz="$clock_ticks" '
  NR == 1 { next }
  {
    key = $3
    ticks = $4 + $5
    if (!(key in first_time)) {
      first_time[key] = $1
      first_ticks[key] = ticks
      first_pid[key] = $2
    }
    last_time[key] = $1
    last_ticks[key] = ticks
    if ($6 > max_rss[key]) max_rss[key] = $6
    if ($7 > max_fd[key]) max_fd[key] = $7
    if ($8 > max_threads[key]) max_threads[key] = $8
  }
  END {
    print "executable\tpid\tavg_cpu_percent\tmax_rss_kib\tmax_fd_count\tmax_thread_count"
    for (key in first_time) {
      elapsed = (last_time[key] - first_time[key]) / 1000000000
      cpu = elapsed > 0 ? ((last_ticks[key] - first_ticks[key]) / hz) / elapsed * 100 : 0
      printf "%s\t%s\t%.3f\t%s\t%s\t%s\n", key, first_pid[key], cpu, max_rss[key], max_fd[key], max_threads[key]
    }
  }
' "$resource_samples" | { IFS= read -r header; printf '%s\n' "$header"; sort; } \
  >"$resource_summary"

upload_rx=$((rx1 - rx0))
upload_tx=$((tx1 - tx0))
download_rx=$((rx2 - rx1))
download_tx=$((tx2 - tx1))
upload_max=$upload_rx
(( upload_tx > upload_max )) && upload_max=$upload_tx
download_max=$download_rx
(( download_tx > download_max )) && download_max=$download_tx
minimum_observed=$((transfer_bytes * 8 / 10))
maximum_observed=$((transfer_bytes * 4 + 16 * 1024 * 1024))
upload_total=$((upload_rx + upload_tx))
download_total=$((download_rx + download_tx))
policy_path_observed=true
loop_bound_ok=true
if (( upload_max < minimum_observed || download_max < minimum_observed )); then
  policy_path_observed=false
fi
if (( upload_total > maximum_observed || download_total > maximum_observed )); then
  loop_bound_ok=false
fi

kill -TERM "$core_pid" 2>/dev/null || true
deadline=$((SECONDS + 15))
while kill -0 "$core_pid" 2>/dev/null && (( SECONDS < deadline )); do
  sleep 0.2
done
cleanup_topology
trap - EXIT
capture_host_state "$output_dir/host-after.txt"
host_state_unchanged=false
if cmp -s "$output_dir/host-before.txt" "$output_dir/host-after.txt"; then
  host_state_unchanged=true
else
  diff -u "$output_dir/host-before.txt" "$output_dir/host-after.txt" \
    >"$output_dir/host-state.diff" || true
fi

resources_json=
while IFS=$'\t' read -r executable pid cpu rss fd threads; do
  [[ $executable == executable ]] && continue
  entry="{\"executable\":\"$executable\",\"pid\":$pid,\"avg_cpu_percent\":$cpu,\"max_rss_kib\":$rss,\"max_fd_count\":$fd,\"max_thread_count\":$threads}"
  resources_json+="${resources_json:+,}$entry"
done <"$resource_summary"

upload_json=$(cat "$output_dir/upload.json")
download_json=$(cat "$output_dir/download.json")
cat >"$output_dir/summary.json" <<EOF
{
  "schema_version": 1,
  "isolated_network_namespaces": true,
  "host_firewall_modified": false,
  "host_forwarding_modified": false,
  "host_state_unchanged": $host_state_unchanged,
  "transfer_bytes_per_direction": $transfer_bytes,
  "upload": $upload_json,
  "download": $download_json,
  "tun": {
    "interface": "$tun_name",
    "upload_rx_bytes": $upload_rx,
    "upload_tx_bytes": $upload_tx,
    "download_rx_bytes": $download_rx,
    "download_tx_bytes": $download_tx
  },
  "resources": [$resources_json],
  "gates": {
    "policy_path_observed": $policy_path_observed,
    "gross_loop_bound_ok": $loop_bound_ok,
    "core_survived_transfer": true,
    "leaf_survived_transfer": true,
    "namespace_cleanup_complete": true
  }
}
EOF

if [[ $policy_path_observed != true || $loop_bound_ok != true \
  || $host_state_unchanged != true ]]; then
  printf 'performance self-test failed; evidence: %s\n' "$output_dir/summary.json" >&2
  exit 1
fi
printf 'performance self-test passed: %s\n' "$output_dir/summary.json"
