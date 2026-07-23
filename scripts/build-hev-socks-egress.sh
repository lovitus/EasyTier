#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 TARGET OUTPUT" >&2
  exit 2
fi

target=$1
output=$2
: "${HEV_SERVER_COMMIT:?HEV_SERVER_COMMIT must pin the HEV source revision}"
: "${RUNNER_TEMP:?RUNNER_TEMP must provide an isolated build directory}"

source_dir="$RUNNER_TEMP/hev-socks5-server-$target"
library_dir="$RUNNER_TEMP/hev-libs-$target"
cargo_target_dir="$RUNNER_TEMP/hev-cargo-target-$target"
if [[ -e $source_dir ]]; then
  echo "refusing to reuse HEV source directory: $source_dir" >&2
  exit 1
fi

git init "$source_dir"
git -C "$source_dir" remote add origin https://github.com/lovitus/hev-socks5-server.git
git -C "$source_dir" fetch --depth=1 origin "$HEV_SERVER_COMMIT"
git -C "$source_dir" checkout --detach FETCH_HEAD
git -C "$source_dir" submodule update --init --recursive --depth=1
test "$(git -C "$source_dir" rev-parse HEAD)" = "$HEV_SERVER_COMMIT"

if command -v nproc >/dev/null 2>&1; then
  jobs=$(nproc)
else
  jobs=$(sysctl -n hw.ncpu)
fi

case $target in
  x86_64-unknown-linux-musl | aarch64-unknown-linux-musl)
    make -C "$source_dir" -j"$jobs" static \
      CC=musl-gcc \
      PP='musl-gcc -E' \
      ENABLE_STATIC=1
    ;;
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu)
    make -C "$source_dir" -j"$jobs" static \
      CC=cc \
      PP='cc -E'
    ;;
  x86_64-apple-darwin)
    make -C "$source_dir" -j"$jobs" static \
      CC='xcrun --sdk macosx --toolchain macosx clang' \
      PP='xcrun --sdk macosx --toolchain macosx clang -E' \
      CFLAGS='-arch x86_64 -mmacosx-version-min=10.14' \
      LFLAGS='-arch x86_64 -mmacosx-version-min=10.14'
    ;;
  aarch64-apple-darwin)
    make -C "$source_dir" -j"$jobs" static \
      CC='xcrun --sdk macosx --toolchain macosx clang' \
      PP='xcrun --sdk macosx --toolchain macosx clang -E' \
      CFLAGS='-arch arm64 -mmacosx-version-min=11.0' \
      LFLAGS='-arch arm64 -mmacosx-version-min=11.0'
    ;;
  *)
    echo "unsupported HEV release target: $target" >&2
    exit 2
    ;;
esac

mkdir -p "$library_dir"
cp "$source_dir/bin/libhev-socks5-server.a" "$library_dir/"
cp "$source_dir/third-part/yaml/bin/libyaml.a" "$library_dir/"
cp "$source_dir/third-part/hev-task-system/bin/libhev-task-system.a" "$library_dir/"

HEV_SOCKS5_LIB_DIR="$library_dir" \
HEV_SERVER_COMMIT="$HEV_SERVER_COMMIT" \
CARGO_TARGET_DIR="$cargo_target_dir" \
  cargo build --locked --release --target "$target" \
    --package easytier-socks-egress \
    --bin easytier-hev-socks-egress \
    --features hev-sidecar-bin

mkdir -p "$(dirname "$output")"
install -m 0755 "$cargo_target_dir/$target/release/easytier-hev-socks-egress" "$output"
"$output" --version > "$RUNNER_TEMP/hev-version-$target.txt" 2>&1 || true
grep -q "${HEV_SERVER_COMMIT:0:7}" "$RUNNER_TEMP/hev-version-$target.txt"
file "$output" | tee "$RUNNER_TEMP/hev-file-$target.txt"
if [[ $target == *linux-musl ]]; then
  grep -Eq 'statically linked|static-pie linked' "$RUNNER_TEMP/hev-file-$target.txt"
fi
