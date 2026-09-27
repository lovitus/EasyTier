# Retained-package TUN write-cost follow-up

Status: syscall observation completed; no Core change or performance-gain claim.
The research-harness mixed-flow labeling defect is disclosed below.

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


## Completed observation: current Core already coalesces, but small writes remain

[Run 36297832614](https://github.com/lovitus/EasyTier/actions/runs/36297832614)
completed SUCCESS at harness `aad76ba58ea394d7dfd9be540b1bc9b9b5a561f7`.
The source/package and both endpoint roles match the scope above. No Core was
rebuilt or deployed. The candidate package is fetched by the shared preparation
contract but is not started in this observation.

The runner was AMD EPYC 7763, four logical CPUs, two physical cores with two
threads/core, Linux `6.8.0-1064-azure`. Both endpoints were namespace processes
on that same runner, not separate physical machines.

Evidence artifact `10924288053` is 4,350,061 bytes. Its complete ZIP SHA-256 was
independently verified:
`b0d71424da34c052286c84dc7725841a02454d83a4234311b76b7fceb4457640`.
The publisher has no per-file manifest; none is claimed. The endpoint Core
SHA-256 in provenance is
`46ab646adbac5eaea1c008f23b1b2340b25ac6ffb80018b1d5b41d6ec404fb86`.

### Trace audit

All four traces have stable before/after Core TIDs and TUN FDs. Their 667,158
events match the summed per-CPU entry counts. Entries/exits pair exactly by
TID, with no unmatched boundary event, overlapping syscall, trace overrun,
commit overrun or dropped event. Non-TUN writes are excluded by the actual
per-process descriptor mapping.

There are **321,452 successful TUN writes**, each returning its requested
length, with no short or failed TUN write. The table sums both endpoints and
both epochs within each inner family. Both endpoints carry data and reverse
traffic; neither is labeled an ACK-only endpoint.

| Inner family | TUN writes | Size <=128 bytes | Exactly 1,370 bytes | Size >1,370 bytes | Share of written bytes in >1,370-byte writes |
| --- | ---: | ---: | ---: | ---: | ---: |
| IPv4 | 160,742 | 45,057 (28.03%) | 90,084 (56.04%) | 21,596 (13.44%) | 54.38% |
| IPv6 | 160,710 | 44,058 (27.41%) | 91,160 (56.72%) | 21,516 (13.39%) | 52.65% |

The maximum is 7,910 bytes for IPv4 and 7,810 bytes for IPv6; no write exceeds
8,192 bytes. The four endpoint/epoch counts at that maximum are
`3734/3786/3818/3673` for IPv4 and `3749/3431/3710/3690` for IPv6.
These sizes are consistent with the six-segment limit for this fixture's TCP
headers and payloads. They are not proof of how many individual flushes hit
a capacity rejection: syscall records do not expose the pending cohort or
GRO rejection reason.

Across families, exactly-one-MTU writes are 56.38% of
calls, and <=128-byte writes are 27.72%. Larger writes
carry 53.51% of the recorded bytes. Thus
**GRO is already functioning**; neither a total GRO failure nor universal
single-packet flushing is supported. Short write size alone does not prove
TCP ACK flags, and increasing capacity cannot be assumed to merge them.

The trace starts immediately before the primary client command; the existing
lab starts the opposite client before tracer attachment. Complete pairing
describes the recorded interval, not guaranteed coverage of every byte of
both transfers. Instrumented rates/CPU are not performance measurements.

### Actual traffic and disclosed metadata defect

The observation phase explicitly passes `--mixed-flow`, but the outer
`provenance.json` and `runs.json` still reflect the default input field
`mixed_flow=false`. This is a new research-harness labeling bug. Raw records
are not rewritten and this report does not treat that field as authoritative.
The production Core is unaffected. The labeling repair is not silently included
in this evidence-only update.

Independent child `results.jsonl` records contain, for each family, two
successful primary transfers with `mixed_flow=true` and two successful
opposite-direction transfers, all exactly 67,108,864 bytes. Their directions
cover upload and download in each role. Total bulk delivery is 512 MiB,
not a single-flow observation relabeled as mixed.

Four integrity/half-close checks, 60 UDP echo datagrams, and all four complete
120/120 ICMP windows pass. The 480 replies include 212 during the measured
load windows. All four Core exits are zero, four namespaces are removed,
all 30 cleanup records are clean, and root routes are unchanged. Maximum
individual Core log size is 3,988 bytes. No claim about long-duration leaks
or saturated control delivery follows.

## Next bounded question, not an implementation decision

The repeated full-MTU writes and observed six-segment-sized emissions justify
comparing **one** reusable 8/16/32 KiB head in the existing locked-library tool.
Keep packet bytes/order, ready cohorts, scratch count and all queue budgets
unchanged. Cover ordinary data, mixed flows and nonmergeable control/short
packets, preserving byte/multiplicity and allocation ownership checks.

This is separate from changing the rejected control-head selector experiment.
No new pool, per-flow buffer, wait-to-batch policy, bigger receive ring or
production default is proposed. A larger head would add 8 or 24 KiB per TUN
sink relative to the current head, not per connection; whether that trade-off
is worthwhile remains unmeasured.

The small tool can establish emission/copy/ownership behavior, not Core
throughput or saturated-loss repair. Only a subsequent same-source actual-Core
comparison could support a performance claim. Existing saturation failures
and the separate Test subnet timeout remain open; this observation changes
neither their status nor any acceptance assertion.

### Small-tool comparison prepared

The existing locked-library contract tool now includes 84 capacity/pattern
observations: both IP families, capacities 8/16/32 KiB, data cohorts of
0/1/2/4/8/16/32/64 packets, two-flow interleaving, leading/trailing ACK and a
short PSH tail. It deliberately retains the parent head selector, isolating
capacity from the separate eligibility correction. A leading ACK is a negative
case where capacity alone must not help.

Each arm reconstructs the same fixture bytes/order, runs the real locked
`handle_gro` and `gso_split`, checks every byte and multiplicity, and retains
the exact single scratch allocation. Added per-flow order checks use this
fixture's fixed address pair and distinct source ports; they are not a general
packet classifier. Existing reverse/prepend fixtures retain their existing
byte/allocation checks rather than acquiring an inappropriate global-order
assertion. Existing 8 KiB checks still require exactly 8 KiB on reclamation.

These are controlled 1,320-byte-payload fixture packets, not a captured-packet
replay, kernel write measurement, CPU benchmark or actual-Core regression.
The prepared run uses the existing capacity-only workflow and compiles only
the small Rust tool. No dependency, lockfile, production constant or queue
changes. Its execution and observed counts are pending at this checkpoint.

### Small-tool run failed before execution

[Run 36298397195](https://github.com/lovitus/EasyTier/actions/runs/36298397195)
at `af8c234e7e3585f9fb29c87c8f91836ffa0c2890` failed with Cargo exit 101.
The retained `build.log` reports `E0689` at
`src/tun_head_contract.rs:558`: `count.saturating_sub(segments)` has an
ambiguous integer receiver because the new cohort table lacks an explicit
`usize` type. This is an error introduced in the research tool, not a Core
failure or an infrastructure diagnosis. Formatting/static checks passed but
did not typecheck this standalone tool.

`results.jsonl` is empty. None of the 84 new observations or the old contracts
executed in this run, so no capacity result, behavioral red/green, or CPU gain
is claimed. The prior exact-package TUN observation and earlier contract
results are separate and remain unchanged.

Evidence artifact `10924157690` is 5,558 bytes. Its complete ZIP SHA-256 was
verified: `100157f9893b1c585fd5c2bd1efb961e355353d6ce8e6ce94d15802f7965c9fc`.
The original compiler error, empty result and source SHA are retained.

The proposed repair is only an explicit `usize` cohort-table type, with every
assertion unchanged. The outer mixed-flow labeling defect is also disclosed
above. Both were reported to the maintainer for the requested decision; no
silent code repair or replacement run has been made at this checkpoint.

## Authorized research-tool repair (2026-09-27)

The maintainer requested continued repair and research after the failed run was
reported. The capacity table now explicitly infers `usize`; no case, expected
value, assertion, buffer size, or production Core code was changed.

The package harness now names the input flag `requested_mixed_flow` in provenance
and records the effective phase-specific value in each `runs.json` row. The
forced opposing-flow TUN observation is therefore recorded as `mixed_flow: true`.
Reading older provenance remains supported. The original artifacts and their
incorrect outer labels remain unchanged; their independently recorded child
transfers are the evidence for the already completed bidirectional observation.

Only the existing locked small-tool capacity workflow is to be repeated. No Core
rebuild or repeated package trace is needed for these repairs. At this checkpoint
the repaired tool has not run; the prior compile failure remains a failure, and
no capacity or CPU result is claimed.

## Terminal small-tool result and bounded next decision (2026-09-27)

Run [36299280759](https://github.com/lovitus/EasyTier/actions/runs/36299280759)
succeeded at `64aa2c1d6408193daaf2ae2e0dfa19fe08bc3090`.
This supersedes the pending repaired-tool checkpoint above, not the earlier
failed run. The only compiler correction was the explicit `usize` table
annotation. No expected value or assertion changed.

Artifact `10925087847` is 7,168 bytes; its complete downloaded archive SHA-256 is
`ec2363923fc7dbceb2f0926dcf124f73f688090b73312ea9ae3e6dc66fc493c4`.
The artifact source record agrees with the workflow SHA. The executable emitted
222 JSON observations, including all 84 new capacity comparisons, 92 existing
`tun-head` rows, 24 existing selection rows and 22 earlier observations.
These are observations from one tool invocation, not 222 independent CI tests.

All 84 capacity cases completed their byte round-trip, per-flow order,
single-buffer allocation identity, retained capacity and empty-buffer assertions.
The unchanged baseline and larger capacities were exercised within the same
executable and invocation. The preceding compiler failure is NOT a functional
red/green regression result.

### Library emissions, not measured kernel syscalls or CPU

IPv4 and IPv6 have the same emission counts in this fixture. Each ordinary
payload is 1,320 bytes; this is generated input, not packet replay.

| Pattern | Input packets | 8 KiB | 16 KiB | 32 KiB |
| --- | ---: | ---: | ---: | ---: |
| data | 8 | 3 | 1 | 1 |
| data | 16 | 11 | 5 | 1 |
| data | 32 | 27 | 21 | 9 |
| data | 64 | 59 | 53 | 41 |
| two-flows | 8 | 5 | 5 | 5 |
| two-flows | 16 | 11 | 9 | 9 |
| two-flows | 32 | 27 | 21 | 17 |
| leading-ack | 33 | 33 | 33 | 33 |

The single promoted head explains the shape of these results: the 8 KiB head
holds six full fixture segments, the 16 KiB head twelve, and the 32 KiB head
twenty-four. Once that head fills, other original packet allocations still lack
spare capacity. Even 32 KiB therefore emits 41 buffers for 64 same-flow packets;
it is not whole-batch coalescing. Interleaved unrelated flows and a leading pure
ACK limit the benefit further.

The earlier retained-package trace remains the real-device evidence:
321,452 successful TUN writes, no short/failed writes, and 53.51% of written bytes
already in larger-than-one-MTU writes. Its size histogram does not identify
individual ACK packets, capacity-hit events, or natural per-flush cohorts.
Consequently this table cannot convert that histogram into a CPU-saving claim.

### Decision

Do not change production buffer capacity or merge the separate ACK-head selector
from this mechanism result. The selector already has a no-measurable-gain actual
Core comparison; increasing capacity does not close the saturation loss gate.

If the capacity hypothesis is pursued, the narrow experiment is a single
existing-runner Core experiment with 8/16/32 KiB arms and the retained parent
selector, not a pool or per-flow allocation redesign. Each larger arm would add
only 8/24 KiB to that sink's one reusable head, not an enlarged receive ring.
It must record actual promoted/full heads, merge lengths and emitted TUN frames;
use untraced, rate-matched opposing flows for CPU/GiB; and retain ICMP, data
integrity, shutdown, RSS and error gates. Tracing and performance samples must
remain separate. No new production option, dependency or platform promise is
justified yet.

The other priority remains the previously observed receive-ring rejection under
saturation. Neither a larger head nor a producer yield is claimed as its cure.
No head-capacity Core build, new deployment or performance acceptance was done
in this repair batch.

## Retained-parent actual-Core capacity experiment (prepared, not yet run)

The next lane is `mesh-udp-flush.yml: tun_capacity_current`. It builds one
runner-only diagnostic Core based on `3166ab67`, not the obsolete `c6772dbf`
TUN template. The only generated Core diff is `linux_tun_offload.rs`: head
allocation/eligibility use 8/16/32 KiB, with the parent selector, stored flush
future, exact allocation reclaim, error result and no-replay semantics retained.
No receive ring, packet class, routing, encryption, dependency or retry changes.

The existing lab explicitly passes the isolated capacity marker through its
ET-environment allowlist. The existing historical 0/4/8 KiB lanes remain intact.
The diagnostic binary defaults to the retained 8 KiB behavior without that
marker. This is not a production configuration option or a deployment artifact.

The fixed lane covers IPv4 and IPv6 inner packets over IPv4 UDP, opposing flows
at 250 and 500 Mbit/s per direction, three interleaved samples per capacity, and
256 MiB per flow per transfer epoch. That is 36 cases, not WAN throughput.
The six separate syscall cases use 64 MiB flows capped at 100 Mbit/s each;
their CPU and rates cannot enter the performance table. The saturation lane is
bounded and stops at the first unchanged functional/progress failure. All lanes
retain the lab's full-transfer ICMP option, TCP integrity/half-close, UDP echo,
resource, process, namespace, route and log-size checks.

Counters are present in every arm and perform no per-packet I/O. They report
cohort sizes and the recovered head's length/growth, not invented emission counts
or a claim that a specific packet was rejected for lack of capacity. Real TUN
emissions come from the separate existing syscall tracer. Any CPU results will
be same-instrumented-binary comparisons, not a comparison against the previous
musl release package. Head memory increases are 8/24 KiB per sink, not per flow;
observation histogram memory is separately research-only.

Failure checklist before execution:

- Refuse a changed base, modified source anchor, extra generated Core file or lock change.
- Compile and run existing TUN ownership/round-trip regressions; no new weakened tests.
- Missing metrics, no growing head, wrong capacity, lost scratch or failed flush fails observation.
- Trace loss, short writes and actual byte/order evidence require artifact audit; a green job alone is insufficient.
- Compare observed aggregate rates before interpreting CPU/GiB; unmatched load is not a gain.
- Preserve any saturation loss; neither the capacity tool nor fixed-load success closes it.
- Keep the existing bounded logs/processes and scoped cleanup; never deploy this overlay to a user host.

The dependency stays pinned to `tun-rs 2.8.7`. Current upstream 2.8.11 still
exposes `GROTable` internals privately and documents `send_multiple`'s return as
bytes, not an emission count; no dependency fork is introduced for observation.
References: [GROTable](https://docs.rs/tun-rs/latest/tun_rs/struct.GROTable.html),
[send_multiple](https://docs.rs/tun-rs/latest/tun_rs/struct.DeviceImpl.html#method.send_multiple).
