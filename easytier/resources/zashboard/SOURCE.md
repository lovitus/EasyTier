# Zashboard bundled web UI

- Project: `Zephyruso/zashboard`
- Version: `v3.16.0`
- Asset: `dist-no-fonts.zip`
- Source: `https://github.com/Zephyruso/zashboard`
- Release asset: `https://github.com/Zephyruso/zashboard/releases/download/v3.16.0/dist-no-fonts.zip`
- SHA-256: `1d8c7aca69e6ddead5e4fe6e92ceda23a499105f675d053362f7c9b53a9730f9`
- License: MIT, reproduced in `LICENSE`

EasyTier embeds the verified archive in the Core binary and extracts it into the
owned per-instance Mihomo runtime directory. Mihomo serves it from `/ui/` on the
same authenticated loopback controller used by the dashboard API. The archive
never modifies the user's Mihomo YAML or persistent configuration directory.
