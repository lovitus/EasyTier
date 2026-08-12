#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
CARGO_BIN="${CARGO_BIN:-cargo}"
export CARGO_INCREMENTAL=0

cd "$REPO_ROOT"
printf '[formal-rust-static-checks] formatting\n'
"$CARGO_BIN" fmt --all -- --check
printf '[formal-rust-static-checks] clippy\n'
"$CARGO_BIN" clippy --locked --all-targets --features full --all -- -D warnings
printf '[formal-rust-static-checks] feature matrix\n'
"$CARGO_BIN" hack check --locked --package easytier --each-feature \
  --exclude-features macos-ne --verbose
printf '[formal-rust-static-checks] Cargo.lock\n'
if ! "$CARGO_BIN" metadata --format-version 1 --locked >/dev/null; then
  printf 'Cargo.lock is out of date\n' >&2
  exit 1
fi
printf '[formal-rust-static-checks] complete\n'
