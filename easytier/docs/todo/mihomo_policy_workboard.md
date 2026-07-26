> **CURRENT ARCHITECTURE DECISION (2026-07-26):** Desktop and Unix neutral
> mesh entry uses one pinned, loopback-only GOST process. It tries ports
> `11080`, `11081`, and `11082` sequentially and retains only the first process
> that passes SOCKS5 TCP and a real 8 KiB UDP echo readiness check. The server
> uses the qualified `udp=true&udpBufferSize=65535` setting. GOST stdout and
> stderr are discarded. Older desktop HEV/Leaf-portable mesh-entry
> requirements are superseded and retained only as design history. Android
> keeps the existing
> in-process HEV path; legacy Leaf policy keeps its existing sidecar until that
> deprecated backend is removed separately.
> Pinned sidecar artifacts cover Linux x86_64/aarch64, macOS x86_64/aarch64,
> FreeBSD x86_64, and Windows x86_64/i686/aarch64. Other targets retain normal
> mesh operation but do not claim this neutral GOST entry.
>
# Mihomo Policy Backend Workboard

**Status:** FINAL COMBINED CANDIDATE `08fb17d3` PASSED `.160`, LINUX, ANDROID
WORKFLOW, AND MACOS WORKFLOW GATES; REMAINING RUNTIME MATRIX PENDING

**Design source:** `docs/todo/mihomo_desktop_unix_policy_backend.md`

**Candidate state:** the pre-containment GOST snapshot passed `.160`, but review
then found and fixed build-affecting sidecar containment, Windows build, Linux
GUI target, runtime-directory, unsupported-target, and acquisition-rollback
issues. The guardian lookup fallback snapshot passed a warning-free `.160`
locked no-run in 47.55 seconds. Final review then found and fixed bounded
controller/validator I/O, short Unix controller paths, stable retry recovery,
explicit-backend round-trip, remote GUI state/capability, and workflow path
filter issues. The pre-combination `.160` rerun passed a warning-free
59.28-second locked no-run build, every configured focused filter, 55 frontend
tests, and clean-output frontend-lib, Web, VPN plugin, and GUI production
builds. The assembled tree now also includes the isolated Darwin `quinn-udp`
truncated-datagram fix and macOS scoped-DNS/resolver/interface updates; these
required one final combined `.160` gate plus the focused macOS workflow. The
combined `.160` gate passed: the main locked no-run completed in 4m25s, all
focused filters passed with three-node in 3.08s, and vendored `quinn-udp`
compiled in 4.49s and passed 8/8 integration tests.

The first exact-SHA workflow dispatch (`5e1f7480`) exposed a missing
`happy-dom` root override in `pnpm-lock.yaml`; Android and macOS stopped during
frozen install before platform compilation, and the Linux run was cancelled.
The lockfile and one stale explicit-Mihomo frontend assertion were corrected.
The `.160` frontend lane then passed frozen install, all 51 focused tests, and
all four dependency-order production builds. The release audit now compares
root and lockfile pnpm overrides before dispatch.

The next dispatch (`4fd25fb3`) passed frozen install and reached Android Rust
compilation, which exposed a target-only diagnostic formatting error in the
neutral mesh-entry restart task. The implementation now handles `Ok(())` and
`Err(error)` explicitly. Linux and macOS for the obsolete SHA were cancelled.
The `.160` armv7 diagnostic was blocked in `ring` by the builder's missing
Android clang before EasyTier compiled, so the final Android workflow remains
the required target evidence.

Android workflow `30190085529` subsequently passed for `bdb8c3d0`, closing the
target-only compile gate. The matching macOS run compiled and passed the 10
resolver ownership/route tests, then exceeded one aggregate 10-minute timeout
while recompiling for the interface-cache filter. Those three test groups now
use separate bounded workflow steps. Linux for that superseded workflow layout
was cancelled before artifact publication.

At `26ae1ff1`, Android passed again. Linux completed and its artifact passed
outer/inner checksums, exact-SHA/toolchain/target metadata, static-PIE format,
symbols, and Build IDs, but inspection found that the profiling bundle omitted
Mihomo entirely. The bundle is rejected and now packages the verified
latest-stable Mihomo runtime plus manifest, release manifest, license/source,
checksum, and build metadata. macOS passed every new network test, GUI build,
and all six executable signature checks; only its final verifier failed because
it assumed Tauri flattened resources. The focused verifier now uses the formal
GUI workflow's recursive resource lookup. Both packaging corrections require
one final exact-SHA workflow set.

The final exact candidate
`08fb17d3bddd05bd5ba4ded02ae1d93103c4c2d6` closed those corrections. Linux
profiling run `30191697425`, Android candidate run `30191697411`, and macOS
ARM64 run `30191701843` all passed. Linux artifact `8629000180` passed outer,
inner, and Mihomo-specific checksums and contains the complete EasyTier,
Leaf/HEV, GOST, Mihomo, metadata, compliance, and profiling payload. The exact
archive also started on both CentOS 7 compatibility hosts; real GOST SOCKS5 TCP
and Mihomo DIRECT cross-host transfers passed and all temporary listeners were
removed. Physical Android, installed macOS runtime, Windows/FreeBSD,
public-dual-stack, performance, and long lifecycle/resource evidence remain
open.

## Current parallel lanes

| Lane | Objective | Write scope | Status |
| --- | --- | --- | --- |
| GOST neutral entry | Run one Core-owned desktop neutral SOCKS service independently of Leaf | Cargo features, GOST owner, guardian, instance-manager lifecycle | Implemented; `.160`, packaging, CentOS startup, and real TCP relay passed; full UDP/lifecycle matrix pending |
| Mihomo runtime | Add backend enum, immutable minimal overlay, supervisor, and status | Rust config/Core/API modules | Implemented; `.160`, workflows, packaging, and CentOS DIRECT smoke passed |
| Packaging | Resolve, verify, and package one latest-stable official Mihomo release per workflow | Resources, scripts, workflows, Tauri bundle config | Linux and macOS exact-SHA workflow packaging passed; other declared targets remain pending |
| Platform safety | Prove underlay bypass, GOST mesh routing, guardian cleanup, and no storm | Platform adapters and immutable-artifact validation | Earlier Linux TUN evidence plus current CentOS sidecar smoke retained; full immutable runtime matrix pending |
| Darwin network fixes | Stop Quinn truncated-datagram receive spin and make scoped DNS/interface changes recoverable | Vendored quinn-udp, macOS resolver guard, scoped DNS parser, interface-index cache | `.160` Linux integration and exact-SHA macOS workflow passed; installed-runtime evidence pending |

## Non-negotiable implementation checks

- Desktop GOST ownership is independent of Leaf; Android HEV and legacy Leaf
  direct egress remain separate compatibility paths.
- EasyTier never changes Mihomo proxies, providers, groups, protocol fields, or
  `dialer-proxy`.
- The runtime overlay touches only the approved TUN, route-exclude, reserved
  process-rule prefix, and private-controller fields.
- The source Mihomo YAML is never modified.
- Each workflow resolves the latest stable official Mihomo release once and
  pins that exact version and digest in an immutable manifest shared by all
  jobs; runtime downloads are prohibited.
- Unknown target ABI mappings fail instead of selecting a similar asset.
- Linux, macOS, Windows, and required Unix targets need real runtime evidence.
- Android and OHOS remain outside this Mihomo backend.
- No workflow starts before the complete batch passes the mandatory remote
  preflight and candidate-manifest lock.

## Planned pre-build evidence

| Gate | Required result |
| --- | --- |
| Rust no-run | GOST-neutral-entry, Leaf, Mihomo, and combined features compile |
| Focused tests | GOST readiness/lifecycle/guardian, backend migration, overlay preservation, supervisor backoff |
| Frontend | Backend selector/status tests, typecheck, and production build |
| Packaging audit | Every supported target maps to one pinned asset and final payload |
| Diff audit | No mesh transport semantic change and no proxy-field mutation |

## Planned runtime evidence

| Platform | Required scenarios |
| --- | --- |
| Linux | GOST TCP/8 KiB UDP, Mihomo TUN, process/route exclusion, crash/restore, IPv4/IPv6/dual-stack |
| macOS | Signed GOST/guardian, mesh routing, sleep/wake, abnormal-exit cleanup |
| Windows | GOST availability, Job Object ownership, Wintun cleanup, IPv6-disabled adapter |
| FreeBSD | GOST/guardian startup, TCP/UDP readiness, abnormal-exit cleanup |

The exact candidate SHA, artifact hashes, run IDs, measurements, and results
belong in post-build evidence and must not be invented or filled before the
immutable candidate exists.

## Historical 2026-07-24 HEV-era mutable-snapshot evidence

These rows describe the superseded HEV neutral-entry snapshot. They remain
useful diagnostic history but do not satisfy the current GOST dispatch gate.

| Gate | Result |
| --- | --- |
| Pre-containment Rust no-run/focused tests | SUPERSEDED by current Rust lifecycle fixes |
| Pre-guardian-fallback Rust no-run/focused tests | SUPERSEDED; warning-free 41.29-second `.160` incremental compile and all filters passed before the final Rust change |
| Pre-final-review Rust no-run/focused tests | SUPERSEDED; warning-free 47.55-second `.160` locked no-run and all focused filters passed before final hardening |
| Current Rust no-run/focused tests | PASS; warning-free 59.28-second `.160` locked no-run, 11 Mihomo tests, all focused filters, and three-node in 3.08 seconds |
| Current frontend/GUI | PASS; focused tests 55/55 and clean-output frontend-lib, Web, VPN plugin, and GUI builds |
| Current delayed-DHCP ingress | PASS; background retry became ready after IPv4 assignment and cleaned up |
| Current three-node regression | PASS in 3.17 seconds without hidden JoinSet panic |
| Current frontend | PASS, focused Vitest 49/49 plus all dependency-order production builds |
| Current sidecar acquisition | PASS, Mihomo `v1.19.29` and GOST `v3.2.6` Linux x86_64 fetch/verify |
| Current workflow syntax | PASS, `bash -n` plus `actionlint` |
| Core debug build | PASS, locked dependencies |
| Historical frontend | PASS, PolicyEditor 25/25 plus all dependency-order production builds |
| Mihomo acquisition | PASS, latest stable `v1.19.29`, upstream digest/ELF/version verified |
| HEV data plane | PASS for old snapshot only, real SOCKS5 TCP and UDP relay |
| HEV neutral mesh entry | PASS for old snapshot only, 11080 normal and 11080 occupied -> 11081 fallback |
| Mihomo supervisor | PASS in isolated netns: readiness, one-child restart, no storm, full cleanup |
| Mihomo TUN DIRECT data | PASS, captured route and exact payload delivered through a veth underlay |
| Early TERM during startup | PASS after registering Unix handlers before instance/sidecar startup |
| Process-global orphan check | PASS, zero Mihomo children after data and crash/restart sessions |
| Workflow YAML | PASS for GUI, Test, and Release |
| Test/GUI real Mihomo mapping | PASS, latest-stable resolve plus MUSL asset under Linux GNU bundle name |
| macOS runtime | BLOCKED: the configured validation host was not reachable over SSH |
| Windows/runtime Unix | PENDING immutable workflow artifacts |

## 2026-07-27 nested SOCKS UDP wiring correction

Exact source references inspected before implementation:

- Mihomo
  `adapter/outbound/socks5.go::listenPacketContext` delegates packet creation
  to the outbound dialer.
- Mihomo
  `component/proxydialer/byname.go::ListenPacket` and
  `component/proxydialer/proxydialer.go::ListenPacket` preserve
  `dialer-proxy` for UDP as well as TCP.
- The pinned GOST wrapper's SOCKS5 UDP handler accepts `publicAddr`; an
  unspecified address is returned as the UDP ASSOCIATE `BND.ADDR`.

Pre-change exact-artifact diagnosis for
`1af984c9931577ab6e40a0ce24effed95ce509fc`:

- Core-owned GOST TCP/UDP readiness passed locally.
- Mihomo selected the configured peer exit for UDP, but the virtual peer
  `11080` ingress terminated at the policy-instance Leaf/HEV direct egress on
  desktop instead of the process-global GOST.
- TCP happened to work because both endpoints could provide a direct stream.
  Nested UDP failed because the private endpoint advertised a loopback relay
  address that was local to the peer.
- A bounded proof using the same artifact and an explicitly mesh-reachable
  peer GOST passed exact `64`, `1200`, and `8192` byte UDP echoes. This isolates
  the failure from the mesh UDP data plane, GOST relay implementation, and
  Mihomo `dialer-proxy`.

Minimal implementation boundary:

- Reuse `CoreMeshEntryHandle::global()` as the endpoint owner on every
  supported platform. Desktop uses GOST; Android retains its in-process HEV
  compatibility backend.
- Keep process-global GOST loopback-only. On desktop, bind ordinary Tokio TCP
  listeners only to each instance's exact virtual IPv4 and the three reserved
  candidate ports, then relay accepted streams to the selected loopback GOST
  endpoint.
- Use fixed `128 KiB` buffers in each direction. Per admitted stream memory is
  therefore `256 KiB`, bounded per network instance by the existing
  `TCP_RELAY_LIMIT` semaphore.
- Keep the userspace virtual TCP ingress only on Android, where VpnService does
  not provide a kernel-local virtual address.
- Make managed GOST advertise `publicAddr=0.0.0.0`, allowing the nested client
  to substitute the configured peer address.
- Do not change mesh transports, policy RPC, proxy configuration, Mihomo YAML,
  routing, or packet formats.

Performance diagnosis before the kernel-ingress correction used the same
source and destination nodes and three sequential 32 MiB rounds:

- direct mesh median: approximately `253 Mbit/s`;
- local GOST to mesh median: approximately `213 Mbit/s`;
- full chain through the desktop userspace virtual ingress: approximately
  `55 Mbit/s`.

Core CPU remained essentially constant in the earlier optimized-artifact
control; the additional cost was isolated to proxy/relay layers. A standalone
kernel-listener prototype then produced these full-chain medians:

- default `8 KiB` copy buffers: approximately `144 Mbit/s`;
- `64 KiB` per direction: approximately `155 Mbit/s`;
- `128 KiB` per direction: approximately `168 Mbit/s` across five runs;
- `256 KiB` per direction: approximately `174 Mbit/s`.

The selected `128 KiB` point stays within about 9% of the no-relay full-chain
control while halving per-flow memory versus `256 KiB`. The implementation
removes the redundant desktop userspace TCP stack without changing mesh
transport selection.

Post-change evidence:

- `.160` locked no-run and the complete focused suite passed, including the
  platform-ingress relay, delayed IPv4 retry, GOST, Mihomo, three-node, Leaf,
  netstack, macOS DNS/route, and Quinn UDP tests.
- A non-release x86_64-musl Core built on `.160` was verified as static PIE
  with Build ID `4cc289c67b7027201807f5ecda7818b8b5b80e41` and SHA256
  `80a6607bf0734d414429926a2e30dc33772f9e13dcc6bcd0e20c910707d64046`.
- On the public dual-stack pair, both directions passed IPv4 and IPv6 mesh
  reachability. GOST remained loopback-only, while Core owned only each
  instance's exact virtual IPv4 ports `11080-11082`.
- The real Mihomo -> local GOST -> peer virtual `11080` -> peer GOST chain
  completed five 32 MiB TCP runs. The median was approximately `170 Mbit/s`,
  versus approximately `55 Mbit/s` through the old desktop smoltcp ingress.
- Exact `64`, `1200`, and `8192` byte UDP echoes all passed through the same
  peer `11080` chain.
- Killing the peer GOST produced a new child on the same port within four
  seconds. Post-recovery TCP was approximately `172 Mbit/s`, and all three UDP
  sizes passed again.
- Graceful Core stop removed the GOST child, all virtual and loopback SOCKS
  listeners, and the TUN. Final cleanup on both nodes found no Core, prototype
  relay, test listener, or TUN residue. No firewall rules were added.

This is strong implementation evidence but not an immutable optimized workflow
artifact. The next single profiling candidate must repeat the TCP/UDP,
crash/recovery, resource, and cleanup checks from its exact packaged artifact.
