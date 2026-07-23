# macOS Leaf policy routing fix

Status: route and packet-framing fixes passed focused checks; the first
candidate failed repeated macOS bridge-backpressure validation, and the
event-driven `tun` follow-up is awaiting a new packaged macOS artifact

## Reference contract

- Failure-baseline Leaf source: `https://github.com/lovitus/leaf.git` at
  `013a1497dd29355a00cd776628ff2de72e02e861`, exactly matching the tested
  artifact's `Cargo.lock`. The intermediate macOS candidate updated the lock to
  `e73ec228883965850f6bfbb339e64fd8fe86ef1f`; the combined desktop candidate
  now locks `682d1dc43585a703c993e8875fe4e937b1038733`.
  - `leaf/src/proxy/tun/inbound.rs::new` passes an externally supplied `fd` to
    `tun::Configuration::raw_fd` without overriding Darwin packet information.
  - Locked `tun` `0.7.22` defaults
    `platform::macos::PlatformConfig::packet_information` to `true`;
    `platform::posix::split::Reader` removes four bytes on receive and `Writer`
    prepends an AF_INET/AF_INET6 word on transmit.
- Mihomo reference: `/Users/fanli/Documents/mihomo-rev` at
  `0a87b94845ef908c15f8495871e4cd8e33116328`.
  - `component/dialer/bind_darwin.go::bindControl` binds IPv4/IPv6 sockets with
    `IP_BOUND_IF`/`IPV6_BOUND_IF`.
  - `listener/sing_tun/server.go::New` owns the TUN, default-interface monitor,
    route state, and their cleanup as one lifecycle.
- sing-tun reference: module `github.com/metacubex/sing-tun@v0.4.17`.
  - `tun_darwin.go::NativeTun` uses a four-byte AF_INET/AF_INET6 header for a
    real Darwin utun descriptor.
  - `tun_rules.go::Options.BuildAutoRouteRanges` uses sub-ranges on Darwin
    instead of replacing the physical default route.

Externally observable semantics followed here:

1. The selected physical interface retains a usable interface-scoped default
   before EasyTier installs the more-specific policy capture routes.
2. The scoped route is owned by the same guard as the capture routes, refreshed
   after physical default-route changes, and removed only if EasyTier installed
   it.
3. Loss of the physical IPv4 default route is fail-closed and reported through
   the existing underlay transition state; recovery does not require polling in
   the packet hot path.
4. A real Darwin utun descriptor keeps its required four-byte packet information
   header. EasyTier's AF_UNIX datagram bridge transports raw IP and explicitly
   disables that header instead of modifying every packet.

Backpressure reference and required semantics:

- Mihomo `/Users/fanli/Documents/mihomo-rev` at
  `0a87b94845ef908c15f8495871e4cd8e33116328`,
  `listener/sing_tun/server.go::New`, pins and constructs sing-tun v0.4.17.
- sing-tun v0.4.17
  `internal/fdbased_darwin/errno.go::TranslateErrno` classifies Darwin
  `ENOBUFS` as `ErrNoBufferSpace`, while
  `internal/fdbased_darwin/endpoint.go::writePacket` documents and implements
  transient non-writability as packet-level pressure rather than endpoint
  teardown.
- EasyTier intentionally keeps the pending framed packet and waits for real
  descriptor writability instead of adopting sing-tun's packet drop. A
  temporary Darwin `ENOBUFS` must not terminate Leaf's TUN inbound or close the
  netstack channels. Permanent descriptor errors still terminate the inbound.
  This difference preserves more traffic without adding a timer, unbounded
  queue, fixed-size tuning, or packet-hot-path polling.
- The exact follow-up dependency is
  `https://github.com/lovitus/rust-tun.git` at
  `028b861d1a8e69cbb8950bfefb7ee81e44b46ff5`, based on `tun` 0.7.22 upstream
  commit `5a0362650f2ba46e15b68cd24853652004b38499`. Only Darwin `ENOBUFS` from
  POSIX `write`/`writev` is normalized to `WouldBlock`, allowing the existing
  Tokio `AsyncFd` readiness path to retain the pending frame, clear stale
  writable readiness, and await a kernel event. Other platforms and permanent
  errors are unchanged.

## Intentional EasyTier boundary

EasyTier does not let Leaf own global macOS routes or a native utun. The existing
EasyTier virtual NIC remains the capture owner and passes selected raw IP packets
to the Leaf worker through a packet-preserving Unix datagram pair. Therefore the
new Leaf setting is optional and preserves its platform default for all existing
callers; only EasyTier's macOS legacy packet bridge sends
`packetInformation=false`. Linux, Android, Windows, and callers supplying a real
Darwin utun FD retain their current framing.

## Failure evidence

The exact packaged artifact from workflow `29972289164`, commit
`316bfb69e1d24831e94f34f24f0caaa3afb69cfc`, was exercised with an A/B/A route
test:

- before the scoped physical default, interface/source-bound TCP and UDP failed;
- while an `en0`-scoped physical default was installed, both passed;
- after removal, both failed again;
- Leaf DIRECT remained unavailable in all phases while Leaf logged
  `Sending packet to NetStack failed: invalid IP packet: wire::Error`.

The first three observations isolate the route recursion. The Leaf log plus the
locked raw-FD/PI source path isolates the independent packet-framing mismatch.

## Candidate implementation

- Extend the macOS policy routing guard with an owned, interface-scoped physical
  default per available enabled family. Discover only unscoped physical defaults,
  install bypass state before capture routes, refresh it on the existing bounded
  supervisor interval, fail closed on loss, and preserve pre-existing scoped
  routes.
- Add an optional Leaf TUN `packetInformation` setting. On macOS it may override
  the `tun` crate platform default only for an external FD; absence preserves
  current Leaf behavior.
- Emit `packetInformation=false` only for EasyTier's macOS AF_UNIX raw-IP bridge.
  Do not add per-packet allocation, copying, polling, or fixed-size buffering.
- Pass a NUL-terminated interface name to EasyTier's Darwin
  `if_nametoindex` call.
- Patch Leaf's locked crates.io `tun` 0.7.22 to the exact follow-up revision
  above. Do not change socket buffers, channel capacities, retry timers, or
  packet queueing.

## Tests

- Leaf JSON/internal config conversion preserves absent/default PI and encodes an
  explicit false value.
- EasyTier config emits PI=false only for the macOS legacy FD bridge.
- Pure macOS route tests cover netstat parsing, scoped-route command arguments,
  gateway scope normalization, ownership, and underlay transition
  classification.
- Existing Leaf config/lifecycle and Linux policy routing tests remain green on
  the remote builder.
- The patched `tun` crate's Darwin `ENOBUFS` normalization and
  non-Darwin/permanent-error preservation tests compile with its `async`
  feature and pass on the remote builder.
- Exact packaged macOS artifact validation covers IPv4 TCP/UDP DIRECT, proxied
  traffic, DNS/FakeDNS, mesh precedence, route loss/recovery, repeated restart,
  cleanup, and a policy-disabled baseline. IPv6 receives a separate scoped-route
  and raw-packet round-trip check before support is claimed.

## Pre-build candidate manifest

- Intended build snapshot: all existing Windows Leaf/Wintun, GUI/runtime-support,
  packaging, README, and test changes already present in the workspace, plus this
  macOS scoped-underlay/PI/CString fix, its dependency revision, generated Leaf
  protocol output, focused tests, and compatibility notes.
- Remote `.160` gate: `scripts/leaf-remote-preflight.sh` after extending its
  focused filters, including `--locked` no-run compilation and exact Leaf config,
  packet bridge, macOS route-pure-logic, lifecycle, Linux routing, and Windows
  platform-neutral tests. macOS target compilation is a recorded GitHub-only
  exception where the Apple SDK is required.
- Required workflows after a successful `.160` gate: one immutable profiling
  beta candidate for Linux regression artifacts and the explicitly authorized
  macOS ARM64 GUI test for the release branch. Do not start Core, the full GUI
  matrix, Mobile, OHOS, Test, tags, or Release without maintainer confirmation.
- Linux evidence: ordinary mesh, policy disabled/enabled, DIRECT/REJECT/proxy,
  managed HEV, worker failure, route recovery, and shutdown cleanup.
- Android evidence: deferred while the device is unavailable unless the
  maintainer explicitly makes it available again.
- macOS evidence: exact DMG identity, policy-disabled baseline, scoped-underlay
  A/B confirmation, Leaf raw IPv4/IPv6 flow, DIRECT/proxy/DNS/FakeDNS, mesh route
  precedence, network-change recovery, repeated restart, and complete route/
  process cleanup.
- Windows evidence: compilation/package manifest, policy-disabled smoke,
  Wintun fail-closed startup, mesh precedence, direct/proxy/DNS rules, runtime
  stop/restart, and installed GUI smoke.
- Work during waits: inspect the complete diff, `Cargo.lock`, target `cfg`s,
  workflow pins, generated files, and prepare bounded validation commands without
  mutating the in-flight snapshot.

## Pre-build gate evidence

Validated on the dedicated remote builder on 2026-07-23 after syncing the
complete candidate snapshot:

- `scripts/leaf-remote-preflight.sh`: PASS. Its single `--locked` no-run build
  produced the exact `easytier`, `easytier_policy`, and `netstack_smoltcp`
  library test binaries; every configured focused filter passed serially.
- macOS route parser/ownership/transition suite: 7 passed.
- locked Leaf `packetInformation` presence/config boundary: 1 passed.
- frontend focused Vitest: 3 files and 43 tests passed.
- dependency-ordered `frontend-lib`, Web frontend, VPN plugin, and GUI production
  builds: PASS.
- The builder has Linux and Android targets but no Apple SDK or Windows target.
  macOS and Windows target compilation/package inspection therefore remain
  workflow-only gates and are not inferred from the Linux preflight.

The event-driven `tun` follow-up was then preflighted against the complete
combined workspace snapshot:

- `tun` 0.7.22 at
  `028b861d1a8e69cbb8950bfefb7ee81e44b46ff5` compiled with its `async` feature;
  both focused error-normalization tests passed.
- A fresh `--locked` EasyTier no-run build resolved that exact `tun` revision
  and Leaf revision `43515219f84df0bf5a9ed9e49bb60fdb4018ac06`.
  Every configured EasyTier, policy, and netstack focused filter passed.
- The frontend policy/runtime suite passed 32 tests across three files.
  Dependency-ordered `frontend-lib`, Web frontend, VPN plugin, and GUI
  production builds passed.
- Rust formatting, `git diff --check`, validation-script syntax, dependency
  source pins, and platform `cfg` boundaries were inspected. A macOS artifact
  and real-device backpressure rerun are still required.

## Post-build macOS evidence

The immutable macOS ARM64 GUI workflow `29989179967` built commit
`9d0ae14c35afcc4bf2e3a63cec7c24116d7d4e73` successfully. The downloaded DMG
checksum was
`8d3136aade2395b1ae96e08051f29c9a30e39da4a5e256715db8bacd1ba49d25`;
the installed core reported `easytier-core 3.0.5-9d0ae14c`, and strict
codesign checks passed for the core, Leaf worker, and HEV sidecar.

One focused real-device run passed on 2026-07-23:

- policy startup reached `transparent policy proxy is ready` in under one
  second and spawned the packaged Leaf worker;
- the policy TUN owned both IPv4 split-default capture routes while the selected
  physical interface retained the matching interface-scoped default;
- IPv4 TCP and UDP `MATCH,DIRECT` probes succeeded through the policy path;
- deleting the EasyTier-owned scoped default caused the existing supervisor to
  restore it in three seconds without replacing the Leaf runtime, after which
  both TCP and UDP probes succeeded again;
- the log contained neither `wire::Error` nor `invalid IP packet`, confirming
  the raw-IP packet-information framing fix on the exercised path;
- SIGTERM removed the core, Leaf worker, split-default routes, policy TUN, and
  EasyTier-owned scoped default while retaining pre-existing system routes.

That single PASS was not accepted as final evidence. A five-round wrapper
reproduced a failure in round two:

- startup, scoped default installation, and initial TCP/UDP DIRECT probes
  succeeded;
- after Leaf reported `Sending packet to TUN failed: No buffer space available
  (os error 55)`, its smoltcp inbound ended and subsequent packet injection
  reported `channel closed`;
- EasyTier then dropped policy packets fail-closed, and the post-route-recovery
  TCP probe timed out even though the scoped default itself was restored in
  three seconds;
- shutdown still removed candidate processes and routes.

The candidate therefore proves the route and raw-IP framing fixes but fails
runtime backpressure/lifecycle acceptance. Proxy, DNS/FakeDNS, mesh precedence,
IPv6, throughput, policy-disabled regression, and a clean repeated-restart run
remain pending and must not be inferred from the earlier single PASS.
