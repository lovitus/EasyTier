# Retained-package TUN write-cost follow-up

Status: prepared observation; no new runtime result or Core change yet.

The control-head eligibility correction has no measured fixed-load CPU benefit
in run 36295644707. It must not be expanded merely to obtain a positive result.
The remaining question here is narrower: how often does the retained current
parent actually write small versus coalesced packets to its TUN?

## Why this observation, not another production patch

The earlier actual-Core capacity experiment (run 36082689922, historical
diagnostic base `c6772dbf`) recorded an average cohort of 13.26 packets with
one 8 KiB scratch head and a largest resulting frame of 8,022 bytes. The locked
library contract with 1,320-byte TCP payloads fits six segments in that head.
Those are historical measurements, not a current traffic distribution or proof
that a larger allocation improves current Core.

The current mixed-flow profile (run 36289395042, source base `3166ab67`) puts
TUN-write in 19.40% of sampled stacks. That includes kernel TCP/IP receive work;
it is not an additive self-cost fraction or a predicted removable percentage.
The older 64 KiB scratch regression, negative receive-quantum/recv-many results,
and current saturation failures remain relevant and are not discarded.

## Scope and reused implementation

Use the existing `TunWriteTrace` and namespace lab with the immutable parent
package `3166ab672d347cdcc5a6768bc77056cd8ec38323`, artifact `10911199419`.
Both endpoints run those exact bytes; no diagnostic overlay or Core rebuild.
The existing package provenance and CLI digest checks remain mandatory.

Only the existing research workflow gains an explicit observation mode.
It does not run the full comparison matrix or install a Rust toolchain for
this mode. No production API, packet allocation, queue capacity, admission,
sleep, scheduler, route, security or protocol implementation is modified.

Two endpoint namespaces on one hosted Linux runner use UDP/IPv4 underlay,
AES-GCM, Stealth off, and separate inner IPv4/IPv6 cases. Each case has two
epochs of simultaneous opposing 64 MiB transfers, capped at 100 Mbit/s per
flow. This is a paced syscall-distribution observation, not saturation,
physical-host, WAN, Stealth, or all-platform acceptance.

The existing tracer selects the actual Core TIDs and TUN FDs. It records
write entry/exit, before/after identities and per-CPU trace loss counters.
Its private trace instance is removed by the existing lab cleanup. The
workflow unmounts tracefs only if this step mounted it. Tracing never targets
a production machine or enables the root tracing instance.

## Failure conditions and required interpretation

- Keep all existing digest/half-close, UDP echo, full-transfer ICMP, bounded
  process, root-route and namespace cleanup assertions unchanged.
- Changed TID/FD identity, missing trace metadata, unmatched TUN write pairs,
  short/error writes, or any trace overrun prevent a complete-count claim.
- Retain requested and returned write bytes per endpoint/direction, successful
  call counts, write-size histograms and the share above one MTU. Counts from
  ACK-only and data-carrying endpoints must not be silently pooled as data.
- A syscall size alone does not identify its TCP flags, exact flush cohort,
  or the reason other frames did not merge. Do not infer a capacity-hit count
  or a counterfactual speedup solely from writes near 8 KiB.
- Rates and CPU collected while tracing are excluded from performance
  comparisons. No inference of kernel costs shifted outside process accounting.
- If counts do not support frequent small-write overhead, stop this direction.
  If they do, first propose one bounded small-tool experiment; do not directly
  restore the old large scratch, add a pool or change production constants.

This is an evidence discriminator, not an additional release gate. Previous
failures stay FAIL; none is waived or retried by this observation.
