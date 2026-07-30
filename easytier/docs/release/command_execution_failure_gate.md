# Command Execution Failure Gate

Status: permanent local validation and release-process gate.

## Failure-handling contract

An unexpected non-zero exit is evidence, not noise. Before retrying:

1. retain the exact first command, exit code, and unfiltered stderr;
2. identify the first failing process in a pipeline or remote command;
3. classify the cause as command syntax, local environment, remote transport,
   dependency/toolchain, workflow infrastructure, or product behavior;
4. correct the source of the failure;
5. rerun the same command and record why the new result is comparable.

Required commands may not be hidden with `|| true`, an unconditional
success-printing command, output truncation that closes the producer early, or
a locale/proxy/toolchain change that is not explained. Pipelines whose producer
matters must use `set -o pipefail` or be split into execution and output-reading
steps.

## C.UTF-8 root cause

The repeated macOS `shasum`/Perl failure was not introduced by EasyTier, zsh
startup files, launchd, or the user's normal terminal locale.

On 2026-07-30:

- launchd had no `LANG`, `LC_CTYPE`, or `LC_ALL` override;
- the ChatGPT GUI and its Codex parent process did not expose those variables
  as inherited launcher settings;
- every Codex unified-exec child nevertheless received all three variables as
  `C.UTF-8`;
- macOS did not list `C.UTF-8` in `locale -a`;
- BSD `locale` silently displayed a fallback to `C`, while Perl failed before
  `shasum` could run.

The bundled `codex-cli 0.146.0-alpha.3.1` binary contains the same unified-exec
environment constants visible in OpenAI Codex source:

[`codex-rs/core/src/unified_exec/process_manager.rs`](https://github.com/openai/codex/blob/88ec932e96e4d18c5701664e726b1c8b18454af1/codex-rs/core/src/unified_exec/process_manager.rs)
defines `UNIFIED_EXEC_ENV` with:

```text
LANG=C.UTF-8
LC_CTYPE=C.UTF-8
LC_ALL=C.UTF-8
```

`apply_unified_exec_env` inserts these values after the normal
`create_env(shell_environment_policy, ...)` result. This explains why merely
excluding inherited locale variables did not solve the problem.

The previous machine configuration explicitly overrode only `LANG`. That was
also insufficient because `LC_ALL` has precedence over the other locale
variables. The durable fix explicitly sets all three variables to the installed
macOS locale `en_US.UTF-8` in `~/.codex/config.toml`.

A conditional `~/.zshenv` guard provides compatibility with the already-running
app and Codex versions that inject the constants after their normal environment
policy. It changes values only on macOS when at least one locale variable is
exactly `C.UTF-8`; valid caller-selected locales are preserved.

## Mechanical verification

Run:

```bash
scripts/check-command-environment.sh
```

On macOS the script:

1. rejects any remaining `C.UTF-8` locale variable with a source-level repair
   instruction;
2. asks Perl to activate the complete process locale;
3. runs the same Perl-backed SHA-256 path used during artifact verification.

`scripts/pre-commit-check.sh` invokes this check automatically. It must also be
run before local release or artifact-verification command groups. Linux exits
without applying the macOS-specific rule.

The 2026-07-30 repair was verified with:

- all three effective variables equal to `en_US.UTF-8`;
- `locale` reporting `en_US.UTF-8` for every category;
- a direct Perl locale smoke;
- a stdin `shasum -a 256` smoke;
- a child zsh repairing injected `C.UTF-8`;
- a child zsh preserving an explicitly selected valid `zh_CN.UTF-8` locale.
