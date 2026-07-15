# Leaf Validation Journal

Append evidence by exact commit SHA. This file is an audit trail, not the current task list.

## Candidate index

| Commit | Linux | Android | Result |
| --- | --- | --- | --- |
| `61c6f313559cedce3453970e2729c6eb7035e48a` | Passed workflow and extended lifecycle | Passed workflow and traffic; failed cycle-10 ownership cleanup | Baseline only, not releasable |
| `e8f7e74549f83791ed43a6f692ff7a034bab070d` | Passed workflow | Passed workflow; native stop command lookup failed | Rejected |
| `afceaab282b92c61c8c8b1e216358fe810d82395` | Workflow cancelled | Workflow cancelled | Source snapshot, not a candidate |
| `949d29e2a5f13c421c40e7e15c72da4497877e84` | Passed workflow and exact artifact checks | Passed workflow, signature, native lifecycle, captured-UID policy TLS, local HEV, and mesh ICMP | Validated baseline; built-in HEV mesh ingress not yet included |
| `00b62e65b9b52bdd2546c0d436e8ffc8acea6d2c` | Passed workflow and exact artifact checks | Passed workflow, signature, built-in HEV in both directions, policy TLS, and lifecycle | Validated baseline; blocked by HEV TCP performance and three-peer route recovery |
| `e1a54d87e08eda80f3d081f10b9a9546cbb268d5` | Run `29440664216` in progress | Run `29440667649` in progress | Exact policy-only KCP and OSPF generation-repair candidate |

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

## `949d29e2a5f13c421c40e7e15c72da4497877e84`

- Single batched candidate created from the preflighted working snapshot; both authoritative workflows and artifact-integrity checks passed.
- Scope: corrected native command contract, authoritative native Android VPN stop, failure-only frontend fallback, dependency-free lifecycle and command tests, Android workflow preflight, HEV platform-host audit, and release-document split.
- The exact commit was created only after the `192.168.2.160` preflight and candidate-level lockfile, `cfg`, workflow-pin, and full-diff review.
- Runtime evidence from this baseline may be reused only for unchanged behavior. The built-in HEV mesh-ingress implementation is a new candidate and requires its own exact artifacts.

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

## `949d29e2` build-wait preparation

- Android wireless ADB `192.168.234.227:5555` was connected and Wi-Fi remained enabled, associated, and usable before candidate installation.
- Existing candidate-package idle baseline before replacement: package `com.kkrainbow.easytier.policycandidate`, PID `7340`, 57 threads, 232 FDs measured through the same app UID, RSS 237,452 KiB, and no running `TauriVpnService`.
- Direct ADB shell cannot enumerate this app's FDs on the current Android build; resource measurements must use `run-as com.kkrainbow.easytier.policycandidate` for the same UID. `run-as` remains invalid for policy-traffic evidence and is used only for process-resource observation.
- Linux host `192.168.1.37` still had the previous `172.31.137.0/30` `ethev-host` route and `10.247.37.0/24` validation route. The next namespace run must clean the prior instance and allocate a different inspected underlay/overlay CIDR rather than reusing or overlapping it.

## 2026-07-16 - Android built-in HEV mesh ingress root cause and implementation boundary

Status: implementation planned; not yet validated in a candidate artifact.

Exact candidate and runtime evidence:

- Candidate `949d29e2a5f13c421c40e7e15c72da4497877e84` passed both Linux and Android workflows, artifact hashes, Android signature verification, Android local HEV SOCKS greeting, and three-peer mesh ICMP.
- From the Linux policy namespace, `10.245.0.1:11080` (Linux peer built-in HEV) returned a SOCKS greeting while `10.245.0.2:11080` (Android peer built-in HEV) timed out. Bidirectional ICMP to the Android peer remained healthy.
- Android `/proc/8620/net/tcp6` showed HEV listening as IPv4-mapped wildcard `0.0.0.0:11080`. A controlled loopback-only listener on Android produced an immediate RST when addressed through the peer virtual IP, proving that an Android `VpnService` virtual address is not interchangeable with a Linux kernel TUN local address.
- Source-side DEBUG showed the route resolving to Android peer `4264210359`, smoltcp connector entries for `10.245.0.2:11080..11082`, and final `connect to remote timeout`; this excludes Leaf DNS and public underlay failure.

Reference semantics inspected before implementation:

- Mihomo `/Users/fanli/Documents/mihomo-rev/component/tsnet/tsnet.go`: `runtime.retryStartSocks5TCP`, `runtime.serveSocks5TCP`, and `runtime.retryStartSocks5UDP` bind the SOCKS service directly on the embedded tailnet `server.Listen`/`ListenPacket` data plane. They do not depend on the host kernel accepting a tailnet virtual IP as a normal local interface.
- Mihomo `/Users/fanli/Documents/mihomo-rev/component/loopback/detector.go`: `Detector.NewConn`, `Detector.CheckConn`, `Detector.NewPacketConn`, and `Detector.CheckPacketConn` track owned connection identities instead of treating every bind failure as proof of an unrelated listener.
- Clash Meta Android `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`: `TunModule.open` creates a `VpnService.Builder` gateway/portal and passes the detached FD into the userspace stack. The address therefore has Android VPN semantics, not Linux kernel-TUN listener semantics.

Chosen EasyTier behavior:

- Keep HEV as an independent crate/process or Android in-process runtime. EasyTier does not absorb the HEV TCP/UDP state machine.
- For the built-in HEV only, accept TCP on a persistent `Socks5Server::data_plane_tcp_bind` listener at the active HEV candidate port and relay it to the runtime's local endpoint. This mirrors Mihomo's embedded-tailnet ingress and removes Android/kernel listener differences.
- Extend the existing peer-authenticated UDP relay request with an explicit built-in-endpoint marker. After validating the RPC caller, exact destination peer virtual IP, and active candidate port, the destination connects to its registered local HEV endpoint. Explicit user SOCKS endpoints retain their existing address semantics and are not silently rewritten.
- Bind/listener failure is fail-closed for policy traffic while mesh remains active. No kernel/direct fallback is permitted for a built-in mesh target.
- Compatibility boundary: EasyTier 2.9.10 mesh behavior remains unchanged because all new paths are policy-feature gated. Built-in HEV relay requires peers containing the new policy relay implementation; unsupported peers fail the policy endpoint rather than altering base mesh routing.

Required validation after implementation:

- Two-peer data-plane TCP relay test with a loopback-only destination service.
- Built-in versus explicit UDP target mapping tests, including wrong candidate and wrong peer rejection.
- Listener cancellation and data-plane reference return to baseline.
- Exact Linux and Android candidate: TCP SOCKS CONNECT, UDP ASSOCIATE/UoT capability fallback, peer restart, Android VPN stop/start, and RSS/FD/thread cleanup.
## 2026-07-16 Android policy semantic correction and combined HEV candidate manifest

### Android `MATCH,REJECT` controlled evidence

- Exact installed candidate: `949d29e2a5f13c421c40e7e15c72da4497877e84`; captured probe UID: `10274`; probe reported `probe_valid=true` and `u:r:untrusted_app`.
- Active policy remained `GEOIP,CN,DIRECT,no-resolve` followed by `MATCH,REJECT`. With VPN/policy enabled, a new `github.com:443` probe completed the local TCP handshake but the TLS handshake timed out after `5080 ms` (`probe_tcp_connected=true`, `probe_tls_handshake=false`).
- The same packaged probe and destination, after a semantic native `stopVpn`, completed TCP and TLS in `394 ms` (`probe_connected=true`, `probe_tls_handshake=true`). The native start path was then invoked again and the original policy remained restored.
- Conclusion: Android TUN capture, classifier dispatch, in-process Leaf, and first-match `MATCH,REJECT` are operating. The earlier connect-only observation was insufficient because the transparent stack may acknowledge TCP before outbound rule resolution. No policy-path code change is justified by that observation. All later Android REJECT evidence must require application data/TLS or controlled remote observation plus a VPN-down baseline.

### Combined HEV candidate manifest (`00b62e65b9b52bdd2546c0d436e8ffc8acea6d2c`)

- Included implementation: stable logical built-in mesh SOCKS endpoint `virtual-ip:11080`; userspace mesh TCP ingress into the locally selected HEV port; explicit built-in marker for UDP RPC remapping; explicit user SOCKS endpoints left unchanged; bounded TCP concurrency; cancellation, association shutdown, and HEV-before-runtime cleanup ownership.
- Included parity evidence: Mihomo `component/tsnet/tsnet.go` binds SOCKS TCP and UDP directly on the embedded mesh data plane; Mihomo `component/loopback/detector.go` owns loop prevention at connection/packet boundaries; Clash Meta Android `TunService.kt::TunModule.open` treats the VpnService FD as a userspace portal rather than a kernel-local virtual-IP listener.
- Included focused tests: built-in endpoint remapping applies only when explicitly marked; mesh data-plane TCP reaches the registered local HEV endpoint; existing UDP association and lifecycle tests remain in the same test binary.
- Mandatory `.160` gate: the complete implementation snapshot passed the smallest `leaf-policy-proxy` `--locked --no-run` build. During the GitHub wait, the already-built exact binary `/workspace/target/debug/deps/easytier-68b83d7d96a53024` ran the complete `policy_proxy::mesh_udp_relay::tests` module directly with one thread: 8 passed, 0 failed in 1.33 seconds. Coverage included explicit built-in mapping, mesh TCP ingress, destination-peer UDP source identity, UDP ASSOCIATE endpoint preservation, UoT v2 framing, and smoltcp burst fallback; no second Cargo build was used.
- Required GitHub work: one push to `codex/profiling-beta`, allowing its single automatic Linux/Android workflow pair to produce authoritative exact artifacts. Do not manually duplicate dispatches.
- Linux artifact evidence: built-in `via: mesh` TCP and UDP from a separate mesh namespace, explicit SOCKS endpoint compatibility, worker/HEV failure isolation, stop/start cleanup, and FD/thread/RSS return toward baseline.
- Android artifact evidence: local HEV TCP/UDP, remote built-in `via: mesh` TCP/UDP, captured-UID DIRECT/REJECT TLS semantics, semantic stop/start, Wi-Fi outage with pre-scheduled device-side recovery, and FD/thread/TUN/listener cleanup.
- Parallel wait work: while GitHub builds, prepare both platform command matrices, preflight/clean isolated Linux hosts, keep Android Wi-Fi and wireless ADB recovery armed, verify CDP and probe readiness, and inspect the immutable candidate metadata. When artifacts arrive, run Linux and Android sessions concurrently where host-global resources do not overlap.

### `00b62e65b9b52bdd2546c0d436e8ffc8acea6d2c` exact artifact and runtime evidence

- Automatic workflows were unique for the pushed SHA and both passed: Linux run `29434184307`; Android run `29434184287`. The Linux ZIP, outer tar checksum, all four inner binary checksums, commit metadata, musl static-pie target, Build ID, symbols, and debug sections passed. The Android ZIP, all three APK checksums, `BUILD_INFO.txt`, and candidate signature passed.
- Android was upgraded with `adb install -r`. Before install and before the first post-install start, archives of `shared_prefs` and `app_webview` were byte-identical; `firstInstallTime` remained `2026-07-14 15:25:47`. The running native version became `2.6.10-00b62e65~`, while instance ID, peer, virtual IP, and policy persisted.
- The Linux policy lease is host-global by design: a second policy instance in a network namespace failed with `policy routing is owned by another process: EAGAIN`. Validation therefore used one policy-enabled instance each on `192.168.1.37`, `192.168.1.38`, and Android rather than pretending netns bypassed the v1 single-instance boundary.
- Linux source `10.246.0.3` through Linux destination built-in HEV `10.246.0.1` completed TCP and UDP payload echo to the controlled `192.168.2.160:25353` service. Linux source through Android destination built-in HEV `10.246.0.2` also completed TCP and UDP payload echo, proving the Android userspace mesh ingress removes the kernel virtual-IP listener dependency.
- Android captured UID `10276` retained `MATCH,REJECT` TLS semantics after upgrade and topology migration. After switching to `MATCH,linux-hev`, the first probe during route convergence failed closed; a bounded retry completed the GitHub TLS handshake in `1346 ms`, proving Android source -> Linux built-in HEV.
- Android semantic stop/start kept PID `13404`. After an HEV load, resources changed from 340 FD / 65 threads to 228 FD / 58 threads on stop; TUN and the `11080` LISTEN socket disappeared. Restart produced 254 FD / 67 threads, restored TUN and LISTEN, and remote TCP/UDP through Android HEV succeeded again.
- Controlled five-second single-stream iperf on the same `192.168.1.38 -> 192.168.2.160` endpoints measured DIRECT at 942.948/936.764 Mbps sent/received and Linux built-in HEV at 59.937/50.363 Mbps, both with zero retransmits. Android built-in HEV measured 33.134/22.936 Mbps. Base DIRECT has no observed regression, but the built-in HEV path is only approximately 5-6% of DIRECT and does not yet satisfy the long-term optimal-performance goal.
- The first external fixture on `10.20.0.65` returned TCP EOF/UDP timeout even for a direct `.37` baseline, so it was rejected as evidence. The same controlled echo/iperf fixture on `192.168.2.160` passed direct TCP/UDP before any HEV claim was accepted.

### Wi-Fi recovery gap discovered in the three-peer topology

- A device-side detached script logged `cmd wifi set-wifi-enabled disabled` and `enabled` return code 0 with a 12-second outage. Wi-Fi, PID `13404`, TUN, and Android captured-UID -> Linux HEV TLS recovered; the TLS handshake completed in `323 ms`.
- The reverse third-peer path did not recover: `192.168.1.38` lost Android virtual IP `10.246.0.2`, and TCP through Android HEV timed out. It remained unreachable after an additional bounded 10-second window.
- Logs show Android removed the old links during `outage!1`, reconnected TCP to `.37` at `01:31:22`, received a new Wi-Fi network key at `01:31:24`, and restored QUIC. `.37` saw the same Android peer ID reconnect. `.38`, however, had previously formed dynamic direct TCP/QUIC connections to Android; after removing that peer it did not install the route re-advertised through `.37`.
- This failure is below the HEV ingress layer: Android as a policy source continued through Linux HEV while third-peer mesh ICMP and Android-as-HEV-destination failed. Do not mark the candidate releasable until this is either fixed with route-generation evidence or proven to be an unchanged 2.9.10 compatibility limitation with an explicit v1 boundary.

## 2026-07-16 - Batched successor: policy-only KCP and OSPF generation repair

Status: exact candidate `e1a54d87e08eda80f3d081f10b9a9546cbb268d5`; mandatory `.160` preflight complete; Linux run `29440664216` and Android run `29440667649` in progress; exact-artifact validation pending.

Performance diagnosis on exact candidate `00b62e65b9b52bdd2546c0d436e8ffc8acea6d2c`:

- Native mesh TCP from `192.168.1.38` through `192.168.1.37` measured about `716.8 Mbps`, excluding the base mesh as the approximately `50 Mbps` bottleneck.
- A temporary kernel-TCP SOCKS forwarder retaining the destination userspace ingress and HEV measured about `511.4 Mbps`, excluding destination HEV and its ingress as the main bottleneck.
- Raising pinned Leaf's `LINK_BUFFER_SIZE` from its default 2 KiB to 32 KiB left the full path around `46-54 Mbps`; a Leaf relay-buffer patch is therefore rejected.
- Enabling the already-existing EasyTier KCP source on the same binaries and endpoints raised the complete built-in HEV path to `480.191/478.684 Mbps` with zero retransmits. The selected implementation reuses that endpoint rather than introducing a new transport or SOCKS sidecar.

Reference and compatibility boundary:

- Mihomo `/Users/fanli/Documents/mihomo-rev/common/net/sing.go::Relay` uses bidirectional `bufio.Copy` with half-close; `/Users/fanli/Documents/mihomo-rev/common/pool/buffer_standard.go` uses a 32 KiB relay buffer; `/Users/fanli/Documents/mihomo-rev/component/tsnet/gateway.go::gatewayTunnel.HandleTCPConn` and `component/tsnet/tsnet.go::serveSocks5TCP` perform a direct dial-and-relay with bounded connection ownership.
- EasyTier intentionally differs because its source-side mesh SOCKS connection otherwise traverses a userspace smoltcp-to-smoltcp TCP path. Policy HEV may use a separately supplied KCP endpoint, but ordinary SOCKS, port forwarding, and the proxy failover selector continue to require the user's explicit `enable_kcp_proxy` setting.
- Policy-only startup registers only `KcpEndpointFilter`; it does not start the generic KCP TCP proxy pipelines. A destination's explicit `disable_kcp_input` remains authoritative. KCP setup failure is bounded and falls back to mesh smoltcp before application payload is sent; kernel/direct fallback remains forbidden for built-in mesh policy traffic.

Three-peer route root cause and repair:

- During the controlled Android Wi-Fi outage, `192.168.1.38` removed Android peer info while retaining topology learned through `192.168.1.37`. Android reconnected to `.37`, and `.37` held a valid new Android `RoutePeerInfo`, but `.38` retained only a version-0 placeholder.
- The `.37 -> .38` session had no saved peer versions after cleanup, yet `build_route_info` stopped at `last_sync_succ_timestamp`, suppressing the older-but-still-valid Android peer info. Recovery happened only after a later direct `.38 <-> Android` connection, not through OSPF relay.
- No protobuf extension is needed. When a cleanup batch actually removes peer info, remaining sessions now rotate their local session generation, set the existing sync-needed flag, and trigger immediate session work. Existing peers already interpret a changed session ID by clearing saved versions, timestamps, and unreachable caches, causing a full resend. The removed peer's own session is excluded.
- This repair preserves the existing OSPF wire format and 2.9.10 session semantics. Regression coverage models the stale remote cache and proves that the new generation clears it; exact three-node outage recovery remains a required artifact test.

Combined next-candidate evidence matrix:

- `.160`: after correcting six stale test-fixture call sites and one missing async test annotation locally, the complete `cargo test --locked --no-run --package easytier --lib --features leaf-policy-proxy` preflight passed. Exact binary `/workspace/target/debug/deps/easytier-68b83d7d96a53024` then passed KCP endpoint isolation 1/1, OSPF generation/cache invalidation 1/1, and the complete mesh relay module 8/8 with one thread. No GitHub workflow was used for compiler feedback.
- Linux exact artifact: DIRECT throughput guard, built-in HEV TCP throughput, explicit user SOCKS behavior with KCP disabled, KCP-input-disabled fallback, three-peer route loss/recovery, worker/HEV crash, stop/start, and FD/thread/RSS cleanup.
- Android exact artifact: captured-UID DIRECT/REJECT/mesh semantics, built-in HEV TCP/UDP, pre-scheduled Wi-Fi disable/enable recovery, reverse third-peer route recovery, semantic stop/start, and FD/thread/TUN/listener cleanup.
- Workflow policy: one frozen code/document snapshot and one automatic Linux/Android workflow pair only.

Build-wait preparation for exact candidate `e1a54d87e08eda80f3d081f10b9a9546cbb268d5`:

- `.37` and `.38` old `00b62e65` validation processes required exact-PID `SIGKILL` after the hosts' `killall` did not remove them. Independent follow-up checks showed no `easytier-core`, `tun0`, or `tun1` residue before the new artifact.
- Fresh `.160` fixtures listen on TCP/UDP `25453` and iperf TCP `25454`. Direct `.38 -> .160` TCP and UDP echo passed; a two-second DIRECT iperf measured `948/941 Mbps` sender/receiver with zero retransmits. Unrelated existing iperf port `28090` was not touched.
- Android pre-upgrade state: package PID `13404`, RSS `203088 KiB`, `250` FDs, `62` threads/tasks, first-install time `2026-07-14 15:25:47`, and `shared_prefs` archive SHA-256 `4bf154e3e19a2b55fcb9c5a87bbc16370df880cf181b9b6fdc1454b677b842d8`.
- Semantic CDP plus direct Tauri `get_config` confirmed peer `tcp://192.168.1.37:25301`, listeners `25340-25344`, virtual IP `10.246.0.2`, `enable_kcp_proxy=false`, `disable_kcp_input=false`, and policy enabled. The exact-artifact run will change only the validation ports through backend config commands; it must preserve the user's explicit KCP setting.
- Device-side `/data/local/tmp/easytier-wifi-cycle-e1a54d87.sh` is installed but not started. It performs the complete delayed disable/wait/enable cycle and records both command return codes so wireless ADB is not responsible for restoring Wi-Fi.

## 2026-07-16 - `e1a54d87e08eda80f3d081f10b9a9546cbb268d5` exact candidate closure

- Candidate discipline: one code snapshot, one Linux workflow (`29440664216`), and one Android workflow (`29440667649`). Both workflows succeeded. The downloaded assets were fetched from byte zero through the configured proxy; ZIP/tar integrity, `SHA256SUMS.txt`, `BUILD_INFO.txt`, exact commit SHA, targets, Android v2 signatures/certificates, Linux static PIE/debug symbols, and Linux Build ID `f1c2a28c3a810b58042c652096aa2b8792b59e3e` were checked before deployment.
- `.160` pre-push diagnostics used the full candidate snapshot with `--locked --no-run` and exact binaries/tests. KCP endpoint isolation passed `1/1`, OSPF generation/cache invalidation passed `1/1`, and `policy_proxy::mesh_udp_relay::tests` passed `8/8`. No GitHub workflow was used as routine compiler feedback.
- All Linux and Android runtime nodes intentionally omitted `--enable-kcp-proxy`. Policy-only KCP still selected `transport_type: Kcp`. Linux HEV TCP throughput was about `478 Mbps` receiver versus a direct baseline near `941 Mbps`; after restoring the normal destination capability later in the run it measured `452 Mbps` receiver.
- Destination capability/fallback: `.37` was restarted with `--disable-kcp-input true`, advertised `kcp_input=false`, and `.38`/Android continued to advertise `true`. `.38` through Linux HEV passed TCP and UDP echo to `.160:25453`; iperf used source `10.246.0.3` and measured `53.5 Mbps` receiver, matching the bounded smoltcp fallback rather than KCP or direct/kernel escape. Restoring `.37` without the flag restored `452 Mbps` receiver on the same path.
- Linux lifecycle/resource evidence: normal SIGTERM of PID `13071` removed core, Leaf, HEV, TUNs, and PID-scoped temporary files. The no-KCP-input cycle used about `20548 KiB`, `47` FDs, and `9` threads and again cleaned to zero. A third standard cycle and the `.38` SOCKS cycle also exited normally with zero candidate processes and no TUN. Cleanup logged four idempotent policy-route deletion `ESRCH` warnings even though resources were absent; treat this as a low-priority observability issue, not a leak.
- Android upgrade preserved first-install time, preferences, instance identity, explicit `enable_kcp_proxy=false`, VPN ownership/ranges, and native version `2.6.10-e1a54d87~`. A captured untrusted-app UID probe completed TCP plus TLS/SNI to `github.com:443` through the configured policy (`probe_valid=true`, `390 ms`).
- Formal Android Wi-Fi lifecycle: `.38` observer PID `18790` was verified with successful samples before the outage. Device-side script PID `18983` was verified before its 30-second delay with `scheduled=1784143102`, `outage=70`, candidate PID `17596`, and connected Wi-Fi. Disable started at `1784143132` and enable at `1784143202`; both commands returned `rc=0`.
- Formal route transition: `.38` first failed at `1784143133` and first recovered at `1784143205`, three seconds after Wi-Fi enable. `.37` removed the stale Android peer, removed cached peer info at `03:20:01`, and rotated the `.37` -> `.38` OSPF session from `2648166354101310701` to `10685380957993188232`. `.38` observed the hub session mismatch at `03:20:01`; its direct Android session did not mismatch/recreate until `03:20:16`. Route recovery therefore preceded direct reconnection by about 11 seconds and used the generation-repair propagation path.
- Post-outage Android data plane: after atomically selecting Android virtual IP `10.246.0.2` as `.38`'s HEV server, both TCP and UDP echo passed. The config was atomically restored to Linux HEV `10.246.0.1`. Android PID remained `17596`; final post-cleanup resources were `194856 KiB`, `253` FDs, and `64` threads, close to the earlier `203088 KiB`, `249` FDs, and `61` threads. `tun0` and the VPN UID ranges remained present.
- Ordinary user SOCKS isolation: `.38` ran `--socks5 25580` without `--enable-kcp-proxy` while policy HEV remained enabled. SOCKS5 greeting, CONNECT to `10.246.0.1:25553`, TCP echo, and simultaneous policy HEV traffic all passed. `.37` had 456 KCP stream records but zero destination-port `25553` matches, proving that policy-only KCP did not silently enable or hijack the ordinary user SOCKS endpoint.
- Final cleanup: `.37/.38` exact-candidate processes, Leaf children, TUNs, observer, and echo fixture were removed; `.160` fixture PIDs for ports `25453/25454` were stopped without touching unrelated services. Probe packages and ADB forwarding were removed. Android exact candidate was deliberately left running with Wi-Fi enabled.
- Release interpretation: no new Linux/Android v1 blocker was found inside the frozen basic Leaf/mesh coexistence boundary. Split DNS, advanced chain/fallback, and high-throughput UDP remain explicit scope/validation decisions and must not be silently advertised as completed.

## 2026-07-16 - HEV worker-count candidate benchmark

- Purpose: decide whether the cross-platform HEV wrapper should raise its default from one worker. This was measured before changing code so a larger idle footprint would not be justified by assumption.
- Artifact: the exact static HEV sidecar shipped with candidate `e1a54d87e08eda80f3d081f10b9a9546cbb268d5` was run on `192.168.1.37`. A bounded HTTP stream fixture ran on `192.168.2.160:25653`; `192.168.1.38` was the direct/SOCKS client. HEV candidates used explicit ports `25680`, `25681`, and `25683` for `workers=1`, `2`, and `4`.
- Single 512 MiB stream: direct `117.04 MB/s`; HEV workers 1 `117.44 MB/s`, workers 2 `117.51 MB/s`, workers 4 `116.57 MB/s`. All paths saturated the same approximately 1 Gbit/s link and the differences were noise.
- Eight concurrent streams with the same aggregate 512 MiB: direct `116.97 MB/s`; HEV workers 1/2/4 each approximately `116.71 MB/s`. Additional workers provided no measured concurrency throughput benefit.
- Idle footprint after traffic: workers 1 used `268 KiB`, `12` FDs, `2` threads; workers 2 used `296 KiB`, `16` FDs, `3` threads; workers 4 used `308 KiB`, `24` FDs, `5` threads.
- Decision: retain `workers=1`. It reaches line rate for both single-stream and eight-stream traffic while minimizing FDs, threads, wakeups, and mobile idle cost. Revisit only with target-specific CPU saturation or materially higher-concurrency evidence; do not auto-scale by CPU count without measurements.
- Cleanup: all three exact HEV PIDs and the `.160` fixture exited; ports `25653/25680/25681/25683` were released. No EasyTier build or workflow was triggered.

## 2026-07-16: unsupported-platform fail-closed preflight and productive-wait batch

- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/listener/inbound/tun.go::Tun.Listen` propagates `sing_tun.New` startup errors; `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go::New` closes partial listener state before returning an initialization error. Observable parity used here: an unavailable policy runtime must fail startup/config generation rather than silently continue as an ordinary mesh instance.
- Intentional EasyTier boundary: enabled policy proxy is accepted only on builds whose cfg/feature combination has a policy runtime. Disabled policy configuration remains portable and preserved. This rejects earlier than Mihomo because EasyTier already knows the build capability during configuration loading/generation.
- `.160` no-feature `cargo test --locked --no-run --package easytier --lib`: passed; both unsupported-runtime exact tests passed.
- `.160` `--features leaf-policy-proxy` no-run: passed; existing config path, disabled-preference, and launcher round-trip exact tests passed.
- Native Windows MSVC no-feature no-run: passed after installing the previously missing host prerequisites LLVM/libclang and Protobuf/protoc. Exact tests `common::config::tests::policy_proxy_enabled_rejects_build_without_runtime` and `launcher::tests::network_config_rejects_enabled_policy_proxy_without_runtime` each passed 1/1.
- GUI policy editor now fetches runtime capability while disabled, blocks only a new enable action when the backend reports `supported=false`, and continues to allow disabling/preserving an already loaded configuration. A pure capability guard test covers unknown, supported, and unsupported responses.
- Validation efficiency used: one `.160` Rust preflight, one retained Windows incremental target directory, no GitHub compiler-debug pushes, and UI/config review plus documentation performed while Windows compiled.

### Candidate manifest: unsupported-runtime fail-closed batch

- Base: `e1a54d87e08eda80f3d081f10b9a9546cbb268d5`; intended candidate SHA is assigned only after this exact snapshot is committed.
- Included functions: `PolicyProxyConfig::runtime_supported`, `PolicyProxyConfig::validate_runtime_support`, TOML load enforcement, `NetworkConfig::gen_config` enforcement, unsupported-build parity tests, policy-editor capability guard/preload, HEV worker-count benchmark evidence, and release-gate/journal updates.
- `.160` gate: no-feature and `leaf-policy-proxy` `--locked --no-run` passed; two unsupported-build and three supported-build exact tests passed.
- Additional platform gate: native Windows MSVC no-feature no-run and both unsupported-build exact tests passed.
- Required workflow set: one push to `codex/profiling-beta`, allowing its automatic Linux profiling-beta and Android candidate workflows exactly once; no manual duplicate dispatch.
- Workflow evidence: exact head SHA, successful frontend/native compilation as applicable, artifact SHA256/build metadata/signing, and no concurrency-cancelled duplicate.
- Exact-artifact validation: Linux confirms enabled policy remains accepted and basic mesh/policy coexistence; Android confirms retained enabled policy starts and a captured-UID policy probe succeeds. Existing full lifecycle evidence remains applicable because this batch does not alter the supported-platform data plane or HEV lifecycle.
- Productive wait: inspect workflow metadata, prepare checksum/artifact commands, and preflight Linux/Android hosts without mutating the committed snapshot.
