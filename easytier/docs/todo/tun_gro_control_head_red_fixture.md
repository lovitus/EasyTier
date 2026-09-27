# Unchanged-runtime regression input: TUN control head

This branch is an immutable experimental test input, not an implementation or
release candidate. Runtime source remains at
`3166ab672d347cdcc5a6768bc77056cd8ec38323`.

Only the actual Core regression fixture and this note are added. The test file
is byte-identical to the corrected candidate's test file. The existing Test
workflow is used unchanged; no assertion is weakened, no failure is suppressed,
and no production deployment is authorized by this branch.

The expected negative control is a runtime assertion in
`gro_head_skips_control_packets_without_changing_packet_bytes`: a leading pure
ACK consumes the parent's sole roomy GRO head, so the real library emits three
packets instead of the asserted two. Compilation failures, setup failures,
missing symbols and unrelated test failures do not establish that result.

The corresponding corrected candidate must pass this same test, retain all
packet bytes and allocation bounds, and pass the existing TUN error/lifecycle
regressions. An optimized-artifact comparison remains a separate acceptance
requirement; no throughput or CPU improvement is claimed by a unit-test result.
