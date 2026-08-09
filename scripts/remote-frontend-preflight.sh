#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BUILDER_HOST="${BUILDER_HOST:-root@192.168.2.160}"
BUILDER_CONTAINER="${BUILDER_CONTAINER:-easytier-debug-builder}"
REMOTE_WORKSPACE="${BUILDER_CONTAINER_WORKSPACE:-/workspace}"
FRONTEND_LOCK_ID="$(git -C "$REPO_ROOT" hash-object pnpm-lock.yaml)"
REMOTE_COREPACK_HOME="$REMOTE_WORKSPACE/.corepack-$FRONTEND_LOCK_ID"
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

run_step() {
  local label="$1"
  local timeout_seconds="$2"
  local command="$3"
  local log_file="/tmp/easytier_frontend_${label}.log"

  printf '[remote-frontend-preflight] %s\n' "$label"
  if ! ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'cd $REMOTE_WORKSPACE && \
export PATH=/opt/node22/bin:\$PATH && \
export COREPACK_HOME=$REMOTE_COREPACK_HOME && \
export HTTP_PROXY=http://127.0.0.1:7890 HTTPS_PROXY=http://127.0.0.1:7890 && \
export http_proxy=http://127.0.0.1:7890 https_proxy=http://127.0.0.1:7890 && \
CI=1 timeout $timeout_seconds $command > $log_file 2>&1'"; then
    ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
      "docker exec $BUILDER_CONTAINER tail -80 '$log_file'" || true
    printf '[remote-frontend-preflight] %s failed\n' "$label" >&2
    exit 1
  fi

  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER tail -20 '$log_file'"
}

"$SCRIPT_DIR/remote-builder-sync.sh" --frontend-deps
run_step frontend_lib_vitest 600 \
  "pnpm --dir easytier-web/frontend-lib test:config-ui"
run_step frontend_lib_build 900 \
  "pnpm --dir easytier-web/frontend-lib build"
run_step frontend_build 900 \
  "pnpm --dir easytier-web/frontend build"
run_step vpnservice_build 600 \
  "pnpm --dir tauri-plugin-vpnservice build"
run_step gui_build 900 \
  "pnpm --dir easytier-gui build"

printf '[remote-frontend-preflight] complete\n'
