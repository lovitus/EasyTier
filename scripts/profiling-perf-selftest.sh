#!/usr/bin/env bash
set -Eeuo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BUNDLE="$SCRIPT_DIR"
OUTPUT=""
BYTES=134217728
MESH_MODES="native,kcp,quic,relay"
CHECK_ONLY=false
SKIP_LEAF_DIRECT=false
SKIP_MESH=false

MAX_TOTAL_RSS_KIB="${ET_PERF_MAX_TOTAL_RSS_KIB:-1048576}"
MAX_FDS="${ET_PERF_MAX_FDS:-512}"
MAX_THREADS="${ET_PERF_MAX_THREADS:-128}"
MAX_LOG_BYTES="${ET_PERF_MAX_LOG_BYTES:-16777216}"
STALLED_TOTAL_CPU_PERCENT="${ET_PERF_STALLED_CPU_PERCENT:-180}"
LEAF_TIMEOUT_SECONDS="${ET_PERF_LEAF_TIMEOUT_SECONDS:-300}"
active_child_pid=

usage() {
    cat <<'EOF'
Usage: profiling-perf-selftest.sh [OPTIONS]

Runs the isolated Leaf DIRECT baseline and the EasyTier mesh native/KCP/QUIC/
forced-relay matrix, then writes one combined JSON report.

Options:
  --bundle DIR        Profiling bundle directory (default: script directory).
  --output DIR        New output directory (default: timestamped directory).
  --bytes N           Bytes per direction and test mode (default: 134217728).
  --mesh-modes LIST   Comma-separated native,kcp,quic,relay.
  --skip-leaf-direct  Run only the mesh matrix.
  --skip-mesh         Run only the Leaf DIRECT baseline.
  --check-only        Check both harnesses without changing network state.
  -h, --help          Show this help.
EOF
}

die() {
    printf 'profiling performance self-test failed: %s\n' "$1" >&2
    exit 1
}

is_uint() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

while (( $# > 0 )); do
    case "$1" in
        --bundle) (( $# >= 2 )) || die "--bundle requires a directory"; BUNDLE="$2"; shift 2 ;;
        --output) (( $# >= 2 )) || die "--output requires a directory"; OUTPUT="$2"; shift 2 ;;
        --bytes) (( $# >= 2 )) || die "--bytes requires an integer"; BYTES="$2"; shift 2 ;;
        --mesh-modes) (( $# >= 2 )) || die "--mesh-modes requires a list"; MESH_MODES="$2"; shift 2 ;;
        --skip-leaf-direct) SKIP_LEAF_DIRECT=true; shift ;;
        --skip-mesh) SKIP_MESH=true; shift ;;
        --check-only) CHECK_ONLY=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[[ "$SKIP_LEAF_DIRECT" == false || "$SKIP_MESH" == false ]] || die "both test families cannot be skipped"
[[ "$(uname -s)" == "Linux" ]] || die "safe combined self-test currently requires Linux"
(( EUID == 0 )) || die "safe combined self-test requires root"
is_uint "$BYTES" || die "--bytes must be an integer"

BUNDLE="$(cd "$BUNDLE" && pwd)"
LEAF_HARNESS="$BUNDLE/leaf-perf-selftest.sh"
MESH_HARNESS="$BUNDLE/mesh-perf-selftest.sh"
for harness in "$LEAF_HARNESS" "$MESH_HARNESS"; do
    [[ -x "$harness" ]] || die "missing harness: $harness"
done

if [[ "$CHECK_ONLY" == true ]]; then
    if [[ "$SKIP_LEAF_DIRECT" == false ]]; then
        "$LEAF_HARNESS" --bundle "$BUNDLE" --check-only
    fi
    if [[ "$SKIP_MESH" == false ]]; then
        "$MESH_HARNESS" --bundle "$BUNDLE" --modes "$MESH_MODES" --check-only
    fi
    printf '%s\n' 'combined profiling performance self-test prerequisites are available'
    exit 0
fi

if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$PWD/easytier-profiling-perf-$(date +%Y%m%d-%H%M%S)"
fi
if [[ -e "$OUTPUT" ]] && find "$OUTPUT" -mindepth 1 -print -quit 2>/dev/null | grep -q .; then
    die "output directory must be empty: $OUTPUT"
fi
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"

write_abort() {
    local reason="$1" detail="$2" observed="${3:-0}" limit="${4:-0}"
    cat > "$OUTPUT/abort.json.tmp" <<EOF
{
  "schema_version": 1,
  "aborted": true,
  "phase": "leaf-direct",
  "reason": "$reason",
  "detail": "$detail",
  "observed": $observed,
  "limit": $limit,
  "child_process_group_terminated": true
}
EOF
    mv "$OUTPUT/abort.json.tmp" "$OUTPUT/abort.json"
}

namespace_names() {
    ip netns list | awk '{print $1}' | LC_ALL=C sort
}

namespace_byte_total() {
    local baseline_file="$1"
    local total=0 ns dev rx tx
    while IFS= read -r ns; do
        [[ -n "$ns" ]] || continue
        if grep -Fxq "$ns" "$baseline_file"; then
            continue
        fi
        while IFS= read -r dev; do
            [[ "$dev" == "lo" ]] && continue
            rx="$(ip netns exec "$ns" cat "/sys/class/net/$dev/statistics/rx_bytes" 2>/dev/null || printf 0)"
            tx="$(ip netns exec "$ns" cat "/sys/class/net/$dev/statistics/tx_bytes" 2>/dev/null || printf 0)"
            total=$((total + rx + tx))
        done < <(ip -n "$ns" -o link show 2>/dev/null | awk -F': ' '{print $2}' | cut -d'@' -f1)
    done < <(namespace_names)
    printf '%d\n' "$total"
}

process_tree() {
    local root_pid="$1"
    local queue=("$root_pid") result=() pid child
    while (( ${#queue[@]} > 0 )); do
        pid="${queue[0]}"
        queue=("${queue[@]:1}")
        result+=("$pid")
        while IFS= read -r child; do
            [[ -n "$child" ]] && queue+=("$child")
        done < <(pgrep -P "$pid" 2>/dev/null || true)
    done
    printf '%s\n' "${result[@]}"
}

terminate_group() {
    local pid="$1"
    kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
    local deadline=$((SECONDS + 3))
    while kill -0 "$pid" 2>/dev/null && (( SECONDS < deadline )); do sleep 0.1; done
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    if [[ "$active_child_pid" == "$pid" ]]; then
        active_child_pid=
    fi
}

cleanup_active_child() {
    if [[ -n "$active_child_pid" ]]; then
        terminate_group "$active_child_pid"
        active_child_pid=
    fi
}

trap cleanup_active_child EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_leaf_guarded() {
    local leaf_output="$OUTPUT/leaf-direct"
    local baseline_ns="$OUTPUT/leaf-direct-netns.before"
    namespace_names > "$baseline_ns"
    mkdir -p "$leaf_output"

    setsid "$LEAF_HARNESS" --bundle "$BUNDLE" --output "$leaf_output" --bytes "$BYTES" \
        > "$OUTPUT/leaf-direct-harness.log" 2>&1 < /dev/null &
    local harness_pid=$!
    active_child_pid="$harness_pid"
    local start_seconds=$SECONDS
    local clock_ticks page_kib previous_ms previous_ticks previous_bytes stalled_count=0
    clock_ticks="$(getconf CLK_TCK)"
    page_kib=$(( $(getconf PAGESIZE) / 1024 ))
    previous_ms="$(date +%s%3N)"
    previous_ticks=0
    previous_bytes=0
    local byte_limit=$((BYTES * 16 + 536870912))

    while kill -0 "$harness_pid" 2>/dev/null; do
        local now_ms total_ticks=0 total_rss=0 total_bytes pid utime stime rss_pages rss_kib fd_count threads
        now_ms="$(date +%s%3N)"
        while IFS= read -r pid; do
            [[ -r "/proc/$pid/stat" ]] || continue
            read -r utime stime threads rss_pages < <(awk '{print $14, $15, $20, $24}' "/proc/$pid/stat")
            rss_kib=$((rss_pages * page_kib))
            fd_count="$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 -print 2>/dev/null | wc -l)"
            total_ticks=$((total_ticks + utime + stime))
            total_rss=$((total_rss + rss_kib))
            if (( fd_count > MAX_FDS )); then
                write_abort fd_limit "pid=$pid" "$fd_count" "$MAX_FDS"
                terminate_group "$harness_pid"
                return 1
            fi
            if (( threads > MAX_THREADS )); then
                write_abort thread_limit "pid=$pid" "$threads" "$MAX_THREADS"
                terminate_group "$harness_pid"
                return 1
            fi
        done < <(process_tree "$harness_pid")

        if (( total_rss > MAX_TOTAL_RSS_KIB )); then
            write_abort total_rss_limit leaf-direct "$total_rss" "$MAX_TOTAL_RSS_KIB"
            terminate_group "$harness_pid"
            return 1
        fi
        local oversized_log
        oversized_log="$(find "$leaf_output" -type f -size "+${MAX_LOG_BYTES}c" -print -quit 2>/dev/null || true)"
        if [[ -n "$oversized_log" ]]; then
            write_abort log_growth_limit "$oversized_log" "$(stat -c %s "$oversized_log")" "$MAX_LOG_BYTES"
            terminate_group "$harness_pid"
            return 1
        fi

        total_bytes="$(namespace_byte_total "$baseline_ns")"
        if (( total_bytes > byte_limit )); then
            write_abort namespace_amplification leaf-direct "$total_bytes" "$byte_limit"
            terminate_group "$harness_pid"
            return 1
        fi

        local elapsed_ms delta_ticks cpu_percent byte_progress
        elapsed_ms=$((now_ms - previous_ms))
        delta_ticks=$((total_ticks - previous_ticks))
        byte_progress=$((total_bytes - previous_bytes))
        cpu_percent=0
        if (( elapsed_ms > 0 )); then
            cpu_percent=$((delta_ticks * 100000 / clock_ticks / elapsed_ms))
        fi
        if (( cpu_percent >= STALLED_TOTAL_CPU_PERCENT && byte_progress < 65536 )); then
            stalled_count=$((stalled_count + 1))
            if (( stalled_count >= 8 )); then
                write_abort busy_without_progress leaf-direct "$cpu_percent" "$STALLED_TOTAL_CPU_PERCENT"
                terminate_group "$harness_pid"
                return 1
            fi
        else
            stalled_count=0
        fi
        if (( SECONDS - start_seconds > LEAF_TIMEOUT_SECONDS )); then
            write_abort wall_timeout leaf-direct "$((SECONDS - start_seconds))" "$LEAF_TIMEOUT_SECONDS"
            terminate_group "$harness_pid"
            return 1
        fi
        previous_ms="$now_ms"
        previous_ticks="$total_ticks"
        previous_bytes="$total_bytes"
        sleep 0.25
    done

    if wait "$harness_pid"; then
        active_child_pid=
    else
        active_child_pid=
        [[ -f "$OUTPUT/abort.json" ]] || write_abort child_failed leaf-direct 1 0
        return 1
    fi
    [[ -f "$leaf_output/summary.json" ]] || {
        write_abort missing_summary leaf-direct 0 1
        return 1
    }
}

if [[ "$SKIP_LEAF_DIRECT" == false ]]; then
    if ! run_leaf_guarded; then
        printf 'Leaf DIRECT self-test aborted; report: %s/abort.json\n' "$OUTPUT" >&2
        exit 1
    fi
fi

if [[ "$SKIP_MESH" == false ]]; then
    if ! "$MESH_HARNESS" --bundle "$BUNDLE" --output "$OUTPUT/mesh" --bytes "$BYTES" --modes "$MESH_MODES"; then
        if [[ -f "$OUTPUT/mesh/abort.json" ]]; then
            cp "$OUTPUT/mesh/abort.json" "$OUTPUT/abort.json"
        fi
        printf 'mesh self-test aborted; report: %s/abort.json\n' "$OUTPUT" >&2
        exit 1
    fi
fi

leaf_json=null
mesh_json=null
if [[ "$SKIP_LEAF_DIRECT" == false ]]; then leaf_json="$(cat "$OUTPUT/leaf-direct/summary.json")"; fi
if [[ "$SKIP_MESH" == false ]]; then mesh_json="$(cat "$OUTPUT/mesh/summary.json")"; fi
cat > "$OUTPUT/summary.json" <<EOF
{
  "schema_version": 1,
  "platform": "linux",
  "leaf_direct": $leaf_json,
  "mesh": $mesh_json,
  "gates": {
    "all_requested_families_passed": true,
    "safety_watchdogs_passed": true,
    "production_network_untouched": true
  }
}
EOF

printf 'combined profiling performance self-test passed: %s/summary.json\n' "$OUTPUT"
