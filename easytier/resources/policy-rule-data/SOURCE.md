# Bundled policy rule data

EasyTier bundles two read-only rule data snapshots so `GEOSITE` and `GEOIP`
rules work without a first-run download.

- Upstream: <https://github.com/MetaCubeX/meta-rules-dat>
- Release: `latest`, published `2026-07-13T23:26:40Z`
- Upstream commit: `4178770badecb1b349fbcd62c737e0d7a2079729`
- License: GPL-3.0; see `METACUBEX-GPL-3.0.txt`

| File | Size | SHA-256 |
| --- | ---: | --- |
| `geosite.dat` | 4,228,973 bytes | `0f464192b311ee9b8a2cdc309118928c532b6b5982b486c6a42060db671e3038` |
| `geoip-lite.dat` | 207,049 bytes | `cba612b84b6c023ad2ec110b57c04c88c6ac888935963279b00884731af53301` |

The bundled files are not updated at runtime. A user-supplied rule set of the
same type takes precedence. Online replacement is tracked separately and must
retain the existing bounded download, format validation, digest verification,
and atomic replacement guarantees.
