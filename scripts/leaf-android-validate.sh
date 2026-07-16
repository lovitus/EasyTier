#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/leaf-android-validate.sh install ARTIFACT_DIR
  scripts/leaf-android-validate.sh snapshot OUTPUT_DIR
  scripts/leaf-android-validate.sh probe MATRIX.tsv OUTPUT_DIR
  scripts/leaf-android-validate.sh wifi-recovery MATRIX.tsv OUTPUT_DIR

The candidate VPN must be started through the EasyTier application before
snapshot, probe, or wifi-recovery. This script intentionally does not bypass
the Tauri VPN ownership and configuration path.

MATRIX.tsv columns (tab separated):
  name  host  port  tls_server_name_or_dash  expected_connected

Environment:
  ADB_SERIAL                 default: 192.168.234.227:5555
  ADB                        default: adb
  WIFI_OUTAGE_SECONDS        default: 12
  WIFI_RECONNECT_SECONDS     default: 120
  NETWORK_SETTLE_SECONDS     default: 8
EOF
}

command_name=${1:-}
if [[ -z $command_name || $command_name == "-h" || $command_name == "--help" ]]; then
  usage
  exit 0
fi
shift

adb_bin=${ADB:-adb}
serial=${ADB_SERIAL:-192.168.234.227:5555}
candidate_package=com.kkrainbow.easytier.policycandidate
probe_package=com.kkrainbow.easytier.policyprobe
probe_runner_package=com.kkrainbow.easytier.policyprobe.test
probe_instrumentation=$probe_runner_package/com.kkrainbow.easytier.policyprobe.PolicyProbeInstrumentation

if ! command -v "$adb_bin" >/dev/null 2>&1; then
  printf 'adb command not found: %s\n' "$adb_bin" >&2
  exit 1
fi

adb_device() {
  "$adb_bin" -s "$serial" "$@"
}

require_device() {
  "$adb_bin" connect "$serial" >/dev/null 2>&1 || true
  if [[ $(adb_device get-state 2>/dev/null || true) != device ]]; then
    printf 'Android device is not reachable: %s\n' "$serial" >&2
    exit 1
  fi
}

capture_snapshot() {
  local output_dir=$1
  local label=$2
  local output_file="$output_dir/${label}.txt"
  mkdir -p "$output_dir"
  (
    set +e
    printf 'captured_at=%s\nserial=%s\npackage=%s\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$serial" "$candidate_package"
    echo '===== device ====='
    adb_device shell getprop ro.build.fingerprint
    adb_device shell dumpsys wifi | sed -n '1,160p'
    echo '===== vpn ====='
    adb_device shell dumpsys vpn
    echo '===== process ====='
    adb_device shell "pid=\$(pidof $candidate_package); echo pid=\$pid; if [ -n \"\$pid\" ]; then cat /proc/\$pid/status; echo -n fd_count=; ls /proc/\$pid/fd 2>/dev/null | wc -l; echo -n thread_count=; ls /proc/\$pid/task 2>/dev/null | wc -l; fi"
    echo '===== memory ====='
    adb_device shell dumpsys meminfo "$candidate_package"
    echo '===== recent EasyTier/VPN logs ====='
    adb_device logcat -d -t 500 \
      'TauriVpnService:V' 'easytier:V' 'RustStdoutStderr:V' '*:S'
  ) >"$output_file" 2>&1
  printf 'snapshot: %s\n' "$output_file"
}

run_probe_matrix() {
  local matrix=$1
  local output_dir=$2
  local phase=$3
  local failures=0
  local name host port tls_name expected extra

  if [[ ! -f $matrix ]]; then
    printf 'probe matrix not found: %s\n' "$matrix" >&2
    return 1
  fi
  mkdir -p "$output_dir/$phase"

  while IFS=$'\t' read -r name host port tls_name expected extra \
    || [[ -n ${name:-} ]]; do
    [[ -z ${name:-} || $name == \#* ]] && continue
    if [[ -n ${extra:-} \
      || ! $name =~ ^[A-Za-z0-9._-]+$ \
      || ! $host =~ ^[A-Za-z0-9._:%-]+$ \
      || ! $port =~ ^[0-9]+$ \
      || ! $tls_name =~ ^(-|[A-Za-z0-9._-]+)$ \
      || ! $expected =~ ^(true|false)$ ]]; then
      printf 'invalid probe row: %s\n' "$name" >&2
      failures=$((failures + 1))
      continue
    fi

    local output_file="$output_dir/$phase/$name.txt"
    local args=(
      shell am instrument -w -r
      -e host "$host"
      -e port "$port"
      -e timeout_ms 10000
    )
    if [[ $tls_name != - ]]; then
      args+=(-e tls_server_name "$tls_name")
    fi
    args+=("$probe_instrumentation")

    set +e
    adb_device "${args[@]}" >"$output_file" 2>&1
    local status=$?
    set -e
    if [[ $status -ne 0 \
      || ! $(cat "$output_file") =~ probe_valid=true \
      || ! $(cat "$output_file") =~ probe_connected=$expected \
      || ! $(cat "$output_file") =~ INSTRUMENTATION_CODE:[[:space:]]*-1 ]]; then
      printf 'probe failed: phase=%s name=%s expected=%s output=%s\n' \
        "$phase" "$name" "$expected" "$output_file" >&2
      failures=$((failures + 1))
    else
      printf 'probe passed: phase=%s name=%s connected=%s\n' \
        "$phase" "$name" "$expected"
    fi
  done <"$matrix"

  [[ $failures -eq 0 ]]
}

wait_for_wifi_adb() {
  local timeout_seconds=$1
  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    "$adb_bin" connect "$serial" >/dev/null 2>&1 || true
    if [[ $(adb_device get-state 2>/dev/null || true) == device ]]; then
      return 0
    fi
    sleep 2
  done
  printf 'wireless ADB did not recover within %ss: %s\n' \
    "$timeout_seconds" "$serial" >&2
  return 1
}

case "$command_name" in
  install)
    artifact_dir=${1:-}
    [[ -n $artifact_dir ]] || { usage >&2; exit 2; }
    require_device
    for apk in \
      easytier-android-policy-candidate-aarch64.apk \
      easytier-android-policy-probe-debug.apk \
      easytier-android-policy-probe-runner-debug.apk; do
      [[ -f $artifact_dir/$apk ]] || {
        printf 'artifact not found: %s/%s\n' "$artifact_dir" "$apk" >&2
        exit 1
      }
    done
    adb_device install -r -t "$artifact_dir/easytier-android-policy-candidate-aarch64.apk"
    adb_device install -r -t "$artifact_dir/easytier-android-policy-probe-debug.apk"
    adb_device install -r -t "$artifact_dir/easytier-android-policy-probe-runner-debug.apk"
    echo 'candidate installed; start/configure VPN through the EasyTier application'
    ;;
  snapshot)
    output_dir=${1:-}
    [[ -n $output_dir ]] || { usage >&2; exit 2; }
    require_device
    capture_snapshot "$output_dir" snapshot
    ;;
  probe)
    matrix=${1:-}
    output_dir=${2:-}
    [[ -n $matrix && -n $output_dir ]] || { usage >&2; exit 2; }
    require_device
    capture_snapshot "$output_dir" before-probe
    run_probe_matrix "$matrix" "$output_dir" steady
    capture_snapshot "$output_dir" after-probe
    ;;
  wifi-recovery)
    matrix=${1:-}
    output_dir=${2:-}
    [[ -n $matrix && -n $output_dir ]] || { usage >&2; exit 2; }
    outage_seconds=${WIFI_OUTAGE_SECONDS:-12}
    reconnect_seconds=${WIFI_RECONNECT_SECONDS:-120}
    settle_seconds=${NETWORK_SETTLE_SECONDS:-8}
    [[ $outage_seconds =~ ^[1-9][0-9]*$ \
      && $reconnect_seconds =~ ^[1-9][0-9]*$ \
      && $settle_seconds =~ ^[0-9]+$ ]] || {
      echo 'Wi-Fi timing values must be positive integers' >&2
      exit 2
    }
    require_device
    adb_device shell svc wifi enable
    run_probe_matrix "$matrix" "$output_dir" before-outage
    capture_snapshot "$output_dir" before-outage

    # Schedule recovery on-device before disabling Wi-Fi. The detached, nohup
    # child survives loss of the wireless ADB transport and restores Wi-Fi.
    set +e
    adb_device shell \
      "nohup sh -c 'sleep $outage_seconds; svc wifi enable' >/data/local/tmp/easytier-wifi-restore.log 2>&1 </dev/null & svc wifi disable"
    set -e
    wait_for_wifi_adb "$reconnect_seconds"
    sleep "$settle_seconds"

    capture_snapshot "$output_dir" after-recovery
    run_probe_matrix "$matrix" "$output_dir" after-recovery
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
