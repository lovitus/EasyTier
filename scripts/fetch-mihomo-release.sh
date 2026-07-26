#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: $0 resolve-latest OUTPUT_MANIFEST" >&2
  echo "usage: $0 fetch TARGET OUTPUT METADATA_OUTPUT" >&2
  echo "       $0 verify TARGET BINARY METADATA" >&2
  echo "       $0 fetch-source OUTPUT_DIRECTORY" >&2
  exit 2
}

mode=${1:-}

case "$mode" in
  resolve-latest)
    [[ $# -eq 2 ]] || usage
    resolved_manifest_output=$2
    ;;
  fetch|verify)
    [[ $# -eq 4 ]] || usage
    target=$2
    binary_path=$3
    metadata_path=$4
    ;;
  fetch-source)
    [[ $# -eq 2 ]] || usage
    source_output_dir=$2
    ;;
  *) usage ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest_template="$repo_root/easytier/resources/mihomo/manifest.json"
manifest="${MIHOMO_RELEASE_MANIFEST:-$manifest_template}"
test -f "$manifest_template"

if command -v python3 >/dev/null 2>&1; then
  python_cmd=python3
elif command -v python >/dev/null 2>&1; then
  python_cmd=python
else
  echo "Python is required for executable format verification" >&2
  exit 1
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

if [[ "$mode" == "resolve-latest" ]]; then
  "$python_cmd" - "$manifest_template" "$resolved_manifest_output" <<'PY'
import hashlib
import json
import os
import pathlib
import re
import sys
import tempfile
import urllib.parse
import urllib.request

template_path = pathlib.Path(sys.argv[1])
output_path = pathlib.Path(sys.argv[2])
template = json.loads(template_path.read_text())
repository = template["repository"]
repo_path = urllib.parse.urlparse(repository).path.strip("/")
api_root = f"https://api.github.com/repos/{repo_path}"

headers = {
    "Accept": "application/vnd.github+json",
    "User-Agent": "EasyTier-Mihomo-release-resolver",
    "X-GitHub-Api-Version": "2022-11-28",
}
token = os.environ.get("GITHUB_TOKEN")
if token:
    headers["Authorization"] = f"Bearer {token}"

def get_json(url):
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)

def sha256_url(url):
    request = urllib.request.Request(url, headers={"User-Agent": headers["User-Agent"]})
    digest = hashlib.sha256()
    with urllib.request.urlopen(request, timeout=600) as response:
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()

release = get_json(f"{api_root}/releases/latest")
if release.get("draft") or release.get("prerelease"):
    raise SystemExit("GitHub latest release is draft or prerelease")
tag = release.get("tag_name", "")
if not re.fullmatch(r"v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", tag):
    raise SystemExit(f"unsupported stable release tag: {tag!r}")
version = tag[1:]

assets = {}
for asset in release.get("assets", []):
    name = asset.get("name")
    if not name or name in assets:
        raise SystemExit(f"missing or duplicate release asset name: {name!r}")
    digest = asset.get("digest", "")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise SystemExit(f"release asset lacks an official SHA-256 digest: {name}")
    assets[name] = asset

targets = {}
for target, entry in template["targets"].items():
    resolved = dict(entry)
    if entry["status"] == "supported":
        asset_name = entry["asset_template"].replace("{version}", version)
        asset = assets.get(asset_name)
        if asset is None:
            raise SystemExit(f"required release asset is missing for {target}: {asset_name}")
        resolved.pop("asset_template", None)
        resolved["asset"] = asset_name
        resolved["asset_sha256"] = asset["digest"].split(":", 1)[1]
        resolved["asset_url"] = asset["browser_download_url"]
    targets[target] = resolved

vendor = assets.get("vendor.tar.gz")
if vendor is None:
    raise SystemExit("stable release lacks vendor.tar.gz")

ref = get_json(f"{api_root}/git/ref/tags/{urllib.parse.quote(tag, safe='')}")
tag_object = ref["object"]
while tag_object["type"] == "tag":
    tag_object = get_json(f"{api_root}/git/tags/{tag_object['sha']}")["object"]
if tag_object["type"] != "commit":
    raise SystemExit(f"tag does not resolve to a commit: {tag_object['type']}")
commit = tag_object["sha"]

quoted_tag = urllib.parse.quote(tag, safe="")
source_url = f"https://codeload.github.com/{repo_path}/tar.gz/refs/tags/{quoted_tag}"
source_name = f"mihomo-{tag}-source.tar.gz"
vendor_name = f"mihomo-{tag}-vendor.tar.gz"
resolved = {
    "schema_version": 2,
    "resolved": True,
    "repository": repository,
    "release_policy": template["release_policy"],
    "release_id": release["id"],
    "version": version,
    "tag": tag,
    "release_url": release["html_url"],
    "published_at": release["published_at"],
    "license": template["license"],
    "license_file": template["license_file"],
    "source_distribution": {
        "commit": commit,
        "archive": {
            "name": source_name,
            "url": source_url,
            "sha256": sha256_url(source_url),
        },
        "vendor_archive": {
            "name": vendor_name,
            "url": vendor["browser_download_url"],
            "sha256": vendor["digest"].split(":", 1)[1],
        },
    },
    "targets": targets,
}
output_path.parent.mkdir(parents=True, exist_ok=True)
with tempfile.NamedTemporaryFile(
    mode="w", encoding="utf-8", dir=output_path.parent, delete=False
) as temporary:
    json.dump(resolved, temporary, indent=2, sort_keys=True)
    temporary.write("\n")
    temporary_path = pathlib.Path(temporary.name)
temporary_path.replace(output_path)
PY
  exit 0
fi

test -f "$manifest"
if [[ "$(jq -r '.resolved // false' "$manifest")" != "true" ]]; then
  echo "Mihomo fetch requires a workflow-resolved release manifest" >&2
  exit 1
fi

version="$(jq -r '.version' "$manifest")"
tag="$(jq -r '.tag' "$manifest")"
repository="$(jq -r '.repository' "$manifest")"
release_id="$(jq -r '.release_id' "$manifest")"
published_at="$(jq -r '.published_at' "$manifest")"

if [[ "$mode" == "fetch-source" ]]; then
  source_name="$(jq -r '.source_distribution.archive.name' "$manifest")"
  source_url="$(jq -r '.source_distribution.archive.url' "$manifest")"
  source_sha256="$(jq -r '.source_distribution.archive.sha256' "$manifest")"
  vendor_name="$(jq -r '.source_distribution.vendor_archive.name' "$manifest")"
  vendor_url="$(jq -r '.source_distribution.vendor_archive.url' "$manifest")"
  vendor_sha256="$(jq -r '.source_distribution.vendor_archive.sha256' "$manifest")"
  for source_value in "$source_name" "$vendor_name"; do
    [[ "$source_value" != */* && "$source_value" != *..* ]]
  done
  [[ "$source_sha256" =~ ^[0-9a-f]{64}$ ]]
  [[ "$vendor_sha256" =~ ^[0-9a-f]{64}$ ]]

  source_tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/easytier-mihomo-source.XXXXXX")"
  trap 'rm -rf "$source_tmp_dir"' EXIT
  curl --proto '=https' --tlsv1.2 --fail --location --retry 3 --retry-all-errors \
    --connect-timeout 20 --max-time 600 --output "$source_tmp_dir/$source_name" "$source_url"
  curl --proto '=https' --tlsv1.2 --fail --location --retry 3 --retry-all-errors \
    --connect-timeout 20 --max-time 600 --output "$source_tmp_dir/$vendor_name" "$vendor_url"
  [[ "$(sha256_file "$source_tmp_dir/$source_name")" == "$source_sha256" ]]
  [[ "$(sha256_file "$source_tmp_dir/$vendor_name")" == "$vendor_sha256" ]]

  mkdir -p "$source_output_dir"
  install -m 0644 "$source_tmp_dir/$source_name" "$source_output_dir/$source_name"
  install -m 0644 "$source_tmp_dir/$vendor_name" "$source_output_dir/$vendor_name"
  printf '%s  %s\n%s  %s\n' \
    "$source_sha256" "$source_name" \
    "$vendor_sha256" "$vendor_name" \
    > "$source_output_dir/MIHOMO_SOURCE_SHA256SUMS.txt"
  exit 0
fi

source_commit="$(jq -r '.source_distribution.commit' "$manifest")"
source_archive_name="$(jq -r '.source_distribution.archive.name' "$manifest")"
source_archive_sha256="$(jq -r '.source_distribution.archive.sha256' "$manifest")"
vendor_archive_name="$(jq -r '.source_distribution.vendor_archive.name' "$manifest")"
vendor_archive_sha256="$(jq -r '.source_distribution.vendor_archive.sha256' "$manifest")"

entry="$(jq -cer --arg target "$target" '.targets[$target] // error("unknown Mihomo target")' "$manifest")"
status="$(jq -r '.status' <<<"$entry")"
if [[ "$status" != "supported" ]]; then
  reason="$(jq -r '.reason // "target is not supported"' <<<"$entry")"
  echo "Mihomo target $target is $status: $reason" >&2
  exit 1
fi

asset="$(jq -r '.asset' <<<"$entry")"
asset_sha256="$(jq -r '.asset_sha256' <<<"$entry")"
archive_kind="$(jq -r '.archive' <<<"$entry")"
binary_format="$(jq -r '.binary_format' <<<"$entry")"
machine="$(jq -r '.machine' <<<"$entry")"

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
[[ "$tag" == "v$version" ]]
[[ "$asset" != */* && "$asset" != *..* ]]
[[ "$asset_sha256" =~ ^[0-9a-f]{64}$ ]]
[[ "$machine" =~ ^[0-9]+$ ]]

verify_executable() {
  "$python_cmd" - "$1" "$binary_format" "$machine" <<'PY'
import pathlib
import struct
import sys

path = pathlib.Path(sys.argv[1])
expected_format = sys.argv[2]
expected_machine = int(sys.argv[3])
data = path.read_bytes()

if expected_format == "elf":
    if len(data) < 20 or data[:4] != b"\x7fELF":
        raise SystemExit("expected ELF executable")
    endian = "<" if data[5] == 1 else ">"
    machine = struct.unpack_from(endian + "H", data, 18)[0]
elif expected_format == "macho":
    if len(data) < 8 or data[:4] != b"\xcf\xfa\xed\xfe":
        raise SystemExit("expected little-endian 64-bit Mach-O executable")
    machine = struct.unpack_from("<I", data, 4)[0]
elif expected_format == "pe":
    if len(data) < 64 or data[:2] != b"MZ":
        raise SystemExit("expected PE executable")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 6 > len(data) or data[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise SystemExit("invalid PE header")
    machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
else:
    raise SystemExit(f"unsupported binary format: {expected_format}")

if machine != expected_machine:
    raise SystemExit(f"machine mismatch: expected {expected_machine}, got {machine}")
PY
}

verify_metadata() {
  test -f "$metadata_path"
  jq -e \
    --arg target "$target" \
    --arg version "$version" \
    --arg tag "$tag" \
    --argjson release_id "$release_id" \
    --arg published_at "$published_at" \
    --arg source_commit "$source_commit" \
    --arg source_archive_sha256 "$source_archive_sha256" \
    --arg vendor_archive_sha256 "$vendor_archive_sha256" \
    --arg asset "$asset" \
    --arg asset_sha256 "$asset_sha256" \
    '
      .target == $target and
      .version == $version and
      .tag == $tag and
      .release_id == $release_id and
      .published_at == $published_at and
      .source_commit == $source_commit and
      .source_archive.sha256 == $source_archive_sha256 and
      .vendor_archive.sha256 == $vendor_archive_sha256 and
      .asset == $asset and
      .asset_sha256 == $asset_sha256
    ' "$metadata_path" >/dev/null
  expected_binary_sha256="$(jq -r '.binary_sha256' "$metadata_path")"
  [[ "$expected_binary_sha256" =~ ^[0-9a-f]{64}$ ]]
  actual_binary_sha256="$(sha256_file "$binary_path")"
  [[ "$actual_binary_sha256" == "$expected_binary_sha256" ]] || {
    echo "Mihomo binary SHA-256 mismatch for $binary_path" >&2
    exit 1
  }
  verify_executable "$binary_path"
}

write_distribution_metadata() {
  output_dir="$(dirname "$metadata_path")"
  staged_name="$(basename "$binary_path")"
  distributed_name="${MIHOMO_DISTRIBUTED_BINARY_NAME:-$staged_name}"
  binary_sha256="$(sha256_file "$binary_path")"
  metadata_sha256="$(sha256_file "$metadata_path")"

  cat > "$output_dir/MIHOMO_BUILD_INFO.txt" <<EOF
component=mihomo
version=$version
tag=$tag
release_id=$release_id
published_at=$published_at
target=$target
upstream_asset=$asset
upstream_asset_sha256=$asset_sha256
distributed_binary=$distributed_name
distributed_binary_sha256=$binary_sha256
manifest_sha256=$metadata_sha256
source_commit=$source_commit
source_archive=$source_archive_name
source_archive_sha256=$source_archive_sha256
vendor_archive=$vendor_archive_name
vendor_archive_sha256=$vendor_archive_sha256
EOF
  printf '%s  %s\n%s  %s\n' \
    "$binary_sha256" "$distributed_name" \
    "$metadata_sha256" "$(basename "$metadata_path")" \
    > "$output_dir/MIHOMO_SHA256SUMS.txt"
}

if [[ "$mode" == "verify" ]]; then
  test -f "$binary_path"
  verify_metadata
  exit 0
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/easytier-mihomo.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
archive_path="$tmp_dir/$asset"
extracted_path="$tmp_dir/easytier-mihomo"
download_url="$repository/releases/download/$tag/$asset"
resolved_download_url="$(jq -r '.asset_url' <<<"$entry")"
[[ "$resolved_download_url" == "$download_url" ]]

curl \
  --proto '=https' \
  --tlsv1.2 \
  --fail \
  --location \
  --retry 3 \
  --retry-all-errors \
  --connect-timeout 20 \
  --max-time 600 \
  --output "$archive_path" \
  "$resolved_download_url"

actual_asset_sha256="$(sha256_file "$archive_path")"
[[ "$actual_asset_sha256" == "$asset_sha256" ]] || {
  echo "Mihomo release asset SHA-256 mismatch for $asset" >&2
  exit 1
}

case "$archive_kind" in
  gzip)
    gzip -dc "$archive_path" > "$extracted_path"
    ;;
  zip)
    "$python_cmd" - "$archive_path" "$extracted_path" <<'PY'
import pathlib
import shutil
import stat
import sys
import zipfile

archive_path = pathlib.Path(sys.argv[1])
output_path = pathlib.Path(sys.argv[2])

with zipfile.ZipFile(archive_path) as archive:
    members = [member for member in archive.infolist() if not member.is_dir()]
    if len(members) != 1:
        raise SystemExit("Mihomo ZIP must contain exactly one file")

    member = members[0]
    member_path = pathlib.PurePosixPath(member.filename)
    if (
        member_path.is_absolute()
        or len(member_path.parts) != 1
        or member_path.name in {"", ".", ".."}
        or "\\" in member.filename
        or "\x00" in member.filename
    ):
        raise SystemExit(f"unsafe Mihomo ZIP member: {member.filename!r}")

    unix_mode = member.external_attr >> 16
    if unix_mode and stat.S_ISLNK(unix_mode):
        raise SystemExit("Mihomo ZIP member must not be a symbolic link")

    with archive.open(member, "r") as source, output_path.open("wb") as output:
        shutil.copyfileobj(source, output)
PY
    ;;
  *)
    echo "unsupported Mihomo archive type: $archive_kind" >&2
    exit 1
    ;;
esac

test -s "$extracted_path"
chmod 0755 "$extracted_path"
verify_executable "$extracted_path"
binary_sha256="$(sha256_file "$extracted_path")"

mkdir -p "$(dirname "$binary_path")" "$(dirname "$metadata_path")"
install -m 0755 "$extracted_path" "$binary_path"
jq -n \
  --arg target "$target" \
  --arg version "$version" \
  --arg tag "$tag" \
  --argjson release_id "$release_id" \
  --arg published_at "$published_at" \
  --arg source_commit "$source_commit" \
  --arg source_archive_name "$source_archive_name" \
  --arg source_archive_sha256 "$source_archive_sha256" \
  --arg vendor_archive_name "$vendor_archive_name" \
  --arg vendor_archive_sha256 "$vendor_archive_sha256" \
  --arg repository "$repository" \
  --arg asset "$asset" \
  --arg asset_sha256 "$asset_sha256" \
  --arg binary_sha256 "$binary_sha256" \
  --arg binary_format "$binary_format" \
  --argjson machine "$machine" \
  '{
    target: $target,
    version: $version,
    tag: $tag,
    release_id: $release_id,
    published_at: $published_at,
    source_commit: $source_commit,
    source_archive: {
      name: $source_archive_name,
      sha256: $source_archive_sha256
    },
    vendor_archive: {
      name: $vendor_archive_name,
      sha256: $vendor_archive_sha256
    },
    repository: $repository,
    asset: $asset,
    asset_sha256: $asset_sha256,
    binary_sha256: $binary_sha256,
    binary_format: $binary_format,
    machine: $machine,
    license: "GPL-3.0-only"
  }' > "$metadata_path"
verify_metadata
write_distribution_metadata
