# Leaf v1 Release Gates

> This is the only execution board for the Leaf v1 candidate. Keep it short and current.
> Architecture and compatibility notes remain in `leaf_optional_policy_proxy.md`; detailed evidence belongs in `leaf_validation_journal.md`; deferred work belongs in `leaf_post_v1_backlog.md`.

## Candidate state

- Current audit snapshot: `c48816f4300f5853525b62d5793d9778923aed80`, Linux profiling run `29486876174`. This commit changes only the manually enabled workflow comparator; product behavior remains the preceding candidate plus the isolated Android DNS fix. Exact no-Leaf and Leaf binaries from one run were verified and deployed together.
- Fixed single-UDP-tunnel comparison found no TCP throughput regression (`700` versus `707 Mbit/s` successful-sample mean). Portless and explicit-port mesh actors each passed `20/20` controlled HTTP requests to the same peer/target; fallback, fail-closed, network restoration, and normal cleanup passed.
- Policy-off is not zero-resource: when packaged, managed HEV remains resident at about `252-256 KiB`, `2 threads`, and `12 FD` per instance. This is the cost of making a peer usable as a portless exit without a local exit-node switch.
- Android survived three scripted Wi-Fi outages with the same PID/TUN and working FakeDNS/Google/Baidu. RSS plateaued after the first reload, but stable idle CPU remained about `10-14%` of one core with frequent SELinux-denied network probes. Attribution against a same-APK policy-off baseline remains open.

- Exact validated runtime baseline: `824ac5a1d47d568113a7e2190d57fecf049dd47b`. Linux run `29461390271` and Android run `29461390297` are the unique automatic workflow pair; both succeeded. Exact artifacts, hashes, signer, symbols, build ID, Linux mesh/policy coexistence, Android VPN ownership, TCP, UDP/UoT, Wi-Fi/route recovery, worker recovery, configuration retention, and cleanup were verified.
- Data-plane baseline: `e1a54d87e08eda80f3d081f10b9a9546cbb268d5`. It closed policy-only KCP performance, bounded smoltcp fallback, OSPF generation recovery, Android native stop, Wi-Fi recovery, and repeated Linux/Android lifecycle/resource cleanup.
- `318497c4` adds build-capability fail-closed enforcement: enabled policy is rejected on builds without a runtime while disabled configuration remains portable and preserved. Native Windows MSVC no-run and exact unsupported-runtime tests passed; supported Linux/Android behavior remained valid.
- Current release work is user-facing capability closure, not core architecture repair: the safe chain/fallback and explicit UDP boundary is frozen in documentation and a compiled example, and overseas GeoSite/custom-rule egress evidence is complete.

## P0 gates

- [ ] Before claiming Android has no idle performance regression, run a same-APK, same-mesh policy-off comparator and attribute the `10-14%` idle CPU plus SELinux-denied packet/proc/sysfs probes. Do not fix this by changing mesh transport without evidence.
- [ ] Decide explicitly whether the portless-exit convenience justifies policy-off managed HEV residency. If strict zero residency is required, design lazy adapter-owned startup; do not require a user-visible exit-node switch and do not bind startup only to local policy configuration.
- [ ] Repeat at least two concurrent failed-UDP association waves and verify the post-300-second Leaf/HEV FD plateau does not grow beyond the observed one-time `+3/+2 FD`. UDP payload failure itself remains an exposed capability error, not an automatic fallback trigger.
- [ ] Do not claim exact EasyTier 2.9.10 performance equivalence from the current no-Leaf comparator; it isolates feature/sidecar cost but still contains current-core global hooks.
- [x] Disable optional native `source_interface_signal()` inspection on Android/iOS/macOS-NE/OHOS while retaining managed-IP hard checks. The earlier `collect_local_ip_addrs_now()` Android attribution was incorrect because bind-address refresh was already mobile-disabled. `.160` compile and platform-ownership unit test passed.
- [ ] Repeat Android idle CPU/SELinux-denial measurement on the exact candidate; desktop bind-address discovery still needs one refresh per network generation. This is a global mesh performance gate, not a Leaf worker optimization.
- [x] Remove policy-specific KCP timeout/fallback semantics from generic `Socks5AutoConnector`; transport selection remains mesh-owned and KCP-to-smoltcp retry is isolated in the mesh dataplane adapter. Endpoint-isolation, fail-closed and UoT fallback tests passed on `.160`.
- [ ] Keep OSPF generation restart classified as an independent ordinary-mesh correctness change. Its exact regression passed on `.160`; deployed no-Leaf compatibility evidence remains required and it must not be counted as a necessary Leaf adapter hook.
- [ ] Define platform-neutral `PolicyRuntimeHost`, `PacketIo`, and managed-egress lifecycle boundaries before claiming Windows/macOS extensibility. Unix raw FD and Android runtime-ID ownership remain platform adapters only.

- [x] Android native VPN stop is independent of WebView readiness and JavaScript queue progress.
- [x] Native success does not schedule a redundant second stop through the frontend.
- [x] Native failure preserves the existing frontend fallback and reports the native failure.
- [x] Stop/start, process death, Wi-Fi loss/recovery, and repeated cycles return TUN, HEV, Leaf, FD, thread, and task ownership to baseline.
- [x] Built-in HEV TCP approaches the proven existing KCP path without changing explicit user SOCKS/KCP configuration, and KCP-disabled destinations fail over to mesh smoltcp without kernel/direct escape.
- [x] A third peer relearns an Android peer through the hub after Wi-Fi loss/recovery without waiting for a new direct peer connection.
- [x] HEV hosting and shutdown boundaries are audited for Windows, macOS, Linux, Android, iOS, and constrained targets; v1 claims only evidence actually obtained.
- [x] The v1 capability boundary is frozen: unsupported advanced transports or rule/DNS fields are rejected, hidden, or explicitly experimental.
- [x] Default configuration remains simple: DIRECT and portless `via: mesh` need no HEV-specific tuning; chain/fallback documentation explicitly separates TCP from UDP and does not imply UoT or KCP.

## One-push preflight

- [x] Format changed Rust files locally with Rust 1.95 and edition 2024.
- [x] Run remote minimal `cargo test --locked --no-run` for the complete KCP/policy/OSPF batch after confirming no cargo/rustc process is active.
- [x] Run KCP endpoint isolation 1/1, OSPF generation/cache invalidation 1/1, and mesh relay 8/8 directly from the built test binary.
- [x] Inspect `Cargo.lock`, platform `cfg` boundaries, workflow pins, generated bindings, and the complete candidate diff; no sensitive/generated file changed and `git diff --check` passed.
- [x] Record exact candidate `e1a54d87e08eda80f3d081f10b9a9546cbb268d5` in the local journal after the single commit.
- [x] Commit and push one complete candidate snapshot to `codex/profiling-beta`.
- [x] Start only the automatic Linux run `29440664216` and Android run `29440667649` for that exact snapshot.

## Exact-candidate acceptance

- [x] Verify workflow commit SHA, `BUILD_INFO.txt`, build ID, symbols, target, signer, and `SHA256SUMS.txt`.
- [x] Linux: normal stop, SIGTERM, Leaf/HEV crash, route/network replacement, fail-closed, repeated lifecycle, and resource baseline.
- [x] Android: cold start, stop/start, Leaf/HEV failure, Wi-Fi loss with Wi-Fi restored before wireless ADB continuation, network recovery, repeated lifecycle, and resource baseline.
- [x] Linux and Android: real TCP and UDP through DIRECT and mesh within the frozen v1 boundary; Linux additionally validated TCP chain/fallback and explicit UDP-to-mesh separation. Android does not claim SOCKS chain UDP.
- [x] Linux policy client with `lv1g2` and `lv1g3`: exact `824ac5a1` artifacts passed GeoSite, GeoIP, custom domain/IP, native SOCKS chain, mesh UDP, single-connection fallback and failback. The first failure transition remains connection-scoped: multi-connection transactions may require a whole-transaction retry. Android already passed the same policy engine's captured-UID GeoSite/GeoIP and mesh UDP paths; this overseas topology did not add a second redundant Android run.
- [x] No screenshots or simulated taps are used for Android control; screenshots are reserved for final visual evidence.

## Exact candidate result: `318497c4fd8450a8fee237ef5826841c60517b0c` (2026-07-16)

- Linux workflow `29447382393` and Android workflow `29447382391` passed from one push; artifact and platform metadata match the exact candidate.
- `.160` no-feature and `leaf-policy-proxy` locked no-run builds passed with all focused supported/unsupported tests. Native Windows MSVC no-feature no-run and both fail-closed tests passed.
- Android `install -r` preserved the complete selected data archive byte-for-byte and retained the enabled policy document. VPN ownership, runtime-UID exclusion, semantic start/stop, and Wi-Fi state were verified without screenshots.
- Linux policy + ordinary mesh namespaces passed 3/3 ICMP. Android `MATCH,linux-hev` reached a controlled `.160` fixture with source `192.168.1.37`; the VPN-down baseline reached it from `192.168.6.36`, proving the mesh egress change.
- All validation processes, TUNs, listeners, probe packages, forwards, sensitive temporary configuration, and remote fixture state were cleaned.

## Workflow rule

The rolling beta validates a complete candidate; it is not the compiler feedback loop. Do not push again for a single mechanical fix. Accumulate related fixes, run the remote minimal preflight and exact tests, inspect the full diff, then create one candidate.

## Exact candidate result: `824ac5a1d47d568113a7e2190d57fecf049dd47b` (2026-07-16)

- Linux workflow `29461390271` and Android workflow `29461390297` passed from one batched push; all artifact metadata and hashes match the exact SHA.
- Linux managed HEV TCP reached 313 Mbit/s. UDP/UoT delivered `4641/4641` at 10 Mbit/s and lost `0.32%` at 20 Mbit/s for 20 seconds; trace proved KCP selection. Worker kill, DIRECT route loss/recovery, startup route delay, fail-closed, mesh continuity, resources, and cleanup passed.
- Android captured-UID mesh owner TCP, policy TLS, and HEV TCP passed before and after Wi-Fi recovery and normal stop/start. Device `iperf3` used policy TUN source `10.247.0.3`; receiver loss was `0/4960` at 10 Mbit/s and `0/19840` at 20 Mbit/s.
- Final stop removed Android `TauriVpnService`/`tun0` and all Linux core, Leaf worker, HEV, TUN, policy rules, table state, and generated private configuration.
- Overseas GeoSite/custom-rule selection is now closed. A longer soak remains optional follow-up evidence, not an unclosed Linux/Android architecture defect or a Leaf v1 release blocker.

## Exact candidate result: `e1a54d87e08eda80f3d081f10b9a9546cbb268d5` (2026-07-16)

- Linux workflow `29440664216`: passed; exact musl artifact metadata, checksums, static PIE/debug symbols, and Build ID verified.
- Android workflow `29440667649`: passed; exact APK metadata, checksums, v2 signatures, candidate certificate, and probe certificate verified.
- Mandatory `.160` locked preflight and exact tests: passed (`1 + 1 + 8` focused tests).
- Policy-only KCP with user KCP disabled: passed on Linux and Android. Linux HEV improved from the earlier ~`50 Mbps` path to ~`478 Mbps`; direct baseline was ~`941 Mbps`.
- Destination `disable_kcp_input`: passed. TCP/UDP remained fail-closed through smoltcp at ~`53.5 Mbps`, with no direct/kernel escape; restoring capability restored ~`452 Mbps` KCP throughput.
- Android formal 70-second Wi-Fi outage: passed. The device-side disable/enable script was verified before outage, PID/VPN ownership survived, route recovered three seconds after Wi-Fi enable, and OSPF generation repair propagated before direct Android reconnection.
- Android post-outage HEV TCP/UDP and captured-UID TLS: passed.
- Linux repeated SIGTERM/resource cleanup and ordinary `--socks5` isolation: passed. Policy-only KCP did not enable KCP for the user SOCKS endpoint.
- Current v1 status: no implementation or architecture blocker remains for the frozen Linux/Android basic Leaf boundary. Advanced split DNS, chain/fallback, and high-throughput UDP remain explicit release-scope decisions, not implied support.
- Non-blocking follow-up: suppress or downgrade idempotent policy-route cleanup `ESRCH` warnings after successful cleanup; continue longer soak/resource sampling after v1 without reopening the architecture.
