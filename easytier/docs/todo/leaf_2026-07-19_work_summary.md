# Leaf performance work summary - 2026-07-19

This document is the durable ledger for the implementation, rollback, restoration,
and multi-host validation performed on 2026-07-19. It complements the detailed
experiment reports and the live parallel workboard; it does not replace their raw
measurements.

## Exact scope

- Comparison baseline: `0cf368072aad4882309e6f6d450e45f5f4e1a9ac`.
- Final candidate: `f6617c5136672016951adb0f79ab0daec7ba7112`.
- Candidate branch: `codex/profiling-beta`.
- Authoritative Linux workflow: run `29688959030`, successful at the exact final SHA.
- Authoritative Android workflow: run `29688959035`, successful at the exact final SHA.
- Locked Leaf baseline inspected at exact SHA:
  `36ba707f6d107886bf3fe22dbd4f2cd9f9be2afb`.
- Final locked Leaf fork SHA:
  `a5bb6a31df2c62200be052b61ca01b01ea5e3c25`.

## Work performed

### PacketBatch experiment: implemented, measured, and reverted

The PacketBatch path was developed in four commits:

- `aae707ca`: initial feature.
- `61e9852f`: Tokio feature correction.
- `39dd4d2f`: coalesced reads.
- `ca5751fb`: reusable buffers.

It was then reverted by `0826e68a`, `8c5e6aee`, `4dcc8d45`, and `0b557b8a`.
The five-host re-audit of the exact artifacts confirmed that the rollback was
correct: the framed stream batching design did not provide a reliable general
performance gain and carried Android initialization/lifecycle risk. This result
rejects that implementation, not every possible in-process packet endpoint.

Detailed failure record:
[FAILED_leaf_external_packet_endpoint_performance.md](../failed_attempts/FAILED_leaf_external_packet_endpoint_performance.md).

### Leaf-owned Linux policy TUN: developed, narrowed, and restored

The Linux path was developed in these commits:

- `fe9b68bc`: initial fast path.
- `02f65d0`: in-process candidate work.
- `87301ee0`: readiness and generic fallback work.
- `8b48153`: GSO-pinned candidate.

Four safe reverts (`95f3279`, `fcf7270`, `872e944`, `2cd94a`) temporarily returned
the tree to the pre-experiment behavior while evidence was reconciled. The broader
host matrix then showed that the earlier rejection was based on a host-specific
negative result. `f6617c51` restored the validated forward path.

The final fallback order is:

1. Leaf fast GSO.
2. Leaf fast generic TUN.
3. Existing EasyTier legacy bridge.

GSO initialization failure therefore does not skip the generic fast path. Failure
to initialize either Leaf-owned mode leaves the existing legacy implementation as
the compatibility fallback.

Detailed design and cross-kernel evidence:
[leaf_linux_owned_policy_tun_cross_kernel_validation.md](leaf_linux_owned_policy_tun_cross_kernel_validation.md).

### Multi-host evidence reconciliation

The exact `87301ee0` dual-TUN artifact and the exact `39dd4d2f` PacketBatch artifact
were rechecked across the available high-performance, old-kernel, internal, and KR
hosts. The resulting evidence established two separate conclusions:

- Generic/GSO Leaf-owned Linux TUN is a useful, bounded optimization; the negative
  result on `192.168.2.160` must not be generalized to every host.
- The PacketBatch implementation remains a failed experiment even after removing
  that single-host bias.

All host-specific medians, kernel boundaries, stop conditions, and cleanup notes
remain in the two detailed reports above and in
[leaf_parallel_workboard.md](leaf_parallel_workboard.md).

## Final net change relative to `0cf36807`

The main repository has 24 changed paths grouped as follows:

- Dependency pin and lock data: approximately `+3/-2` lines.
- Documentation: approximately `+415` committed lines, plus the local audit
  additions and this summary.
- Frontend/configuration integration: approximately `+13` lines.
- Runtime/backend integration: approximately `+579/-54` lines.
- Validation scripts and workflow support: approximately `+614/-4` lines.

The locked Leaf fork changes four files and approximately `+294/-38` lines:

- `leaf/Cargo.toml`.
- `leaf/src/proxy/tun/inbound.rs`.
- `leaf/src/proxy/tun/linux_tun_offload.rs`.
- `leaf/src/proxy/tun/mod.rs`.

The existing EasyTier mesh TUN implementation, including
`easytier/src/instance/linux_tun_offload.rs`, was not replaced by this work.
Mesh routing, HEV behavior, DNS/rule semantics, and the proxy-protocol plugins are
not part of the final performance change.

## Final architecture boundary

- The optimization is Linux-specific and feature-gated.
- Leaf owns only its policy TUN fast path; EasyTier retains mesh data-plane
  ownership.
- Unsupported platforms and failed fast-path initialization preserve the legacy
  path.
- GSO is an optional first choice, not a prerequisite for generic fast TUN.
- No PacketBatch code remains in the final candidate.
- Android remains on its existing single active `VpnService` TUN architecture;
  possible Android equivalents are recorded separately and are not silently
  implied by the Linux implementation.

## Documentation state

### Policy editor consistency pass

The Android/desktop visual policy editor was reconciled with the validated v1
policy schema after the performance candidate was frozen:

- removed the invalid HTTP outbound choice;
- made UDP labels protocol-specific and exposed the three Shadowsocks UDP modes;
- added the missing IPv4/IPv6 FakeIP range controls;
- added examples to free-form fields and localized previously hard-coded labels;
- made node, group, and rule cards compact by default with explicit edit expansion;
- exposed `FINAL`, constrained `NETWORK` to TCP/UDP, and aligned conditional
  `EXTERNAL` no-resolve support with backend validation;
- aligned omitted-DNS backend defaults with the published visual template.
- added runtime GeoSite/GeoIP category indexes keyed by file size and the
  configured/index SHA-256 identity; temporary bundled snapshots and future
  downloaded snapshots use the same sidecar format, so updating rule data
  refreshes the editor without rebuilding the frontend while preserving editable
  input for custom rule data.

This is a local build-affecting follow-up and requires the frontend plus focused
policy parser preflight before it can join a candidate.

The implementation, failed experiments, reverts, restored candidate, cross-host
measurements, exact workflow identities, and remaining platform decision are now
all represented by durable repository documents. These documentation-only updates
remain local until they accompany a future build-affecting candidate or an explicit
documentation publication request.
