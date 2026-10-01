> ARCHIVED / RESEARCH STOPPED by maintainer on 2026-10-02. This is a historical research snapshot, not an active TODO. Its unchecked items, proposed gates and original current-cursor wording are not assignments. See [the final disposition](README.md). No implementation or acceptance is implied.

# Selective upstream integration research and staged TODO

Status: SOURCE_REVIEW_ONLY. Corrected 2026-09-12.

No code implementation, cherry-pick, build, benchmark, deployment or release
is approved or reported complete here. Source differences are not runtime
reproductions. Integrate coherent behaviors with regression tests, not whole
commits merely because they are upstream fixes, nor reject them by line count.

## Pinned inputs and provenance

| Role | SHA | Evidence |
| --- | --- | --- |
| Common ancestor | `15e5d89f70c3f64f047c48d60e67c12c74c5bebc` | Verified merge base, not the historical GitHub fork-creation event |
| Reviewed fork | `c6772dbfef2395ff96b39bd4801945d92212dffb` | codex/current; local HEAD reconfirmed on 2026-09-12 |
| Earlier locally fetched upstream | `a7383114d06e427eace9ea168f0ed62598c7036b` | Earlier local comparison: 480 fork-only / 63 upstream-only |
| Continuation report upstream | `e0bdb516b6dc8a654940efbe12960dbfa846f424` | Reported comparison: 480 fork-only / 64 upstream-only |

The supplied report describes e0bdb516 as its later observed snapshot. The
earlier assertion that a7383114 is newer is withdrawn. Local cat-file and
ancestry queries failed because e0bdb516 is not present locally. Fetch and
verify that exact object before implementation. Neither SHA is claimed to be
today's latest upstream.

Materials reconciled:

- Supplied `upstream_selective_pick_research_continuation_2026-09-11-1.md`.
- Supplied `upstream_selective_pick_todo_2026-09-11-1.md`.
- Both accompanying pasted reviews, previously omitted and now explicitly read.
- Earlier local paired-source inspections at c6772dbf.
- Historical plan: `easytier/docs/todo/upstream_7_commits_selective_integration_plan.md`.

This document is the current research task cursor. Downloaded materials are
research supplements, not execution authority or evidence of local tests.
Coverage is the named candidate families, not every upstream-only commit.

## Corrections

- f24735a8 and 57eb6908 are absent from the inspected current implementations;
  similarly named commits on other refs are not evidence of integration.
- 46f1b573 is an ancestor of the common baseline and must not be picked again.
- Async indefinite waiting is not inherently uncancellable: dropping the
  future or aborting its owner can cancel it. Actual supervision must be traced.
- A ready recv().await need not park; raw try_recv can bypass cooperative
  budgeting. A small diff can have significant fairness effects.
- EasyTier gateway smoltcp and Leaf's vendored netstack are different stacks.
  Prior Leaf runner fixes do not establish gateway listener equivalence.
- Rejecting wholesale portable-core integration is a scope choice, not proof
  that its individual fixes are useless or its architecture can never be adopted.

## Disposition matrix

| Candidate | Status | Unit to evaluate |
| --- | --- | --- |
| 425a2427 | SELECTIVE_PORT_CANDIDATE | Destination guard and activity-aware session GC |
| 86222771 | SPLIT_AND_REVIEW | Peer-center, relay routing, egress progress and pinger |
| abf03ca5 | SELECTIVE_PORT_CANDIDATE | System lookup budget/limiter preserving DNS policy |
| 8d475dc3 | ADAPTATION_CANDIDATE | Sink delegation retaining Stealth sealing |
| e130e9e4 | PLATFORM_CANDIDATE | GUI system-daemon environment |
| 636390ec | SPLIT_WITH_WIRE_BLOCKERS | Non-wire pinger first; separate echo design |
| 4304b065 | COHERENT_ALGORITHM_ADAPTATION | Bounded logical listener and ownership |
| e0bdb516 | VERSIONED_PROTOCOL_CANDIDATE | Checksum and negotiated-version compatibility |
| 282cd92f | SUPERVISION_REVIEW | Readiness plus actual cancellation chain |
| f24735a8 | BENCHMARK_GATED | Consuming buffer extraction |
| 57eb6908 | FAIRNESS_AND_BENCHMARK_GATED | Receive scheduling and throughput |
| 7fb42c3b | DECOMPOSE | Filter snapshots and measured residual costs |
| 46f1b573 | ALREADY_IN_ANCESTRY | Closed integration item, not universal IPv6 proof |

## Stage A: baseline

- [ ] Fetch exact e0bdb516; verify ancestry and divergence counts.
- [ ] Reconcile newer HEAD/uncommitted changes without overwriting user work.
- [ ] Record actual Cargo.lock, Tokio/smoltcp versions, toolchain and features.
- [ ] Run relevant unchanged baseline tests in GitHub workflows; preserve failures.
- [ ] Establish packet, proxy and crypto fixtures preserving fork wire contracts.

Only local HEAD reconciliation was performed during this correction. Other
unchecked gates are not implied complete by the attached reports.

## Stage B: correctness extractions

### B01-B02: secure relay (425a2427)

Fork `peers/peer_conn.rs::PeerSessionTunnelFilter::before_send` checks source
but lacks the next-hop destination guard. `peer_session.rs` evicts by strong
reference count alone. Relay references are temporary, not durable activity evidence.

- [ ] Restrict next-hop encryption to packets addressed to that next hop.
- [ ] Retain valid recently active sessions for a bounded idle interval.
- [ ] Separate activity-touch access from status inspection, which must not
  continually refresh idle lifetime.
- [ ] Preserve direct/relay sharing, rotation and connection-local Stealth.
- [ ] Test invalid next-hop sessions with destination-encrypted relay traffic;
  recent, idle, invalid, externally held and rotating session reclamation.

Do not bundle optional ArcSwap optimization into correctness work. Preserve
other StdMutex uses and existing secure/Stealth behavior.

### B03-B04: peer-center and relay routing (86222771 subsets)

Fork `peer_center/server.rs` inserts reported edges without replacing omitted
neighbors; digest ignores latency and expiry does not refresh it.

- [ ] Replace reporter snapshots, including empty reports.
- [ ] Publish consistent snapshot/digest covering topology, latency and expiry.
- [ ] Test removal, latency-only updates, expiry, unchanged digest, clone sharing
  and independent instances, without introducing new RPC fields.

The report also identifies LeastHop ACK/missing-session recovery paths in
`peers/relay_peer_map.rs`.

- [ ] Preserve request routing intent and derive reply policy from evidence.
- [ ] Account for fork forward_counter starting at 1: upstream > 0 also matches
  fresh fork packets and cannot be copied literally.
- [ ] Preserve foreign networks, static-key validation, cipher negotiation and
  pending packets; test direct blackhole with working relay in both directions.

### B05: bounded system DNS (abf03ca5)

Fork `common/dns.rs::socket_addrs` awaits system DNS without an inner deadline.
DNS TCP-connect timeout does not bound that stage. Unmarked
`lookup_control_plane_host` is a separate system-only entry point.

- [ ] Fit lookup plus limiter wait within actual caller budgets.
- [ ] Share a persistent limiter and hold its permit until underlying blocking
  resolution finishes, even after caller timeout; timeout alone is insufficient.
- [ ] Preserve marked Hickory sockets and libc bypass for marked contexts.
- [ ] Explicitly decide fallback for the system-only entry; do not silently
  leak internal names to public resolvers.
- [ ] Test stalls, repeated retries, concurrency bounds, recovery, internal
  names, IPv4/IPv6 literals and explicit zero port.

Upstream 800 ms/one-slot behavior is a reference, not an approved universal
budget. Adapt the mechanism without replacing the fork DNS architecture.

### B06: WebSocket cleanup (8d475dc3)

Fork listener/connector `.with(...)` conversion also seals Stealth records.
Upstream's direct Sink delegation must retain that behavior.

- [ ] Preserve exactly-once per-connection sealing and associated ownership.
- [ ] Delegate readiness/send/flush/close with existing errors/backpressure.
- [ ] Force independent phase errors; prove close reaches the underlying sink
  after send failure. Cover cancellation/repeated close and plain/Stealth WS/WSS.

### B07: launchd HOME (e130e9e4)

Fork service installation has no explicit environment. Upstream plumbs it and
sets HOME=/var/root for the GUI system daemon.

- [ ] Preserve keep-alive behavior and every options initializer.
- [ ] Treat CLI and Network Extension behavior separately.
- [ ] Verify plist and actual pre-login config/data/log access.

This does not solve RPC URL normalization, GUI elevation or config migration.

## Stage C: controlled behavior and compatibility

### C01: egress progress (86222771 subset)

- [ ] Audit all awaited NIC sends, including TCP proxy, not only peer manager.
- [ ] Define lossy-data policy and pressure/closed counters before using try_send.
- [ ] Preserve control progress and deferred/claimed setup; stress full queues
  with Pongs, route updates, stop and reload. Never blanket-drop control packets.

### C02: coherent non-wire pinger (86222771 + 636390ec)

Fork allows overlapping work and resets losses on unrelated ingress.

- [ ] Review one correlated in-flight ping and coalesced scheduling together.
- [ ] Preserve underlay-breaker clearing and metrics.
- [ ] Test one-way traffic, dropped Pongs, delayed/out-of-order completion,
  queue/scheduler pressure and recovery without unacceptable false disconnects.
- [ ] Recalculate detection timing instead of retaining overlapping-ping deadlines.

Implement overlapping pinger corrections once. This stage introduces no new
wire fields and does not claim to provide business-packet echo semantics.

### C03: bounded smoltcp listener (4304b065)

The report identifies per-SYN wrapper tasks, a generic listener socket replaced
on accept, and bulk reactor ingress. Upstream pending ownership is bounded to 16.

- [ ] Verify locked smoltcp APIs.
- [ ] Adapt registration, SYN eligibility/duplicates, allocation, poll ordering,
  acceptance, reclamation and ownership-safe drop as one coherent algorithm.
- [ ] Modify socket.rs, reactor.rs, socket_allocator.rs and tcp_proxy.rs as
  needed: no whole-file replacement does not mean no reactor modification.
- [ ] Preserve claimed/deferred NAT and stream ownership.
- [ ] Test malformed/burst/duplicate SYN, saturation, timeout, CloseWait before
  accept, drop/rebind, cancellation and wake races; measure bounded resources.

Later upstream notification improvements need separate attribution.

### C04: QUIC checksum and ETQ1 (e0bdb516)

The supplied report identifies a matching fork checksum primitive that accepts
but does not bind decoded packet number. Local inspection of this newly
reported upstream object and all construction paths remains pending.

- [ ] Confirm paired implementation and canonical ETQ1 calculation.
- [ ] Bind checksum behavior to negotiated version; keep legacy-v1 behavior.
- [ ] Do not assume a suitable ETQ1 implementation already exists in the fork.
- [ ] Bound legacy fallback/memory; preserve Stealth authentication, Brutal,
  proxy/tunnel dialers and connection/cache lifecycle.
- [ ] Test wrong decoded numbers, reordering, old/new interoperability and
  upgrade after expiry. Never change checksum while advertising unchanged v1.

Checksum is not cryptographic authentication; reported upstream performance is
not fork acceptance evidence.

### C05: Windows readiness (282cd92f)

The ten-second deadline remains in the fork. Upstream removes it and awaits
interface arrival. This is not proof of uncancellable behavior.

- [ ] Trace the owning startup future through Stop/reconfigure/shutdown.
- [ ] Decide supervised waiting/retry based on that ownership.
- [ ] Prove prompt cancellation and cleanup, late and never-arriving interfaces.
- [ ] Keep waiting observable and polling/logging bounded.

The gate is verified lifecycle control, not necessarily a finite wait.

### C06: liveness echo compatibility (636390ec remainder)

| Location | Upstream | Fork |
| --- | --- | --- |
| Packet flag 0x20 | Liveness probe | DEFERRED_PROXY |
| Noise request tag 6 | repeated string features | optional string outer_cipher_suite |
| Noise response tag 11 | repeated string features | optional string outer_cipher_suite |

- [ ] Audit local-only deferred flag normalization and self-delivery boundaries.
- [ ] Design compatible negotiation; renamed Rust fields do not fix wire conflicts.
- [ ] Do not advertise liveness-echo-v1 with different flag/token semantics.
- [ ] Test old/new fork/upstream in both directions, classic/Noise and
  plain/Stealth; cover stale/wrapped tokens and deferred-SYN preservation.

Echo is blocked on explicit design, not abandoned. Successful protobuf parsing
does not prove semantic compatibility for the colliding string fields.

## Stage D: measured performance

- [ ] D01 / f24735a8: advance-and-move only where prefix/original handle are
  discarded. Test offsets, shared/unique storage, conversions and TUN packet-info;
  benchmark extraction apart from construction and count allocations.
- [ ] D02 / 57eb6908: compare locked-Tokio recv, raw try_recv and budget-aware/
  bounded bursts. Gate on timer/Pong tail latency, shutdown, idle wakeups and CPU
  as well as throughput. Test close/drain and empty wakeup.
- [ ] D03 / 7fb42c3b: assess immutable filter snapshots against registry locks
  across awaits; preserve priority/reverse order, removal/reload, in-flight
  ownership and Continue/StopAndSend/Consume.
- [ ] D04 / 7fb42c3b: reconcile reported existing native TCP halves, crypto
  selection and owned UDP paths; close those specific subchanges, not all paths.
  Audit UDP truncation/Stealth overhead instead of importing an 8 KiB constant.

Do not import HostPacket/portable-core/WASI simply to remove overhead that
their introduction caused upstream. Investigate actual residual fork costs.

## Delivery gates and current cursor

Prioritize correctness fixtures and missing behavior, then supervised/protocol
changes, then measured performance. try_recv is not automatically the first pick.

- All layers/tests/docs of an approved user milestone form one delivery batch;
  behavioral decomposition does not authorize micro-commits per method/test.
- Tests/build/release use GitHub workflows under the latest user policy. Older
  private-builder prescriptions in historical materials are superseded.
- Record exact SHA/provenance, baseline failures, regression output, platform
  coverage and untested boundaries. Preserve original test semantics.
- Use prescribed global workflow status/log helpers with bounded backoff.
- No workflow is triggered by this documentation correction. No blanket merge
  or business-code implementation is approved.

Current cursor: research documents reconciled; exact e0bdb516 object/ancestry
and additional paired-source checks remain pending before any implementation.
No build, test, reproduction, performance or deployment gate has passed here.

## Upstream source references

- [Secure relay](https://github.com/EasyTier/EasyTier/commit/425a2427)
- [Asymmetric recovery](https://github.com/EasyTier/EasyTier/commit/86222771)
- [DNS](https://github.com/EasyTier/EasyTier/commit/abf03ca5)
- [WebSocket](https://github.com/EasyTier/EasyTier/commit/8d475dc3)
- [launchd](https://github.com/EasyTier/EasyTier/commit/e130e9e4)
- [Liveness](https://github.com/EasyTier/EasyTier/commit/636390ec)
- [smoltcp](https://github.com/EasyTier/EasyTier/commit/4304b065)
- [QUIC](https://github.com/EasyTier/EasyTier/commit/e0bdb516b6dc8a654940efbe12960dbfa846f424)
- [Windows](https://github.com/EasyTier/EasyTier/commit/282cd92f)
- [Buffer extraction](https://github.com/EasyTier/EasyTier/commit/f24735a8)
- [Receive fast path](https://github.com/EasyTier/EasyTier/commit/57eb6908)
- [Native throughput](https://github.com/EasyTier/EasyTier/commit/7fb42c3b)
