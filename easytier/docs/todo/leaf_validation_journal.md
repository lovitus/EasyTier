# Leaf Validation Journal

Append evidence by exact commit SHA. This file is an audit trail, not the current task list.

## Candidate index

| Commit | Linux | Android | Result |
| --- | --- | --- | --- |
| `61c6f313559cedce3453970e2729c6eb7035e48a` | Passed workflow and extended lifecycle | Passed workflow and traffic; failed cycle-10 ownership cleanup | Baseline only, not releasable |
| `e8f7e74549f83791ed43a6f692ff7a034bab070d` | Passed workflow | Passed workflow; native stop command lookup failed | Rejected |
| `afceaab282b92c61c8c8b1e216358fe810d82395` | Workflow cancelled | Workflow cancelled | Source snapshot, not a candidate |

## `61c6f313559cedce3453970e2729c6eb7035e48a`

### Artifact provenance

- Linux workflow `29420954296`: success.
- Android workflow `29420954300`: success.
- Exact workflow artifacts, hashes, build metadata, and signing evidence were checked before deployment.

### Linux lifecycle

- Normal operation, fallback, SIGTERM, SIGKILL/PDEATH, and crash recovery passed.
- Ten namespace lifecycle cycles passed real OpenSSH TCP and UDP echo traffic.
- Core remained at 9 threads and 36 FDs; Leaf at 4 threads and 11 FDs; HEV at 2 threads and 12 FDs.
- Core RSS remained 13,992-14,444 KiB; Leaf 5,540-5,552 KiB; HEV 272-280 KiB.
- Every stop completed in approximately 200 ms and removed processes, listener `11080`, TUN devices, policy rules, table `52000`, and temporary configuration.

### Android lifecycle

- Ten same-process cycles exercised real TCP and UDP traffic.
- Cycles 1-9 returned to 58 threads and 232 FDs; RSS rose slightly and then plateaued near 195,888 KiB.
- Cycle 10 stopped core, Leaf, HEV, and listener `11080`, but the VPN TUN remained for more than 11 seconds and FD count remained 233.
- Logs showed the backend `vpn_service_stop` event without the frontend `stop vpn` action. `TauriVpnService` still had `startRequested=true` and retained VPN ownership.
- Force-stopping the app removed the TUN while Wi-Fi remained connected.
- Conclusion: a WebView event and JavaScript queue cannot own safety-critical native VPN shutdown.

### Reference behavior

- Mihomo Android reference: `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`.
- `TunService` closes TUN ownership in the native service runtime `finally` block under `NonCancellable`; `onDestroy()` requests stop and waits for completion.
- Required EasyTier semantic: native service shutdown must proceed without WebView readiness; UI notification is observation/fallback rather than the sole owner.

## `e8f7e74549f83791ed43a6f692ff7a034bab070d`

- Linux workflow `29424320711`: success.
- Android workflow `29424320648`: success.
- APK SHA-256: `191d6588c4a02869bf6be9463399a39d95286f42a6223521896cfee2fdb3ccb2`.
- Installation preserved app data and signer identity.
- Direct Rust stop reached the plugin but requested `stop_vpn`; the native plugin exports `stopVpn`.
- The frontend fallback later cleaned the TUN and HEV, but the direct command correctly returned an error. This candidate is rejected.

## `afceaab282b92c61c8c8b1e216358fe810d82395`

- Corrected Rust-to-native command strings to `prepareVpn`, `startVpn`, `stopVpn`, and `getVpnStatus`.
- Linux workflow `29426468397` and Android workflow `29426468266` were cancelled intentionally.
- No artifact or runtime claim may be attached to this snapshot.
- Further Android lifecycle semantics and HEV platform work must be batched before the next workflow pair.

## HEV transport semantics

- The current HEV path uses standard SOCKS5 `CONNECT` and `UDP ASSOCIATE`.
- It does not enable UoT or KCP by default.
- For a mesh exit, client-to-HEV UDP is carried as EasyTier overlay traffic; HEV-to-destination remains native UDP.
- KCP itself depends on UDP and does not eliminate UDP reachability failures.
- UoT requires an explicitly compatible remote endpoint and cannot be silently substituted for ordinary SOCKS5 UDP.

## HEV platform-host audit after `afceaab2`

- `easytier-socks-egress::ProcessRuntime` is structurally portable across desktop Rust targets; Linux alone adds `PR_SET_PDEATHSIG`, while other targets rely on Tokio child ownership and `kill_on_drop`.
- EasyTier currently starts that process runtime on every non-mobile `leaf-policy-proxy` build, but only the Linux profiling workflow builds and bundles `easytier-hev-socks-egress`.
- Android uses the separate in-process static-library host and has exact workflow/runtime evidence.
- HEV documents Unix, Android, Apple XCFramework, and MSYS2 build entry points. Its README still marks standard UDP ASSOCIATE unsupported on Windows; the local fork's Windows repair therefore requires exact Windows UDP lifecycle evidence before any support claim.
- macOS/iOS have no EasyTier HEV artifact packaging or in-process wiring yet. Windows/macOS/iOS and other targets remain design targets, not Leaf v1 supported HEV hosts.
- Leaf v1 remains Linux and Android only. Do not add speculative platform branches merely to make a cross-platform claim; add one host model at a time with build, packaging, shutdown, TCP/UDP, and resource-baseline evidence.

## Pre-candidate remote preflight after `afceaab2`

- The complete local working snapshot was formatted and synchronized to the dedicated builder at `192.168.2.160`; no GitHub workflow was started for the intermediate changes.
- A full Cargo test build of `tauri-plugin-vpnservice` was attempted first and stopped at the builder's missing `gdk-3.0` development package. This was a Linux Tauri host dependency, not a source compiler failure; installing a full GUI stack was deliberately avoided.
- Android VPN stop decisions were extracted into a dependency-free module. Direct Rust 2024 test compilation on `192.168.2.160` passed, followed by 4/4 exact tests: TUN-present no-op, authoritative native success, native-failure frontend fallback, and non-native frontend ownership.
- The Rust/Kotlin mobile command-name contract was compiled separately on `192.168.2.160` and passed 1/1. The Android candidate workflow now runs both dependency-free lifecycle contracts before its expensive HEV and APK build.
- `cargo test --locked --no-run --package easytier-socks-egress` passed on `192.168.2.160`, followed by 3/3 exact tests for port validation, bounded HEV configuration, and occupied-listener safety.
- These results are pre-candidate compiler/test evidence only. They do not create an artifact SHA or replace the next exact Linux/Android workflow and real-device lifecycle matrix.
