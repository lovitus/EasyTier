#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

BUILDER_HOST="${BUILDER_HOST:-root@192.168.2.160}"
BUILDER_CONTAINER="${BUILDER_CONTAINER:-easytier-debug-builder}"
BUILDER_HOST_WORKSPACE="${BUILDER_HOST_WORKSPACE:-/data/easytier-builder/workspace}"
BUILDER_CONTAINER_WORKSPACE="${BUILDER_CONTAINER_WORKSPACE:-/workspace}"
BUILDER_STATE_DIR="${BUILDER_STATE_DIR:-/data/easytier-builder/state}"
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

usage() {
  cat <<'EOF'
Usage: scripts/remote-builder-sync.sh [MODE]

Modes:
  --source                 Safely synchronize source only (default).
  --frontend-deps          Synchronize source and incrementally install the
                           exact pnpm lockfile when needed.
  --repair-frontend-deps   Synchronize source, remove only the known remote
                           workspace node_modules directories, then reinstall.

The synchronizer never deletes Cargo target/registry, Corepack data, or
node_modules. Source deletion is delayed until a complete rsync transfer.
EOF
}

mode="source"
case "${1:---source}" in
  --source) mode="source" ;;
  --frontend-deps) mode="frontend" ;;
  --repair-frontend-deps) mode="repair-frontend" ;;
  -h|--help) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac

log() {
  printf '[remote-builder-sync] %s\n' "$*"
}

check_builder_idle() {
  local output
  output="$(ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'if pgrep -x cargo >/dev/null || pgrep -x rustc >/dev/null || pgrep -x node >/dev/null || pgrep -x pnpm >/dev/null; then pgrep -a -x cargo || true; pgrep -a -x rustc || true; pgrep -a -x node || true; pgrep -a -x pnpm || true; echo BLOCKED; else echo CLEAR; fi'")"
  if [[ "$output" != *CLEAR* || "$output" == *BLOCKED* ]]; then
    printf '%s\n' "$output" >&2
    printf 'remote builder is busy; source synchronization was not started\n' >&2
    exit 1
  fi
}

sync_source() {
  log "checking builder before modifying $BUILDER_HOST_WORKSPACE"
  check_builder_idle

  log "synchronizing source with dependency and build caches protected"
  rsync -a --delete-delay \
    --filter='protect /target/***' \
    --filter='protect /.corepack*/***' \
    --filter='protect **/node_modules/***' \
    --exclude '/.git/' \
    --exclude '/target/' \
    --exclude '/.artifacts/' \
    --exclude '/.codex-artifacts/' \
    --exclude '/.claude/' \
    --exclude '/.corepack*/' \
    --exclude '/.envrc.local' \
    --exclude 'node_modules/' \
    --exclude '/easytier-gui/src-tauri/gen/' \
    --exclude '/easytier-gui/src-tauri/.gradle/' \
    -e "ssh ${SSH_OPTIONS[*]}" \
    "$REPO_ROOT/" "$BUILDER_HOST:$BUILDER_HOST_WORKSPACE/"

  check_builder_idle
}

frontend_lock_id() {
  git -C "$REPO_ROOT" hash-object pnpm-lock.yaml
}

installed_frontend_lock_id() {
  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "cat '$BUILDER_STATE_DIR/pnpm-lock.git-blob' 2>/dev/null || true"
}

remove_known_frontend_dependencies() {
  log "removing only the documented remote frontend dependency directories"
  ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'rm -rf -- \
$BUILDER_CONTAINER_WORKSPACE/node_modules \
$BUILDER_CONTAINER_WORKSPACE/easytier-web/node_modules \
$BUILDER_CONTAINER_WORKSPACE/easytier-web/frontend/node_modules \
$BUILDER_CONTAINER_WORKSPACE/easytier-web/frontend-lib/node_modules \
$BUILDER_CONTAINER_WORKSPACE/easytier-gui/node_modules \
$BUILDER_CONTAINER_WORKSPACE/tauri-plugin-vpnservice/node_modules'"
}

install_frontend_dependencies() {
  local lock_id="$1"
  local installed_id
  local corepack_home="$BUILDER_CONTAINER_WORKSPACE/.corepack-$lock_id"
  installed_id="$(installed_frontend_lock_id)"

  if [[ "$mode" != "repair-frontend" && "$installed_id" == "$lock_id" ]]; then
    if ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
      "docker exec $BUILDER_CONTAINER test -d '$BUILDER_CONTAINER_WORKSPACE/node_modules/.pnpm'"; then
      log "frontend dependencies already match pnpm-lock.yaml"
      return
    fi
  fi

  if [[ "$mode" == "repair-frontend" ]]; then
    remove_known_frontend_dependencies
  fi

  log "installing frontend dependencies incrementally from the frozen lockfile"
  if ! ssh "${BUILD_SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "docker exec $BUILDER_CONTAINER bash -c 'cd $BUILDER_CONTAINER_WORKSPACE && \
export PATH=/opt/node22/bin:\$PATH && \
export COREPACK_HOME=$corepack_home && \
export HTTP_PROXY=http://127.0.0.1:7890 HTTPS_PROXY=http://127.0.0.1:7890 && \
export http_proxy=http://127.0.0.1:7890 https_proxy=http://127.0.0.1:7890 && \
CI=1 timeout 1200 pnpm install --frozen-lockfile \
> /tmp/easytier_frontend_dependencies.log 2>&1'"; then
    ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
      "docker exec $BUILDER_CONTAINER tail -80 /tmp/easytier_frontend_dependencies.log" || true
    printf 'incremental frontend dependency installation failed' >&2
    if [[ "$mode" != "repair-frontend" ]]; then
      printf '; inspect the log, then use builder-frontend-repair only if dependency state is corrupt' >&2
    fi
    printf '\n' >&2
    exit 1
  fi

  ssh "${SSH_OPTIONS[@]}" "$BUILDER_HOST" \
    "mkdir -p '$BUILDER_STATE_DIR' && printf '%s\\n' '$lock_id' > '$BUILDER_STATE_DIR/pnpm-lock.git-blob'"
  log "frontend dependency state recorded as $lock_id"
}

sync_source
if [[ "$mode" != "source" ]]; then
  install_frontend_dependencies "$(frontend_lock_id)"
fi

log "complete"
