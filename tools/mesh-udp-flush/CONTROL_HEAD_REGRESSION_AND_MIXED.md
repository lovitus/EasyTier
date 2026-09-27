# TCP control-head correction: actual Core evidence and mixed-load boundary

## Frozen source and actual regression result

Parent: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
Unchanged-runtime input: `d0cf2e0b36b7831cfb81ea0000af9a3fd4c18894`.
Corrected candidate: `6dce45e745da2b796e10266423f9a8d0f9cb28fe`.
The two regression files have identical SHA-256:
`7ff93a89a573553065662bff1eabc9e6eca95180e2989c42e26bd611d1054ed6`.

[Baseline Test](https://github.com/lovitus/EasyTier/actions/runs/36292331111)
fails the actual Core regression on IPv4 ACK-first: three emissions instead
of two. The failure is a runtime assertion, not a compile/setup error.
[Candidate Test](https://github.com/lovitus/EasyTier/actions/runs/36292333849)
passes the identical regression in 0.021 seconds and all four unchanged TUN
byte/allocation, prepend/error, partial-write/no-replay and cancellation/drop
tests. The correction only selects the existing bounded scratch buffer for
payload-bearing ACK/PSH-ACK TCP. It does not discard control packets, add a pool,
enlarge queues or change packet bytes, routing, encryption or policy behavior.

This proves the regression correction, not an end-to-end performance gain.

## Full-suite failure is still open

The corrected full Test workflow is **FAIL**, not accepted. Its subnet-proxy
partition passes 255 of 256 cases. The failing case has native TUN, no public
relay, source KCP/QUIC enabled, destination KCP/QUIC disabled and both input
protocols enabled. It expires in `subnet_proxy_test_tcp` at
`easytier/src/tests/three_node.rs:1398`. Earlier subnet TCP/UDP exchanges succeed;
the final listener accepts but the connector does not print its post-connect
marker before the deadline. This identifies the observed phase, not the cause.

The unchanged-runtime run passes all 256 subnet cases; that same case takes
2.410 seconds. All three trees share test blob
`26f652b2c669a09f493e796669961c5458438ce3`. Unchanged test source and one baseline
pass do not prove flakiness or exclude a runtime regression. No assertion,
deadline or expectation was weakened and no failure was waived or retried.

The candidate non-three-node partitions report 1326 passing tests, including
one nextest LEAK annotation on the existing Leaf worker lifecycle test. Retain
that annotation; it does not establish Core memory growth or leak freedom.

## Same-package mixed-flow experiment

Optimized no-Leaf candidate build:
[36294320946](https://github.com/lovitus/EasyTier/actions/runs/36294320946),
existing `profiling-beta.yml` with `audit_comparator=true`. This mode does not
publish or move release tags. Reuse parent artifact `10911199419`; do not rebuild
the baseline. At this note's preparation, artifact/performance results are pending.

The existing `artifact_pair` JSON accepts optional `mixed_flow: true`. The
default remains the existing single-flow matrix. Both package/source/hash checks
remain mandatory. The resolved choice is saved in `provenance.json` and each
phase's run records. There are no workflow-definition or production-code edits.

The selected experiment reuses `lab.py` without modifying it:

- Fixed load: simultaneous opposite-direction flows, 100 Mbit/s cap and 64 MiB
  per flow; 200 Mbit/s is the aggregate configured cap, not measured goodput.
- Existing IPv4/IPv6 inner-family, Stealth and three interleaved sample matrix.
- Existing 120 timestamped ICMP replies must cover each complete fixed transfer,
  including its secondary flow, with at least 20 replies during load.
- Core/host CPU accounting already divides by the bytes from both completed
  flows. The secondary flow has its own completion/byte-count assertion.
- Saturation retains the existing 1 GiB per-flow transfer size and strict
  ICMP/integrity/cleanup assertions, now with opposite-direction bulk traffic.
  Its existing 20-probe window is not claimed as full-transfer coverage.
- The separate GSO activation trace remains single-flow and is excluded from
  performance comparisons. Compatibility/relay/profile-only inputs cannot be
  silently combined with the new mixed-flow choice.
- Any failure stops its phase and retains raw evidence. Neither a later pass
  nor this different workload erases the earlier saturation-loss result.

Both endpoints are disposable namespaces on one GitHub host. Record its CPU,
kernel, both binary identities, actual rates, CPU/GiB and RSS before interpreting
results. No physical-host, WAN, original-device or whole-project speedup claim
is supported by this setup. No merge, production deployment or release follows
from this research run. Full-suite and receive-ring acceptance remain open.

## First experiment stopped on a missing historical CLI artifact

[36295269376](https://github.com/lovitus/EasyTier/actions/runs/36295269376)
failed during preparation: GitHub returned HTTP 404 for CLI artifact
`10895437417`. Both Core-package validation loops had completed, but no traffic
case ran. This is an artifact-availability failure, not performance evidence.
The failed run remains failed; no Core was rebuilt and no integrity gate bypassed.

The replacement is the preserved integrated diagnostic artifact `10912354766`
from run `36263145990`, with complete ZIP SHA-256
`b6c7fd95e007339644d16a104bc9a70a1e5f449f86937cddaa0c02d3f46be165`.
Its root `easytier-cli` has independently measured SHA-256
`b462eb22b7a9d54b0eab6c69892c4af6c5f499d78a7ce492087963f2a92b27e0`.
Only that CLI is extracted for use; its diagnostic Core is not substituted for
either comparison endpoint. The integrated diagnostic source uses `3166ab67`;
the CLI source and `easytier/src/proto` are unchanged between the old base
`c6772dbf` and that integrated parent. The two endpoint packages, source SHAs,
new regression, traffic cases and assertions remain frozen. Results of the
corrected preparation and actual mixed-load execution are still pending.
