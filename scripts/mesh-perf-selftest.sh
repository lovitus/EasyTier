#!/usr/bin/env bash
set -Eeuo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BUNDLE="$SCRIPT_DIR"
OUTPUT=""
BYTES=134217728
MODES_CSV="native,kcp,quic,relay"
CHECK_ONLY=false

MAX_PROCESS_RSS_KIB="${ET_PERF_MAX_PROCESS_RSS_KIB:-524288}"
MAX_TOTAL_RSS_KIB="${ET_PERF_MAX_TOTAL_RSS_KIB:-1048576}"
MAX_FDS="${ET_PERF_MAX_FDS:-512}"
MAX_THREADS="${ET_PERF_MAX_THREADS:-128}"
MAX_LOG_BYTES="${ET_PERF_MAX_LOG_BYTES:-16777216}"
IDLE_PROCESS_CPU_PERCENT="${ET_PERF_IDLE_CPU_PERCENT:-80}"
STALLED_TOTAL_CPU_PERCENT="${ET_PERF_STALLED_CPU_PERCENT:-180}"
MODE_TIMEOUT_SECONDS="${ET_PERF_MODE_TIMEOUT_SECONDS:-180}"

CORE=""
CLI=""
PROBE=""
HARNESS_PID=$$
CURRENT_MODE="startup"
CURRENT_A=""
CURRENT_B=""
CURRENT_R=""
CURRENT_PIDS=()
CURRENT_NAMESPACES=()
CORE_LABELS=()
CORE_PIDS=()
CORE_LOGS=()
WATCHDOG_PID=""
WATCHDOG_STOP=""
WATCHDOG_PHASE=""
WATCHDOG_INTERFACES=""
ABORTED=false

usage() {
    cat <<'EOF'
Usage: mesh-perf-selftest.sh [OPTIONS]

Options:
  --bundle DIR       Profiling bundle containing easytier-core, easytier-cli,
                     and easytier-perf-probe. Defaults to this script's directory.
  --output DIR       New output directory. Defaults to a timestamped directory.
  --bytes N          Bytes transferred per direction and mode (default: 134217728).
  --modes LIST       Comma-separated native,kcp,quic,relay (default: all).
  --check-only       Check platform, privileges, tools, and bundle contents only.
  -h, --help         Show this help.

Safety limits may be adjusted with ET_PERF_MAX_PROCESS_RSS_KIB,
ET_PERF_MAX_TOTAL_RSS_KIB, ET_PERF_MAX_FDS, ET_PERF_MAX_THREADS,
ET_PERF_MAX_LOG_BYTES, ET_PERF_IDLE_CPU_PERCENT,
ET_PERF_STALLED_CPU_PERCENT, and ET_PERF_MODE_TIMEOUT_SECONDS.
EOF
}

is_uint() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

json_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/[[:cntrl:]]/ /g'
}

write_abort() {
    local reason="$1"
    local detail="${2:-}"
    local value="${3:-0}"
    local limit="${4:-0}"

    [[ -n "$OUTPUT" ]] || return 0
    mkdir -p "$OUTPUT"
    cat > "$OUTPUT/abort.json.tmp" <<EOF
{
  "schema_version": 1,
  "aborted": true,
  "mode": "$(json_escape "$CURRENT_MODE")",
  "reason": "$(json_escape "$reason")",
  "detail": "$(json_escape "$detail")",
  "observed": $value,
  "limit": $limit,
  "cleanup_attempted": true
}
EOF
    mv "$OUTPUT/abort.json.tmp" "$OUTPUT/abort.json"
}

die() {
    local message="$1"
    write_abort "harness_error" "$message"
    printf 'mesh performance self-test failed: %s\n' "$message" >&2
    exit 1
}

collect_abort_logs() {
    [[ -n "$OUTPUT" && -d "$OUTPUT" ]] || return 0
    local report="$OUTPUT/abort-logs.txt"
    : > "$report"
    while IFS= read -r log_file; do
        printf '===== %s =====\n' "$log_file" >> "$report"
        tail -200 "$log_file" >> "$report" 2>/dev/null || true
    done < <(find "$OUTPUT" -type f -name '*.log' -print 2>/dev/null | LC_ALL=C sort)
}

stop_watchdog() {
    if [[ -n "$WATCHDOG_STOP" ]]; then
        : > "$WATCHDOG_STOP"
    fi
    if [[ -n "$WATCHDOG_PID" ]]; then
        wait "$WATCHDOG_PID" 2>/dev/null || true
    fi
    WATCHDOG_PID=""
    WATCHDOG_STOP=""
    WATCHDOG_PHASE=""
    WATCHDOG_INTERFACES=""
}

cleanup_mode() {
    set +e
    stop_watchdog

    local pid
    for pid in "${CURRENT_PIDS[@]:-}"; do
        [[ -n "$pid" ]] || continue
        kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
    done
    sleep 0.25
    for pid in "${CURRENT_PIDS[@]:-}"; do
        [[ -n "$pid" ]] || continue
        kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done

    local ns
    for ns in "${CURRENT_NAMESPACES[@]:-}"; do
        [[ -n "$ns" ]] || continue
        ip netns delete "$ns" 2>/dev/null || true
    done

    CURRENT_PIDS=()
    CURRENT_NAMESPACES=()
    CORE_LABELS=()
    CORE_PIDS=()
    CORE_LOGS=()
    CURRENT_A=""
    CURRENT_B=""
    CURRENT_R=""
    set -e
}

on_signal() {
    local signal="$1"
    if [[ ! -f "$OUTPUT/abort.json" ]]; then
        write_abort "signal" "$signal"
    fi
    cleanup_mode
    collect_abort_logs
    printf 'mesh performance self-test aborted; report: %s/abort.json\n' "$OUTPUT" >&2
    exit 143
}

on_exit() {
    local rc=$?
    if (( rc != 0 )) && [[ -n "$OUTPUT" && ! -f "$OUTPUT/abort.json" ]]; then
        write_abort "unexpected_exit" "shell exited before completing the active mode" "$rc" 0
    fi
    cleanup_mode
    if (( rc != 0 )); then
        collect_abort_logs
    fi
    exit "$rc"
}

trap 'on_signal TERM' TERM
trap 'on_signal INT' INT
trap on_exit EXIT

while (( $# > 0 )); do
    case "$1" in
        --bundle)
            (( $# >= 2 )) || die "--bundle requires a directory"
            BUNDLE="$2"
            shift 2
            ;;
        --output)
            (( $# >= 2 )) || die "--output requires a directory"
            OUTPUT="$2"
            shift 2
            ;;
        --bytes)
            (( $# >= 2 )) || die "--bytes requires an integer"
            BYTES="$2"
            shift 2
            ;;
        --modes)
            (( $# >= 2 )) || die "--modes requires a comma-separated list"
            MODES_CSV="$2"
            shift 2
            ;;
        --check-only)
            CHECK_ONLY=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

[[ "$(uname -s)" == "Linux" ]] || die "safe mesh self-test requires Linux network namespaces"
(( EUID == 0 )) || die "safe mesh self-test requires root for isolated network namespaces"

for command_name in ip awk sed grep date find stat setsid nice timeout getconf; do
    command -v "$command_name" >/dev/null 2>&1 || die "required command is missing: $command_name"
done

is_uint "$BYTES" || die "--bytes must be an integer"
(( BYTES >= 1048576 && BYTES <= 4294967296 )) || die "--bytes must be between 1 MiB and 4 GiB"
for value in "$MAX_PROCESS_RSS_KIB" "$MAX_TOTAL_RSS_KIB" "$MAX_FDS" "$MAX_THREADS" \
    "$MAX_LOG_BYTES" "$IDLE_PROCESS_CPU_PERCENT" "$STALLED_TOTAL_CPU_PERCENT" \
    "$MODE_TIMEOUT_SECONDS"; do
    is_uint "$value" || die "safety limits must be positive integers"
    (( value > 0 )) || die "safety limits must be greater than zero"
done

BUNDLE="$(cd "$BUNDLE" && pwd)"
CORE="$BUNDLE/easytier-core"
CLI="$BUNDLE/easytier-cli"
PROBE="$BUNDLE/easytier-perf-probe"
for executable in "$CORE" "$CLI" "$PROBE"; do
    [[ -x "$executable" ]] || die "missing executable: $executable"
done

IFS=',' read -r -a REQUESTED_MODES <<< "$MODES_CSV"
declare -A SEEN_MODES=()
for mode in "${REQUESTED_MODES[@]}"; do
    case "$mode" in
        native|kcp|quic|relay) ;;
        *) die "unsupported mode: $mode" ;;
    esac
    [[ -z "${SEEN_MODES[$mode]:-}" ]] || die "duplicate mode: $mode"
    SEEN_MODES[$mode]=1
done
(( ${#REQUESTED_MODES[@]} > 0 )) || die "at least one mode is required"

if [[ "$CHECK_ONLY" == true ]]; then
    printf 'mesh performance self-test prerequisites are available: modes=%s\n' "$MODES_CSV"
    exit 0
fi

if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$PWD/easytier-mesh-perf-$(date +%Y%m%d-%H%M%S)"
fi
if [[ -e "$OUTPUT" ]] && find "$OUTPUT" -mindepth 1 -print -quit 2>/dev/null | grep -q .; then
    die "output directory must be empty: $OUTPUT"
fi
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"

capture_host_state() {
    local destination="$1"
    {
        printf '%s\n' '===== links ====='
        ip -o link show
        printf '%s\n' '===== addresses ====='
        ip -o address show | sed -E \
            -e 's/valid_lft [0-9]+sec/valid_lft <dynamic>/g' \
            -e 's/preferred_lft [0-9]+sec/preferred_lft <dynamic>/g'
        printf '%s\n' '===== ipv4 routes ====='
        ip route show table all
        printf '%s\n' '===== ipv6 routes ====='
        ip -6 route show table all
        printf '%s\n' '===== ipv4 rules ====='
        ip rule show
        printf '%s\n' '===== ipv6 rules ====='
        ip -6 rule show
        printf '%s\n' '===== namespaces ====='
        ip netns list
    } > "$destination"
}

capture_host_state "$OUTPUT/host-state.before"

new_namespace() {
    local ns="$1"
    ip netns add "$ns"
    CURRENT_NAMESPACES+=("$ns")
    ip -n "$ns" link set lo up
}

listener_list() {
    local base="$1"
    printf 'udp://0.0.0.0:%d,tcp://0.0.0.0:%d,quic://0.0.0.0:%d,wg://0.0.0.0:%d,ws://0.0.0.0:%d/' \
        "$base" "$((base + 1))" "$((base + 2))" "$((base + 3))" "$((base + 4))"
}

start_core() {
    local ns="$1"
    local label="$2"
    local log_file="$3"
    shift 3

    ip netns exec "$ns" setsid nice -n 5 env RUST_LOG=warn "$CORE" "$@" \
        > "$log_file" 2>&1 < /dev/null &
    local pid=$!
    CURRENT_PIDS+=("$pid")
    CORE_LABELS+=("$label")
    CORE_PIDS+=("$pid")
    CORE_LOGS+=("$log_file")
}

wait_for_mesh() {
    local source_ns="$1"
    local target_ip="$2"
    local deadline=$((SECONDS + 30))
    while (( SECONDS < deadline )); do
        if ip netns exec "$source_ns" ping -c 1 -W 1 "$target_ip" >/dev/null 2>&1; then
            return 0
        fi
        local pid
        for pid in "${CORE_PIDS[@]}"; do
            kill -0 "$pid" 2>/dev/null || return 1
        done
        sleep 0.2
    done
    return 1
}

interface_bytes() {
    local total=0
    local ns dev rx tx
    while IFS=$'\t' read -r ns dev; do
        [[ -n "$ns" && -n "$dev" ]] || continue
        rx="$(ip netns exec "$ns" cat "/sys/class/net/$dev/statistics/rx_bytes" 2>/dev/null || printf 0)"
        tx="$(ip netns exec "$ns" cat "/sys/class/net/$dev/statistics/tx_bytes" 2>/dev/null || printf 0)"
        total=$((total + rx + tx))
    done < "$WATCHDOG_INTERFACES"
    printf '%d\n' "$total"
}

tun_bytes() {
    local total=0
    local ns rx tx
    for ns in "$CURRENT_A" "$CURRENT_B"; do
        [[ -n "$ns" ]] || continue
        rx="$(ip netns exec "$ns" cat /sys/class/net/tun0/statistics/rx_bytes 2>/dev/null || printf 0)"
        tx="$(ip netns exec "$ns" cat /sys/class/net/tun0/statistics/tx_bytes 2>/dev/null || printf 0)"
        total=$((total + rx + tx))
    done
    printf '%d\n' "$total"
}

set_watchdog_phase() {
    local phase="$1"
    local tun_budget="$2"
    local underlay_budget="$3"
    local tun_base underlay_base
    tun_base="$(tun_bytes)"
    underlay_base="$(interface_bytes)"
    printf '%s\t%d\t%d\t%d\t%d\n' "$phase" "$tun_base" "$tun_budget" \
        "$underlay_base" "$underlay_budget" > "$WATCHDOG_PHASE.tmp"
    mv "$WATCHDOG_PHASE.tmp" "$WATCHDOG_PHASE"
}

watchdog_abort() {
    local reason="$1"
    local detail="$2"
    local value="$3"
    local limit="$4"
    write_abort "$reason" "$detail" "$value" "$limit"
    kill -TERM "$HARNESS_PID" 2>/dev/null || true
    exit 1
}

watchdog_loop() {
    local targets_file="$1"
    local samples_file="$2"
    local watchdog_started_seconds=$SECONDS
    local clock_ticks page_kib
    clock_ticks="$(getconf CLK_TCK)"
    page_kib=$(( $(getconf PAGESIZE) / 1024 ))
    declare -A previous_ticks=()
    declare -A idle_busy_count=()
    local previous_ms previous_total_ticks previous_underlay
    previous_ms="$(date +%s%3N)"
    previous_total_ticks=0
    previous_underlay="$(interface_bytes)"
    local stalled_count=0

    printf 'epoch_ms\tlabel\tpid\tutime\tstime\trss_kib\tfd_count\tthread_count\n' > "$samples_file"

    while [[ ! -e "$WATCHDOG_STOP" ]]; do
        if (( SECONDS - watchdog_started_seconds > MODE_TIMEOUT_SECONDS )); then
            watchdog_abort "wall_timeout" "$CURRENT_MODE" \
                "$((SECONDS - watchdog_started_seconds))" "$MODE_TIMEOUT_SECONDS"
        fi
        local now_ms total_rss total_ticks phase tun_base tun_budget underlay_base underlay_budget
        local current_tun current_underlay
        now_ms="$(date +%s%3N)"
        total_rss=0
        total_ticks=0

        while IFS=$'\t' read -r label pid log_file; do
            [[ -n "$label" && -n "$pid" ]] || continue
            kill -0 "$pid" 2>/dev/null || watchdog_abort "process_exited" "$label" 0 1

            local utime stime threads rss_pages rss_kib fd_count log_bytes ticks delta_ticks delta_ms cpu_percent
            read -r utime stime threads rss_pages < <(awk '{print $14, $15, $20, $24}' "/proc/$pid/stat")
            rss_kib=$((rss_pages * page_kib))
            fd_count="$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 -print 2>/dev/null | wc -l)"
            log_bytes="$(stat -c %s "$log_file" 2>/dev/null || printf 0)"
            ticks=$((utime + stime))
            total_ticks=$((total_ticks + ticks))
            total_rss=$((total_rss + rss_kib))
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$now_ms" "$label" "$pid" \
                "$utime" "$stime" "$rss_kib" "$fd_count" "$threads" >> "$samples_file"

            (( rss_kib <= MAX_PROCESS_RSS_KIB )) || watchdog_abort "rss_limit" "$label" "$rss_kib" "$MAX_PROCESS_RSS_KIB"
            (( fd_count <= MAX_FDS )) || watchdog_abort "fd_limit" "$label" "$fd_count" "$MAX_FDS"
            (( threads <= MAX_THREADS )) || watchdog_abort "thread_limit" "$label" "$threads" "$MAX_THREADS"
            (( log_bytes <= MAX_LOG_BYTES )) || watchdog_abort "log_growth_limit" "$label" "$log_bytes" "$MAX_LOG_BYTES"

            if [[ -n "${previous_ticks[$label]:-}" && -f "$WATCHDOG_PHASE" ]]; then
                delta_ticks=$((ticks - previous_ticks[$label]))
                delta_ms=$((now_ms - previous_ms))
                if (( delta_ms > 0 )); then
                    cpu_percent=$((delta_ticks * 100000 / clock_ticks / delta_ms))
                    phase="$(cut -f1 "$WATCHDOG_PHASE")"
                    if [[ "$phase" == "idle" && cpu_percent -ge IDLE_PROCESS_CPU_PERCENT ]]; then
                        idle_busy_count[$label]=$(( ${idle_busy_count[$label]:-0} + 1 ))
                        (( idle_busy_count[$label] < 8 )) || watchdog_abort \
                            "idle_cpu_storm" "$label" "$cpu_percent" "$IDLE_PROCESS_CPU_PERCENT"
                    else
                        idle_busy_count[$label]=0
                    fi
                fi
            fi
            previous_ticks[$label]="$ticks"
        done < "$targets_file"

        (( total_rss <= MAX_TOTAL_RSS_KIB )) || watchdog_abort \
            "total_rss_limit" "all_easyTier_processes" "$total_rss" "$MAX_TOTAL_RSS_KIB"

        if [[ -f "$WATCHDOG_PHASE" ]]; then
            IFS=$'\t' read -r phase tun_base tun_budget underlay_base underlay_budget < "$WATCHDOG_PHASE"
            current_tun="$(tun_bytes)"
            current_underlay="$(interface_bytes)"
            (( current_tun - tun_base <= tun_budget )) || watchdog_abort \
                "tun_amplification" "$phase" "$((current_tun - tun_base))" "$tun_budget"
            (( current_underlay - underlay_base <= underlay_budget )) || watchdog_abort \
                "underlay_amplification" "$phase" "$((current_underlay - underlay_base))" "$underlay_budget"

            local elapsed_ms total_delta_ticks total_cpu_percent underlay_progress
            elapsed_ms=$((now_ms - previous_ms))
            total_delta_ticks=$((total_ticks - previous_total_ticks))
            underlay_progress=$((current_underlay - previous_underlay))
            total_cpu_percent=0
            if (( elapsed_ms > 0 )); then
                total_cpu_percent=$((total_delta_ticks * 100000 / clock_ticks / elapsed_ms))
            fi
            if [[ "$phase" != "idle" && total_cpu_percent -ge STALLED_TOTAL_CPU_PERCENT && underlay_progress -lt 65536 ]]; then
                stalled_count=$((stalled_count + 1))
                (( stalled_count < 8 )) || watchdog_abort \
                    "busy_without_progress" "$phase" "$total_cpu_percent" "$STALLED_TOTAL_CPU_PERCENT"
            else
                stalled_count=0
            fi
            previous_underlay="$current_underlay"
        fi

        previous_total_ticks="$total_ticks"
        previous_ms="$now_ms"
        sleep 0.25
    done
}

start_watchdog() {
    local mode_dir="$1"
    local targets_file="$mode_dir/watchdog-targets.tsv"
    WATCHDOG_STOP="$mode_dir/watchdog.stop"
    WATCHDOG_PHASE="$mode_dir/watchdog-phase.tsv"
    WATCHDOG_INTERFACES="$mode_dir/watchdog-interfaces.tsv"
    rm -f "$WATCHDOG_STOP" "$WATCHDOG_PHASE"
    : > "$targets_file"
    local index
    for index in "${!CORE_PIDS[@]}"; do
        printf '%s\t%s\t%s\n' "${CORE_LABELS[$index]}" "${CORE_PIDS[$index]}" \
            "${CORE_LOGS[$index]}" >> "$targets_file"
    done
    watchdog_loop "$targets_file" "$mode_dir/resources.tsv" &
    WATCHDOG_PID=$!
}

summarize_resources() {
    local samples_file="$1"
    local output_file="$2"
    local clock_ticks
    clock_ticks="$(getconf CLK_TCK)"
    awk -F '\t' -v hz="$clock_ticks" '
        NR == 1 { next }
        {
            key=$2
            if (!(key in first_ms)) {
                order[++count]=key
                first_ms[key]=$1
                first_ticks[key]=$4+$5
                pid[key]=$3
            }
            last_ms[key]=$1
            last_ticks[key]=$4+$5
            if ($6 > max_rss[key]) max_rss[key]=$6
            if ($7 > max_fd[key]) max_fd[key]=$7
            if ($8 > max_threads[key]) max_threads[key]=$8
        }
        END {
            printf "["
            for (i=1; i<=count; i++) {
                key=order[i]
                elapsed=(last_ms[key]-first_ms[key])/1000.0
                cpu=elapsed > 0 ? ((last_ticks[key]-first_ticks[key])/hz)/elapsed*100.0 : 0
                if (i > 1) printf ","
                printf "{\"executable\":\"%s\",\"pid\":%d,\"avg_cpu_percent\":%.3f,\"max_rss_kib\":%d,\"max_fd_count\":%d,\"max_thread_count\":%d}", key, pid[key], cpu, max_rss[key], max_fd[key], max_threads[key]
            }
            print "]"
        }
    ' "$samples_file" > "$output_file"
}

start_probe_server() {
    local ns="$1"
    local address="$2"
    local bytes_hint="$3"
    local log_file="$4"
    ip netns exec "$ns" setsid nice -n 5 "$PROBE" server --listen "$address" --sessions 1 \
        --timeout-seconds "$MODE_TIMEOUT_SECONDS" > "$log_file" 2>&1 < /dev/null &
    PROBE_SERVER_PID=$!
    CURRENT_PIDS+=("$PROBE_SERVER_PID")
    sleep 0.15
    kill -0 "$PROBE_SERVER_PID" 2>/dev/null || die "probe server failed to start for $bytes_hint bytes"
}

run_transfer() {
    local mode_dir="$1"
    local direction="$2"
    local port="$3"
    local transfer_bytes="$4"
    local probe_direction="$direction"
    if [[ "$direction" == "warmup" ]]; then
        probe_direction=upload
    fi
    local output_file="$mode_dir/$direction.json"
    local server_log="$mode_dir/probe-$direction.log"
    local tun_budget=$((transfer_bytes * 8 + 67108864))
    local underlay_budget=$((transfer_bytes * 12 + 67108864))

    set_watchdog_phase "transfer-$direction" "$tun_budget" "$underlay_budget"
    start_probe_server "$CURRENT_B" "10.89.0.2:$port" "$transfer_bytes" "$server_log"
    ip netns exec "$CURRENT_A" setsid nice -n 5 timeout "$MODE_TIMEOUT_SECONDS" \
        "$PROBE" client --target "10.89.0.2:$port" --direction "$probe_direction" \
        --bytes "$transfer_bytes" --timeout-seconds "$MODE_TIMEOUT_SECONDS" \
        > "$output_file" 2> "$mode_dir/probe-$direction-client.log" < /dev/null &
    local client_pid=$!
    CURRENT_PIDS+=("$client_pid")
    if ! wait "$client_pid"; then
        die "$CURRENT_MODE $direction client failed"
    fi
    if ! wait "$PROBE_SERVER_PID"; then
        die "$CURRENT_MODE $direction server failed"
    fi
    grep -q '"ok":true' "$output_file" || die "$CURRENT_MODE $direction result was not byte-exact"
    set_watchdog_phase idle 67108864 67108864
}

verify_proxy_transport() {
    local mode_dir="$1"
    local expected="$2"
    local port=39000
    local evidence_bytes=17179869184

    set_watchdog_phase "transport-check-$expected" 1073741824 1610612736
    start_probe_server "$CURRENT_B" "10.89.0.2:$port" "$evidence_bytes" \
        "$mode_dir/probe-transport-check.log"
    ip netns exec "$CURRENT_A" setsid nice -n 5 "$PROBE" client \
        --target "10.89.0.2:$port" --direction upload --bytes "$evidence_bytes" \
        --timeout-seconds 60 > "$mode_dir/transport-check-client.log" 2>&1 < /dev/null &
    local client_pid=$!
    CURRENT_PIDS+=("$client_pid")

    local found=false
    local attempt
    for attempt in $(seq 1 50); do
        ip netns exec "$CURRENT_A" "$CLI" -p 127.0.0.1:15888 -o json proxy \
            > "$mode_dir/proxy.json.tmp" 2> "$mode_dir/proxy-cli.log" || true
        if grep -q "\"transport_type\": \"$expected\"" "$mode_dir/proxy.json.tmp"; then
            mv "$mode_dir/proxy.json.tmp" "$mode_dir/proxy.json"
            found=true
            break
        fi
        sleep 0.1
    done

    kill -TERM -- "-$client_pid" 2>/dev/null || kill -TERM "$client_pid" 2>/dev/null || true
    kill -TERM -- "-$PROBE_SERVER_PID" 2>/dev/null || kill -TERM "$PROBE_SERVER_PID" 2>/dev/null || true
    wait "$client_pid" 2>/dev/null || true
    wait "$PROBE_SERVER_PID" 2>/dev/null || true
    [[ "$found" == true ]] || die "$CURRENT_MODE did not expose transport_type=$expected"
    set_watchdog_phase idle 67108864 67108864
}

setup_direct_topology() {
    local mode="$1"
    local suffix="${HARNESS_PID: -5}"
    CURRENT_A="etmp-${suffix}-${mode}-a"
    CURRENT_B="etmp-${suffix}-${mode}-b"
    new_namespace "$CURRENT_A"
    new_namespace "$CURRENT_B"

    local host_a="em${suffix}a"
    local host_b="em${suffix}b"
    ip link add "$host_a" type veth peer name "$host_b"
    ip link set "$host_a" netns "$CURRENT_A"
    ip link set "$host_b" netns "$CURRENT_B"
    ip -n "$CURRENT_A" link set "$host_a" name under0
    ip -n "$CURRENT_B" link set "$host_b" name under0
    ip -n "$CURRENT_A" addr add 203.0.113.1/30 dev under0
    ip -n "$CURRENT_B" addr add 203.0.113.2/30 dev under0
    ip -n "$CURRENT_A" link set under0 up
    ip -n "$CURRENT_B" link set under0 up
}

setup_relay_topology() {
    local suffix="${HARNESS_PID: -5}"
    CURRENT_A="etmp-${suffix}-relay-a"
    CURRENT_B="etmp-${suffix}-relay-b"
    CURRENT_R="etmp-${suffix}-relay-r"
    new_namespace "$CURRENT_A"
    new_namespace "$CURRENT_B"
    new_namespace "$CURRENT_R"

    local a_host="ea${suffix}a" a_relay="ea${suffix}r"
    local b_host="eb${suffix}b" b_relay="eb${suffix}r"
    ip link add "$a_host" type veth peer name "$a_relay"
    ip link add "$b_host" type veth peer name "$b_relay"
    ip link set "$a_host" netns "$CURRENT_A"
    ip link set "$a_relay" netns "$CURRENT_R"
    ip link set "$b_host" netns "$CURRENT_B"
    ip link set "$b_relay" netns "$CURRENT_R"
    ip -n "$CURRENT_A" link set "$a_host" name under0
    ip -n "$CURRENT_R" link set "$a_relay" name under0
    ip -n "$CURRENT_B" link set "$b_host" name under0
    ip -n "$CURRENT_R" link set "$b_relay" name under1
    ip -n "$CURRENT_A" addr add 203.0.113.1/30 dev under0
    ip -n "$CURRENT_R" addr add 203.0.113.2/30 dev under0
    ip -n "$CURRENT_B" addr add 198.51.100.2/30 dev under0
    ip -n "$CURRENT_R" addr add 198.51.100.1/30 dev under1
    ip -n "$CURRENT_A" link set under0 up
    ip -n "$CURRENT_R" link set under0 up
    ip -n "$CURRENT_B" link set under0 up
    ip -n "$CURRENT_R" link set under1 up
    ip netns exec "$CURRENT_R" sysctl -qw net.ipv4.ip_forward=0
}

write_watchdog_interfaces() {
    : > "$WATCHDOG_INTERFACES"
    printf '%s\tunder0\n' "$CURRENT_A" >> "$WATCHDOG_INTERFACES"
    printf '%s\tunder0\n' "$CURRENT_B" >> "$WATCHDOG_INTERFACES"
    if [[ -n "$CURRENT_R" ]]; then
        printf '%s\tunder0\n%s\tunder1\n' "$CURRENT_R" "$CURRENT_R" >> "$WATCHDOG_INTERFACES"
    fi
}

verify_route() {
    local mode_dir="$1"
    ip netns exec "$CURRENT_A" "$CLI" -p 127.0.0.1:15888 -o json route > "$mode_dir/route.json"
    local flattened
    flattened="$(tr -d '[:space:]' < "$mode_dir/route.json")"
    if [[ "$CURRENT_MODE" == "relay" ]]; then
        [[ "$flattened" =~ \"hostname\":\"perf-b\".*\"next_hop_hostname\":\"perf-r\".*\"path_len\":2 ]] \
            || die "relay route is not a forced two-hop route"
        if ip netns exec "$CURRENT_A" ip route get 198.51.100.2 >/dev/null 2>&1; then
            die "relay client unexpectedly has an underlay route to peer B"
        fi
        if ip netns exec "$CURRENT_B" ip route get 203.0.113.1 >/dev/null 2>&1; then
            die "relay peer B unexpectedly has an underlay route to client A"
        fi
    else
        [[ "$flattened" =~ \"hostname\":\"perf-b\".*\"next_hop_hostname\":\"DIRECT\".*\"path_len\":1 ]] \
            || die "$CURRENT_MODE route is not direct"
    fi
}

run_mode() {
    local mode="$1"
    CURRENT_MODE="$mode"
    local mode_dir="$OUTPUT/$mode"
    mkdir -p "$mode_dir"
    local base expected_transport="native"
    local a_extra_flag=""

    cleanup_mode
    if [[ "$mode" == "relay" ]]; then
        setup_relay_topology
        base=27400
    else
        setup_direct_topology "$mode"
        case "$mode" in
            native) base=27100 ;;
            kcp) base=27200; expected_transport="Kcp"; a_extra_flag="--enable-kcp-proxy" ;;
            quic) base=27300; expected_transport="Quic"; a_extra_flag="--enable-quic-proxy" ;;
        esac
    fi

    local network_name="et-perf-$mode-$HARNESS_PID"
    local common=(--network-name "$network_name" --network-secret isolated-secret \
        --disable-ipv6 true --disable-upnp true --accept-dns false --dev-name tun0)

    if [[ "$mode" == "relay" ]]; then
        start_core "$CURRENT_R" perf-r "$mode_dir/core-r.log" \
            --network-name "$network_name" --network-secret isolated-secret --hostname perf-r \
            --instance-name perf-r --no-tun true --disable-p2p true --relay-network-whitelist "*" \
            --listeners "$(listener_list "$base")" --disable-ipv6 true --disable-upnp true --accept-dns false
        start_core "$CURRENT_B" perf-b "$mode_dir/core-b.log" "${common[@]}" \
            --ipv4 10.89.0.2 --hostname perf-b --instance-name perf-b \
            --peers "tcp://198.51.100.1:$((base + 1))" --disable-p2p true \
            --listeners "$(listener_list "$((base + 10))")"
        start_core "$CURRENT_A" perf-a "$mode_dir/core-a.log" "${common[@]}" \
            --ipv4 10.89.0.1 --hostname perf-a --instance-name perf-a \
            --peers "tcp://203.0.113.2:$((base + 1))" --disable-p2p true \
            --listeners "$(listener_list "$((base + 20))")"
    else
        start_core "$CURRENT_B" perf-b "$mode_dir/core-b.log" "${common[@]}" \
            --ipv4 10.89.0.2 --hostname perf-b --instance-name perf-b \
            --listeners "$(listener_list "$base")"
        if [[ -n "$a_extra_flag" ]]; then
            start_core "$CURRENT_A" perf-a "$mode_dir/core-a.log" "${common[@]}" \
                --ipv4 10.89.0.1 --hostname perf-a --instance-name perf-a \
                --peers "tcp://203.0.113.2:$((base + 1))" \
                --listeners "$(listener_list "$((base + 10))")" "$a_extra_flag" true
        else
            start_core "$CURRENT_A" perf-a "$mode_dir/core-a.log" "${common[@]}" \
                --ipv4 10.89.0.1 --hostname perf-a --instance-name perf-a \
                --peers "tcp://203.0.113.2:$((base + 1))" \
                --listeners "$(listener_list "$((base + 10))")"
        fi
    fi

    wait_for_mesh "$CURRENT_A" 10.89.0.2 || die "$mode mesh did not converge"
    verify_route "$mode_dir"

    WATCHDOG_INTERFACES="$mode_dir/watchdog-interfaces.tsv"
    write_watchdog_interfaces
    start_watchdog "$mode_dir"
    set_watchdog_phase idle 67108864 67108864
    sleep 2.25

    if [[ "$expected_transport" != "native" ]]; then
        verify_proxy_transport "$mode_dir" "$expected_transport"
    fi

    run_transfer "$mode_dir" warmup 39001 4194304
    local tun_before tun_after underlay_before underlay_after
    tun_before="$(tun_bytes)"
    underlay_before="$(interface_bytes)"
    run_transfer "$mode_dir" upload 39002 "$BYTES"
    run_transfer "$mode_dir" download 39003 "$BYTES"
    tun_after="$(tun_bytes)"
    underlay_after="$(interface_bytes)"

    local pid
    for pid in "${CORE_PIDS[@]}"; do
        kill -0 "$pid" 2>/dev/null || die "$mode core exited after transfer"
    done

    stop_watchdog
    summarize_resources "$mode_dir/resources.tsv" "$mode_dir/resources.json"
    local tun_delta=$((tun_after - tun_before))
    local underlay_delta=$((underlay_after - underlay_before))
    local gross_limit=$((BYTES * 12 + 134217728))
    (( tun_delta > BYTES * 2 )) || die "$mode did not traverse both mesh TUN endpoints"
    (( tun_delta <= gross_limit )) || die "$mode TUN traffic exceeded the gross amplification bound"
    (( underlay_delta <= gross_limit )) || die "$mode underlay traffic exceeded the gross amplification bound"

    cat > "$mode_dir/summary.json" <<EOF
{
  "schema_version": 1,
  "mode": "$mode",
  "topology": "$([[ "$mode" == "relay" ]] && printf forced-relay || printf direct-peer)",
  "expected_transport": "$expected_transport",
  "transfer_bytes_per_direction": $BYTES,
  "upload": $(cat "$mode_dir/upload.json"),
  "download": $(cat "$mode_dir/download.json"),
  "tun_bytes_during_measured_transfers": $tun_delta,
  "underlay_bytes_during_measured_transfers": $underlay_delta,
  "resources": $(cat "$mode_dir/resources.json"),
  "safety_limits": {
    "max_process_rss_kib": $MAX_PROCESS_RSS_KIB,
    "max_total_rss_kib": $MAX_TOTAL_RSS_KIB,
    "max_fds": $MAX_FDS,
    "max_threads": $MAX_THREADS,
    "max_log_bytes": $MAX_LOG_BYTES,
    "idle_process_cpu_percent": $IDLE_PROCESS_CPU_PERCENT,
    "stalled_total_cpu_percent": $STALLED_TOTAL_CPU_PERCENT
  },
  "gates": {
    "route_verified": true,
    "transport_verified": true,
    "byte_exact": true,
    "resource_watchdog_passed": true,
    "amplification_bound_passed": true,
    "cores_survived": true
  }
}
EOF

    local namespaces=("${CURRENT_NAMESPACES[@]}")
    cleanup_mode
    local ns
    for ns in "${namespaces[@]}"; do
        ip netns list | awk '{print $1}' | grep -Fxq "$ns" && die "$mode namespace cleanup failed: $ns"
    done
    return 0
}

MODE_SUMMARIES=()
for mode in "${REQUESTED_MODES[@]}"; do
    run_mode "$mode"
    MODE_SUMMARIES+=("$OUTPUT/$mode/summary.json")
done

CURRENT_MODE="final-cleanup"
capture_host_state "$OUTPUT/host-state.after"
if ! cmp -s "$OUTPUT/host-state.before" "$OUTPUT/host-state.after"; then
    diff -u "$OUTPUT/host-state.before" "$OUTPUT/host-state.after" > "$OUTPUT/host-state.diff" || true
    die "host network state changed"
fi

{
    cat <<EOF
{
  "schema_version": 1,
  "platform": "linux",
  "isolated_network_namespaces": true,
  "host_firewall_modified": false,
  "host_forwarding_modified": false,
  "host_state_unchanged": true,
  "modes": [
EOF
    for index in "${!MODE_SUMMARIES[@]}"; do
        (( index == 0 )) || printf ',\n'
        cat "${MODE_SUMMARIES[$index]}"
    done
    cat <<'EOF'
  ],
  "gates": {
    "all_requested_modes_passed": true,
    "watchdog_never_triggered": true,
    "namespace_cleanup_complete": true,
    "host_state_unchanged": true
  }
}
EOF
} > "$OUTPUT/summary.json"

rm -f "$OUTPUT"/*/watchdog.stop "$OUTPUT"/*/watchdog-phase.tsv
trap - EXIT TERM INT
printf 'mesh performance self-test passed: %s/summary.json\n' "$OUTPUT"
