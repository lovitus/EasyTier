#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BUILDER_HOST="${BUILDER_HOST:-root@192.168.2.160}"
BUILDER_CONTAINER="${BUILDER_CONTAINER:-easytier-debug-builder}"
BUILDER_HOST_WORKSPACE="${BUILDER_HOST_WORKSPACE:-/data/easytier-builder/workspace}"
BUILDER_CONTAINER_WORKSPACE="${BUILDER_CONTAINER_WORKSPACE:-/workspace}"
BUILDER_STATE_DIR="${BUILDER_STATE_DIR:-/data/easytier-builder/state}"
STATIC_TIMEOUT="${STATIC_TIMEOUT:-3600}"
STATIC_MIN_FREE_GIB="${STATIC_MIN_FREE_GIB:-8}"
LOG_DIR="$BUILDER_HOST_WORKSPACE/.validation-logs"
LOG_FILE="$LOG_DIR/formal-static-preflight.log"
LOCK_FILE="$BUILDER_STATE_DIR/release-preflight.lock"
SSH_OPTIONS=(
  -o ServerAliveInterval=30
  -o ServerAliveCountMax=3
  -o ConnectTimeout=10
)
BUILD_SSH_OPTIONS=(
  "${SSH_OPTIONS[@]}"
  -o ExitOnForwardFailure=yes
  -R 7890:127.0.0.1:7890
)

inside_builder() {
  cd "$BUILDER_CONTAINER_WORKSPACE"
  export PATH="/usr/local/cargo/bin:/usr/local/rustup/toolchains/1.95-x86_64-unknown-linux-gnu/bin:/usr/local/bin:/usr/bin:/bin"
  export CARGO_BIN=/usr/local/cargo/bin/cargo
  export RUSTC_BIN=/usr/local/cargo/bin/rustc
  export CARGO_INCREMENTAL=0
  for command_path in "$CARGO_BIN" "$RUSTC_BIN" /usr/bin/curl /usr/bin/pgrep \
    /usr/bin/timeout; do
    test -x "$command_path"
  done
  command -v python3 >/dev/null
  command -v cargo-hack >/dev/null

  if /usr/bin/pgrep -x cargo >/dev/null || /usr/bin/pgrep -x rustc >/dev/null; then
    printf 'refusing to compete with an existing Cargo or rustc process\n' >&2
    /usr/bin/pgrep -a -x cargo >&2 || true
    /usr/bin/pgrep -a -x rustc >&2 || true
    exit 75
  fi

  target_dir="$BUILDER_CONTAINER_WORKSPACE/target"
  incremental_dir="$target_dir/debug/incremental"
  minimum_free_kib=$((STATIC_MIN_FREE_GIB * 1024 * 1024))
  available_kib="$(df -Pk "$target_dir" | awk 'NR == 2 { print $4 }')"
  [[ "$available_kib" =~ ^[0-9]+$ ]]
  if ((available_kib < minimum_free_kib)) && [[ -d "$incremental_dir" ]]; then
    printf '[remote-formal-static-preflight] pruning only %s (%s KiB free)\n' \
      "$incremental_dir" "$available_kib"
    rm -rf -- "$incremental_dir"
    available_kib="$(df -Pk "$target_dir" | awk 'NR == 2 { print $4 }')"
  fi
  if [[ ! "$available_kib" =~ ^[0-9]+$ ]] || ((available_kib < minimum_free_kib)); then
    printf 'builder has %s KiB free; at least %s GiB is required without deleting dependencies\n' \
      "$available_kib" "$STATIC_MIN_FREE_GIB" >&2
    exit 28
  fi

  /usr/bin/curl --fail --silent --show-error --output /dev/null \
    --connect-timeout 10 --max-time 30 https://api.github.com/rate_limit

  manifest_dir=target/mihomo-release
  manifest="$manifest_dir/mihomo-release-manifest.json"
  mkdir -p "$manifest_dir"
  if [[ ! -f "$manifest" ]] ||
    ! python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["resolved"] is True' \
      "$manifest" >/dev/null 2>&1; then
    bash scripts/fetch-mihomo-release.sh resolve-latest "$manifest"
  fi

  MIHOMO_RELEASE_MANIFEST="$BUILDER_CONTAINER_WORKSPACE/$manifest" \
    RUSTC_BIN="$RUSTC_BIN" \
    STATIC_SIDECAR_CACHE_DIR="$BUILDER_CONTAINER_WORKSPACE/target/static-sidecar-cache" \
    bash scripts/prepare-gui-static-sidecars.sh
  CARGO_BIN="$CARGO_BIN" bash scripts/formal-rust-static-checks.sh
}

if [[ "${1:-}" == "--inside-builder" ]]; then
  inside_builder
  exit 0
fi

if [[ $# -ne 0 ]]; then
  printf 'usage: %s\n' "$0" >&2
  exit 2
fi

if ! /usr/bin/nc -z 127.0.0.1 7890; then
  printf 'local proxy 127.0.0.1:7890 is unavailable; refusing an unproxied builder run\n' >&2
  exit 69
fi

ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
  "mkdir -p '$BUILDER_STATE_DIR' '$LOG_DIR'; \
/usr/bin/flock -n -E 75 '$LOCK_FILE' true" || {
  printf 'remote builder is unavailable or its release preflight lock is busy\n' >&2
  exit 75
}

env BUILDER_HOST="$BUILDER_HOST" \
  BUILDER_CONTAINER="$BUILDER_CONTAINER" \
  BUILDER_HOST_WORKSPACE="$BUILDER_HOST_WORKSPACE" \
  BUILDER_CONTAINER_WORKSPACE="$BUILDER_CONTAINER_WORKSPACE" \
  BUILDER_STATE_DIR="$BUILDER_STATE_DIR" \
  "$SCRIPT_DIR/remote-builder-sync.sh" --source

printf '[remote-formal-static-preflight] workflow-equivalent static checks\n'
exit_code=0
ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
  "set -uo pipefail; \
mkdir -p '$BUILDER_STATE_DIR' '$LOG_DIR'; \
/usr/bin/flock -n -E 75 '$LOCK_FILE' \
  /usr/bin/timeout --signal=TERM --kill-after=30s '$((STATIC_TIMEOUT + 90))' \
  /usr/bin/docker exec \
    -e HTTP_PROXY=http://127.0.0.1:7890 \
    -e HTTPS_PROXY=http://127.0.0.1:7890 \
    -e http_proxy=http://127.0.0.1:7890 \
    -e https_proxy=http://127.0.0.1:7890 \
    -e CARGO_NET_GIT_FETCH_WITH_CLI=true \
    -e CARGO_BUILD_JOBS=\$(nproc) \
    -e CARGO_INCREMENTAL=0 \
    -e BUILDER_CONTAINER_WORKSPACE='$BUILDER_CONTAINER_WORKSPACE' \
    -e STATIC_MIN_FREE_GIB='$STATIC_MIN_FREE_GIB' \
    '$BUILDER_CONTAINER' \
    /usr/bin/timeout --signal=TERM --kill-after=30s '$STATIC_TIMEOUT' \
    /bin/bash '$BUILDER_CONTAINER_WORKSPACE/scripts/remote-formal-static-preflight.sh' \
    --inside-builder > '$LOG_FILE' 2>&1; \
code=\$?; \
case \$code in \
  0) ;; \
  75) printf 'remote builder lock is busy or another compiler is active\\n' >&2 ;; \
  124|137) printf 'formal static preflight exceeded its bounded timeout\\n' >&2 ;; \
  *) printf 'formal static preflight failed; see %s\\n' '$LOG_FILE' >&2 ;; \
esac; \
exit \$code" || exit_code=$?

ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" "tail -120 '$LOG_FILE'" || true
if ((exit_code != 0)); then
  printf '[remote-formal-static-preflight] failed with exit code %d\n' "$exit_code" >&2
  exit "$exit_code"
fi
printf '[remote-formal-static-preflight] complete\n'
