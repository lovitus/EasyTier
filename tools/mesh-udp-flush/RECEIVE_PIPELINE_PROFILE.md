# Exact receive-pipeline CPU diagnosis

Status: **SAMPLER SETUP FAILED; NO CORE SAMPLES**. This is a diagnostic
follow-up, not an optimization or release candidate.

## First execution, preserved failure

[Run 36288378865](https://github.com/lovitus/EasyTier/actions/runs/36288378865)
used harness `16b48f63a138129968d633563a15c51b02ac72d7`. Verified reuse
completed, but the new `perf` executable search returned an empty path.
The compound `test -n ... && test -x ...` guard did not terminate the script
before it tried to execute that empty string; setup exited 127.

This is an error in the newly added sampling harness, not a Core failure or
an environment-based excuse to bypass a gate. Core was not started, no
traffic case ran, and no CPU profile was produced. Neither the original
loss nor the consumer bottleneck is resolved by this run.

- Evidence artifact: `10921281411`, 99,027 bytes.
- Archive SHA-256: `9d994cb33c8d46b7f9c4191793c82f2c4c820015aa38021e1df738739e2cfea4`.
- ZIP CRC/member checks and all 20 manifest file hashes passed.
- Reused Core hash and Build ID match the intended diagnostic binary.
- Prior 39 UDP / four hole-punch results were preserved, not rerun.
- Core compilation, deployment and measurement counts are all zero.

The repository already contains a working package-file based locator in
`mesh-natural-cohort.yml`: install the HWE tools package, enumerate its
files with `dpkg-query`, then select the actual perf executable. That is the
proposed correction, together with separate mandatory nonempty/executable
checks. Do not rerun the unchanged failing harness or rebuild Core. The
correction has not been applied in this evidence update.

The locked producer-quantum model completed 72 trials but did not establish
an unconditional-yield fix. Its source and limitations are recorded in
`../mesh-recv-budget/PRODUCER_QUANTUM_RESULTS.md`. The next question is actual
Core service cost in the mixed-flow topology where ordinary Data admission
rejected an IPv6 echo, not whether a synthetic producer can be slowed down.

## Immutable payload and workload

- Core base: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
- Observation overlay/build harness: `2956e72cdb9df06c56183e02cb467508b27b30f2`.
- Retained binary artifact: `10920492391`, original run `36284039979`.
- Core SHA-256: `12d53e5d8ddbd89252c254aa37df2be7740cf98b6d6538180fe5a28b641b25f5`.
- Core Build ID: `75e3065b0f93801ded9ba9e834a42fd1980435a0`.
- Retained contract evidence: `10920377665`, including 39 UDP and four
  hole-punch tests. Reuse preserves that provenance; it does not claim new
  test execution or override the original functional failure.
- One hosted runner, two separately identified Core processes/namespaces,
  veth UDP/IPv4 underlay, IPv6 overlay, AES-GCM, Stealth off, GRO **off**.
- One mixed-flow case. Each direction uses the existing pair of opposing
  one-GiB TCP transfers and full-transfer ordinary ICMP observation. The lab
  retains byte-integrity, original probe-loss and bounded cleanup checks.

Use `mesh-udp-flush.yml` with `udp_gro_reuse=true`, `udp_gro_loss=true`,
`udp_gro_loss_profile=true`, `udp_gro_current=false`, and
`udp_gro_pressure=false`. Incompatible selections fail before building.
Only the existing verified artifact reuse branch runs: no Rust compiler,
Core rebuild, source overlay change, endpoint installation or production
setting change is requested. The normal loss matrix and performance modes
remain unchanged. The earlier exact-binary activation evidence is reused;
this GRO-off CPU diagnosis does not rerun a GRO-on activation matrix.

## Sampling and failure handling

The existing lab samples both Core PIDs with `perf`, CPU-clock at 99 Hz and
frame-pointer call graphs. The retained binary was built with frame pointers
and debug symbols. A userspace perf executable is selected explicitly rather
than relying on the distro wrapper matching the runner kernel.

Before applying unchanged functional assertions, the lab preserves its raw
profile and aggregate reports. The wrapper then emits per-endpoint self,
callgraph and stack reports, including after a functional failure. Endpoint
PID mapping, source hashes and sampling errors remain in the evidence. No
sampling exception may convert a failing functional run into PASS.

Potential failure modes are explicit: profiler unavailable/permission error,
missing or lost samples, unresolved symbols, incomplete stack unwinding,
probe loss, incomplete transfer, or unsafe cleanup. Preserve the first
failure. No automatic retry, assertion relaxation, queue enlargement or
probe-priority change is part of this run.

Interpret self samples separately from cumulative callers, and distinguish
receive/decrypt/filter/NIC work from sending and kernel processing. Do not
sum overlapping cumulative callgraph percentages. Mixed traffic means both
endpoints transmit and receive; a process name alone does not identify a
receive-stage sample. Bounded tracing and profiling alter timing, so this
run cannot establish throughput/CPU improvement or close the original
unprofiled saturation acceptance.

The deliverable is an endpoint-separated bottleneck assessment with symbol
and source evidence, followed by a narrowly justified optimization choice.
If the profile does not resolve the receive stage, record that limitation
rather than naming a bottleneck from source structure alone.
