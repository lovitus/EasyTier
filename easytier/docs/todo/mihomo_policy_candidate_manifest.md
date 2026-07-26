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
# Mihomo desktop/Unix policy backend candidate manifest

Status: FINAL REVIEW HARDENING PASSED SOURCE PREFLIGHT; IMMUTABLE CANDIDATE
NOT YET DISPATCHED

This is a pre-build manifest. Build IDs, workflow run IDs, measurements, and
post-build results belong in the validation matrix keyed to the immutable SHA.

## Intended build snapshot

- Add `off | mihomo | leaf` policy backend selection.
- Preserve legacy policy configuration by mapping it to Leaf only when no
  explicit backend is present; an explicit backend is authoritative.
- Mark Leaf deprecated only in desktop/Unix user interfaces; Android/OHOS
  behavior is unchanged.
- Run one Core-owned neutral GOST SOCKS mesh entry on one available loopback
  port from 11080-11082 for every supported EasyTier Core process.
- Keep Leaf's internal DIRECT egress loop prevention independent from the
  neutral GOST mesh entry.
- Load an immutable user Mihomo YAML and generate a private runtime copy.
- Preserve all user proxies, providers, groups, rules, transports,
  `dialer-proxy`, DNS, and subscription semantics except explicitly reported
  EasyTier-owned TUN/controller/route safety overlays.
- Prepend required EasyTier, GOST, HEV, Mihomo, Leaf, and Tailscale DIRECT
  process rules.
- Merge EasyTier mesh and Tailscale prefixes into `route-exclude-address`.
- Own Mihomo lifecycle, readiness, bounded restart, status, lock, and cleanup in
  the Core process, not the GUI.
- Resolve the latest stable Mihomo release once at workflow start, pin that
  version and upstream digest in one immutable workflow manifest, and package
  the executable plus its license/source notice for every declared supported
  desktop/Unix artifact.
- Expose backend, source path, runtime state, selected GOST port, error, and
  restart count through Core API and GUI.

## Required source audit before preflight

- Exact Mihomo source revision used for behavioral comparison:
  `0a87b94845ef908c15f8495871e4cd8e33116328`.
- Exact desktop neutral-entry release: GOST `v3.2.6`, with archive and binary
  digests verified by the pinned manifest.
- Exact HEV source revision for Android and legacy Leaf paths:
  `97e74f1068bd924e740032382cdc94ca83741ae6`.
- Confirm controller fields/readiness, process matcher semantics, TUN route
  exclusion, shutdown, and Windows named endpoint behavior against those
  revisions.
- Confirm the packaged Mihomo release version, archive names, SHA-256 values,
  architectures, executable formats, and license/source-offer contents.

## Remote builder gate

The complete batch must be synchronized to `192.168.2.160` once all known code,
tests, generated protocol inputs, dependency pins, workflows, packaging, GUI,
and pre-build documentation are complete.

Required checks:

1. No existing Cargo or rustc process in the builder.
2. Locked smallest-feature Rust no-run build containing Mihomo supervisor,
   GOST lifecycle/readiness, neutral-entry status, and legacy Leaf/HEV tests.
3. Direct execution of the exact generated test binary with serial focused
   filters.
4. Node 22 dependency-order frontend gate:
   frontend-lib tests and build, frontend build, VPN plugin build, GUI build.
5. Mihomo fetch script checksum/format/architecture tests using a fresh output
   directory.
6. Lockfile, protobuf generation, platform `cfg`, workflow pin, package
   manifest, credentials, and complete-diff audit.

The 2026-07-24 HEV-era snapshot passed the earlier form of these checks. The
current GOST snapshot adds the real 8 KiB UDP readiness exchange, guardian
reaping, effective-backend validation dispatch, Android selector correction,
neutral-entry status propagation, and cancellation-aware delayed mesh-ingress
startup. Those changes invalidated the old preflight as a dispatch gate.
The later guardian lookup fix prefers the packaged guardian beside EasyTier
and falls back to the directory of a custom managed executable. The final
`.160` locked no-run build completed without warnings in 47.55 seconds; both
lookup-order tests, the complete focused suite, and the three-node regression
passed.

The final review hardening follows Mihomo
`hub/route/server.go::{startUnix,startPipe,router}`,
`hub/route/configs.go::getConfigs`, `main.go`, and
`listener/sing_tun/server.go::Listener.Close`: controller probes and validator
children are bounded and cancellable, Unix controller paths remain private but
short enough for Darwin/FreeBSD, stable operation restores retry budgets, and
explicit Mihomo selection never sets the legacy Leaf boolean. These
build-affecting changes supersede the preceding source preflight.

The final `.160` gate completed a warning-free locked no-run build in 59.28
seconds. All configured focused filters passed, including 11 Mihomo tests,
explicit Mihomo backend round-trip, guardian lookup, GOST readiness, netstack,
and the three-node regression in 3.08 seconds. Frontend focused tests passed
55/55. Clean-output frontend-lib, Web, VPN plugin, and GUI production builds
all returned success; build scripts now remove only their generated output
directories before regeneration so stale declarations cannot make runner
results cache-dependent.

## Superseded mutable-source preflight evidence

On 2026-07-26 the preceding GOST source snapshot passed:

- `.160` locked Rust no-run compilation for the exact EasyTier, policy,
  SOCKS-egress, and netstack library test binaries.
- All configured serial focused filters, including Mihomo overlay/supervisor,
  effective-backend validation, GOST TCP plus real 8 KiB UDP readiness,
  guardian/lifecycle, Leaf/DNS/Geo/FakeIP, netstack backpressure, and
  three-node default-drop ACL behavior.
- The delayed-DHCP regression: mesh TCP ingress starts in the background,
  retries after the data plane obtains IPv4, and shuts down without retaining
  a retry task.
- The exact three-node case completed in 3.17 seconds after removing the
  obsolete blocking startup wait.
- Node 22 focused frontend tests, 49/49, followed by frontend-lib, Web,
  VPN-plugin declaration, and GUI production builds in dependency order.
- Latest-stable Mihomo resolution selected `v1.19.29` at commit
  `e26714a181ac0e2fa803453c0a8e9a9ce94e31cb`; its Linux x86_64 archive digest,
  extracted ELF machine, static format, and generated metadata passed fetch
  plus independent verify. Pinned GOST `v3.2.6` passed the equivalent archive,
  ELF machine, binary digest, metadata, and independent verify checks.
- `bash -n` passed for all changed acquisition/audit/preflight scripts and
  `actionlint` passed for all six changed workflows.
- `git diff --check` and the changed/untracked private-credential scan.

The subsequent review found and fixed build-affecting issues in shared sidecar
containment, Windows portable-sidecar construction, Linux GUI GOST target
mapping, private runtime-directory creation, unsupported-target status, and
late acquisition rollback. Those Rust and workflow changes invalidate the
preceding Rust preflight for dispatch. Sidecar acquisition and frontend
evidence remain applicable because their inputs did not change.

The current containment snapshot then passed a clean `.160` locked no-run and
focused-test rerun. Its final warning-free incremental compile completed in
41.29 seconds; all configured filters passed and the exact three-node case
completed in 3.06 seconds. This source evidence never substitutes for an
immutable workflow artifact, sidecar package inspection, or platform runtime
validation.

## Historical 2026-07-24 pre-candidate evidence

The following evidence belongs only to the earlier HEV-era mutable snapshot. It
is retained for diagnosis and must not be cited as validation of the current
GOST implementation.

- `scripts/leaf-remote-preflight.sh`: the earlier locked Rust no-run build and
  focused Mihomo, neutral HEV entry, Leaf/netstack, and selected three-node
  tests passed.
- Core debug build: `cargo build --locked --package easytier --bin
  easytier-core` passed.
- Frontend gate, in required dependency order: PolicyEditor Vitest 25/25,
  frontend-lib build, frontend production build, VPN plugin build, and GUI
  production build all passed with Node 22.
- Latest-stable release resolution selected Mihomo `v1.19.29`, release ID
  `356097664`, upstream commit
  `e26714a181ac0e2fa803453c0a8e9a9ce94e31cb`; upstream asset digest, ELF
  architecture, and `mihomo -v` were verified before `mihomo -t` passed.
- HEV source `97e74f1068bd924e740032382cdc94ca83741ae6`
  built as the Linux backend and passed real SOCKS5 TCP and UDP relay smoke.
- The earlier HEV-backed Core-only startup published `127.0.0.1:11080`;
  shutdown removed the
  listener. With 11080 held by an unrelated owner, Core selected 11081 and
  left the existing owner untouched.
- In an isolated Linux network namespace, real Core plus real Mihomo reached
  private-controller/TUN readiness, preserved the source YAML byte-for-byte,
  restarted exactly one child after `SIGKILL`, showed no restart storm during
  the bounded observation, and removed the child, TUN, runtime state, and
  11080-11082 listeners on Core shutdown.
- A veth-backed endpoint proved that an ordinary process route entered the
  generated `etm*` TUN and that Mihomo DIRECT delivered the complete payload to
  the underlay endpoint.
- This data-path test initially exposed a real startup signal race: Core
  registered SIGINT/SIGTERM only after instance startup, allowing an early
  TERM to orphan Mihomo. Unix handlers are now registered before any instance
  or sidecar starts, and an explicit process-scope owner shutdown reaps the
  supervisor before the Tokio runtime exits. The exact regression now leaves
  no child, TUN, runtime directory, or listener.
- Workflow YAML parsing passed for GUI, Test, and Release. A fresh
  latest-stable resolve plus real fetch/format/digest/metadata verification
  passed using the Linux GUI GNU output name and the compatible Mihomo MUSL
  asset mapping.
- Packaging audit corrections require every Core artifact and every one of
  the seven GUI target manifests to match the immutable candidate manifest.
  Test now fetches the real resolved Mihomo binary, and release archives keep
  compliance text at mode 0644 while setting only directories, executables,
  and scripts executable.

These historical mutable results must not be cited as current preflight,
workflow, release-artifact, GOST readiness, or guardian-lifecycle evidence.

## Required current pre-candidate evidence

- `.160` locked no-run build for the complete current snapshot.
- Focused tests for effective-backend validation dispatch, Android explicit
  Leaf selection, GOST UDP packet parsing, neutral-entry status, lifecycle,
  restart, and guardian cleanup.
- Real pinned GOST startup with TCP CONNECT and an 8 KiB UDP echo through
  `udp=true&udpBufferSize=65535`.
- Port conflict, abnormal GOST exit/restart, repeated lifecycle cleanup, and
  zero residual child/guardian processes.
- The dependency-order frontend/GUI gate and complete packaging/target audit.

## Focused Rust behavior

- backend legacy mapping and invalid combinations;
- immutable source YAML and protected proxy subtree equivalence;
- deterministic safety overlay and duplicate suppression;
- route exclusion for mesh/Tailscale prefixes;
- private controller and random secret handling;
- runtime file atomic replacement and stale cleanup;
- process-wide owner and second-process lock failure;
- readiness versus child spawn;
- bounded restart and generation reset;
- stop cleanup and no adoption of unrelated child processes;
- neutral GOST singleton, real UDP readiness, candidate port fallback, and
  nonfatal absence;
- separation of neutral mesh entry from Leaf DIRECT egress;
- API status serialization and backward compatibility.

## Planned immutable workflows

Start one workflow set only after the complete `.160` gate succeeds:

- Core;
- GUI;
- Test;
- platform jobs required by the declared desktop/Unix support matrix.

Do not dispatch Android/OHOS merely for this backend because they do not package
or select Mihomo. Their existing Leaf code must nevertheless continue compiling
when shared protocol/config code changes.

## Planned runtime evidence

- Linux compatibility/lifecycle hosts: install, Core-only startup, port fallback,
  proxy/TUN function, crash/restart, stop cleanup.
- Public dual-stack Linux pair: IPv4, IPv6, dual-stack, native, GOST mesh chain,
  relay/KCP/QUIC, throughput, CPU, RSS, FD, and thread baselines.
- macOS host: signed package, Core/GUI lifecycle, `utun`, route preservation,
  sleep/wake, service kill/restart, port fallback, dual-stack, and cleanup.
- Windows host: Wintun, named controller endpoint, Core/GUI ownership, IPv4
  with IPv6 disabled, dual-stack, GOST packaging, Job Object ownership,
  restart, and cleanup.
- Other Unix targets: compile/package evidence and runtime tests where a
  maintained host exists.

## Dispatch blockers

- Any protected Mihomo proxy subtree changes.
- Any unsupported target silently claims or packages Mihomo or GOST support.
- The neutral GOST entry is not Core-owned and process-global.
- Spawned Mihomo is reported ready before controller/TUN readiness.
- Runtime secret or user credential is logged or written with broad permission.
- Remote builder evidence is absent or stale for the source snapshot being
  dispatched.
- Known same-area implementation remains outside the batch.
