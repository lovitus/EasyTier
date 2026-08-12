#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
RUSTC_BIN="${RUSTC_BIN:-rustc}"
MIHOMO_ASSET_TARGET="${MIHOMO_ASSET_TARGET:-x86_64-unknown-linux-musl}"
MIHOMO_RELEASE_MANIFEST="${MIHOMO_RELEASE_MANIFEST:-}"
STATIC_SIDECAR_CACHE_DIR="${STATIC_SIDECAR_CACHE_DIR:-}"

[[ -n "$MIHOMO_RELEASE_MANIFEST" && -f "$MIHOMO_RELEASE_MANIFEST" ]] || {
  printf 'MIHOMO_RELEASE_MANIFEST must name a resolved manifest\n' >&2
  exit 2
}
export MIHOMO_RELEASE_MANIFEST

cd "$REPO_ROOT"
host_target="$($RUSTC_BIN -vV | sed -n 's/^host: //p')"
test -n "$host_target"
binary_dir="easytier-gui/src-tauri/binaries"
mkdir -p "$binary_dir"
cache_dir="${STATIC_SIDECAR_CACHE_DIR:-$REPO_ROOT/target/static-sidecar-cache}"
mkdir -p "$cache_dir"

for sidecar in easytier-leaf-worker easytier-hev-socks-egress; do
  sidecar_path="$binary_dir/${sidecar}-${host_target}"
  install -m 0755 /bin/true "$sidecar_path"
  test -x "$sidecar_path"
done

mihomo_cache_binary="$cache_dir/easytier-mihomo-${MIHOMO_ASSET_TARGET}"
mihomo_cache_metadata="$cache_dir/easytier-mihomo-manifest.json"
if [[ ! -x "$mihomo_cache_binary" || ! -f "$mihomo_cache_metadata" ||
  ! -f "$cache_dir/MIHOMO_SHA256SUMS.txt" ||
  ! -f "$cache_dir/MIHOMO_BUILD_INFO.txt" ]] ||
  ! MIHOMO_DISTRIBUTED_BINARY_NAME=easytier-mihomo \
    bash scripts/fetch-mihomo-release.sh verify \
      "$MIHOMO_ASSET_TARGET" "$mihomo_cache_binary" "$mihomo_cache_metadata" \
      >/dev/null 2>&1; then
  MIHOMO_DISTRIBUTED_BINARY_NAME=easytier-mihomo \
    bash scripts/fetch-mihomo-release.sh fetch \
      "$MIHOMO_ASSET_TARGET" "$mihomo_cache_binary" "$mihomo_cache_metadata"
fi
install -m 0755 "$mihomo_cache_binary" \
  "$binary_dir/easytier-mihomo-${host_target}"
cp "$mihomo_cache_metadata" "$binary_dir/easytier-mihomo-manifest.json"
cp "$cache_dir/MIHOMO_SHA256SUMS.txt" "$binary_dir/MIHOMO_SHA256SUMS.txt"
cp "$cache_dir/MIHOMO_BUILD_INFO.txt" "$binary_dir/MIHOMO_BUILD_INFO.txt"
cp easytier/resources/policy-rule-data/METACUBEX-GPL-3.0.txt \
  "$binary_dir/MIHOMO_LICENSE.txt"
cp easytier/resources/mihomo/SOURCE.md "$binary_dir/MIHOMO_SOURCE.md"

gost_cache_binary="$cache_dir/easytier-gost-${MIHOMO_ASSET_TARGET}"
gost_cache_metadata="$cache_dir/easytier-gost.manifest.json"
if [[ ! -x "$gost_cache_binary" || ! -f "$gost_cache_metadata" ]] ||
  ! bash scripts/fetch-gost-release.sh verify \
    "$MIHOMO_ASSET_TARGET" "$gost_cache_binary" "$gost_cache_metadata" \
    >/dev/null 2>&1; then
  bash scripts/fetch-gost-release.sh fetch \
    "$MIHOMO_ASSET_TARGET" "$gost_cache_binary" "$gost_cache_metadata"
fi
install -m 0755 "$gost_cache_binary" "$binary_dir/easytier-gost-${host_target}"
cp "$gost_cache_metadata" "$binary_dir/easytier-gost.manifest.json"
cp easytier/resources/gost/LICENSE "$binary_dir/GOST_LICENSE.txt"
cp easytier/resources/gost/SOURCE.md "$binary_dir/GOST_SOURCE.md"
printf 'fixture=true\nbackend=hev-native\n' > "$binary_dir/SOCKS_EGRESS_BUILD_INFO.txt"
