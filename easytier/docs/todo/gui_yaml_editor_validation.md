# GUI YAML editor and save validation

Status: implementation candidate, validation pending.

## Required behavior

- Leaf and Mihomo use one code-editor primitive with line numbers, YAML highlighting,
  syntax diagnostics, read-only runtime mode, and OS smart punctuation disabled.
- Invalid YAML never reaches a source file.
- Leaf keeps its existing EasyTier policy parser as the semantic authority.
- Mihomo uses the packaged executable's real `-t -d MANAGED_HOME -f RUNTIME_COPY`
  path before the source file is atomically replaced.
- A failed validator leaves the source unchanged and returns its bounded diagnostic.
- Existing Unix source owner and mode survive an atomic save.

## Reference semantics

- Clash Verge Rev lazy-loads its code editor in
  `/Volumes/micron512g/code/clash-verge-rev/src/services/monaco.ts::loadMonacoEditor`
  and configures read-only state, YAML validation, line numbers, and validation
  decorations in
  `/Volumes/micron512g/code/clash-verge-rev/src/components/profile/editor-viewer.tsx`.
- Mihomo `main.go` handles `-t` by calling
  `hub/executor/executor.go::{Parse,ParseWithBytes}`; those functions delegate to
  `config/config.go::Parse`. EasyTier therefore invokes the packaged Mihomo
  validator instead of claiming compatibility from a separate schema.

EasyTier intentionally uses CodeMirror rather than Clash Verge's React Monaco
wrapper. This keeps the existing Vue library free of React and YAML-worker
bootstrapping while preserving the requested editor and diagnostics behavior.
