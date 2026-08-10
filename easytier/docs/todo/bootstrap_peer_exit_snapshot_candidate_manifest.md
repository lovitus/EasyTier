# Bootstrap Peer Exit Snapshot Candidate Manifest

Status: PRE-BUILD SNAPSHOT

Date: 2026-08-10

## Candidate boundary

- Base: `codex/current` at `3bc3e603`.
- Candidate SHA: assigned only after the mandatory `.160` preflight succeeds.
- Persist live outbound EasyTier connection URLs only during normal Stop/Exit.
- Read the auxiliary file once at startup and append valid URLs to an independent runtime-only
  initial-peer list after configured peers.
- Keep user TOML, GUI configuration, imported files and serialized initial peers unchanged.
- Reuse `PeerMap::alive_client_urls`; do not add peer events, timers, TTL, scoring or packet-path
  work.
- GUI/Tauri uses its existing app-data directory; Core uses `--config-dir`; Android JNI uses the
  application's private files directory; OHOS uses its existing configuration root.

## Build-affecting files

- `easytier/src/peers/peer_map.rs`
- `easytier/src/instance/instance.rs`
- `easytier/src/launcher.rs`
- `easytier/src/instance_manager.rs`
- `easytier-contrib/easytier-ffi/src/instance_api.rs`
- `easytier-contrib/easytier-ffi/src/lib.rs`
- `easytier-contrib/easytier-android-jni/src/network_api.rs`
- `easytier-contrib/easytier-android-jni/src/lib.rs`
- `easytier-contrib/easytier-android-jni/kotlin/com/easytier/jni/EasyTierJNI.kt`
- `easytier-contrib/easytier-android-jni/kotlin/com/easytier/jni/EasyTierManager.kt`
- `easytier-contrib/easytier-ohrs/src/lib.rs`

## Pre-build documentation

- `easytier/docs/todo/bootstrap_peer_exit_snapshot.md`
- `easytier/docs/todo/bootstrap_peer_exit_snapshot_candidate_manifest.md`

## Mandatory `.160` evidence

- Standard `scripts/release-operator.sh builder-preflight` with locked dependencies.
- `easytier --lib` no-run build containing the focused tests.
- Focused tests:
  - `bootstrap_url_uses_live_client_remote_then_resolved_fallback`
  - `bootstrap_url_excludes_inbound_closed_missing_and_ring_connections`
  - `runtime_bootstrap_peers_follow_configured_peers_without_duplicates`
  - `bootstrap_cache_is_stable_isolated_and_tolerates_malformed_lines`
  - `nonempty_bootstrap_snapshot_replaces_file_and_empty_preserves_it`
  - `manager_persistent_path_is_optional_and_initialized_once`
- Locked no-run builds for `easytier-ffi`, `easytier-android-jni` and `easytier-ohrs` using the
  feature/target combinations already used by their workflows.

Actual commands, test binary, counts and results belong in post-build evidence keyed to the final
candidate SHA; adding that evidence must not redefine or rebuild the candidate.

## Required workflow and artifact gates

- Linux Profiling Beta: required for an optimized exact artifact.
- Android Policy Candidate: required because JNI/Kotlin mobile integration changes.
- Mobile and OHOS formal compile workflows: required before release consideration, but no release
  is part of this implementation-only task unless separately requested.
- Core/Test workflow execution is required only if this candidate proceeds into a release.

## Planned exact-artifact evidence

- Linux three-node lifecycle: configured bootstrap B, discovered outbound C, normal stop, isolate
  B, restart unchanged config, rejoin through cached C, then verify empty stop preserves cache.
- Corrupt one cache line and prove remaining valid URLs still load.
- Confirm the cache path is isolated by stable network identity for two simultaneous network
  configurations.
- Android: initialize app-private path, connect, normal VPN stop, verify auxiliary file through
  app-private inspection, restart with configured bootstrap unavailable, and confirm route/peer
  recovery without changing the saved configuration.
- OHOS: compile and automated path/lifecycle coverage; native-device evidence is recorded as
  `BLOCKED` if no authorized device is available, never silently treated as PASS.

## Work during build waits

- Prepare isolated Linux ports and three-node configurations without mutating the candidate.
- Confirm Android ADB and policy-probe availability without changing device networking.
- Audit `Cargo.lock`, mobile feature `cfg`, generated JNI surface and workflow target pins.
- Stop immediately on a process/connection/CPU storm; preserve first failure output and clean test
  services before continuing.

## Pre-build evidence (2026-08-10)

- `scripts/release-operator.sh builder-preflight`: PASS on `192.168.2.160` for the complete working snapshot before the final warning-only cleanup and focused-test-list correction. The locked EasyTier library no-run build, existing Leaf/HEV focused suite, 122 frontend-lib tests, frontend-lib production build, frontend production build, VPN plugin build, and GUI production build all passed.
- `scripts/leaf-remote-preflight.sh`: PASS on the final Rust snapshot after adding the six bootstrap-cache tests to the default suite. The locked no-run build and every default focused filter passed.
- New focused contracts included in that PASS: live outbound URL selection with original-address preference and resolved fallback; inbound/closed/missing/`ring://` rejection; configured-first runtime merge and exact deduplication; stable per-network cache isolation with malformed-line tolerance; non-empty complete replacement with empty-snapshot preservation; and one-time/idempotent persistent-directory initialization.
- `cargo test --locked --no-run -p easytier-ffi -p easytier-android-jni`: PASS on `192.168.2.160`; exact host test binaries were produced for both bindings.
- Generic-builder OHOS check: `cargo test --locked --no-run -p easytier-ohrs` is not applicable because `easytier-ohrs` is an independent workspace. Re-running from its own manifest correctly failed closed because its existing independent `Cargo.lock` requests an update under `--locked`. No lockfile was changed and no unlocked result is accepted. Exact OHOS compilation remains a required dedicated OHOS-workflow gate.
- USB Android device `5e655959` (`polaris`) is online. Compilation is not accepted as real-device evidence; the workflow-built exact APK must still prove directory initialization, normal Stop persistence, restart-time in-memory merge, multi-network isolation, and unchanged user configuration.
