# Core receive-ring rejection: exact failed-packet localization

Status: REPRODUCED_AND_LOCALIZED, NOT_FIXED. Production unchanged. No merge or
release. The original combined saturation acceptance is still FAIL/OPEN.

## Identity and executed scope

- Workflow: [36284039979](https://github.com/lovitus/EasyTier/actions/runs/36284039979), terminal FAILURE.
- Harness: `2956e72cdb9df06c56183e02cb467508b27b30f2`.
- Frozen Core base: `3166ab672d347cdcc5a6768bc77056cd8ec38323`.
- Disposable overlays: unchanged GRO receiver, existing bounded ICMP ciphertext
  correlation, and rejection-only power-of-two counters. They do not change
  packet flow, ring capacity, retries, scheduling or wire/crypto behavior.
- Core SHA-256 recorded by CI: `12d53e5d8ddbd89252c254aa37df2be7740cf98b6d6538180fe5a28b641b25f5`.
- Core Build ID: `75e3065b0f93801ded9ba9e834a42fd1980435a0`.
- Reusable binary artifact: `10920492391`, 210,776,761 bytes; GitHub archive digest
  `7cc0f399e6759d10e7ec48f37a48fc9c91e6d2f026eb97ca305994bcc3611a43`.
  The large archive was not downloaded locally; these are CI/metadata identities.

Endpoint A and B are separate Core processes/network namespaces joined by veth
on one GitHub-hosted AMD EPYC 9V74 runner, four logical CPUs/two cores. The failed
case uses UDP/IPv4 underlay, IPv6 overlay, AES-GCM, Stealth off and **GRO off**.
It is a synthetic saturated mesh path, not physical-host/WAN, Leaf or Mihomo
acceptance. Instrumentation can affect scheduling: no throughput/CPU improvement
is inferred from this diagnostic binary.

Compilation and the 39 UDP plus four hole-punch tests passed. Four independent
IPv4/IPv6 off/on activation cases also passed: on had three successful enables
per family and aggregates from both sources; off had neither. The first planned
mixed-load pressure case then failed. The other five were not executed.

## The missing echo is accounted for, not guessed

The first pressure case launched opposite-direction 1 GiB TCP transfers. Both
completed with successful probe byte counts. The original ICMP assertion failed:
120 requests, 119 replies, 0.833333% loss, exactly sequence 11 missing.

| Stage | Endpoint | Evidence |
|---|---|---|
| Before send, after encryption | A | `encrypted_tx`, echo `(128, 24907, 11)`, ciphertext fingerprint `13650756ab3e7264` |
| After UDP receive/parsing | B | `udp_rx`, same ciphertext fingerprint |
| Rejected by the receive ring | B | `ring_reject`, same ciphertext fingerprint |
| Consumed by peer receive path | B | No `peer_rx` for this fingerprint |
| Submitted toward TUN after decryption | B | No `nic_enqueue` for echo `(128, 24907, 11)` |

The failure is therefore inside Core's existing receive queue for this packet,
not a guessed return-route, NAT, firewall or policy-engine failure. It already
occurs without GRO. This does not prove that GRO has no additional saturation
trade-off; the on pressure samples were not reached.

At the frozen source, `UdpConnection::handle_packet_from_remote()` calls
`RingSink::try_send()` for lossy Data. The ring has capacity 128 with four slots
reserved from lossy admission. On rejection the packet is discarded. The
observed echo was classified through that existing lossy path. We did not
change that classification or grant ICMP a priority bypass.

The process-lifetime census emitted lossy lower bounds of 256 packets at A and
512 at B. The corresponding exclusive upper bounds are 512 and 1,024. These
are intervals, not exact counts or per-transfer percentages. Functional failure
means the planned matrix is incomplete, even though the failed case's logs and
process cleanup were recovered. No non-lossy rejection marker was observed.

## Counters, cleanup and limits

The failed transfer window had 349 TCP retransmitted segments at A and 305 at B,
92/88 SACK recovery episodes, and two lost-retransmit increments at A.
Fast-retransmit counts overlap retransmitted segments; do not add them.
Each endpoint also recorded four `Ip.OutNoRoutes` increments. No UDP receive
error/drop increase was recorded, and all 42 link error/drop deltas were zero.
The source of the NoRoutes traffic is not established. It cannot explain away
the identified request's later, directly observed rejection inside Core.

This also exposed a reporting omission in earlier evidence: run 36281869121
had 198 `Ip.OutNoRoutes` increments across 56 endpoint windows, and run
36279752505 had 300 across 92 windows. The previous blanket IP-zero wording is
withdrawn. Raw counter files were retained; there was no missing capture or
test-expectation change. CPU/byte measurements and packet-stage evidence are
separate from this correction.

All 16 cleanup records for the failed case were clean. Both Core processes
exited zero without forced kill; both namespaces were removed with no remaining
PID, and root routes matched their pre-run snapshots. Core logs were 41,155 and
40,686 bytes, below the unchanged 2 MiB limit; correlation did not overflow.
The Core+CLI-only fixture logs a missing optional managed-GOST executable once
per endpoint at startup and socket-close messages during shutdown. Those logs
are retained, not represented as a successful sidecar startup. No sidecar
functionality is being accepted by this experiment.

Evidence artifact `10920377665` is 568,533 bytes, ZIP SHA-256
`99cdf21189136fdab956ed26aed385f9c3312e1cc54873cfefb3b4ec5452c6e0`.
All 746 members passed CRC and all 745 manifest hashes matched. CI failed-step
log retrieval encountered an API EOF; the saved artifact contains the original
lab traceback and all records cited here. Failure was
`AssertionError: ICMP progress failed`, not compilation or a weakened assertion.

## Next bounded research step, not a production fix

The ring is now a proven drop location. What remains unproven is whether the
dominant cause is avoidable receive-task service bursts, downstream work, or
ordinary CPU saturation. Locked Tokio already uses cooperative I/O budgeting;
the absence of any yield is not the demonstrated bug.

The narrow next experiment should compare producer/consumer progress using the
locked runtime/ring semantics and a bounded service quantum, while retaining
the same queue capacity and packet admission rules. It must measure rejection,
timer/control latency, throughput and CPU together. A result that merely trades
away throughput or adds indefinite per-peer blocking is not acceptable.
Reuse the preserved diagnostic binary for subsequent observation; do not rebuild
it to repeat the same failed matrix. A production scheduling change requires
separate functional and same-load performance evidence.

Do not enlarge the ring, add unbounded staging, blindly insert a yield after
every packet, prioritize echo packets, weaken loss assertions, or restart the
large upstream/Mihomo architecture investigation as a substitute for this
localized Core problem. No universal zero-loss-under-overload claim is made.
