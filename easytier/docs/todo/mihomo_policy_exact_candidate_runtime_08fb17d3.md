# Mihomo policy exact-candidate runtime report (`08fb17d3`)

Status: RUNTIME VALIDATION COMPLETE WITH TWO FUNCTIONAL FAILURES AND ONE
RESOURCE CONCERN

Date: 2026-07-26

This report records only evidence collected from immutable candidate
`08fb17d3bddd05bd5ba4ded02ae1d93103c4c2d6`. Private maintainer hostnames,
public addresses, credentials, and user proxy configuration are intentionally
omitted.

## Candidate identity

- Linux profiling workflow: `30191697425`, PASS.
- Android policy workflow: `30191697411`, PASS.
- macOS ARM64 policy/Quinn workflow: `30191701843`, PASS.
- Linux x86_64-musl archive SHA-256:
  `97d922f61f64ceee55f3b99b8530c63974c311209d1c7092f6ce83c4d4360fb9`.
- macOS ARM64 DMG SHA-256:
  `ba77a4ab437e3a1311fe494e32c525a351490ae9181515c239affddbbea19ec5`.
- Every deployed artifact was matched to the exact candidate before runtime
  testing. No local build was used.

## Result summary

| Area | Result | Boundary |
| --- | --- | --- |
| Android exact-artifact upgrade and policy runtime | PASS | Physical arm64 Android 15 device on cellular and Wi-Fi |
| CentOS 7 mesh/GOST compatibility and lifecycle | PASS | Two internal Linux 3.10 hosts |
| Linux Core-managed Mihomo | PASS | Isolated network namespace with real policy TUN traffic |
| Public IPv4/IPv6 underlay and data plane | PASS | Two public dual-stack 10 Gbps hosts |
| Automatic virtual IPv6 peer route | FAIL | Manual `/128` host routes were required |
| Mihomo/GOST TCP chain through mesh | PASS | User `dialer-proxy` semantics preserved |
| Mihomo/GOST UDP chain through mesh | FAIL | SOCKS UDP association source identity mismatch |
| Relay fallback | PASS | Direct test ports blocked, third internal relay retained traffic |
| macOS package/signature and packaged GOST | PASS | ARM64 DMG; real TCP and 8 KiB UDP |
| macOS GUI-managed Core/Mihomo runtime | PARTIAL | GUI reached macOS authorization trampoline; no semantic authorization was supplied |
| Android idle resource target | CONCERN | No traffic storm, but a 30-second sample still used about 8% of one core |

## Android physical-device evidence

- Installed the exact candidate with `adb install -r`; no uninstall or data
  clearing was used.
- `firstInstallTime` remained unchanged.
- `shared_prefs` and WebView Local Storage/IndexedDB were archived before the
  first post-upgrade start and remained byte-identical. The retained archive
  SHA-256 was
  `76b938...`.
- The UI reported version `3.0.5-08fb17d3`, Leaf Running, ten peers, retained
  policy configuration, and the expected relayed mesh peer.
- The workflow-built captured-UID probe UID was present in the VPN UID ranges.
- DIRECT TLS to a mainland target passed in about 190 ms.
- Policy TLS to a target blocked by the uncontrolled direct baseline passed in
  about 1.9 seconds.
- One detached device-side Wi-Fi cycle recorded successful disable and enable
  return codes. Native state advanced to `outage!1`, recovered with a new
  network key, restored DNS, retained the VPN, and passed both DIRECT and policy
  TLS after recovery.
- Semantic stop removed the VPN and TUN. Threads fell from 69 to 61.
- With the VPN down, the controlled policy target failed through the direct
  path after ten seconds. After restarting the same EasyTier network, its TLS
  probe succeeded in about 1.58 seconds.
- Probe packages, ADB forwarding, temporary run-as data, and Wi-Fi recovery
  files were removed after validation.

### Android resource boundary

A 30-second no-user-traffic sample reported:

- about 7.96% of one CPU core;
- 69 stable threads;
- RSS moving from 267,924 KiB to 264,744 KiB;
- zero measured UID network-traffic growth.

The sample did not reproduce the previous full-core Leaf netstack busy loop.
It did contain periodic SELinux denials from route/ARP/sysfs and a short
packet-socket denial burst. This is not evidence of a traffic or restart storm,
but it misses the desired below-5%-of-one-core idle target and remains a power
investigation item.

## Internal Linux compatibility and lifecycle

Two CentOS 7 / Linux 3.10 hosts ran the exact static PIE artifact with isolated
virtual addresses, explicit listeners, KCP enabled, and QUIC enabled.

- Bidirectional mesh ICMP passed with about 0.5-0.6 ms latency and no loss.
- Core-managed GOST TCP reached a fixed HTTP service on the peer.
- A direct SOCKS5 UDP ASSOCIATE to managed GOST echoed 8,192 bytes exactly.
- One unrelated process occupied `127.0.0.1:11080`; Core selected `11081`,
  preserved the unrelated owner, and passed TCP through the selected port.
- Killing the managed GOST produced one replacement on the same selected port
  in about two seconds. TCP passed after replacement.
- Three complete Core start/stop cycles used 38 FDs and 14-15 threads. RSS was
  13,976-14,084 KiB. Each stop removed the Core, GOST, TUN, listener, and
  temporary state while preserving the unrelated port owner.

## Linux Core-managed Mihomo

The exact artifact was started inside an isolated network namespace. The
source Mihomo YAML contained a minimal DIRECT rule and was hashed before
startup.

- The correct TOML envelope used `[policy_proxy]`, `enabled = false`, and
  `backend = "mihomo"`.
- Core started one EasyTier TUN, one Mihomo `etm*` TUN, one managed GOST, and
  one packaged Mihomo child.
- A one-MiB HTTP transfer increased policy-TUN RX from 1,259 bytes to
  1,058,523 bytes, proving the process traffic traversed Mihomo rather than the
  namespace default route alone.
- The source YAML SHA-256 remained
  `85386f54e2dbaaed4a15ebd62e1ca9945b1ff4e8c9a5a51d8de0f991ab24591a`
  before traffic, after startup, and after restart.
- Killing Mihomo PID `4648` produced exactly one replacement PID `4968`.
  Managed GOST was not restarted and DIRECT traffic passed after recovery.
- The running snapshot used 34 Core FDs, 6 GOST FDs, and 13 Mihomo FDs.
- Graceful Core stop removed both TUNs, GOST, Mihomo, all candidate listeners,
  and the private `/tmp/etm-*` runtime directory.
- Namespace veth, NAT/FORWARD rules, and the fixed HTTP service were removed.

The first diagnostic TOML placed policy fields at top level and therefore
started only ordinary EasyTier plus GOST. It was rejected as evidence. A
second diagnostic placed logging fields under `[flags]`; strict parsing
correctly rejected the unknown fields. Only the valid `[policy_proxy]` run
above is accepted.

## Public dual-stack and performance evidence

The public pair has 10 Gbps IPv4 and IPv6 connectivity but only one vCPU and
about one GiB RAM per host.

### Raw baseline

| Direction | IPv4 | IPv6 |
| --- | ---: | ---: |
| Upload receiver | 7,951 Mbit/s | 7,776 Mbit/s |
| Download receiver | 7,684 Mbit/s | 7,670 Mbit/s |

### EasyTier data plane

- IPv4 underlay plus IPv4 overlay was noisy: upload receiver results were
  61.7, 80.5, and 103 Mbit/s; successful downloads were 292 and 301 Mbit/s.
- IPv4 underlay plus IPv6 overlay reached about 294 Mbit/s upload and
  471 Mbit/s download.
- IPv6 underlay plus IPv4 overlay reached about 413 Mbit/s upload and
  304 Mbit/s download.
- IPv6 underlay plus IPv6 overlay reached about 384 Mbit/s upload and
  530 Mbit/s download.
- A 96.5 Mbit/s upload used about 46.5% of one source CPU core and 30.3% of one
  destination core.
- A 516 Mbit/s IPv6 download used about 70.2% and 53.6% of the two single
  cores. RSS remained about 25-38 MiB.
- Thirty-second process samples after the load were stable and did not show a
  full-core idle loop or monotonic RSS/thread growth.

These results prove IPv4, IPv6, and dual-stack data-plane operation after a
route is present. They do not meet the raw-kernel baseline and should be
interpreted as a single-core userspace ceiling plus path noise, not as a 10
Gbps claim.

### Virtual IPv6 route failure

Passing a bare virtual IPv6 address to the CLI produced a `/128` local address.
The peer `/128` route was not installed, so ordinary IPv6 overlay traffic
returned `Network unreachable`. Supplying `/64` was rejected by
`--check-config`.

Adding explicit peer `/128` routes to the EasyTier TUN made IPv6 ICMP and all
IPv6 performance cases above work immediately. Packet framing and forwarding
therefore work, but automatic IPv6 route publication/installation does not.
This remains a functional failure for virtual IPv6 users and must not be
reported as complete IPv6 support.

## Mihomo, GOST, and mesh chaining

The user-owned Mihomo configuration defined:

- a local SOCKS5 `mesh-entry` at the Core-managed loopback GOST port;
- a peer SOCKS5 exit at a virtual EasyTier address;
- `dialer-proxy: mesh-entry` on that peer exit;
- no EasyTier-side proxy rewrite.

### TCP

- The complete Mihomo -> local GOST -> mesh -> peer GOST -> Internet chain
  exited from the peer host as expected.
- TLS to a policy target passed.
- Three 32 MiB full-chain transfers were about 159, 186, and 191 Mbit/s.
- The matching direct mesh HTTP path was about 301, 337, and 360 Mbit/s.
- Local managed GOST to the same mesh HTTP service was about 229, 261, and
  270 Mbit/s.

The additional Mihomo and peer-GOST hops have a measurable throughput cost but
no TCP black hole, loop, or unbounded retry was observed.

### UDP failure and root cause

- Ordinary EasyTier overlay UDP echoed 64, 1,200, and 8,192 bytes exactly.
- Managed local GOST echoed 8,192 bytes exactly on the internal pair.
- The peer GOST was explicitly configured with
  `udp=true&udpBufferSize=65535`.
- Direct peer-GOST UDP ASSOCIATE through the mesh failed even for small
  datagrams; the full Mihomo chain also timed out.

The peer GOST observed the TCP association source as its own virtual
destination address, while subsequent UDP datagrams arrived from the real
source peer virtual address. GOST pins UDP association identity to the TCP
source and therefore discarded the datagrams.

This is not a GOST buffer limit, public UDP filtering, or ordinary EasyTier UDP
failure. It is a source-address identity mismatch introduced by the current
EasyTier TCP proxy path. Mihomo SOCKS `dialer-proxy` applies only to the TCP
dialer and has no SOCKS UoT option, so the configuration cannot hide the
mismatch. Standard SOCKS UDP through this mesh chain remains unsupported until
the identity contract is fixed or explicitly narrowed.

## Relay evidence

An internal third Core connected independently to both public nodes. Temporary
IPv4 and IPv6 firewall rules, limited to the isolated EasyTier listener range,
blocked direct test transports between the public pair.

- During the block, each public Core retained only its established connection
  to the internal relay; peer-direct attempts were not established.
- Overlay ICMP and a fixed HTTP transfer continued to pass.
- Relay-path latency was noisy at about 371-1,562 ms because the path crossed
  the mainland network.
- Removing the rules restored the public pair's direct IPv6 transport and
  reduced overlay ping to about 0.8-1.0 ms.

The relay host's TUN counters did not increase, which is expected: EasyTier
relays overlay frames without injecting the inner packet into the relay host's
local TUN. The transport connection table and direct-path block/recovery form
the accepted relay evidence.

All temporary firewall rules were removed by their unique `et08relay` comment
and both IPv4 and IPv6 rule counts were verified as zero.

## macOS ARM64 evidence

- The DMG image checksum and HFS image checksum passed.
- The App bundle and all nested executables passed
  `codesign --verify --deep --strict`.
- GUI, GOST, GOST guardian, Leaf worker, HEV, and Mihomo were arm64 Mach-O
  executables.
- The GUI binary launched and reached the macOS authorization trampoline.
  Automation did not submit an authorization UI, so GUI-managed Core/Mihomo,
  route, and DNS behavior is not claimed by this report.
- The packaged GOST listened only on `127.0.0.1:11080`.
- A real SOCKS5 TCP request returned the exact fixed payload.
- A real SOCKS5 UDP ASSOCIATE echoed 8,192 bytes exactly.
- The idle GOST sample reported 0.0% CPU, 28,192 KiB RSS, and 11 open-file
  records.
- GOST stop released TCP and UDP listeners. The DMG was detached and no
  candidate process remained.

## Final cleanup

- Internal namespace, veth, NAT/FORWARD rules, HTTP service, Core, GOST,
  Mihomo, TUNs, and listeners were removed.
- Public test Core instances, managed GOST children, standalone Mihomo, peer
  GOST, HTTP/UDP/iperf services, TUNs, and listeners were removed.
- No `et08relay` IPv4 or IPv6 firewall rule remained.
- The macOS DMG was detached and candidate processes/listeners were absent.
- Android was returned to its running VPN state; validation probes and
  temporary automation state were removed.

## Release interpretation

The candidate is substantially functional across Android, Linux, and packaged
macOS components, and the Core-managed Mihomo lifecycle is validated on Linux.
It is not a clean all-features release candidate while either of these claims
remains public:

1. automatic virtual IPv6 peer connectivity;
2. standard SOCKS5 UDP through a peer exit reached via the neutral mesh entry.

If the release explicitly excludes both capabilities, they may be documented
as known limitations instead of blocking the remaining TCP/IPv4 feature set.
The Android idle CPU result and installed macOS GUI-managed policy runtime
remain release-risk items rather than proven functional regressions.
