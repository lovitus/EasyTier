# GOST binary distribution notice

EasyTier desktop and Core policy artifacts contain a minimally patched GOST
executable renamed to `easytier-gost` or `easytier-gost.exe`.

- Upstream project: <https://github.com/go-gost/gost>
- Fork release: <https://github.com/lovitus/gust/releases/tag/v3.2.9-easytier.1>
- GOST source commit: `3ec9f448ee321c401723e219aba534011b689482`
- GOST source baseline: `a3e2d354a6ee9ed93abf30ece67767ba93fb028e`
- GOST protocol source commit: `935146f3215735e4f5c9dd9e7ee625bc6429c672`
- GOST protocol baseline: `3c32d4cb2001ce9178392bb30b913c0db38d63c2`
- Upstream baseline: `v3.2.6`
- License: MIT
- Integrity: the fork release archive name and GitHub SHA-256 digest are
  recorded in `manifest.json`; the extracted binary digest is recorded in the
  packaged `easytier-gost.manifest.json`.

The only protocol behavior added by this fork is the opt-in
`udpSourceCheck=first-packet` SOCKS5 listener option. It pins an association to
the source IP, port, and IPv6 zone of its first UDP packet. GOST's default
control-connection source check is unchanged when the option is absent.

EasyTier does not download GOST at runtime. GOST is used only as the
loopback-only neutral mesh SOCKS5 entry on desktop and Unix targets. The
legacy Leaf policy implementation and Android HEV runtime remain separate.
