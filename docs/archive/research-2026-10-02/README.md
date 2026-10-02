# Research archive: upstream integration and pure mesh performance

Status: **ARCHIVED / RESEARCH STOPPED**, by maintainer decision on 2026-10-02.

This is the final disposition, not another acceptance plan. The maintainer
explicitly ended exploratory work in preparation for project archival. There
is no pending request to rebuild, rerun, diagnose, implement or release these
experiments. Reopening any of them requires a new maintainer decision.

## Production boundary

- The production code, dependencies, tests, workflows and version remain at
  `c6772dbfef2395ff96b39bd4801945d92212dffb`. This closeout adds documentation only.
- No research PR is merged, no failed gate is waived, and no release is created.
- Measured local-lab gains are retained as evidence, not relabeled as a completed
  cross-platform performance fix. Unresolved failures remain unresolved.
- Mihomo and Leaf performance are outside the final mesh research scope. Earlier
  stack-replacement proposals are historical, not instructions to resume them.
- Devin's role was advisory. Discussion closure is not formal PR approval,
  implementation acceptance or permission to change code.
- The GitHub repository itself is not switched to read-only by this change.

## Final disposition

| Work | Preserved source | Closure reason and limits |
| --- | --- | --- |
| Upstream research #1 and proposal PR #3 | `2f98eec7f8bef6c4cdb711a1431f23e98965893e` plus the previously untracked research document below | Research ended; proposed picks and unchecked prerequisites are not implemented or accepted by closing the issue. |
| Secure relay #2, B02 activity-aware session GC | Proposal only | Not implemented or runtime-validated in the candidates below. Closed as not planned, not as fixed. |
| Mesh diagnosis #4 and PR #5 | `a658039a45fff794d4312e3dba5fcd6aa19d4fb7` | Historical diagnosis and experimental tooling; not a production merge bundle. |
| Fixture PR #6 | `100a10d8fb1e23e59f99786c2f9cec6a06677c44` | Fixture repair retained separately; it is not in PR #5 history. No claim that PR #5's original result became valid. |
| Natural-cohort PR #7 | `d396140d0b21cb05670f4f7be2e52abc65df4e01` | Experimental GSO/fallback evidence retained; not merged. |
| GSO issue #8 and PR #9 | `3a1f3d9fc840a37a7649448a042bf44035bfaf7b` | Bounded Linux implementation has measured gains; full-window saturation acceptance is incomplete. Archived, not accepted. |
| TUN head PR #10 | `6e90dbf102e5c93d56c531b87eea1858c4a8e61e` | Stacked on #9; measured incremental gains and regression evidence retained. Complete saturation acceptance is missing. |
| Stacked relay guard PR #11, B01 | `3166ab672d347cdcc5a6768bc77056cd8ec38323` | Runtime red/green and bounded packaged relay passed at this stacked SHA. Not a standalone or full performance acceptance claim. |
| Standalone relay guard PR #12 | `84992aaf312290932d239c7aa0b501af59a2367a` | Comparator build passed; exact-SHA formal Test failed. Standalone packaged relay was not run. Closed without merge. |
| Control-head selection | `6dce45e745da2b796e10266423f9a8d0f9cb28fe` | No material fixed-load gain; formal Test failure and incomplete saturation evidence remain. Experiment abandoned. |
| Larger 16/32 KiB capacity | Harness `25115ab08252f3dbcf4744d89f648695be4e0b58` | Launcher failed before traffic. No Core performance conclusion; launcher repair and further trials are cancelled. |
| Last acceptance-harness edit | `5c8d8b729403ee8bbf1145ed4a5c57dac029bc02` | Existing full-transfer assertions enabled for saturation; local static checks passed, hosted E2E not run after the change. Archived without claiming the gap is closed. |
| Advisor metrics proposal | `c12b73e1ae5fc052edea5ceffa8b87088435c150` | Unmerged branch preserved, not re-reviewed or accepted by this archival batch. |
| Advisor bounded RPC queues proposal | `82c66acc233e0d5047c083b64f07c9155e5aac6b` | Unmerged branch preserved, not re-reviewed or accepted by this archival batch. |

The issue and PR numbers refer to [lovitus/EasyTier](https://github.com/lovitus/EasyTier).
Issues #1, #2, #4 and #8 are closed as **not planned**. PRs #3, #5, #6, #7, #9,
#10, #11 and #12 are closed **without merge** and labeled in their titles as
archived. Their original discussion and evidence remain available.

## Retained measurements and failures

These results were collected before archival. No new experiment was run for
this document. Endpoints in the controlled comparisons were separate EasyTier
Core processes in Linux network namespaces on one GitHub-hosted runner, joined
by controlled veth paths, not independent physical hosts or a WAN.

| Evidence | Result retained | Boundary |
| --- | --- | --- |
| #9 Test `36219724313`, build `36221363359`, comparison `36223095097`, compatibility `36224742826` | Fixed-load CPU seconds/GiB decreased 25.7-28.6%; saturation goodput increased 23.7-26.1%; 24 compatibility cases passed. | Same-run hosted Linux results; not complete full-transfer saturation, physical-device or cross-platform acceptance. |
| #10 Test `36231652201`, build `36233922318`, comparison `36234743171`, compatibility `36235807927` | Four actual TUN/GRO regressions and 1772 partition tests passed; additional fixed-load CPU/GiB decrease 14.46-18.75%; saturation goodput increase 8.58-12.23%; 24 compatibility cases passed. | TUN devices were in hosted namespaces, not a physical-device test. Previously disclosed PASS/LEAK and SLOW annotations remain. RSS ranges overlap; no established memory reduction. |
| Combined comparison `36260893872` | Same-run fixed-load CPU/GiB decrease 34.48-38.60%. IPv6 saturation failed with 19/20 ICMP replies despite completed bulk transfer. | Percentages are not added across runs. The missing reply is not waived and its cause was not proved. |
| Relay red `36240736489`, corrected Test `36256718846`, build `36258711567`, packaged relay `36259577427` | Actual regression failed before the fix and passed after it. Packaged matrix passed 24 cases, 48 bulk digest checks and 720 UDP echoes. 72 Core logs totaled 288,248 bytes, largest 7,050; no already-encrypted warning. | Both matrix labels used the same fixed artifact. Not A/B throughput or mixed-version evidence. The earlier large-log failure remains recorded. |
| Control-head comparison `36295644707` | No material fixed-load CPU benefit; parent saturation lost sequence 16 before candidate saturation ran. | An unrun candidate is not a passing candidate. Full Test `36292333849` also failed with an unattributed subnet TCP timeout. |
| Capacity experiment `36300227079` | Core and tests compiled; two pure tests passed; two namespace tests failed at `unshare(CLONE_NEWNET)` with EPERM before their bodies. | Zero traffic. Launcher privilege failure is not a Core regression and does not invalidate the earlier hosted TUN regression results. |
| Standalone build `36901628820`, Test `36901622178` | Build passed on `84992aaf`; the subnet-proxy partition had 255/256 combinations pass and one TCP deadline failure. | Exact-SHA Test is FAIL. No automatic retry, assertion change, timeout increase, standalone packaged relay result, merge or release. |

The final standalone failure involved no-TUN/public relay, source KCP enabled
and QUIC disabled, destination KCP input disabled and QUIC input allowed. The
TCP helper failed at `easytier/src/tests/three_node.rs:1398`. Existing stdout
completed TCP and two UDP groups for the first two targets, but no
accept/connect/echo marker appeared for the destination's own virtual-IP TCP
attempt. Repeated KCP-input denial does not prove whether native fallback was
sent, received or answered. Static tracing places the destination-guard change
outside the inspected fixture's secure-payload path; that is not a runtime
waiver, a demonstrated baseline failure or proof of flakiness. Further diagnosis
was expressly stopped by the maintainer.

## Evidence entry points

- [UDP GSO report](https://github.com/lovitus/EasyTier/blob/21195e9b/tools/mesh-udp-flush/PACKAGED.md)
- [TUN head report](https://github.com/lovitus/EasyTier/blob/39fb29ecc7b405a89851f1df2d4eb983da83b2c8/tools/mesh-udp-flush/TUN_HEAD_PACKAGED.md)
- [Stacked relay report](https://github.com/lovitus/EasyTier/blob/dc28066955ec7a71a95f1120a379ffb98fc400a2/tools/mesh-udp-flush/RELAY_GUARD_PACKAGED.md)
- [Combined comparison and failure](https://github.com/lovitus/EasyTier/blob/7c10eccb52febbfb1bd75c46afeb2929a8d74e19/tools/mesh-udp-flush/COMBINED_PACKAGED.md)
- [Standalone Test failure](https://github.com/lovitus/EasyTier/actions/runs/36901622178)
- [Standalone comparator build](https://github.com/lovitus/EasyTier/actions/runs/36901628820)
- [Archived upstream research snapshot](upstream_selective_pick_research.md)
- [Exact source/ref/tree inventory](refs.tsv)

The per-run reports retain transport, address-family, CPU, sample, baseline and
artifact details. Raw logs, host details and local artifact archives remain in
the maintainer's private evidence storage, not in this public document.

Historical artifact IDs `10899308750`, `10898758495`, `10903497520` and
`10912354766` returned HTTP 404 during the last lookup. This only establishes
their unavailability to that request. The cause is not attributed to expiry,
deletion, permissions or Core. Some previously downloaded private archives
remain; unavailable packages are not regenerated for archival. The successful
standalone package had artifact ID `11181649899` and recorded archive SHA-256
`36ce7a5366f713acc8a657f20020795f712813c75c9fa5dd5656950779f54a52`.
No indefinite GitHub artifact retention is promised.

## Preservation and cleanup contract

The 15 source tips in `refs.tsv` are preserved under
`codex/archive/research-2026-10-02/`. Each row records the old branch, archive
branch, exact commit and tree. Archive refs retain code, tests, experimental
workflows and historical documents without integrating them into production.
Old active research branch names are removed only after archive refs have been
pushed and fetched back with matching commit/tree identities and PRs closed.

The 11 experimental worktrees are removed after preservation. Only the five
identified generated Python bytecode files are discarded; their source remains
in Git. The previously untracked upstream TODO is preserved in this archive;
the public copy adds an archival banner and redacts one private-host reference.
The unchanged original is retained privately. Existing release tags,
release branches, older recovery archives, stash objects, canonical build
caches and private diagnostic evidence are not deleted by this batch.

This document supersedes all earlier research cursors, pending scope choices,
review requests and `tools/mesh-udp-flush/CLOSEOUT.md` at `5c8d8b72`. Historical
reports and the archived upstream snapshot are kept as historical records;
their unchecked boxes and future-tense instructions are not an active backlog.
No proposed follow-up is converted into a mandatory release gate or a PASS.

Closure is administrative and evidentiary: **research stopped, source retained,
unaccepted work unmerged, no further action scheduled**. It is not a statement
that the fork's throughput, CPU and memory concerns have all been solved.

## Devin closeout review (2026-10-02)

Devin returned a read-only review of the archive at
`4a42e703feef29e4c440b936d40934e78a215468`: no concrete archival omission,
data-loss risk or factual contradiction was found. Administrative closeout was
accepted without further experiments. Its optional wording clarification about
the public TODO copy's banner and redaction is incorporated above.

Independently checked in that review: the clean canonical checkout and
documentation-only diff; all 15 local/remote-tracking commit and tree identities
against `refs.tsv`; absence of the old local and remote-tracking branch names;
one remaining worktree and one retained stash; live GitHub states for all eight
closed, unmerged PRs and four NOT_PLANNED issues, with zero open PRs/issues; and
the private original TODO's recorded SHA-256 and its two public-copy changes.

Inherited, not independently revalidated in that review: historical workflow
outcomes and performance measurements, artifact IDs/hashes, each closed PR's
head OID, the deletion history and former worktree count, bytecode contents and
other private evidence. No test, build, CI query/dispatch or code edit was made
by the advisor. The reply is advisory review, not formal GitHub approval or
production acceptance. Future task start/end communication is recorded in
`AGENTS.md`; this does not reopen the archived research.
