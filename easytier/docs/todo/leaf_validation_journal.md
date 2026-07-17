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

## 2026-07-16: exact candidate `318497c4` artifact and device closure

- Candidate: `318497c4fd8450a8fee237ef5826841c60517b0c` (`fix(policy): reject unsupported runtime activation`). One push to `codex/profiling-beta` triggered exactly one automatic workflow pair; no manual duplicate dispatch.
- Linux workflow `29447382393` succeeded in 18m24s. Artifact `8356470329` was exactly `119224308` bytes; outer ZIP integrity, outer tar SHA256, inner `easytier-core`, `easytier-cli`, `easytier-leaf-worker`, and `easytier-hev-socks-egress` SHA256 all passed. BUILD_INFO records the exact candidate, run, x86_64-musl target, Rust 1.95.0, and pinned HEV `97e74f1068bd924e740032382cdc94ca83741ae6`. Core is static PIE, unstripped, with `.debug_info`, `.debug_line`, `.symtab`, and build ID `3e38600a137bdf1ee3166ce799890ae6bd9f0e6f`.
- Android workflow `29447382391` succeeded in 18m45s. Artifact `8356481302` was exactly `32218009` bytes. The first concurrent download stopped at `8978432` bytes and failed ZIP central-directory validation; it was rejected and a fresh byte-zero retry passed ZIP integrity. APK/probe/runner/BUILD_INFO SHA256 all passed. Candidate certificate SHA256 is `14d2d885ce1bc361923a493210865f86390ffcd32eb2b555042bbd1a8b6c38e0`; both probe packages use `05242c2efd4415f80d7b4bf8837d667f115fcbd97420652cf5a957fd55095ada`.
- Linux exact-artifact isolation: `.37` ran a policy-enabled node (`10.250.77.1`) and ordinary node (`10.250.77.2`) in point-to-point network namespaces. The policy node had core + Leaf + HEV, `transparent policy proxy is ready`, and TUN `et318p`. The ordinary node connected over TCP and established a QUIC direct tunnel. Mesh ICMP to the policy node passed 3/3 (0.518 ms average). Normal exit removed both cores, TUNs, sidecars, listeners, namespaces, and temporary files.
- The first bridge namespace topology was rejected before product startup because host bridge forwarding blocked namespace-to-namespace ICMP. The first policy startup without a namespace default route correctly failed closed with `policy outbound interface eth0 has no IPv4 default route` and cleaned its partial TUN. Neither result was counted as a candidate failure.
- Android upgrade retention: before `adb install -r`, the candidate data tar SHA256 was `d4170e818e1243477218b1aa6ec47f39c07e404a11141ca5ed509762bfdbaeb2`; the post-install, pre-start archive was byte-identical. `firstInstallTime` remained `2026-07-14 15:25:47`; only `lastUpdateTime` changed. CDP confirmed `enable_policy_proxy=true` and a retained 145-byte v1 rule document without printing secrets.
- Android startup used semantic CDP/Tauri invocation, not screenshots or coordinate clicks. `TauriVpnService` owned connected VPN network 268, TUN `10.246.0.2/24`, OwnerUid/AdminUid `10254`, underlying Wi-Fi network 267, and captured UID ranges `{0-10253, 10255-20253, 20255-99999}`. Excluding runtime UID `10254` preserves HEV loop prevention; `revoked_by_system=false`.
- Captured-UID probe: exact probe UID `10280`, SELinux `u:r:untrusted_app:s0:c24,c257,c512,c768`, rule first-match `MATCH,linux-hev`. With VPN active, TCP probe to controlled `.160:25450` completed in 9 ms and `.160` observed source `192.168.1.37:41780`, proving traversal through the mesh HEV peer rather than a local TUN handshake. After `update_network_config_state(disabled=true)` and explicit VPN-down confirmation, the identical probe completed in 17 ms but `.160` observed direct/NAT source `192.168.6.36:42356`. The source change is the controlled baseline.
- The first Android probe attempts to `1.1.1.1:443` were rejected as policy-success evidence. Before `.37:25401` was restored, Tauri network events showed repeated `ConnectionRefused` and the probe timed out before TCP. After peer restoration, TCP connected but TLS read timed out; `.37` direct HTTPS also timed out, proving that host has no usable public 443 egress. The final controlled intranet remote-observation fixture avoids this infrastructure dependency.
- Final cleanup: Android VPN stopped, probe packages uninstalled, CDP forward removed, Wi-Fi remained enabled and connected; `.37` core/TUN/HEV/Leaf/listeners/temp files removed; `.160` listener and log removed.

## 2026-07-16 `318497c4` chain/fallback capability audit and exact-artifact matrix (in progress)

Reference semantics inspected before changing any policy behavior:

- Mihomo `/Users/fanli/Documents/mihomo-rev/adapter/outboundgroup/parser.go::ParseProxyGroup` and `adapter/outboundgroup/fallback.go::{findAliveProxy,DialContext,ListenPacketContext,SupportUDP}`: Mihomo fallback selects from actively health-checked proxies, uses the first alive proxy in configured order, and feeds stream first-write failure back to health state. EasyTier intentionally does not claim this active-probe semantic.
- Pinned Leaf `lovitus/leaf@b1e33b50e37ea3b396e3cee2a1d60bb0c599655c`, `leaf/src/proxy/failover/stable.rs::{StableFailover,RequestDecision}` plus `failover/{stream,datagram}.rs`: EasyTier emits `healthCheck=false`, `failover=true`, `stableFailover=true`; TCP and UDP keep separate preference-first passive state, compare actors after actor/association establishment failure, require differential observations across 15-second windows before switching, and use bounded outage backoff/recovery probes.
- Pinned Leaf `leaf/src/proxy/chain/outbound/{stream,datagram}.rs`: TCP applies actors in declaration order. UDP chooses unreliable datagram composition only when all remaining actors report unreliable support.
- Pinned Leaf `leaf/src/proxy/socks/outbound/datagram.rs::Handler::handle`: the implementation explicitly says `TODO support chaining`, ignores the supplied chain transport, and opens a new direct TCP/UDP association. The ten upstream `test_out_chain_1` through `test_out_chain_10` cases cover TCP-oriented WS/Trojan/Shadowsocks combinations and contain no SOCKS5 UDP multi-hop case.

Exact artifact and isolated topology:

- Candidate: `318497c4fd8450a8fee237ef5826841c60517b0c`, previously verified profiling-beta Linux artifact from run `29447382393`.
- `.37` policy client runs in network namespace `leaf318`; `.38` is ordinary mesh peer plus HEV; `.160` is controlled TCP/UDP echo plus second HEV. No new build or workflow was started.
- Controlled target source-address evidence: DIRECT TCP/UDP reached `.160` from `192.168.1.37`; `via: mesh` TCP/UDP reached it from `192.168.1.38`; two-native-SOCKS chain TCP reached it from `192.168.2.160`.
- Two-native-SOCKS chain UDP timed out after both HEV servers accepted associations. This is the pinned Leaf SOCKS UDP chaining limitation above, not evidence that HEV lacks UDP.
- Passive fallback `[unavailable native, mesh, DIRECT]` succeeded for TCP and UDP on the first request and across three 16-second observations; every controlled target request came from `192.168.1.38`.
- User-shape fallback `[two-hop chain, mesh, DIRECT]` kept TCP on the chain and reached the target from `192.168.2.160`, but UDP timed out and did not fall through. The chain returns a successfully created datagram before payload loss, so failover has no actor-establishment error to observe. Do not document or test this as automatic UDP rescue.
- Safe v1 configuration boundary already exists: `NETWORK,udp,<mesh actor/group>` is accepted with strict `tcp|udp` validation and has parser coverage. Put DIRECT/domestic first-match rules before it when those UDP flows must remain direct; route remaining UDP explicitly to mesh instead of relying on chain fallback.
- Tooling gap discovered: `easytier-core::validate_config` requires `--config-file` and only runs `TomlConfigLoader`; it does not parse `--policy-config`. The initial policy-only invocation did not record its exit status and is not evidence of successful policy validation. Normal startup correctly failed closed on the invalid top-level mesh `virtual-ip`. Batch policy parsing into `--check-config` with the next build-affecting candidate rather than triggering a workflow for this diagnostic-only fix.
- Normal phase restart removed the Leaf worker, policy TUN, policy routes, generated `/tmp/easytier-leaf-*` JSON, and left only the namespace underlay route. The fallback recovery and explicit `NETWORK,udp` data-plane checks remain in progress; do not mark this matrix complete yet.

### `318497c4` exact-artifact matrix completion and next candidate manifest

Additional exact-artifact evidence:

- With a recovered native HEV primary, passive fallback TCP moved from mesh source `192.168.1.38` back to primary source `192.168.1.37` across the observation windows. Native SOCKS UDP remained invalid in the policy TUN path even though a standalone RFC1928 probe through the same HEV server succeeded.
- HEV debug showed native UDP ASSOCIATE completing and the UDP splicer starting, followed by immediate session destruction in the policy path. Leaf `async-socks5 0.6.0::SocksDatagram` itself retains the control stream, so the remaining fault is in Leaf TUN/native-datagram session ownership rather than HEV UDP support. This is a separate post-v1 investigation unless native SOCKS UDP becomes a release promise.
- Final safe v1 rules put the controlled DIRECT rule first and then `NETWORK,udp,mesh38`. Ten TCP/UDP probes all passed. Server-observed TCP sources remained DIRECT `.37`, mesh `.38`, two-hop chain `.160`, recovered native fallback `.37`, and chain fallback `.160`; all non-DIRECT UDP sources were mesh `.38`, while DIRECT UDP remained `.37`.
- This proves the supported transition configuration: use chain/fallback for TCP and a first-match `NETWORK,udp,<mesh actor/group>` after required DIRECT/domestic exceptions. Do not claim native SOCKS UDP, SOCKS-over-SOCKS UDP chain, or automatic payload-timeout fallback.

Next build-affecting candidate manifest (local snapshot, SHA assigned only after `.160` passes):

- Implementation files: `easytier/src/policy_proxy.rs`, `easytier/src/core.rs`.
- Reference: Mihomo `/Users/fanli/Documents/mihomo-rev/main.go` test mode calls `executor.Parse`/`ParseWithBytes`; `/Users/fanli/Documents/mihomo-rev/hub/executor/executor.go::{ParseWithPath,ParseWithBytes}` routes the selected source through full `config.Parse` without starting runtime services.
- Intended semantics: EasyTier `--check-config` validates every supplied TOML and its enabled inline/file policy, or validates CLI `--policy-config` without requiring an unrelated TOML. It reuses the same worker path, outbound-interface, strict policy parser, and built-in rule-set resolution as startup, but does not initialize global policy state or start networking.
- Parity test: `check_config_fully_parses_policy_only_input_like_mihomo_test_mode` accepts a valid policy-only invocation and rejects an unknown policy field.
- Mandatory `.160` lane: preflight Cargo/rustc process check; sync full snapshot; `cargo test --locked --no-run --package easytier --bin easytier-core --features leaf-policy-proxy`; run only `check_config_fully_parses_policy_only_input_like_mihomo_test_mode`.
- Pre-push review: Cargo.lock unchanged; Linux/macOS policy cfg remains aligned with existing CLI fields; no workflow pin/generated output change; documentation remains local until this code candidate is ready.
- Authoritative workflow after `.160`: one push to `codex/profiling-beta`, automatically starting the Linux and Android candidate workflows once. While they build, prepare policy-only valid/invalid CLI checks and retain the already-running exact-artifact matrix as the data-plane baseline.

## 2026-07-16 - Local batch candidate preflight and validation cleanup

- Candidate implementation scope remained batched: policy-only `--check-config` now performs the same full policy parse used by startup, rather than validating only the outer CLI/TOML shape. The implementation reuses `policy_proxy::resolve_process_inputs`/`validate_process_config`; startup remains the only path that installs global policy state.
- Reference boundary: Mihomo `main.go` test mode calls `executor.Parse`/`ParseWithBytes`, and `hub/executor/executor.go::{ParseWithPath,ParseWithBytes}` routes through the full `config.Parse` path without starting the runtime. EasyTier intentionally follows that externally observable parse-before-runtime behavior.
- Local formatting used `rustup run 1.95 rustfmt --edition 2024` only. No EasyTier compilation was performed on the maintainer Mac.
- Remote builder `root@192.168.2.160`, container `easytier-debug-builder`, exact synchronized local snapshot: `cargo test --locked --no-run --package easytier --lib --features leaf-policy-proxy` completed successfully in 46.24 seconds with all available CPU cores and debug/incremental settings. The only diagnostic was the pre-existing `parse_system_dns_servers` dead-code warning.
- Exact test binary `/workspace/target/debug/deps/easytier-68b83d7d96a53024` executed `core::tests::check_config_fully_parses_policy_only_input_like_mihomo_test_mode`: 1 passed, 0 failed, 1507 filtered out, 0.02 seconds. The test proves a valid policy-only `NETWORK,udp` configuration succeeds and an unknown policy field fails during check mode.
- An earlier `--bin easytier-core` no-run was the wrong target for this test and produced a harness with zero tests. It is not acceptance evidence. Future candidate manifests must name the library test target before starting the build so one incremental compile and one direct test-binary run are sufficient.
- Efficiency rule applied: `.160` supplied cheap compiler/test feedback for the whole pending Rust batch; GitHub workflow was not triggered for this local candidate or documentation. A workflow should be started once, only after the batch is intentionally committed as the authoritative candidate requiring artifacts/device validation.
- Waiting-time work completed against the already-built exact `318497c4fd8450a8fee237ef5826841c60517b0c` artifact: DIRECT, mesh, two-hop native SOCKS chain, native fallback, chain fallback, TCP/UDP rule separation, cleanup, and pinned-Leaf boundary analysis were combined into one Linux topology rather than separate builds.
- Cleanup completed on `192.168.1.37`, `192.168.1.38`, and `192.168.2.160`: test PID-file processes stopped, `leaf318` netns/veth removed, dedicated forwarding/NAT rules removed, temporary aliases `192.168.2.161/.162/.163/.165/.166` removed, and test files removed. Builder Cargo artifacts were intentionally retained for incremental reuse.

## 2026-07-16 - HEV worker candidate and cross-platform host boundary

### Existing transport semantics confirmed

- The current built-in mesh path already prefers policy UoT v2 for UDP and uses a separately registered policy-only KCP endpoint for TCP. HEV remains the terminal SOCKS5 server and native destination socket owner; it does not absorb EasyTier mesh, Leaf DNS/rules, KCP, or UoT state.
- `RemoteUdpAssociation::open` requests UoT v2 first and closes the reserved association before bounded fallback to the legacy authenticated mesh datagram relay. This is an EasyTier peer capability negotiation, not an unsupported attempt to make ordinary third-party SOCKS servers speak UoT.
- Policy-only KCP starts only the endpoint filter and does not enable the user-facing KCP proxy selector. Existing exact-artifact evidence remains approximately 478 Mbps for the complete Linux built-in HEV TCP path, versus approximately 50 Mbps through the smoltcp fallback and approximately 941 Mbps DIRECT.

### HEV worker-count candidate rejected after no-build comparison

- Reference: pinned HEV `README.md` demonstrates `main.workers: 4`; `src/hev-socks5-proxy.c::{hev_socks5_proxy_init,hev_socks5_proxy_task_entry,hev_socks5_proxy_run}` creates one task-system worker thread per configured worker. `src/hev-config.c::hev_config_init` forces Windows/MSYS back to one worker.
- The exact `318497c4fd8450a8fee237ef5826841c60517b0c` HEV sidecar was run unchanged on `192.168.2.160`. A fixed 64 MiB HTTP object on `192.168.1.37` was fetched through SOCKS from `192.168.1.38` using eight simultaneous connections. No EasyTier build or host-network modification was required.
- `workers: 1`: 512 MiB total in 4.727860800 seconds, 908.438 Mbps aggregate, zero failed transfers.
- `workers: 4`: 512 MiB total in 4.705139323 seconds, 912.825 Mbps aggregate, zero failed transfers.
- The approximately 0.5% difference is below a useful operational gain and both results are already at the controlled network ceiling. Keep the v1 default at one worker to avoid three additional resident threads on Android and low-end peers. Do not add CPU-count auto-tuning without a workload that proves a material gain.
- Dedicated benchmark PIDs, ports `27680/27690`, files, and the copied exact sidecar were removed after the comparison.

### Cross-platform host boundary

- The `easytier-socks-egress::ProcessRuntime` supervisor is structurally usable by macOS and Windows: platform-neutral Tokio child ownership is the primary mechanism and Linux alone adds `PR_SET_PDEATHSIG`. The HEV fork documents Unix, Apple, Android, and MSYS2 build paths.
- macOS is the next lowest-risk platform batch. EasyTier already has `policy_proxy::macos_routing`, builds the Leaf worker, and declares it as a Tauri external binary. The missing work is a pinned HEV executable build, target-suffixed Tauri external-binary packaging, exact macOS lifecycle/TCP/UDP evidence, and route/DNS recovery validation. Do not mix this packaging-only work into the Linux/Android v1 release candidate.
- Windows is not equivalent to macOS. Although the HEV sidecar and its repaired UDP path can run, EasyTier currently gates `policy_proxy` on `unix`, and `easytier-policy::packet` uses Unix datagram FD transport. Windows therefore needs a native packet/TUN handoff plus route/DNS ownership design before HEV packaging is meaningful. Do not claim Windows Leaf support from the standalone HEV result.
- iOS, OHOS, FreeBSD, MIPS, and other special targets remain design targets. Reuse the same narrow HEV lifecycle/config contract where their host model permits it, but track each packet transport and package independently rather than adding platform conditionals to HEV's SOCKS state machine.

## 2026-07-16 - Reusable `.160` Leaf/HEV preflight entry point

- Added `scripts/leaf-remote-preflight.sh` so the mandatory builder sequence is executable policy rather than a manually reconstructed set of SSH commands.
- The script synchronizes one complete snapshot, rejects concurrent Cargo/rustc work, carries the required reverse `7890` tunnel on the Cargo invocation, performs one `--locked` `easytier --lib --features leaf-policy-proxy` no-run build, resolves the emitted `src/lib.rs` test binary, and runs the focused tests directly with one test thread.
- Default focused coverage is policy-only full-config check mode, policy-KCP/user-SOCKS isolation, OSPF route-generation cache invalidation, the complete mesh UDP/UoT relay module, and awaited HEV guard shutdown. Additional batch-specific filters can be appended without starting another Cargo build.
- The tool does not create optimized artifacts, trigger GitHub, commit, push, or deploy. GitHub remains the sole source of authoritative release/profile artifacts after this diagnostic gate passes.

## 2026-07-16 - Managed mesh actor UDP default and visible capability

- Mihomo reference: `/Users/fanli/Documents/mihomo-rev/adapter/parser.go::ParseProxy` decodes `Socks5Option`; `/Users/fanli/Documents/mihomo-rev/adapter/outbound/socks5.go::{Socks5Option,NewSocks5}` copies the explicit `udp` field into `BaseOption`; `/Users/fanli/Documents/mihomo-rev/adapter/outbound/base.go::SupportUDP` returns that declared capability. A generic omitted SOCKS5 `udp` field therefore remains false because Mihomo cannot know the server's behavior.
- EasyTier intentionally differs only for a newly created `via: mesh` row. That row selects the managed built-in HEV endpoint, not an arbitrary SOCKS server; exact Linux and Android artifacts have already validated UDP ASSOCIATE plus policy UoT/mesh fallback. The editor now defaults that known actor to `udp: true` while retaining the serialized capability field.
- The UDP checkbox is now visible in the normal node table instead of being hidden behind the advanced-feature unlock. If the user switches the row to an independently managed native SOCKS server, they can explicitly clear the capability rather than unknowingly inheriting an invisible value.
- Added a frontend regression contract: adding a node in ordinary policy mode serializes `via: mesh`, omits `port`, and emits `udp: true`. This closes the previous mismatch where the documented mesh example supported UDP but the normal Add Node action silently created a TCP-only actor.

## 2026-07-16: Route-aware DNS semantics audit (current working candidate)

Status: compiler contract test added locally; no runtime implementation change and no workflow dispatch.

Reference behavior inspected before editing:

- Mihomo `/Users/fanli/Documents/mihomo-rev/adapter/outbound/direct.go::Direct::DialContext` appends `dialer.WithResolver(resolver.DirectHostResolver)`; `Direct::ResolveUDP` also resolves the destination host with `DirectHostResolver`.
- Mihomo `/Users/fanli/Documents/mihomo-rev/adapter/outbound/base.go::Base::ResolveUDP` uses `resolver.DefaultResolver` for actors that require local UDP resolution. Proxy server host bootstrap is separately wired through `ProxyServerHostResolver` in `hub/executor/executor.go::updateDNS`.
- Pinned Leaf `leaf/src/proxy/mod.rs::{connect_stream_outbound,connect_datagram_outbound,new_tcp_stream}` and `leaf/src/common/resolver.rs::Resolver::new` show that a DIRECT destination is locally resolved through `DnsClient::direct_lookup`. A SOCKS actor locally resolves only its server address, then receives the original destination domain in its session.
- Pinned Leaf `leaf/src/proxy/failover/{stream,datagram}.rs` calls `connect_*_outbound` for the actual candidate actor, so a fallback containing SOCKS and DIRECT preserves each member's own DNS behavior. `leaf/src/proxy/chain/outbound/stream.rs::{next_connect_addr,next_session,handle}` resolves/connects the first hop and passes the final destination domain through the chain.
- Android Mihomo `/Users/fanli/Documents/clashmeta-android-rev/core/src/foss/golang/clash/dns/resolver.go::{Resolvers::ClearCache,Resolvers::ResetConnection}` treats network-change recovery as a resolver lifecycle issue across default, proxy, and direct resolvers. EasyTier's Android Leaf restart on underlay/DNS generation change is the current v1 equivalent; cache/transport mutation inside pinned Leaf remains intentionally avoided.

Externally observable EasyTier contract:

- Domain rules are first-match evaluated without global pre-resolution because generated Leaf JSON keeps `router.domainResolve=false`.
- A DIRECT rule uses the generated `direct:` resolver set and exits on the local underlay.
- A SOCKS/managed-mesh rule leaves the destination as a domain; the selected SOCKS server/mesh peer resolves it at that exit. Only the proxy server's own hostname is bootstrap-resolved locally.
- A fallback or chain does not have one fixed DNS exit of its own; resolution follows the actual member/hop behavior. This is required for a fallback such as `[native-chain, mesh, DIRECT]`.
- Explicit `dns.proxy` remains available for Leaf rule-resolution and DNS-service paths; it does not replace SOCKS remote destination resolution.

Added regression: `easytier-policy::leaf_config::tests::preserves_route_aware_dns_contract_for_direct_socks_and_fallback` locks the EasyTier-generated half of this contract: separate direct/proxy DNS entries, disabled global domain resolution, preserved SOCKS server domain, fallback actor order, and no implicit `resolveDomain` on domain rules.

Validation plan: include this test in the next batched `scripts/leaf-remote-preflight.sh` run on `192.168.2.160`; do not start a separate build or GitHub workflow for this one test.
EOF\n
## 2026-07-16: Next batched candidate manifest before .160 preflight

Snapshot identity: current complete local working snapshot based on validated candidate `318497c4fd8450a8fee237ef5826841c60517b0c`; no candidate commit/SHA assigned yet.

Included implementation and regression scope:

- `easytier/src/policy_proxy.rs`: side-effect-free full process-policy validation shared by startup and `--check-config`.
- `easytier/src/core.rs`: policy-only and embedded/file `--check-config` fully parse the policy document instead of validating only outer CLI syntax.
- `easytier-policy/src/leaf_config.rs`: route-aware DNS compiler contract for DIRECT, SOCKS, fallback, and chain semantics; no new DNS runtime.
- `easytier-web/frontend-lib/src/components/policy/PolicyEditor.vue` and its component test: newly added managed mesh actor defaults to known-supported UDP while native SOCKS remains user-controlled.
- `scripts/leaf-remote-preflight.sh` and `AGENTS.md`: one complete snapshot sync, one locked no-run Cargo invocation, exact EasyTier and policy test binaries, and the full focused Leaf/HEV suite.

Mandatory `192.168.2.160` evidence:

- Run `scripts/leaf-remote-preflight.sh` after Rust 1.95/edition-2024 formatting.
- Required focused filters: full policy-only check-config; route-aware DNS compiler contract; policy-only KCP endpoint isolation; peer-generation invalidation/restart; mesh UoT relay suite; owned HEV task shutdown.
- One Cargo build invocation only. Both package test binaries are resolved from that build log; a package-mismatched filter that runs zero tests is not evidence.

Authoritative workflow plan after the complete candidate is intentionally committed:

- One push to `codex/profiling-beta`; rely on its automatic Linux and Android workflow pair and do not manually duplicate dispatch.
- Linux artifact evidence: policy-only check-config failure/success boundary, DIRECT/SOCKS/fallback DNS egress observation, existing HEV TCP/UDP and cleanup smoke.
- Android artifact evidence: policy document retention, managed mesh UDP default, underlay/DNS generation restart, Wi-Fi detached disable/enable recovery, exact application-UID policy probe, resource return after stop/start.
- No macOS/Windows workflow in v1 routine scope.

Productive wait work:

- During `.160`: finish source-level DNS/group audit and prepare bounded Linux DNS fixtures without mutating the synchronized snapshot.
- During GitHub: pre-clean Linux hosts, verify wireless ADB and detached Wi-Fi recovery script, prepare artifact checksum/build-info verification, and stage Linux/Android probes.
- Run independent Linux and Android sessions concurrently from the exact workflow artifacts.

Dispatch lock: do not push or start workflows unless this complete snapshot passes the `.160` locked no-run and all focused filters. Documentation remains local until it accompanies that code candidate.
EOF\n
### Correction: fallback-to-DIRECT DNS server selection boundary

The preceding route-aware DNS entry overstated one pinned Leaf case. The connection target behavior is correct: SOCKS receives the domain and DIRECT calls `direct_lookup`. However, `leaf/src/app/dns/client.rs::query_record_type` currently selects the DNS server subset solely from `is_direct_outbound(host)`, which asks the router for the outer rule target. `leaf/src/app/outbound/manager.rs` marks only the literal direct handler as `is_direct=true`; failover/chain handlers remain false.

Consequence: a literal `...,DIRECT` rule selects `direct:` DNS as intended, and a SOCKS member still resolves the target remotely. But if a rule targets a fallback group and that group actually reaches its DIRECT member, `direct_lookup(host)` can select the normal/proxy DNS subset because the outer group tag is not direct. The actual network exit remains DIRECT; the DNS server choice is wrong for the requested contract.

Preferred minimal Leaf fix: in both `DnsClient::query_record_type` and `DnsClient::query_ech_record_type`, select direct-marked servers when the explicit lookup call is direct OR the router classifies the host as direct: `collect_servers(is_direct || is_direct_outbound)`. Add pinned Leaf unit coverage for (1) explicit direct lookup under a non-direct group route, (2) normal lookup under literal DIRECT, and (3) normal proxy lookup. This keeps EasyTier decoupled from Leaf group internals.

The EasyTier regression was renamed to `preserves_unresolved_domain_contract_for_direct_socks_and_fallback`; it locks only the compiler-owned half and explicitly records that the pinned Leaf fix is still required. The first `.160` batch attempt also correctly rejected the anonymous test resolver because its closure was not higher-ranked over `&str`; it now reuses the existing `Unresolved` trait implementation. No GitHub workflow was started.
EOF\n
### Second correction: exact pinned Leaf already fixes group-to-DIRECT DNS selection

The immediately preceding correction is invalid because it was derived from the stale Cargo checkout worktree `.../leaf-6ac41ef5369474b3/2f99d4f`, not the revision in EasyTier's lockfile. `Cargo.lock` authoritatively pins `lovitus/leaf@b1e33b50e37ea3b396e3cee2a1d60bb0c599655c`. A dedicated worktree at that exact SHA shows eight commits beyond the repository's current `master`.

At `b1e33b50`:

- `leaf/src/app/dns/client.rs::_lookup_inner` maps every explicit `direct_lookup` to `DnsQueryRoute::direct()` before cache lookup or query construction.
- `query_record_type` selects servers with `collect_servers(route.direct)`; `resolve_with_server` also uses the same route to force the physical direct socket.
- Direct and policy DNS caches are keyed in separate resolver scopes.
- Normal application lookup still uses `query_route_for_host`, preserving first-match rule selection and an explicit proxy outbound tag for dispatched DNS.
- Consequently, when failover reaches its DIRECT member, that member's `direct_lookup` does use the `direct:` server subset even though the outer group handler is not marked direct. The proposed two-line Leaf patch is unnecessary and must not be applied.

The correct v1 conclusion is restored: literal DIRECT and fallback-to-DIRECT resolve with the direct set; SOCKS/managed mesh keeps the target domain for the selected node; normal Leaf DNS-service/rule lookups follow their selected outbound route. EasyTier only needs the compiler-owned regression `preserves_unresolved_domain_contract_for_direct_socks_and_fallback`.

Process fix added to `AGENTS.md`: every Cargo git dependency audit must start from the exact `Cargo.lock` URL/SHA and an explicitly aligned worktree. Cache directory names and stale checkout contents are not evidence.
EOF\n
## 2026-07-16: Batched .160 preflight and focused frontend evidence

Remote builder: `root@192.168.2.160`, container `easytier-debug-builder`. No GitHub workflow or optimized artifact build was started.

First preflight attempt:

- Complete snapshot sync and busy-process check succeeded (`CLEAR`).
- The single multi-package no-run correctly failed in `easytier-policy/src/leaf_config.rs`: the anonymous resolver closure was not higher-ranked over every `&str` lifetime.
- This was routine compiler feedback caught on `.160`, not sent to GitHub. The test now reuses the existing concrete `Unresolved: MeshServerResolver`.

Final preflight:

- Command owner: `scripts/leaf-remote-preflight.sh`.
- One Cargo invocation: locked, debug/no-run, all CPU cores, packages `easytier` and `easytier-policy`, feature `easytier/leaf-policy-proxy`.
- Build result: success in 31.11 seconds.
- Exact EasyTier test binary: `/workspace/target/debug/deps/easytier-68b83d7d96a53024`.
- Exact policy test binary: `/workspace/target/debug/deps/easytier_policy-9738e97381dbc100`.
- EasyTier focused results: policy-only full check-config 1/1; KCP endpoint isolation 1/1; peer-generation invalidation/restart 1/1; mesh UDP/UoT suite 8/8; owned HEV task shutdown 1/1.
- Policy focused result: unresolved-domain DIRECT/SOCKS/fallback compiler contract 1/1.
- Aggregate focused result: 13 passed, 0 failed. Network-sensitive filters ran serially.
- The only build warning was the pre-existing test-only helper `parse_system_dns_servers` reported as dead code in the non-test library phase.

Independent local small test, not a package/build:

- `pnpm --dir easytier-web/frontend-lib exec vitest run tests/policy-editor.spec.ts`.
- Result: one test file passed, 9/9 tests, 43 ms test time, 1.62 s total.
- This covers the managed mesh actor default `udp: true`, omitted mesh port, and existing policy editor behavior.
- The first command typo used an extra `easytier/` path and exited before Vitest started; it is not counted as evidence.

Current dispatch status:

- Mandatory `.160` pre-push gate is satisfied for the complete build-affecting snapshot.
- Local formatting and focused frontend evidence are complete.
- No candidate commit/SHA exists yet, so authoritative Linux/Android workflows and exact-artifact real-device validation remain pending.
EOF\n

## 2026-07-16: Candidate freeze boundary after preflight

Repository alignment:

- Local branch: `codex/profiling-beta`.
- Local HEAD: `318497c4fd8450a8fee237ef5826841c60517b0c`.
- `origin/codex/profiling-beta`: the same exact SHA.
- No ahead/behind delta and no staged files. This removes the prior branch-pointer/worktree ambiguity.

Explicit next-candidate scope:

- `AGENTS.md`
- `easytier-policy/src/leaf_config.rs`
- `easytier-web/frontend-lib/src/components/policy/PolicyEditor.vue`
- `easytier-web/frontend-lib/tests/policy-editor.spec.ts`
- `easytier/docs/todo/leaf_v1_release_gates.md`
- `easytier/docs/todo/leaf_validation_journal.md`
- `easytier/src/core.rs`
- `easytier/src/policy_proxy.rs`
- `scripts/leaf-remote-preflight.sh` (executable)

The tracked diff is 452 insertions and 42 deletions before this journal entry; `git diff --check` reported no whitespace errors. The only file under untracked `scripts/` is the intended preflight script.

Must remain unstaged and uncommitted:

- `.artifacts/`
- `.claude/`
- `tauri-plugin-vpnservice/android/.gradle/`

These paths are currently untracked and not ignored, so the candidate must use an explicit file list and must never use `git add .`.

Pinned Leaf investigation workspace:

- `/Volumes/micron512g/code/leaf` is a clean dedicated clone on local branch `codex/direct-dns-group`, checked out at exact EasyTier lock revision `b1e33b50e37ea3b396e3cee2a1d60bb0c599655c`.
- No Leaf source modification is required or present. The exact pin already implements `DnsQueryRoute::direct`, route-scoped DNS dispatch, and direct/policy cache separation.
- This workspace is reference evidence only and is not part of the EasyTier candidate.

No commit, push, workflow dispatch, or artifact build was performed during this freeze step.


### Candidate scope docs-only addition after preflight

After the successful `.160` gate, `easytier/docs/todo/user_undecided_mesh_peer_egress.md` received section 41 only. It reconciles the adopted HEV v1 implementation with the still-undecided native MeshEgressDialer and records macOS, Windows, iOS/OHOS, FreeBSD, special-target, DNS, UoT/KCP, and v1/v2 boundaries. This is documentation-only, does not alter the synchronized build-affecting snapshot, and therefore does not justify another Cargo build or workflow dispatch. Add this exact file to the explicit candidate stage list.


### Preflight log cleanup and local staging guard

Post-preflight, `scripts/leaf-remote-preflight.sh` changed only its remote test-header construction so logs render a real newline instead of `=== TEST ... ===n`. `bash -n` and the script's `--help` path both pass. No Cargo command or workflow was repeated because this does not affect source compilation, test selection, or execution.

Local-only `.git/info/exclude` now contains:

- `.artifacts/`
- `.claude/`
- `tauri-plugin-vpnservice/android/.gradle/`

The three unrelated generated/local trees no longer appear in `git status`; they remain untracked and are not part of repository commits. Candidate status now consists only of the nine intended tracked files plus `scripts/leaf-remote-preflight.sh`.

Tooling discovery: in zsh, assigning loop variable `path` overwrites the special `path` array and therefore `PATH`; this caused three non-fatal `rg: command not found` messages during the initial exclude loop. Shell builtins still appended the entries, and a separate status check confirmed the intended result. The rule is now recorded in `AGENTS.md`; future commands use `candidate_path` or `file_path`.

## 2026-07-16 - Exact candidate artifact intake prepared

- Inspected the current Linux profiling and Android policy-candidate workflows once. A single push to `codex/profiling-beta` triggers both; no duplicate manual dispatch is needed.
- Added `scripts/leaf-candidate-artifacts.sh` to bind artifact intake to one 40-character commit SHA. It requires exactly one successful run per workflow, verifies the rolling Linux tag before and after download, checks outer and inner Linux hashes, checks Android hashes, validates both `BUILD_INFO.txt` files, and rejects mismatched HEV pins.
- The script deliberately performs no build, deployment, ADB action, route change, or service start. Its output is an immutable per-SHA directory under `.artifacts/leaf-candidates/`, preventing evidence from two candidates from being mixed.
- This automation has not contacted GitHub yet because the frozen candidate has not been authorized for commit/push. After authorization, the two workflows should run once; artifact verification and Linux/Android deployment preparation can proceed in parallel while real-device gates remain tied to the same SHA.

## 2026-07-16 - Android non-visual candidate validation prepared

- Inspected `PolicyProbeInstrumentation`, `VpnServicePlugin`, and `TauriVpnService`. The existing probe executes bounded TCP/TLS connections from a separate captured UID and reports connection, TLS, elapsed-time, UID, SELinux context, and error evidence through instrumentation output.
- The production Android start/stop API is intentionally available through the Tauri plugin, not an exported test service. Directly starting `TauriVpnService` with ADB would bypass the real ownership, callback, revocation, and configuration-generation path, so validation automation must not use that shortcut.
- Added `scripts/leaf-android-validate.sh` with four explicit phases: exact artifact installation, text-only state/resource snapshots, matrix-driven captured-UID probes, and Wi-Fi outage/recovery probes.
- The outage command schedules a detached on-device `svc wifi enable` before executing `svc wifi disable`. This preserves the wireless ADB recovery path instead of disconnecting the device indefinitely.
- The script uses ADB/instrumentation and dumpsys/logcat evidence; it does not use screenshots or simulated taps. Initial VPN authorization and start still use the real application path. A final screenshot remains optional presentation evidence, not functional evidence.
- No device command has been run in this preparation step. Runtime evidence remains pending the exact pushed candidate artifact.

## 2026-07-16 - Linux isolated fault execution prepared

- Reviewed the existing Linux policy validation report. The accepted evidence already covers routing ownership, fail-closed underlay loss, worker supervision, UoT/KCP behavior, mixed-version fallback, cleanup, and resource return, but the fault commands and measurements were not yet exposed as a reusable execution surface.
- Added `scripts/leaf-linux-validate.sh`. It refuses to operate without root and an existing named network namespace, so it cannot delete or replace the host default route.
- The script captures addresses, rules, main/table-52000 routes, sockets, per-process RSS, FD counts, and thread counts. It supports supervised Leaf-worker kill/restart and exact default-route removal/restoration with an EXIT/INT/TERM restoration trap.
- Data-plane checks are executable hooks rather than embedded topology assumptions: mesh continuity, normal policy success, and outage fail-closed probes remain independently replaceable and can reuse the current curl/iperf tooling.
- Combined with the Android instrumentation runner and exact artifact intake, the post-workflow path is now parallel: verify one SHA, deploy Linux and Android independently, run text-based fault/resource matrices, then collect only final presentation screenshots if desired.
- No namespace, route, process, device, builder, or GitHub command was executed while preparing this script. Runtime qualification still requires the exact pushed candidate.

## 2026-07-16 - Workflow authorization clarified

- Maintainer authorization is standing rather than per-push: after a batched candidate is frozen and the `.160` preflight passes, the agent may commit and push it to `codex/profiling-beta` autonomously.
- The efficiency constraint remains strict: GitHub Actions must not be used for one-line compiler feedback. Related implementation, tests, platform fixes, artifact intake, and validation scenarios are accumulated, checked together on `.160`, and submitted as one exact candidate SHA.

## 2026-07-16 - Android FD evidence correction after candidate push

- A read-only pre-install snapshot on the Android 8 validation device showed that shell can read process status but cannot enumerate the candidate application's `/proc/<pid>/fd`; the first script version therefore printed a misleading zero FD count.
- Confirmed that the debug candidate's `run-as` identity can enumerate the same PID: 231 FDs and 58 tasks at the observed pre-install baseline, while `/proc/<pid>/status` reported 182476 KiB VmRSS and 58 threads.
- Updated the local validation script to use `run-as` only for FD/task enumeration. This post-push tooling correction is intentionally not being pushed as a second Workflow candidate; it will accumulate with the next code batch unless the current candidate itself fails and requires a replacement.

- The first captured-UID matrix run executed only its first row because `adb` inherited the TSV loop's stdin and consumed the remaining rows. The local validation script now redirects instrumentation stdin from `/dev/null`; this is validation-tool-only and remains accumulated for the next code batch.

## 2026-07-16 - exact candidate `824ac5a1d47d568113a7e2190d57fecf049dd47b`

Candidate and artifact binding:

- Branch: `codex/profiling-beta`; commit: `824ac5a1d47d568113a7e2190d57fecf049dd47b`.
- Linux profiling-beta run `29461390271` succeeded. Android policy candidate run `29461390297` succeeded.
- Downloaded Linux release and Android workflow artifacts were bound to the full commit SHA. `SHA256SUMS.txt`, `BUILD_INFO.txt`, run ID, target, build ID, symbols, four Linux binaries, APK, probe, and runner hashes all matched.
- A local `shasum` invocation initially failed because Perl could not load the host `C.UTF-8` locale. Re-running artifact verification with `LC_ALL=C LANG=C` passed; this was a local verification environment issue, not an artifact mismatch.

Linux exact-artifact validation:

- Hosts: `192.168.1.37` hub plus isolated namespace source, destination `192.168.1.38`; virtual addresses `10.247.0.1/10.247.0.2`; explicit listener ranges `25400..25404` and `25410..25414`.
- Managed and DIRECT configurations passed target-side `--check-config`. Mesh ICMP passed, FakeDNS returned `198.18.0.1`, and TLS/HTTP to `example.com` passed through policy routing.
- GitHub access timed out from both validation hosts without EasyTier, so it was classified as validation-host underlay reachability rather than a candidate regression.
- Managed HEV TCP reached 313 Mbit/s receiver with zero retransmissions.
- UDP/UoT at 10 Mbit/s delivered `4641/4641` datagrams with zero loss. At 20 Mbit/s for 20 seconds it lost `119/37692` datagrams (`0.32%`). Trace logging showed `dst_allow_kcp: true` and `socks5 auto connector selected kcp` for the managed endpoint and private UoT stream; an empty `easytier-cli proxy` view is not accepted as transport proof.
- Killing the Leaf worker changed PID `30481 -> 31509`; mesh and policy traffic recovered. After recovery: core about 19 MiB, 31 FD, 9 threads; HEV 252 KiB, 12 FD, 2 threads; worker 6308 KiB, 12 FD, 4 threads.
- Removing the DIRECT default route kept mesh ICMP working and made DIRECT policy DNS fail closed. Restoring the route restored policy traffic. Resource counts returned to the same range.
- Startup with the managed endpoint temporarily absent produced an expected initial policy failure. The peer route arrived about one second later, system DNS recovered, and the Leaf worker recovered about 4.3 seconds after the initial error without restarting the core.
- Normal stop removed namespace processes, TUN, policy rules `10899/10900`, and table `52000`. Final host and namespace cleanup found no EasyTier core, Leaf worker, HEV process, `tun0`, or `tun1`.

Android exact-artifact validation:

- Device: `192.168.234.227:5555`; package `com.kkrainbow.easytier.policycandidate`; installed with `install -r -t` while preserving the saved configuration.
- Configuration was updated through WebView DevTools DOM events rather than screenshots or coordinate clicks: network `leaf-hev-824ac5a1`, virtual IP `10.247.0.3`, managed mesh endpoint `10.247.0.1`, `via: mesh`, `udp: true`, and no user-specified managed SOCKS port.
- The peer view contained the exact candidate on local `10.247.0.3`, hub `10.247.0.1`, and source `10.247.0.2`. `TauriVpnService` owned the VPN and `tun0` held `10.247.0.3/24`.
- Captured-UID probes passed for mesh owner TCP (`10.247.0.1:25401`), policy-domain TLS (`example.com:443` with SNI), and managed HEV TCP (`192.168.1.38:24500`). The validation script must redirect each `adb shell` invocation from `/dev/null`; otherwise ADB consumes the matrix loop's stdin and only the first row executes.
- Before a Wi-Fi outage all three probes passed. Wi-Fi re-enable was scheduled before disabling Wi-Fi so wireless ADB could recover. After reconnection the same process, `TauriVpnService`, and `tun0` were present, `vpn_network_changed` reported the new Wi-Fi generation and DNS servers, and all three probes passed again.
- Resource snapshots around Wi-Fi recovery were 220076 KiB/369 FD/68 threads before and 222932 KiB/359 FD/69 threads after. The first immediate snapshot missed the process transiently and was not accepted until VPN ownership and a later resource snapshot were confirmed.
- A normal stop removed the VPN service and `tun0` while retaining the app configuration. Restart restored all three peers and all three probes. Stop/start resource snapshots were 224128 KiB/366 FD/69 threads before and 222572 KiB/371 FD/69 threads after.
- Device `/system/bin/iperf3` supplied direct UDP evidence. At 10 Mbit/s for 5 seconds the local source was VPN address `10.247.0.3` and receiver loss was `0/4960`. At 20 Mbit/s for 10 seconds the source remained `10.247.0.3` and receiver loss was `0/19840`. This excludes physical-Wi-Fi bypass and validates Android HEV/Leaf UDP forwarding for this candidate. The final UDP snapshot was 237732 KiB RSS, 356 FD, 69 threads.
- Final normal stop removed `TauriVpnService` and `tun0`. The temporary Linux hub also stopped cleanly with no remaining core, worker, HEV, `tun0`, or `tun1`.

Assessment boundary:

- This closes the current exact candidate's first-pass Linux and Android artifact, ownership, TCP, UDP/UoT, network-change recovery, worker recovery, fail-closed, configuration retention, and cleanup matrix.
- It does not replace longer repeated lifecycle/resource soak testing, and it does not promote unvalidated split-DNS, chain/fallback generation changes, high-loss UDP behavior, multi-instance, netns, HTTP actor, or non-Linux/Android targets into the v1 compatibility claim.
- Post-push script corrections (`run-as` FD counting and `adb shell </dev/null`) and this journal entry remain local until the next code-bearing candidate or an explicit documentation push; they must not trigger a workflow by themselves.

## 2026-07-16: `824ac5a1` overseas chain/fallback validation

Candidate and deployment:

- Exact candidate: `824ac5a1d47d568113a7e2190d57fecf049dd47b` (`2.6.10-824ac5a1`).
- The verified x86_64-musl four-binary bundle was deployed to `lv1g2` and `lv1g3`; hashes and embedded version matched the profiling-beta artifact.
- Public overseas host names and addresses are intentionally omitted. Validation used the locally configured aliases only.
- Policy client: `192.168.1.37`, virtual IP `10.249.0.1`.
- `lv1g2`: virtual IP `10.249.0.2`, managed HEV plus a loopback native SOCKS service.
- `lv1g3`: virtual IP `10.249.0.3`, managed HEV plus controlled TCP/UDP targets.
- Every EasyTier listener used an explicit isolated port in the `25500..25524` range. Test SOCKS and target services used separate explicit ports.

Validated policy topology:

- `mesh-hop`: portless `via: mesh` hop to `lv1g2`.
- `peer-local-socks`: native SOCKS at loopback on `lv1g2`.
- `peer-chain`: ordered chain `[mesh-hop, peer-local-socks]`.
- `mesh-direct`: portless `via: mesh` hop to `lv1g3`.
- `overseas-fallback`: `[peer-chain, mesh-direct]`.
- Domestic and default groups remained DIRECT.
- Explicit UDP rules selected `mesh-direct`; GeoSite, GeoIP, custom domain and custom IP rules selected the overseas fallback.

Results:

- Bundled rule resources loaded: GeoIP CN `112008`, GeoSite GitHub `63`, GeoSite `geolocation-!cn` `26948` entries.
- `GEOSITE,github`: HTTPS returned 200 and the `lv1g2` SOCKS log recorded `github.com:443`.
- Custom domain `example.com`: HTTPS returned 200 and the `lv1g2` SOCKS log recorded the destination.
- `GEOIP,google`: TCP to `8.8.8.8:443` passed through the primary chain and appeared in the `lv1g2` SOCKS log.
- Custom IP TCP throughput reached approximately 20 Mbps; the controlled target observed `lv1g2` as the source.
- Explicit UDP via `mesh-direct`: 2321/2321 datagrams, zero loss, approximately 4.9 Mbps; the controlled target observed `lv1g3` as the source.
- After stopping the `lv1g2` SOCKS service, a new single-connection HTTP request returned 200 through `lv1g3`, proving fallback.
- After restoring the SOCKS service, four consecutive requests appeared in its log, proving failback to the first member.

Failure boundary established by validation:

- Fallback is connection-scoped. It does not migrate established connections.
- During the first transition, a multi-connection protocol can establish its control connection through one member and its data connection after the fallback state changes. The initial `iperf3` transaction demonstrated this boundary and required a whole-transaction retry.
- This is not documented as seamless in-flight failover. Single-connection HTTP fallback and subsequent failback are the v1 guarantee.

Resource snapshot before cleanup on the policy client:

- EasyTier core: 20872 KiB RSS, 10 threads, 32 file descriptors.
- Leaf worker: 17196 KiB RSS, 4 threads, 25 file descriptors.
- HEV: 256 KiB RSS, 2 threads, 12 file descriptors.

Validation tooling note:

- The macOS resolver initially returned a FakeIP for an overseas alias. Using that address as a controlled target caused the first `iperf3` attempt to hang. The target address was replaced with the address reported by the overseas host itself and the policy was hot-reloaded. This was an orchestration error, not a product failure.

Cleanup evidence:

- All exact EasyTier, Leaf/HEV, test SOCKS, HTTP and `iperf3` processes were stopped.
- Test TUN devices, policy rules/tables, explicit listeners and temporary firewall rules were absent after cleanup.
- The hosts' pre-existing production SOCKS service was not modified.
- Exact candidate binaries were retained on the overseas nodes for later validation reuse.

Documentation test preflight note:

- The first remote execution of `documented_leaf_policy_v1_example_parses_and_compiles` reached production validation and rejected the test-only bridge password `documented-test-secret` because bridge credentials are alphanumeric. The fixture was corrected to `documentedtestsecret`; the exact remote test then passed (`1 passed`, 1508 filtered out). Production behavior was not changed.

## 2026-07-16: Android browser domain-loss diagnosis and host fix

Observed failure and isolation:

- The saved Android policy selected the explicit mesh SOCKS actor for foreign GeoSite/domain rules. Mesh ICMP, TCP port `24443`, and the SOCKS greeting were healthy. A SOCKS request carrying the destination as a domain returned HTTP 200, while the same request carrying the locally resolved Google IP timed out after the SOCKS server accepted the TLS ClientHello.
- Android reported `DnsAddresses: []`, and `TauriVpnService` logged `dns:null`. The browser therefore resolved domains before packets entered Leaf. Leaf received only the real destination IP and could not reconstruct the domain for GeoSite matching or a SOCKS `ATYP=DOMAIN` request.
- The reverted mesh-transport experiment changed the explicit actor leg from KCP-backed transport to ordinary mesh TCP but produced the same failure. Mesh encapsulation and overlay selection are therefore outside this fix and must remain unchanged.

Reference behavior followed before editing:

- Mihomo `/Users/fanli/Documents/mihomo-rev/adapter/outbound/socks5.go`: `StreamConnContext` and `DialContext` pass the original metadata destination to `dialSocksServer`, so a preserved domain is encoded as a SOCKS domain rather than prematurely resolved to an IP.
- ClashMeta Android `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt`: `startTun` publishes the TUN DNS address with `VpnService.Builder.addDnsServer`. `TunModule.attach` passes the TUN FD and virtual DNS parameters to the proxy core.
- Pinned Leaf `2f62208187f7980d066e479bd70bb55613c066d2`: `proxy/tun/inbound.rs::handle_inbound_datagram_{lwip,smoltcp}` intercepts UDP destination port 53, `app/fake_dns.rs::generate_fake_response` allocates a FakeIP, and `handle_inbound_stream_{lwip,smoltcp}` restores the paired domain before dispatch.

Implementation and compatibility boundary:

- Android policy-only mode now publishes `198.19.0.1` as a virtual DNS sink. Policy mode already installs IPv4/IPv6 default VPN routes, so DNS packets reach the existing Leaf TUN inbound. The address is inside reserved `198.18.0.0/15` but outside Leaf's `198.18.0.0/16` allocation pool, avoiding an alias with generated FakeIPs.
- Magic DNS still takes priority at `100.100.100.101` and remains mesh-owned. This fix intentionally does not pretend to implement split DNS when Magic DNS and policy routing are enabled together; that documented compatibility boundary remains unchanged.
- `getDnsForVpn` unit coverage pins disabled, policy-only, Magic-DNS-only, and combined-feature selection. Real Android evidence still required: VPN DNS ownership, browser GeoSite/domain routing, DIRECT domestic browsing, explicit mesh SOCKS domain form, Magic DNS non-regression, Wi-Fi recovery, and normal stop/start cleanup.

## 2026-07-16 - `5d71abed` Linux combined failover/UDP and Android Wi-Fi evidence

- Exact Linux artifact: `5d71abed66a1ad1957834a40bc67b0d0092a95af` from the already verified profiling-beta run. No local build and no additional workflow were used.
- A target restart changed the target peer identity and disabled QUIC input. The source EasyTier core stayed alive, the supervised Leaf worker was recreated, and the next portless flow selected KCP. Twenty repeated TCP requests succeeded, with one source Leaf child and one target managed HEV child.
- A stacked policy used TCP `peer-chain -> mesh-direct` fallback and an explicit `NETWORK,udp,mesh-direct` rule. The healthy chain's two target-side hops were observed directly in `ss`. Stopping only the peer-local SOCKS listener moved new TCP connections to managed HEV direct egress while a long UDP association continued without loss.
- During the subsequent target restart, a 400-datagram socket had 23 expected outage timeouts and resumed on the same application socket for 377 successful datagrams. A narrow source `ens192 -> 203.0.113.10:28101/udp` REJECT/counter rule stayed at zero and was removed immediately afterward.
- Ten concurrent UDP associations produced a measurable active-state FD increase, then returned below the earlier not-yet-idle snapshot after the 120-second timeout: source core/Leaf `57/36 -> 37/16`; target core/HEV `88/48 -> 38/18`. Thread counts returned to `9/4` and `9/2`; no extra child remained.
- Android candidate data was preserved and the candidate package was not uninstalled. A formal detached device-side Wi-Fi script was verified alive with its log created before the delay, then recorded `disabled rc=0`, 12 seconds of outage, and `enabled rc=0`. Candidate PID `22669`, VPN network `305`, owner UID `10254`, and TUN ownership survived. The underlying Wi-Fi network changed to `309`, and native callbacks recorded `outage!4` followed by a recovered key with the DHCP DNS set.
- Captured UID `10290` completed Baidu and Cloudflare TLS handshakes before and after recovery. Mesh ICMP also recovered. Screenshots and coordinate taps were not used.
- Android resource closure remains pending: correct entry-wise FD counts changed `402 -> 413` and threads `67 -> 68`, while RSS decreased. Do not claim no leak until the delta is settled or attributed.
- Read-only platform boundary: policy parsing, Leaf config compilation, supervisor state, and mesh stream selection are decoupled. Current process packet bridge is still `#[cfg(unix)]`; therefore Windows support needs a host adapter and cannot be claimed from Linux/Android evidence. This does not require policy ownership of QUIC/KCP or changes to mesh overlay semantics.

## 2026-07-16 - final resource correction and ownership audit

- This entry supersedes the earlier pending Android resource statement. Correct `ls -1` snapshots around one isolated Wi-Fi cycle were `323/67` FD/thread before, `347/68` immediately after runtime recovery, `335/68` during delayed cleanup, and `323/67` after bounded cleanup. RSS returned below its pre-cycle value. No persistent Android task, FD or in-process Leaf runtime leak was found.
- A normal semantic Stop released the VPN runtime from FD/thread `422/68` to stopped steady state `280/60`; the same unchanged configuration cold-started and returned to the lower running baseline. This also proves eventual ownership cleanup without uninstall or clear-data.
- A captured browser UID produced WebRTC UDP host candidates on the VPN TUN but no server-reflexive candidate through either of two public STUN endpoints. The active explicit SOCKS actor therefore has no observable UDP roundtrip. This is the documented `udp: true` user capability boundary, not evidence for automatic fallback or a reason to modify mesh transport selection.
- All temporary Linux cores, Leaf/HEV children, TUNs, policy/firewall rules, HTTP/UDP fixtures and test addresses were removed. Android probe target/runner packages, browser probe, CDP forwards and local HTTP fixture were removed; `com.kkrainbow.easytier.policycandidate`, its data and VPN remained intact.
- Read-only ownership audit found no Leaf/policy type in mesh route/overlay selection. Necessary hooks are limited to a generic mesh-only stream primitive, a policy-local loopback SOCKS bridge, and bounded cross-peer HEV/UDP preparation RPC. Windows and macOS Network Extension need host adapters but no mesh protocol redesign.
- Authoritative final audit: `docs/leaf_overdesign_performance_audit_2026_07_16_cn.md`.
