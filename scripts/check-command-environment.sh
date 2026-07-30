#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

for locale_variable in LANG LC_CTYPE LC_ALL; do
  locale_value="${!locale_variable-}"
  if [[ "$locale_value" == "C.UTF-8" ]]; then
    printf '%s\n' \
      "unsupported macOS locale: $locale_variable=C.UTF-8" \
      "repair ~/.codex/config.toml so shell_environment_policy.set overrides LANG, LC_CTYPE, and LC_ALL; then restart the app" \
      >&2
    exit 1
  fi
done

# BSD locale may silently fall back to C for an unavailable locale. Exercise
# the Perl runtime used by shasum so the check detects the real failure mode.
perl -e 'use POSIX qw(LC_ALL setlocale); die "unusable process locale\n" unless defined setlocale(LC_ALL, "");'
printf 'easytier-command-environment-smoke' | shasum -a 256 >/dev/null
