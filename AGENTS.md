# Repository Agent Instructions

- Do not compile this repository on the maintainer's local machine. Use the configured remote builder at `root@192.168.2.160` when compilation is required.
- For a pre-release fix on `releases/**`, include `[skip ci]` in the commit message, push the commit, and manually trigger only `.github/workflows/gui-macos-aarch64-test.yml` (`EasyTier GUI macOS ARM64 Test`).
- Do not trigger Core, the full GUI matrix, Mobile, OHOS, the full Test workflow, tags, or Release until the maintainer explicitly confirms real-device validation.
- After that confirmation, run the formal release workflows against the exact validated commit before starting `EasyTier Release`.
