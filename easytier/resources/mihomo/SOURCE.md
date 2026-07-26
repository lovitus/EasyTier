# Mihomo binary distribution notice

EasyTier desktop and Core release artifacts may contain an unmodified official
Mihomo executable renamed to `easytier-mihomo` or `easytier-mihomo.exe`.

- Upstream project: <https://github.com/MetaCubeX/mihomo>
- Release selection: latest stable release at workflow start; draft and
  prerelease entries are rejected.
- Exact release identity: recorded in the packaged
  `easytier-mihomo.manifest.json`, `MIHOMO_BUILD_INFO.txt`, and
  `MIHOMO_SHA256SUMS.txt`.
- License: GNU General Public License version 3 only (`GPL-3.0-only`)

The complete GPL-3.0 license text is distributed as
`MIHOMO_LICENSE.txt`. Exact upstream asset names and SHA-256 digests are
recorded in `manifest.json`. EasyTier does not download Mihomo at runtime.

The matching EasyTier GitHub Release also publishes:

- `mihomo-<tag>-source.tar.gz`, the complete source tree for the exact tag;
- `mihomo-<tag>-vendor.tar.gz`, the upstream vendored Go dependencies;
- `MIHOMO_SOURCE_SHA256SUMS.txt`, SHA-256 digests for both archives.

Their exact tag, commit, URLs, and digests are recorded in the resolved release
manifest. These source assets are release-time compliance artifacts and are
never fetched at runtime.
