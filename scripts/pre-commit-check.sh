#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

for required_command in actionlint bash git jq rustup; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    printf 'missing required pre-commit command: %s\n' "$required_command" >&2
    exit 1
  fi
done

git diff --check
git diff --cached --check
rustup run 1.95 cargo fmt --all -- --check

while IFS= read -r -d '' shell_file; do
  bash -n "$shell_file"
done < <(git ls-files -z '*.sh')

check_json_files() {
  while IFS= read -r -d '' json_file; do
    if [[ "$json_file" == *.json && -f "$json_file" ]]; then
      jq empty "$json_file"
    fi
  done
}

check_json_files < <(git diff --name-only --diff-filter=ACMR -z)
check_json_files < <(git diff --cached --name-only --diff-filter=ACMR -z)
check_json_files < <(git ls-files --others --exclude-standard -z)

# Preserve workflow structure, expression, and shell syntax validation while
# excluding two pre-existing quoting/unused-variable style diagnostics.
actionlint -ignore 'SC2086' -ignore 'SC2034'
