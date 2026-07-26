#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: $0 fetch TARGET OUTPUT METADATA_OUTPUT" >&2
  echo "       $0 verify TARGET BINARY METADATA" >&2
  echo "       $0 record-distributed TARGET BINARY METADATA" >&2
  exit 2
}

mode=${1:-}
case "$mode" in
  fetch|verify|record-distributed)
    [[ $# -eq 4 ]] || usage
    target=$2
    binary_path=$3
    metadata_path=$4
    ;;
  *) usage ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$repo_root/easytier/resources/gost/manifest.json"
test -f "$manifest"

if command -v python3 >/dev/null 2>&1; then
  python_cmd=python3
elif command -v python >/dev/null 2>&1; then
  python_cmd=python
else
  echo "Python is required for GOST release verification" >&2
  exit 1
fi

"$python_cmd" - "$mode" "$manifest" "$target" "$binary_path" "$metadata_path" <<'PY'
import hashlib
import io
import json
import pathlib
import stat
import struct
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

mode, manifest_path, target, binary_path, metadata_path = sys.argv[1:]
manifest = json.loads(pathlib.Path(manifest_path).read_text())
entry = manifest["targets"].get(target)
if entry is None:
    raise SystemExit(f"unsupported GOST target: {target}")

binary_path = pathlib.Path(binary_path)
metadata_path = pathlib.Path(metadata_path)

def digest(data):
    return hashlib.sha256(data).hexdigest()

def verify_format(data):
    fmt = entry["binary_format"]
    if fmt == "elf":
        if data[:4] != b"\x7fELF":
            raise SystemExit("invalid GOST ELF header")
        endian = "<" if data[5] == 1 else ">"
        machine = struct.unpack_from(endian + "H", data, 18)[0]
    elif fmt == "macho":
        machine = struct.unpack_from("<I", data, 4)[0]
    elif fmt == "pe":
        offset = struct.unpack_from("<I", data, 0x3c)[0]
        if data[offset:offset + 4] != b"PE\0\0":
            raise SystemExit("invalid GOST PE header")
        machine = struct.unpack_from("<H", data, offset + 4)[0]
    else:
        raise SystemExit(f"unsupported GOST binary format: {fmt}")
    if machine != entry["machine"]:
        raise SystemExit(
            f"GOST machine mismatch: expected {entry['machine']}, got {machine}"
        )

if mode == "fetch":
    asset = entry["asset"]
    url = f"{manifest['repository']}/releases/download/{manifest['tag']}/{asset}"
    request = urllib.request.Request(
        url, headers={"User-Agent": "EasyTier-GOST-release-fetcher"}
    )
    with urllib.request.urlopen(request, timeout=600) as response:
        archive = response.read()
    if digest(archive) != entry["asset_sha256"]:
        raise SystemExit("GOST release archive SHA-256 mismatch")

    wanted = entry.get(
        "binary",
        "gost.exe" if entry["binary_format"] == "pe" else "gost",
    )
    if entry["archive"] == "tar.gz":
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as package:
            matches = [
                member for member in package.getmembers()
                if member.isfile() and pathlib.PurePosixPath(member.name).name == wanted
            ]
            if len(matches) != 1:
                raise SystemExit(f"GOST archive contains {len(matches)} {wanted} files")
            extracted = package.extractfile(matches[0])
            if extracted is None:
                raise SystemExit("failed to extract GOST binary")
            data = extracted.read()
    elif entry["archive"] == "zip":
        with zipfile.ZipFile(io.BytesIO(archive)) as package:
            matches = [
                name for name in package.namelist()
                if pathlib.PurePosixPath(name).name == wanted and not name.endswith("/")
            ]
            if len(matches) != 1:
                raise SystemExit(f"GOST archive contains {len(matches)} {wanted} files")
            data = package.read(matches[0])
    else:
        raise SystemExit(f"unsupported GOST archive: {entry['archive']}")

    verify_format(data)
    binary_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=binary_path.parent, delete=False) as temporary:
        temporary.write(data)
        temporary_path = pathlib.Path(temporary.name)
    temporary_path.chmod(
        temporary_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    )
    temporary_path.replace(binary_path)

    metadata = {
        "schema_version": 1,
        "repository": manifest["repository"],
        "version": manifest["version"],
        "tag": manifest["tag"],
        "license": manifest["license"],
        "target": target,
        "asset": entry["asset"],
        "asset_sha256": entry["asset_sha256"],
        "binary_sha256": digest(data),
        "binary_format": entry["binary_format"],
        "machine": entry["machine"],
    }
    metadata_path.parent.mkdir(parents=True, exist_ok=True)
    metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")
else:
    data = binary_path.read_bytes()
    metadata = json.loads(metadata_path.read_text())
    expected = {
        "repository": manifest["repository"],
        "version": manifest["version"],
        "tag": manifest["tag"],
        "license": manifest["license"],
        "target": target,
        "asset": entry["asset"],
        "asset_sha256": entry["asset_sha256"],
        "binary_format": entry["binary_format"],
        "machine": entry["machine"],
    }
    for key, value in expected.items():
        if metadata.get(key) != value:
            raise SystemExit(f"GOST metadata mismatch for {key}")
    verify_format(data)
    actual_digest = digest(data)
    source_digest = metadata.get("binary_sha256")
    if not isinstance(source_digest, str) or len(source_digest) != 64:
        raise SystemExit("GOST source binary SHA-256 is missing or invalid")

    if mode == "record-distributed":
        if entry["binary_format"] != "macho" or "apple-darwin" not in target:
            raise SystemExit(
                "record-distributed is restricted to Apple Mach-O GOST binaries"
            )
        if actual_digest == source_digest:
            raise SystemExit("GOST distributed binary was not transformed by codesign")
        metadata["distribution_transform"] = "apple-codesign-ad-hoc"
        metadata["distributed_binary_sha256"] = actual_digest
        with tempfile.NamedTemporaryFile(
            dir=metadata_path.parent, mode="w", delete=False
        ) as temporary:
            temporary.write(json.dumps(metadata, indent=2, sort_keys=True) + "\n")
            temporary_path = pathlib.Path(temporary.name)
        temporary_path.replace(metadata_path)
    elif actual_digest != source_digest:
        if (
            entry["binary_format"] != "macho"
            or "apple-darwin" not in target
            or metadata.get("distribution_transform") != "apple-codesign-ad-hoc"
            or metadata.get("distributed_binary_sha256") != actual_digest
        ):
            raise SystemExit("GOST binary SHA-256 mismatch")
PY
