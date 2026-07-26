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
# Mihomo policy backend validation matrix

Status: FINAL EXACT CANDIDATE `08fb17d3` PASSED REMOTE PREFLIGHT, WORKFLOW,
PHYSICAL ANDROID, LINUX, AND PARTIAL MACOS RUNTIME GATES; IPV6 ROUTE AND
MESH-CHAIN SOCKS UDP FAILURES REMAIN OPEN

This matrix validates the optional desktop/Unix Mihomo policy backend without
changing Mihomo proxy, provider, group, rule, or `dialer-proxy` semantics.
Mihomo remains unavailable on Android and OHOS; their existing policy behavior
is outside this matrix.

## Immutable candidate identity

- EasyTier commit:
- EasyTier tree:
- Mihomo version:
- Mihomo archive SHA-256:
- GOST version:
- GOST archive SHA-256:
- GOST binary SHA-256:
- HEV source commit for Android/legacy Leaf:
- Core artifact SHA-256:
- GUI artifact SHA-256:
- Build ID:

No result may be copied between different candidate identities.

## Historical 2026-07-24 mutable pre-candidate evidence

The following evidence was collected from the HEV-era mutable workspace on
`192.168.2.160` on 2026-07-24. It does not cover the current GOST runtime,
8 KiB UDP readiness exchange, guardian reaping, selector fixes, or status
propagation and therefore does not satisfy the current dispatch gate.

| Scenario | Result |
| --- | --- |
| Locked Rust no-run and focused tests | PASS |
| Node 22 PolicyEditor tests | PASS, 25/25 |
| Dependency-order frontend/GUI production builds | PASS |
| Latest stable Mihomo resolution, upstream digest, ELF and version | PASS, `v1.19.29` |
| Real Mihomo `-t` with generated-safe source | PASS |
| Historical real HEV SOCKS5 TCP and UDP | PASS for old snapshot only |
| Historical HEV neutral entry startup/cleanup | PASS for old snapshot only, selected 11080 |
| Kernel port conflict | PASS, 11080 owner preserved and Core selected 11081 |
| Real Core plus Mihomo readiness in isolated netns | PASS |
| Ordinary-process route captured by `etm*` TUN | PASS |
| Mihomo DIRECT through veth underlay | PASS, exact payload returned |
| Source YAML immutable before readiness and after stop | PASS |
| Mihomo child `SIGKILL` | PASS, exactly one replacement child |
| Bounded post-restart observation | PASS, one child and no restart storm |
| Core stop cleanup | PASS, no child, Mihomo TUN, runtime, or mesh-entry listener |
| TERM immediately after sidecar readiness | PASS after early Unix signal registration |
| Container-global orphan process check | PASS, zero matching Mihomo children |
| GUI/Test/Release workflow YAML parse | PASS |
| Latest-stable GNU bundle-name/MUSL asset fetch | PASS, digest/format/metadata verified |

## Current mutable-source snapshot

| Gate | State | Evidence |
| --- | --- | --- |
| Pre-containment `.160` locked Rust no-run | SUPERSEDED | Exact EasyTier, policy, SOCKS-egress, and netstack library test binaries compiled before the shared sidecar-containment fixes. |
| Pre-guardian-fallback locked Rust no-run and focused tests | SUPERSEDED | Warning-free `.160` incremental compile completed in 41.29 seconds and all configured filters passed before the guardian lookup fallback changed Rust source. |
| Pre-final-review locked Rust no-run and focused tests | SUPERSEDED | Warning-free `.160` locked no-run completed in 47.55 seconds; the final readiness, validation-child, socket-path, retry, round-trip, GUI-status, and workflow-filter hardening changed source afterward. |
| Pre-combination locked Rust no-run and focused tests | SUPERSEDED | Warning-free `.160` locked no-run completed in 59.28 seconds; all configured filters passed, but the later Quinn and macOS DNS/interface commits changed Rust source. |
| Final combined locked Rust/quinn-udp no-run and focused tests | PASS | Exact combined snapshot: EasyTier/policy/SOCKS/netstack locked no-run completed in 4m25s; all configured filters passed, three-node completed in 3.08s; vendored quinn-udp compiled in 4.49s and passed 8/8 integration tests. |
| First combined workflow dispatch `5e1f7480` | FAIL | Android and macOS stopped in frozen pnpm install because the lockfile omitted the root happy-dom override; Linux was cancelled after the SHA became invalid. No platform compilation or artifact from these runs is accepted. |
| Corrected final frontend gate | PASS | `.160` frozen install passed; policy/editor/runtime/remote-management tests passed 51/51; frontend-lib, Web, VPN-plugin, and GUI production builds passed in dependency order. |
| Second combined workflow dispatch `4fd25fb3` | FAIL | Android found a cfg-only Result formatting compile error after frozen install; Linux and macOS were cancelled because the SHA became obsolete. No artifact is accepted. |
| `.160` Android cfg diagnostic | BLOCKED | Installed armv7 Rust target reached `ring`, then stopped because the builder has no `arm-linux-androideabi-clang`; it did not reach EasyTier. Final Android workflow remains mandatory. |
| Android candidate `bdb8c3d0` | PASS | Workflow 30190085529 completed successfully, including the cfg(android) neutral mesh-entry branch that failed in the prior SHA. |
| macOS candidate `bdb8c3d0` | FAIL | Resolver ownership/route suite passed 10/10; the combined step then hit its 10-minute aggregate timeout during the next Cargo invocation. Tests are split into separate bounded steps for revalidation. |
| Linux candidate `bdb8c3d0` | CANCELLED | Cancelled after the macOS workflow definition required a tracked correction; no artifact is accepted. |
| Android candidate `26ae1ff1` | PASS | Workflow 30190613355 completed successfully for the exact SHA. |
| Linux candidate `26ae1ff1` build | PASS | Workflow 30190613343 completed; artifact ZIP, outer and inner checksums, exact commit/target/toolchain/HEV pin, static PIE format, symbols, and Build IDs verified. |
| Linux candidate `26ae1ff1` package completeness | FAIL | Profiling bundle omitted easytier-mihomo and its metadata, so it cannot validate the implemented backend. Packaging now includes the verified runtime and compliance/build metadata; old artifact rejected. |
| macOS candidate `26ae1ff1` implementation/build | PASS | Quinn truncation, resolver 10/10, interface cache, scoped DNS, GUI build, and all six executable signatures passed. |
| macOS candidate `26ae1ff1` final verifier | FAIL | Verifier assumed resources were flattened at Contents/Resources root; formal GUI uses recursive lookup. Focused workflow now matches formal semantics; old run produced no accepted artifact. |
| Final Linux profiling candidate `08fb17d3` | PASS | Workflow 30191697425 passed. Artifact 8629000180 passed outer, inner, and Mihomo-specific checksums and identifies the exact SHA, run, toolchain, target, and HEV pin. |
| Final Android policy candidate `08fb17d3` | PASS | Workflow 30191697411 passed compilation, unit tests, APK packaging, and captured-UID probe packaging. The exact APK was subsequently upgraded in place on a physical arm64 Android 15 device and passed captured-UID DIRECT, policy TLS, Wi-Fi outage/recovery, configuration retention, stop/start, and cleanup checks. |
| Final macOS policy and Quinn workflow tests | PASS | Workflow 30191701843 passed truncated-datagram, resolver ownership, interface-cache, scoped-DNS, GUI build, recursive sidecar discovery, and signature checks for the exact SHA. |
| Final Linux package completeness | PASS | Bundle contains Core, CLI, Leaf, HEV, GOST, Mihomo, perf tools, manifests, source/license notices, checksums, and build metadata. |
| CentOS 7 executable compatibility | PASS | Exact artifact checksums and startup passed on `.37` and `.38`; GOST SOCKS5 TCP and Mihomo DIRECT each delivered a real cross-host HTTP response and were cleaned up. |
| Current focused frontend tests | PASS | Policy runtime/editor/document tests passed 35/35 and RemoteManagement tests passed 20/20. |
| Current clean-output frontend builds | PASS | frontend-lib, Web, VPN plugin, and GUI production builds returned success after generated output was removed and recreated; GUI consumed the refreshed API and VPN declarations. |
| Current three-node regression | PASS | Exact default-drop ACL case completed in 3.07 seconds after the guardian fallback change. |
| Pre-containment three-node regression | SUPERSEDED | Default-drop ACL case completed in 3.17 seconds before the current Rust snapshot. |
| Current frontend/GUI gate | PASS | Node 22 focused Vitest 49/49; frontend-lib, Web, VPN plugin, and GUI builds passed in dependency order. |
| Current sidecar acquisition | PASS | Latest-stable Mihomo `v1.19.29` and pinned GOST `v3.2.6` Linux x86_64 archives, digests, ELF machines, static binaries, generated metadata, and independent verify modes passed. |
| Current script/workflow syntax | PASS | Changed shell scripts passed `bash -n`; all six changed workflows passed `actionlint`. |
| Diff whitespace and private candidate data | PASS | `git diff --check` passed; changed and untracked content contains none of the maintainer-only host or credential patterns. |
| Immutable candidate SHA and artifacts | PASS | Final candidate is `08fb17d3bddd05bd5ba4ded02ae1d93103c4c2d6`; Linux 30191697425, Android 30191697411, and macOS 30191701843 all completed successfully for that SHA. |

Physical Android, CentOS 7, public dual-stack, relay, performance/resource,
Linux Core-managed Mihomo, and macOS packaged-GOST runtime evidence is recorded
in
[`mihomo_policy_exact_candidate_runtime_08fb17d3.md`](mihomo_policy_exact_candidate_runtime_08fb17d3.md).
Installed macOS GUI-managed policy runtime, Windows, and other Unix runtime rows
remain `PENDING`. Workflow success must not be promoted to those rows.

## Safety abort conditions

Stop the test immediately and preserve logs when any condition is observed:

- one idle process continuously consumes more than one CPU core for 15 seconds;
- child restart rate exceeds the configured bounded restart policy;
- connection attempts grow without bounded backoff while no test traffic exists;
- RSS, file descriptors, threads, routes, TUN devices, or temporary files grow
  monotonically across three stop/start cycles;
- management access, the host default route, or an unrelated production service
  is disrupted;
- the generated runtime configuration contains a credential not present in the
  immutable source configuration.

## Common semantic checks

| ID | Scenario | Required result | Status |
| --- | --- | --- | --- |
| C01 | Backend `off` | No Mihomo child or policy TUN; mesh remains usable | |
| C02 | Legacy `enable_policy_proxy=true` without backend | Selects Leaf with deprecation-compatible behavior | |
| C03 | Explicit backend conflicts with legacy boolean | Explicit backend wins; current serializers normalize the legacy boolean | |
| C04 | Backend `leaf` | Existing Leaf path and configuration remain unchanged | |
| C05 | Backend `mihomo`, valid source YAML | Source file remains byte-identical; only runtime copy is used; Leaf parser is not invoked | |
| C06 | Source YAML includes proxies/providers/groups/dialer-proxy | Runtime copy preserves those subtrees semantically and byte-independent round-trip tests pass | |
| C07 | Conflicting TUN/controller/route fields | Runtime copy reports each EasyTier-owned override; source remains unchanged | |
| C08 | Missing reserved process rules | Required EasyTier, GOST, HEV, Leaf, Mihomo, and Tailscale DIRECT rules are prepended without duplicates | |
| C09 | Existing equivalent reserved rules | No duplicate insertion | |
| C10 | Invalid YAML or failed `mihomo -t` | Fail closed before route/TUN activation; mesh remains usable | |
| C11 | Mihomo starts but controller/TUN is not ready | Never reports `Running`; bounded failure and cleanup | |
| C12 | Mihomo crash | Bounded restart with visible reason/count; no restart storm | |
| C13 | Restart budget exhausted | State remains failed until explicit restart or config generation changes | |
| C14 | EasyTier stop | Child, controller, TUN, routes, runtime YAML, PID/lock, and temporary files are removed | |
| C15 | Core-only package | Starts GOST and Mihomo without GUI on a supported target | |
| C16 | GUI package | Uses the same Core lifecycle and status; does not host a second supervisor | |
| C17 | Multiple EasyTier networks | One process-wide Mihomo owner and one process-wide neutral GOST mesh entry | |
| C18 | Second EasyTier process | Deterministic lock conflict; never manages the first process's child | |

## GOST neutral mesh-entry checks

| ID | Scenario | Required result | Status |
| --- | --- | --- | --- |
| G01 | Normal startup | Exactly one loopback listener is selected from 11080-11082 and status publishes the selected port | |
| G02 | First port occupied | Existing owner survives; GOST selects the next candidate | |
| G03 | All ports occupied | Mesh starts; GOST status is unavailable with a precise error | |
| G04 | UDP readiness | `udp=true&udpBufferSize=65535` passes a real 8 KiB loopback echo before publication | |
| G05 | SOCKS target is a mesh peer | GOST follows the operating-system EasyTier mesh route, not a forced physical interface | |
| G06 | Leaf DIRECT egress | Existing platform loop-prevention binding remains isolated from neutral GOST | |
| G07 | Abnormal GOST exit | Same port restarts with bounded backoff; macOS/FreeBSD guardian is reaped | |
| G08 | Ten stop/start cycles | No child, guardian, listener, task, FD, or temporary-file residue | |

## Loop and route checks

| ID | Scenario | Required result | Status |
| --- | --- | --- | --- |
| L01 | EasyTier underlay while Mihomo is active | Underlay remains DIRECT and does not enter the policy TUN repeatedly | |
| L02 | Mihomo own DIRECT traffic | No self-capture loop | |
| L03 | GOST to mesh peer | Uses the EasyTier mesh route exactly once | |
| L04 | Tailscale process traffic | Required process rules remain DIRECT | |
| L05 | User proxy through `dialer-proxy: p1` | Mihomo honors the user's original chain; EasyTier does not inspect or rewrite it | |
| L06 | Same mesh-range address intentionally not using mesh | User can choose a non-GOST dialer path; no automatic endpoint rewrite occurs | |
| L07 | Relay-only mesh path | Blocking direct peer transport does not create a retry or route-generation storm | |

## Platform matrix

### Linux

- Core-only and GUI packages.
- x86_64 release artifact plus at least one non-x86_64 compile/package target.
- TUN/auto-route, IPv4, IPv6, dual-stack, DNS, TCP, UDP, relay, KCP, and QUIC.
- Compatibility host lifecycle and cleanup.
- Public dual-stack cross-host throughput and idle resource baseline.

### macOS

- Signed GUI/Core package on the dedicated macOS validation host.
- `utun` ownership, default route preservation, sleep/wake, service kill/restart,
  port fallback, IPv4/IPv6/dual-stack, and stop cleanup.
- Existing EasyTier service may be stopped with administrator privileges before
  the isolated test; restore only the service that the test explicitly replaced.

### Windows

- Core-only and GUI packages on x86_64.
- Wintun ownership, service/process lifecycle, named controller endpoint,
  IPv4 with IPv6 disabled, dual-stack, port fallback, and reboot-safe cleanup.
- GOST is not accepted as supported until the packaged sidecar has the expected
  architecture, runs under Job Object ownership, and passes real TCP/UDP
  readiness.

### Other Unix targets

- Build/package evidence for each declared supported target.
- Runtime validation where a maintained host exists.
- A target without a proven Mihomo and GOST artifact must not claim the desktop
  backend or neutral GOST entry. Normal EasyTier mesh operation may remain
  supported without that optional capability.

## Performance and resource comparison

For each runnable platform, compare the same host, route, proxy, payload, and
three-run median:

- policy disabled mesh baseline;
- Mihomo DIRECT;
- Mihomo native proxy;
- Mihomo `dialer-proxy` through local GOST and a mesh peer SOCKS service;
- idle CPU/RSS/FD/thread counts;
- loaded TCP throughput and CPU;
- loaded UDP loss/jitter and CPU;
- crash/restart and ten-cycle cleanup baselines.

Functional acceptance requires no loop, black hole, or unbounded retry.
Performance acceptance requires no stable regression below 95% of the matching
parent path unless a documented platform limitation is explicitly accepted.
