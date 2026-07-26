# Core GRO, GOST, and IPv6 candidate manifest

Status: GRO PRODUCTION OPTIMIZATION REJECTED; IPV6 AND GOST FIXES RETAINED

Date: 2026-07-26

The immutable profiling candidate was
`49682dbdf864749f7a17eab8e1ab14ad30bb718f`, workflow
`30200437716`. The workflow passed, but public dual-stack runtime validation
rejected the Linux GRO scratch change. The next candidate must remove only that
production optimization and retain the independently tested IPv6 and GOST
fixes.

## Baseline and scope

- Code baseline: `08fb17d3bddd05bd5ba4ded02ae1d93103c4c2d6`.
- Documentation-only baseline descendant:
  `ad26e0ac16d6a0316a67b081af9456e1e68824c3`.
- The immutable candidate SHA and workflow run will be recorded only after the
  complete snapshot is committed and pushed once.
- No Leaf routing, mesh transport selection, generic TUN path, Windows, macOS,
  BSD, or policy rule semantics are changed by this candidate.

The complete build-affecting batch is:

1. Treat a bare virtual IPv6 address as `/64`, preserve an explicit prefix,
   reject invalid input consistently in TOML and CLI paths, and cover both
   paths with focused tests.
2. Pin the Core-managed GOST runtime to `v3.2.9-easytier.1`, verify every
   packaged target by SHA-256, and support platform-specific archive member
   names without changing the runtime interface.
3. Enable GOST SOCKS5 UDP first-packet source pinning and a 65,535-byte UDP
   buffer. This preserves strict checking after the first packet while allowing
   EasyTier's TCP and UDP mesh paths to present different initial source
   addresses.
4. On the existing Linux TUN offload path only, give multi-packet TCP GRO one
   reusable expansion buffer. Single-packet, two-packet, UDP, ICMP, legacy TUN,
   and non-Linux paths retain their existing allocation and send behavior.
5. Add a standalone Linux Rust probe that reproduces the exact `tun-rs` GRO
   capacity constraint before changing production code. The probe is
   diagnostic evidence, not a substitute for optimized artifact validation.

## Bounded resource and compatibility contract

- The GRO change adds at most one reusable buffer of approximately 65 KiB per
  Linux offload sink, not one buffer per packet, peer, flow, or queued batch.
- The existing channel capacity, maximum batch size, packet order, checksum
  preparation, GSO framing, and legacy fallback remain unchanged.
- If the offload path is unavailable, existing fallback remains
  `fast GSO -> fast -> legacy`; this candidate does not alter that selection.
- The GOST source-check relaxation is opt-in in the pinned EasyTier listener
  URL and still pins all later UDP packets to the first observed UDP source.
- The GOST fork and protocol fork are built from recorded upstream baselines
  and exact source commits. No private endpoint or credential is embedded.

## Mandatory `.160` preflight

The complete working snapshot was synchronized to `192.168.2.160`.

- `cargo test --locked --package easytier --lib --features mesh-socks-egress --no-run`
  passed using all builder CPUs and the required bounded command.
- Exact test binary: `target/debug/deps/easytier-9c3f71ef8fc43189`.
- Focused tests passed:
  - `gro_scratch_only_promotes_multi_packet_tcp_batches_and_is_reusable`
  - `gro_scratch_ignores_batches_without_tcp_candidates`
  - bare and explicit virtual IPv6 prefix tests
  - GOST listener URL test
- The latest pinned Linux GOST asset was fetched by the repository script,
  checksum verified, and identified as `v3.2.9-easytier.1`.
- A real SOCKS5 UDP test showed that strict source matching rejected the
  EasyTier-style source change while first-packet mode accepted the first UDP
  source and rejected a later different source.
- `git diff --check` passed.

The standalone GRO probe used valid sequential IPv4/TCP packets and the pinned
`tun-rs` implementation:

- ordinary approximately 4 KiB packet buffers produced about 50,000 expanded
  GRO frames for 100,000 packets;
- one reusable approximately 65 KiB scratch buffer produced 3,125 expanded GRO
  frames for the same 100,000 packets;
- the probe therefore demonstrates a 16x reduction in legal expanded frames
  for full batches, but does not claim a 16x Core throughput improvement.

## One-workflow dispatch

Only `.github/workflows/profiling-beta.yml` is required for this performance
candidate. It must build the immutable final SHA once and package the pinned
GOST asset. Documentation-only evidence updates after that run must not trigger
another build.

During the workflow:

- locate and verify the exact `08fb17d3` baseline artifact;
- prepare isolated, host-specific directories on the shared public validation
  storage;
- verify listener ports, process cleanup, raw IPv4/IPv6 health, and profiling
  commands;
- inspect only profile-supported Core hotspots without modifying the in-flight
  candidate.

## Public dual-stack validation matrix

Use the two public 10 Gbps dual-stack validation hosts. Record host role and
direction for every number without storing private hostnames in this document.
Use identical topology, ports, payload, duration, and transport settings for
baseline and candidate.

| Area | Required evidence |
| --- | --- |
| Artifact identity | Outer/inner checksum, candidate SHA, build ID, target, GOST version and checksum |
| Raw network | IPv4 and IPv6 throughput immediately before each EasyTier series |
| Overlay correctness | IPv4-only, IPv6-only, and dual-stack ICMP/TCP in both directions |
| Legacy control | Existing non-offload path unchanged and functional |
| GSO control | Existing offload path functional with no packet corruption or fallback storm |
| Core throughput | Three runs per direction; median and range for baseline and candidate |
| Mesh entry | Direct mesh and local GOST-to-mesh controls before the full chain |
| CPU | Sender and receiver Core CPU for the same measurement interval |
| Resources | RSS, FDs, threads, context switches, and start/stop cleanup |
| GRO mechanism | TUN writes/syscalls or equivalent profile evidence showing whether larger batches reach the kernel |
| Safety | Abort immediately on retry storm, sustained full-core idle use, monotonic resource growth, packet corruption, or host impairment |

Acceptance requires:

- no correctness, lifecycle, or cleanup regression;
- no stable throughput regression greater than 5% in an unchanged path;
- a repeatable Core throughput or CPU-efficiency gain on the Linux GSO path;
- measured evidence that any gain comes from reduced GRO output/write work,
  rather than unrelated WAN variation.

If the optimized artifact does not satisfy those conditions, revert only the
GRO scratch optimization. The independent IPv6 and pinned GOST fixes remain
separately reviewable and testable.

## Runtime result and disposition

The public pair was healthy immediately before the comparison:

- raw IPv4: 7,773.3 Mbit/s;
- raw IPv6: 7,482.2 Mbit/s;
- raw IPv4 recheck during the candidate regression: 7,779.6 Mbit/s.

Exact `08fb17d3` baseline, three ten-second runs per direction:

- public host 2 to public host 3:
  76.8, 336.5, and 336.6 Mbit/s; median 336.5 Mbit/s. The first run was a
  path-warmup outlier and was not used to inflate the candidate comparison.
- public host 3 to public host 2:
  308.0, 320.0, and 302.1 Mbit/s; median 308.0 Mbit/s.

Exact `49682dbd` candidate:

- public host 2 to public host 3:
  168.5, 163.8, and 158.3 Mbit/s; median 163.8 Mbit/s.
- public host 3 to public host 2:
  297.1, 302.4, and 343.4 Mbit/s; median 302.4 Mbit/s.
- an additional first-direction candidate run was 176.7 Mbit/s.

An A/B/A confirmation switched the same hosts, ports, virtual addresses,
unencrypted mesh, and enabled KCP/QUIC settings back to `08fb17d3`. The
first-direction results recovered to 276.7, 352.5, and 337.1 Mbit/s; median
337.1 Mbit/s.

The candidate therefore caused an approximately 51% median regression in the
direction whose receiver used the expanded GRO scratch while leaving the
reverse control direction almost unchanged. Raw networking remained above
7.7 Gbit/s. This is sufficient causal evidence to reject the production
change; no additional workflow or platform rollout is justified for that
implementation.

The standalone probe remains useful as a failed-assumption record: increasing
buffer capacity allowed larger legal GRO frames in isolation, but that
mechanism did not improve the real Core/kernel path and materially reduced
throughput on the tested Linux receiver.

## Core optimization boundary during the build

Further implementation is allowed only after the exact candidate profile
identifies a material Core-owned hotspot and a standalone focused tool can
reproduce its cost. Do not revisit generic writer flushing, larger channels,
TUN `writev`, interface-name routing, or speculative transport changes: earlier
controlled evidence already rejected those approaches or they violate the
cross-platform contract.
