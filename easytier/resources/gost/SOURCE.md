# GOST binary distribution notice

EasyTier desktop and Core policy artifacts contain an unmodified official GOST
executable renamed to `easytier-gost` or `easytier-gost.exe`.

- Upstream project: <https://github.com/go-gost/gost>
- Pinned release: `v3.2.6`
- License: MIT
- Integrity: the upstream release archive name and official SHA-256 digest are
  recorded in `manifest.json`; the extracted binary digest is recorded in the
  packaged `easytier-gost.manifest.json`.

EasyTier does not download GOST at runtime. GOST is used only as the
loopback-only neutral mesh SOCKS5 entry on desktop and Unix targets. The
legacy Leaf policy implementation and Android HEV runtime remain separate.
