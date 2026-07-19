#!/usr/bin/env bash
set -Eeuo pipefail
export LC_ALL=C LANG=C

usage() {
  cat <<'EOF'
Usage: leaf-policy-dataplane-validation.sh OPTIONS

Required:
  --bundle DIR             Exact Linux profiling bundle.
  --output-root DIR        Parent directory for one evidence directory per run.
  --candidate-sha SHA      SHA expected in BUILD_INFO.txt and evidence output.
  --mode MODE              legacy or leaf-owned-tun.
  --run N                  Run number in 1..99.

Optional:
  --trace                  Collect strace -f -c summaries; traced throughput is diagnostic only.
  --max-idle-cpu PERCENT   Per-process idle CPU abort threshold (default: 20).
EOF
}

bundle=
output_root=
candidate_sha=
mode=
run_number=
trace_mode=none
max_idle_cpu=20
while (( $# > 0 )); do
  case "$1" in
    --bundle) bundle=${2:-}; shift 2 ;;
    --output-root) output_root=${2:-}; shift 2 ;;
    --candidate-sha) candidate_sha=${2:-}; shift 2 ;;
    --mode) mode=${2:-}; shift 2 ;;
    --run) run_number=${2:-}; shift 2 ;;
    --trace) trace_mode=strace; shift ;;
    --max-idle-cpu) max_idle_cpu=${2:-}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$bundle" && -n "$output_root" && -n "$candidate_sha" && -n "$mode" && -n "$run_number" ]] \
  || { usage >&2; exit 2; }
[[ "$run_number" =~ ^[1-9][0-9]?$ ]] || { echo "run must be in 1..99" >&2; exit 2; }
case "$mode" in
  legacy|leaf-owned-tun) ;;
  *) echo "invalid mode: $mode" >&2; exit 2 ;;
esac
case "$trace_mode" in
  none|strace) ;;
  *) echo "invalid trace mode: $trace_mode" >&2; exit 2 ;;
esac
[[ $(uname -s) == Linux && $EUID -eq 0 ]] \
  || { echo "Linux root is required for isolated network namespaces" >&2; exit 1; }
[[ "$max_idle_cpu" =~ ^[0-9]+([.][0-9]+)?$ ]] \
  || { echo "max-idle-cpu must be a non-negative number" >&2; exit 2; }
for command_name in ip iperf3 setsid timeout awk find readlink basename date getconf python \
  grep cmp diff sysctl; do
  command -v "$command_name" >/dev/null 2>&1 \
    || { echo "required command not found: $command_name" >&2; exit 1; }
done
if [[ "$trace_mode" == strace ]]; then
  command -v strace >/dev/null 2>&1 \
    || { echo "required command not found: strace" >&2; exit 1; }
fi

bundle=$(cd "$bundle" && pwd)
mkdir -p "$output_root"
output_root=$(cd "$output_root" && pwd)
core="$bundle/easytier-core"
worker_bin="$bundle/easytier-leaf-worker"
for executable in "$core" "$worker_bin"; do
  [[ -x "$executable" ]] || { echo "missing executable: $executable" >&2; exit 1; }
done
grep -Fq "commit=$candidate_sha" "$bundle/BUILD_INFO.txt" \
  || { echo "bundle BUILD_INFO does not match candidate SHA" >&2; exit 1; }

output="$output_root/${mode}-${run_number}"
[[ ! -e "$output" ]] || { echo "evidence already exists: $output" >&2; exit 1; }
mkdir -p "$output"
run_id="${mode:0:1}${run_number}$$"
client_ns="etpd-${run_id}-c"
router_ns="etpd-${run_id}-r"
fixture_ns="etpd-${run_id}-f"
suffix=$(( $$ % 10000 ))
client_veth="ec${suffix}"
router_client_veth="er${suffix}"
router_fixture_veth="es${suffix}"
fixture_veth="ef${suffix}"
core_pid=
worker_pid=
sampler_pid=
core_trace_pid=
worker_trace_pid=
sample_marker="$output/.sample"
cleanup_done=false

namespace_exists() {
  ip netns list | awk '{print $1}' | grep -Fxq "$1"
}

cleanup() {
  if [[ "$cleanup_done" == true ]]; then return; fi
  local namespace process_id
  cleanup_done=true
  rm -f "$sample_marker"
  for process_id in "${core_trace_pid:-}" "${worker_trace_pid:-}"; do
    [[ -n "$process_id" ]] || continue
    kill -INT "$process_id" 2>/dev/null || true
    wait "$process_id" 2>/dev/null || true
  done
  if [[ -n ${sampler_pid:-} ]]; then
    kill "$sampler_pid" 2>/dev/null || true
    wait "$sampler_pid" 2>/dev/null || true
  fi
  for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
    if namespace_exists "$namespace"; then
      for process_id in $(ip netns pids "$namespace" 2>/dev/null); do
        kill -TERM "$process_id" 2>/dev/null || true
      done
    fi
  done
  sleep 1
  for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
    if namespace_exists "$namespace"; then
      for process_id in $(ip netns pids "$namespace" 2>/dev/null); do
        kill -KILL "$process_id" 2>/dev/null || true
      done
      ip netns delete "$namespace" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT
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
    ip netns list | LC_ALL=C sort
  } >"$1"
}

find_namespace_executable() {
  local namespace=$1 wanted=$2 process_id executable matches=
  for process_id in $(ip netns pids "$namespace" 2>/dev/null); do
    executable=$(basename "$(readlink -f "/proc/$process_id/exe" 2>/dev/null || true)")
    if [[ "$executable" == "$wanted" ]]; then
      matches+="${matches:+ }$process_id"
    fi
  done
  [[ -n "$matches" && "$matches" != *' '* ]] || return 1
  printf '%s\n' "$matches"
}

process_ticks() {
  awk '{print $14 + $15}' "/proc/$1/stat"
}

capture_host_state "$output/host-before.txt"
for namespace in "$client_ns" "$router_ns" "$fixture_ns"; do
  namespace_exists "$namespace" && { echo "namespace collision: $namespace" >&2; exit 1; }
  ip netns add "$namespace"
  ip -n "$namespace" link set lo up
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
ip netns exec "$router_ns" sysctl -qw net.ipv4.ip_forward=1

cat >"$output/policy.yaml" <<'YAML'
version: 1
rules:
  - MATCH,DIRECT
YAML

iperf_port=$((5200 + run_number))
ip netns exec "$fixture_ns" setsid iperf3 -s -B 198.51.100.2 -p "$iperf_port" \
  >"$output/iperf-server.log" 2>&1 < /dev/null &

base=$((28500 + run_number * 20))
core_args=(
  --instance-name "packetbatch-$run_id" \
  --network-name "packetbatch-$run_id" \
  --network-secret "isolated-$run_id" \
  -i 10.90.0.1 \
  -l "udp://0.0.0.0:$base,tcp://0.0.0.0:$((base+1)),quic://0.0.0.0:$((base+2)),wg://0.0.0.0:$((base+3)),ws://0.0.0.0:$((base+4))/" \
  --disable-ipv6 true \
  --disable-upnp true \
  --accept-dns false \
  --policy-config "$output/policy.yaml" \
  --policy-outbound-interface eth0 \
  --policy-leaf-executable "$worker_bin"
)
if [[ "$mode" == leaf-owned-tun ]]; then
  core_args+=(--policy-leaf-tun-fast-path true)
else
  core_args+=(--policy-leaf-tun-fast-path false)
fi
ip netns exec "$client_ns" setsid env RUST_LOG=info "$core" \
  "${core_args[@]}" \
  >"$output/easytier-core.log" 2>&1 < /dev/null &

deadline=$((SECONDS + 40))
tun_name=
leaf_tun_name=
while :; do
  core_pid=$(find_namespace_executable "$client_ns" easytier-core || true)
  worker_pid=$(find_namespace_executable "$client_ns" easytier-leaf-worker || true)
  tun_name=$(ip -n "$client_ns" -o -4 address show 2>/dev/null \
    | awk '$4 ~ /^10[.]90[.]0[.]1\// {print $2; exit}')
  leaf_tun_name=$(ip -n "$client_ns" -o -4 address show 2>/dev/null \
    | awk '$4 ~ /^198[.]18[.]/ {print $2; exit}')
  if [[ -n "$core_pid" && -n "$worker_pid" && -n "$tun_name" ]]; then
    if [[ "$mode" == legacy || -n "$leaf_tun_name" ]]; then break; fi
  fi
  if (( SECONDS >= deadline )); then
    echo "candidate readiness timed out" >&2
    tail -100 "$output/easytier-core.log" >&2
    exit 1
  fi
  sleep 0.2
done

tr '\0' ' ' <"/proc/$worker_pid/cmdline" >"$output/worker-cmdline.txt"
ip -n "$client_ns" -4 route show table main >"$output/capture-routes.txt"
if [[ "$mode" == leaf-owned-tun ]]; then
  [[ -n "$leaf_tun_name" ]] || { echo "Leaf-owned TUN was not created" >&2; exit 1; }
  [[ $(awk -v dev="$leaf_tun_name" '$1 ~ /[/]1$/ && $0 ~ ("dev " dev " ") && $0 ~ /metric 65535/ {count++} END {print count+0}' "$output/capture-routes.txt") -eq 2 ]] \
    || { echo "Leaf-owned primary /1 capture routes are incomplete" >&2; exit 1; }
  [[ $(awk -v dev="$tun_name" '$1 ~ /[/]1$/ && $0 ~ ("dev " dev " ") && $0 ~ /metric 65536/ {count++} END {print count+0}' "$output/capture-routes.txt") -eq 2 ]] \
    || { echo "legacy fail-closed fallback /1 routes are incomplete" >&2; exit 1; }
  capture_tun_name="$leaf_tun_name"
else
  [[ -z "$leaf_tun_name" ]] || { echo "legacy mode unexpectedly created a Leaf-owned TUN" >&2; exit 1; }
  [[ $(awk -v dev="$tun_name" '$1 ~ /[/]1$/ && $0 ~ ("dev " dev " ") && $0 ~ /metric 65535/ {count++} END {print count+0}' "$output/capture-routes.txt") -eq 2 ]] \
    || { echo "legacy /1 capture routes are incomplete" >&2; exit 1; }
  if grep -q 'metric 65536' "$output/capture-routes.txt"; then
    echo "legacy mode unexpectedly installed fast-path fallback routes" >&2
    exit 1
  fi
  capture_tun_name="$tun_name"
fi

clock_ticks=$(getconf CLK_TCK)
core_ticks_before=$(process_ticks "$core_pid")
worker_ticks_before=$(process_ticks "$worker_pid")
sleep 3
core_ticks_after=$(process_ticks "$core_pid")
worker_ticks_after=$(process_ticks "$worker_pid")
core_idle_cpu=$(awk -v d="$((core_ticks_after-core_ticks_before))" -v h="$clock_ticks" 'BEGIN{printf "%.3f", d/h/3*100}')
worker_idle_cpu=$(awk -v d="$((worker_ticks_after-worker_ticks_before))" -v h="$clock_ticks" 'BEGIN{printf "%.3f", d/h/3*100}')
awk -v core="$core_idle_cpu" -v worker="$worker_idle_cpu" -v limit="$max_idle_cpu" \
  'BEGIN {exit !((core > limit) || (worker > limit))}' \
  && { echo "idle CPU abort threshold exceeded: core=$core_idle_cpu worker=$worker_idle_cpu limit=$max_idle_cpu" >&2; exit 1; }

timeout 30 ip netns exec "$client_ns" iperf3 -c 198.51.100.2 -p "$iperf_port" -t 1 \
  >"$output/warmup.txt"

if [[ "$trace_mode" == strace ]]; then
  strace -f -c -qq -p "$core_pid" -o "$output/strace-core.txt" &
  core_trace_pid=$!
  strace -f -c -qq -p "$worker_pid" -o "$output/strace-worker.txt" &
  worker_trace_pid=$!
  sleep 0.5
  kill -0 "$core_trace_pid"
  kill -0 "$worker_trace_pid"
fi

read_tun_total() {
  local receive transmit
  receive=$(ip netns exec "$client_ns" cat "/sys/class/net/$capture_tun_name/statistics/rx_bytes")
  transmit=$(ip netns exec "$client_ns" cat "/sys/class/net/$capture_tun_name/statistics/tx_bytes")
  printf '%s\n' "$((receive + transmit))"
}

resources="$output/resources.tsv"
printf 'time_ns\tpid\texecutable\tuser_ticks\tsystem_ticks\trss_kib\tfd_count\tthread_count\n' >"$resources"
touch "$sample_marker"
(
  while [[ -e "$sample_marker" ]]; do
    now=$(date +%s%N)
    for process_id in "$core_pid" "$worker_pid"; do
      [[ -r "/proc/$process_id/stat" ]] || continue
      executable=$(basename "$(readlink -f "/proc/$process_id/exe")")
      read -r user_ticks system_ticks < <(awk '{print $14, $15}' "/proc/$process_id/stat")
      rss_kib=$(awk '$1=="VmRSS:" {print $2; exit}' "/proc/$process_id/status")
      fd_count=$(find "/proc/$process_id/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
      thread_count=$(find "/proc/$process_id/task" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$now" "$process_id" "$executable" "$user_ticks" "$system_ticks" \
        "${rss_kib:-0}" "$fd_count" "$thread_count" >>"$resources"
    done
    sleep 0.1
  done
) &
sampler_pid=$!

tun_before=$(read_tun_total)
timeout 90 ip netns exec "$client_ns" iperf3 -c 198.51.100.2 -p "$iperf_port" -t 6 -J \
  >"$output/upload.json"
timeout 90 ip netns exec "$client_ns" iperf3 -c 198.51.100.2 -p "$iperf_port" -t 6 -R -J \
  >"$output/download.json"
tun_after=$(read_tun_total)
rm -f "$sample_marker"
wait "$sampler_pid"
sampler_pid=
if [[ "$trace_mode" == strace ]]; then
  kill -INT "$core_trace_pid"
  kill -INT "$worker_trace_pid"
  wait "$core_trace_pid" || true
  wait "$worker_trace_pid" || true
  core_trace_pid=
  worker_trace_pid=
fi

read -r upload_bps upload_bytes < <(python - "$output/upload.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
record = data["end"]["sum_received"]
print("%d %d" % (record["bits_per_second"], record["bytes"]))
PY
)
read -r download_bps download_bytes < <(python - "$output/download.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
record = data["end"]["sum_received"]
print("%d %d" % (record["bits_per_second"], record["bytes"]))
PY
)

tun_delta=$((tun_after - tun_before))
minimum_tun=$(((upload_bytes + download_bytes) * 8 / 10))
maximum_tun=$(((upload_bytes + download_bytes) * 6 + 67108864))
(( tun_delta >= minimum_tun )) || { echo "policy TUN traffic was not observed" >&2; exit 1; }
(( tun_delta <= maximum_tun )) || { echo "traffic amplification bound exceeded" >&2; exit 1; }
kill -0 "$core_pid"
kill -0 "$worker_pid"
oversized_log=$(find "$output" -type f -size +16777216c -print -quit)
[[ -z "$oversized_log" ]] || { echo "log growth exceeded 16 MiB: $oversized_log" >&2; exit 1; }

core_syscalls=
worker_syscalls=
if [[ "$trace_mode" == strace ]]; then
  core_syscalls=$(awk '$NF=="total" {print $4; exit}' "$output/strace-core.txt")
  worker_syscalls=$(awk '$NF=="total" {print $4; exit}' "$output/strace-worker.txt")
  [[ "$core_syscalls" =~ ^[0-9]+$ && "$worker_syscalls" =~ ^[0-9]+$ ]] \
    || { echo "failed to parse strace syscall totals" >&2; exit 1; }
fi

awk -F '\t' '
  NR==1 {next}
  {
    key=$3
    if ($6>rss[key]) rss[key]=$6
    if ($7>fds[key]) fds[key]=$7
    if ($8>threads[key]) threads[key]=$8
  }
  END {
    print "executable\tmax_rss_kib\tmax_fd_count\tmax_thread_count"
    for (key in rss) printf "%s\t%s\t%s\t%s\n",key,rss[key],fds[key],threads[key]
  }
' "$resources" | { IFS= read -r header; printf '%s\n' "$header"; sort; } >"$output/resources-summary.tsv"

kill -TERM "$core_pid"
deadline=$((SECONDS + 15))
while kill -0 "$core_pid" 2>/dev/null && (( SECONDS < deadline )); do sleep 0.2; done
if kill -0 "$core_pid" 2>/dev/null; then
  echo "core did not stop within 15 seconds" >&2
  exit 1
fi
deadline=$((SECONDS + 5))
while kill -0 "$worker_pid" 2>/dev/null && (( SECONDS < deadline )); do sleep 0.2; done
if kill -0 "$worker_pid" 2>/dev/null; then
  echo "worker survived core shutdown" >&2
  exit 1
fi

cleanup
trap - EXIT
capture_host_state "$output/host-after.txt"
host_state_unchanged=false
if cmp -s "$output/host-before.txt" "$output/host-after.txt"; then
  host_state_unchanged=true
else
  diff -u "$output/host-before.txt" "$output/host-after.txt" >"$output/host-state.diff" || true
fi
[[ "$host_state_unchanged" == true ]] || { echo "host state changed" >&2; exit 1; }

{
  printf 'candidate_sha\t%s\n' "$candidate_sha"
  printf 'mode\t%s\n' "$mode"
  printf 'run\t%s\n' "$run_number"
  printf 'trace_mode\t%s\n' "$trace_mode"
  printf 'worker\t%s\n' "$worker_bin"
  printf 'worker_cmdline\t%s\n' "$(cat "$output/worker-cmdline.txt")"
  printf 'easytier_tun\t%s\n' "$tun_name"
  printf 'leaf_owned_tun\t%s\n' "$leaf_tun_name"
  printf 'capture_tun\t%s\n' "$capture_tun_name"
  printf 'upload_bps\t%s\n' "$upload_bps"
  printf 'download_bps\t%s\n' "$download_bps"
  printf 'upload_bytes\t%s\n' "$upload_bytes"
  printf 'download_bytes\t%s\n' "$download_bytes"
  printf 'tun_bytes\t%s\n' "$tun_delta"
  printf 'core_idle_cpu_percent\t%s\n' "$core_idle_cpu"
  printf 'worker_idle_cpu_percent\t%s\n' "$worker_idle_cpu"
  printf 'max_idle_cpu_percent\t%s\n' "$max_idle_cpu"
  printf 'core_syscalls\t%s\n' "$core_syscalls"
  printf 'worker_syscalls\t%s\n' "$worker_syscalls"
  printf 'host_state_unchanged\t%s\n' "$host_state_unchanged"
  printf 'core_shutdown_clean\ttrue\n'
  printf 'worker_shutdown_clean\ttrue\n'
} >"$output/summary.tsv"
cat "$output/summary.tsv"
cat "$output/resources-summary.tsv"
