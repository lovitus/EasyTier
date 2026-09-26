# Integrated Core packet-stage diagnosis

## Result and boundary

[Run 36263145990](https://github.com/lovitus/EasyTier/actions/runs/36263145990)
completed all six bounded cases. No missing echo reproduced. This is **not**
a repair of, or replacement PASS for, the uninstrumented IPv6 failure in
[run 36260893872](https://github.com/lovitus/EasyTier/actions/runs/36260893872).
Its disposition remains as recorded in [COMBINED_PACKAGED.md](COMBINED_PACKAGED.md).

The first setup attempt, `36262853021`, failed before compilation because
the diagnostic job omitted installation of the pinned rustfmt component.
That failure remains recorded. The repaired run installs the component;
there was no change to Core behavior, test assertions, or the already built
production candidates.

## Identity and topology

- Production source: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
- Harness: `cfd1dc2ec1157c6d1449eed845b97eedf9a30c69`.
- Observation overlay changes only `peer_manager.rs` and `tunnel/udp.rs`:
  bounded ICMP identity and encrypted-payload fingerprint records at existing
  send, UDP receive, ring rejection, peer receive and NIC enqueue boundaries.
- Queue limits, scheduling, ciphertext, route and TUN behavior are unchanged.
- Rust 1.95, optimized x86_64 musl, jemalloc, no Leaf/policy features.
- Both endpoints are namespaces on one GitHub runner, IPv4 UDP/veth underlay,
  IPv6 overlay, AES-GCM, Stealth off, one mesh TUN per endpoint, MTU 1360.
- Runner: **AMD EPYC 9V74**, four logical CPUs, Linux 6.8.0-1064-azure.
  The earlier failed uninstrumented run used **EPYC 7763**. Do not pool their
  performance or treat them as controlled same-hardware repetitions.

Evidence artifact `10912384647` is 384,898 bytes. Its complete ZIP digest,
ZIP CRC and all **325 internal manifest entries** were checked:
`ee5f413bc2435a2d9f40c8a76212eaee418ab0a20526f25bb67c93b323172b6e`.

The independently retained diagnostic-binary artifact is `10912354766`,
106,159,206 bytes. Its API-reported outer digest is
`b6c7fd95e007339644d16a104bc9a70a1e5f449f86937cddaa0c02d3f46be165`;
it has **not** been downloaded and rehashed locally. The CI evidence records:

- Core SHA-256:
  `b165f67da30203ae2eea86c05e5f4b157552d5168c23710ae39c75d518006680`.
- Core Build ID: `38e037e78a479de41cf48d22b6ada82db5b0f65a`.
- CLI SHA-256:
  `b462eb22b7a9d54b0eab6c69892c4af6c5f499d78a7ce492087963f2a92b27e0`.
- Probe SHA-256:
  `62596a8dd4f53e538c0696e3f746d6010e03758f62fe00822510bc8d15c61ab0`.

These are diagnostic executables, not deployable production replacements.
Reuse them for further observation instead of recompiling unchanged Core.

## Audited observations

- Six complete cases, twelve 1 GiB bulk transfers, twelve independent
  payload-integrity checks and 180 UDP echoes.
- Twelve concurrent ping invocations: **240/240 replies**.
- **480 ICMP packets** (request and reply counted separately) correlate
  exactly once from encrypted transmit to remote UDP receive and peer receive,
  then by decoded identity to remote NIC enqueue. None has a ring-reject record.
- Each endpoint's TUN capture contains 80 matching ICMP records per case,
  covering two ping IDs and request/reply sequences 1 through 20. All twelve
  captures report zero capture-kernel drops.
- No packet-trace overflow. Largest Core log: **17,499 bytes**, below the
  unchanged 2 MiB process file bound.
- All 114 tracked cleanup records are clean, twelve namespaces removed,
  six root-route comparisons unchanged. No forced-kill or residual-PID record.

The trace filters observation to the fixture's ICMP-size packets. Absence of
their rejection does **not** prove zero ring rejection of other Data traffic.
Instrumented throughput and CPU are not incorporated into performance medians.
Short process observations do not establish long-term memory stability.

## Disposition and next step

The current integrated code can complete the full observed ICMP packet path,
but the original rare saturation loss remains unlocalized. Historical
`36088594844` still proves receive-ring rejection for a different lost echo;
the new successful observation neither disproves that mechanism nor proves
the latest failed sample has the same cause.

Do not repeat unchanged single-flow observations until they happen to fail
or pass. The next useful bounded observation is the already outstanding
**concurrent upload/download** case, using these retained diagnostic bytes
and the existing lab's mixed-flow mode, original integrity/control-progress
assertions, per-process log bounds and cleanup. This investigates service
pressure on both receive paths; it is not a new performance comparison.

No production patch, queue enlargement, control-packet prioritization,
`recv_many`/`recvmmsg` revival, merge or release follows from this result.

### Mixed-flow follow-up prepared

The existing workflow now accepts `packet_trace_reuse=true` with
`extended_tun=true`. It downloads only the pinned diagnostic binary/evidence
artifacts on the hosted runner, checks both full archive digests, originating
run and SHA, and all three executable digests before execution. Compiler,
toolchain preparation, Rust cache and Core build steps are skipped. No new
binary artifact duplicates the retained payload.

The same bounded lab enables its existing concurrent opposite-direction
transfer. At most six cases run; the first assertion failure stops the batch.
Strict ICMP/integrity assertions and the log/cleanup bounds are unchanged.
This is a different outstanding load shape, not another unchanged single-flow
retry. Outcomes are pending and must not be inferred from the prior six passes.
