# Pure mesh performance research closeout

Status (2026-10-02): implementation candidates retained; acceptance incomplete.
This is the current disposition index, not a release approval. Historical
reports and failed runs remain evidence and are not rewritten as successful.
Mihomo and Leaf performance are outside this research.

## Candidate disposition

| Candidate | Exact source | Disposition |
| --- | --- | --- |
| Accepted integration baseline | `c6772dbfef2395ff96b39bd4801945d92212dffb` | Unchanged production baseline |
| PR #9, private Linux UDP GSO | `3a1f3d9fc840a37a7649448a042bf44035bfaf7b` | Frozen candidate; full-window saturation acceptance remains open |
| PR #10, bounded TUN GRO head | `6e90dbf102e5c93d56c531b87eea1858c4a8e61e` | Frozen candidate stacked on #9; full-window incremental saturation acceptance remains open |
| PR #11, relay destination guard | `3166ab672d347cdcc5a6768bc77056cd8ec38323` | Validated stacked correctness fix; standalone delivery being prepared from the integration baseline |
| Control-head selection | `6dce45e745da2b796e10266423f9a8d0f9cb28fe` | Deferred: no measurable fixed-load gain; full Test and saturation not accepted |
| 16/32 KiB capacity experiment | Harness `25115ab08252f3dbcf4744d89f648695be4e0b58` | Deferred with EPERM and zero-traffic evidence; no Core performance gain claimed |
| PR #7 / older GSO experiments | `d396140d0b21cb05670f4f7be2e52abc65df4e01` | Research evidence, not a production merge target; #9 is the bounded implementation candidate |
| PR #5 and fixture PR #6 | `a658039a45fff794d4312e3dba5fcd6aa19d4fb7` / `100a10d8fb1e23e59f99786c2f9cec6a06677c44` | Historical diagnosis; #6 is not in #5 history; neither is a release candidate |

Devin reviewed the closeout direction as an advisor. That discussion is not a
formal GitHub approval and does not substitute for exact-source test evidence.
No production receive-ring, scheduling, queue-size or ICMP-priority change is
justified by this closeout.

## Retained results and limitations

- #9: Test 36219724313; comparator build 36221363359; same-run comparison
  36223095097. Fixed-load CPU seconds/GiB fell 25.7-28.6%; saturation goodput
  rose 23.7-26.1% in the recorded hosted namespace lab.
- #10: Test 36231652201 passed the four actual TUN/GRO regressions and 1772
  partition tests; previously disclosed PASS/LEAK and SLOW annotations remain.
  Comparator build 36233922318 and comparison 36234743171 measured an
  additional 14.46-18.75% fixed-load CPU/GiB reduction. Paired RSS ranges
  overlapped; this is not a demonstrated memory reduction.
- Combined comparison 36260893872 measured 34.48-38.60% lower fixed-load
  CPU/GiB, not a sum of percentages from different runs. It failed an IPv6
  saturation ICMP check (19/20 replies). The failure is not waived; the cause
  of that particular missing packet remains unproven.
- #11: runtime red 36240736489; corrected Test 36256718846; build
  36258711567; bounded packaged relay 36259577427. The latter used the same
  fixed artifact for both labels, not an A/B performance comparison.
- Control-head candidate: 36295644707 showed no material fixed-load CPU gain,
  then failed in the retained parent saturation arm before candidate saturation.
  The candidate full Test also had an unattributed subnet TCP timeout.
- Capacity experiment 36300227079 compiled Core but failed two unchanged
  real-device tests at unshare(CLONE_NEWNET), before their bodies, because the
  experiment launcher omitted privileges. No traffic ran. This does not
  invalidate #10's earlier successful real-device tests. Repairing that unused
  launcher is deferred with the capacity experiment, not a release prerequisite.

Comparisons identify both endpoints as separate EasyTier Core processes in
isolated Linux namespaces on the same GitHub-hosted runner, linked by controlled
veth paths. These are not separate physical hosts, WAN, Windows, macOS, or
original-device acceptance. Exact CPU models, IP families, transport, sample
counts, resource baselines and hashes remain in the linked per-run reports.

## Minimal acceptance recovery

The only lab behavior change is passing the existing `--full-transfer-control`
to saturation. All packet, sequence, time-coverage, cleanup and log-bound
assertions remain unchanged. The new `artifact_saturation_only` workflow input
skips already completed fixed-load and syscall-trace phases. It compiles only
the existing load generator, not EasyTier Core.

Use paired frozen packages for baseline versus #9, then #9 versus #9+#10. Each
pair must execute on the same runner; do not compare absolute rates across
different runner CPUs. Do not expand the platform or WAN matrix. Stop at a
real failure and preserve it; no queue tuning or exception for ping is allowed
merely to obtain a green result.

The existing full-transfer mode requires all 120 replies, first/last timestamps
spanning the transfer and at least 20 in-load replies. If it has zero loss but
insufficient temporal coverage, report a coverage limitation, not a Core
regression or PASS. Dynamic probe lifetime is not part of this batch.

## Current artifact availability blocker

On 2026-10-02 the GitHub artifact metadata requests for baseline `10899308750`,
#9 `10898758495`, #10 `10903497520` and shared CLI `10912354766` returned HTTP
404, while the repository artifact-list endpoint was accessible. This proves
those IDs are unavailable to this request, not why they became unavailable.
Do not label this as a code failure or silently rebuild the candidate set.
Some private archives remain; recovery and rebuild permission are unresolved.
No new saturation result is claimed until the exact binaries are available.

The standalone relay candidate needs its own green Test and bounded packaged
relay result. Old red evidence is reusable only after the actual pre-fix peer
source, session implementation and dependency equivalence are checked. The
stacked artifact is not a substitute for the new standalone artifact.

## Immutable report entry points

- [UDP GSO](https://github.com/lovitus/EasyTier/blob/21195e9b/tools/mesh-udp-flush/PACKAGED.md)
- [TUN head](https://github.com/lovitus/EasyTier/blob/39fb29ecc7b405a89851f1df2d4eb983da83b2c8/tools/mesh-udp-flush/TUN_HEAD_PACKAGED.md)
- [Relay guard](https://github.com/lovitus/EasyTier/blob/dc28066955ec7a71a95f1120a379ffb98fc400a2/tools/mesh-udp-flush/RELAY_GUARD_PACKAGED.md)
- [Combined failure and bounded gains](https://github.com/lovitus/EasyTier/blob/7c10eccb52febbfb1bd75c46afeb2929a8d74e19/tools/mesh-udp-flush/COMBINED_PACKAGED.md)

Historical reports are immutable evidence. This index and issue/PR status
describe the current decision. Documentation-only evidence updates do not
invalidate a previously built production SHA. Neither research closure nor
advisory agreement authorizes merge or release.
