# Core relay destination ownership guard

Status: unchanged-baseline regression confirmed; corrected candidate verification pending.

## Evidence and scope

- Parent: `6e90dbf102e5c93d56c531b87eea1858c4a8e61e`.
- Reference: upstream `425a24273b192399d3de3509fd868382f8ec89cc`,
  only the next-hop destination guard (B01 in issue #2).
- Exact-package evidence: [relay run](https://github.com/lovitus/EasyTier/actions/runs/36236617653)
  and [audited report](https://github.com/lovitus/EasyTier/blob/39fb29ecc7b405a89851f1df2d4eb983da83b2c8/tools/mesh-udp-flush/TUN_HEAD_PACKAGED.md).
  The functional matrix passed, but secure/Stealth relay logs grew to about
  116 MB per endpoint, repeatedly dumping already-encrypted packets. Both
  parent-only and candidate-only placements exhibited the problem.
- `RelayPeerMap` encrypts locally originated relay packets for their final
  destination. `PeerSessionTunnelFilter::before_send` currently checks source
  ownership but not final destination ownership. The next-hop session may
  therefore consume nonce/crypto lookup work and emit a large warning, or drop
  the packet if that unrelated session has become invalid.

## Minimal change and preserved contracts

Move the existing source ownership check before the session lock and include
`to_peer_id == peer_id` in the ownership condition. No new state, allocation,
queue, timer, dependency, wire field or configuration is introduced.

Keep the existing `StdMutex`, connection-local Stealth behavior, session GC,
crypto backends, nonce/replay behavior, incoming destination check, route
selection and control-packet exclusions. Do not suppress the crypto warning to
conceal an incorrect caller. B02 activity-aware session GC remains separate.

## Failure modes and regression

The single table-driven regression uses the real filter and real sessions:

- A-to-C authenticated ciphertext goes through next hop B byte-for-byte intact.
- B's session is tested both valid and invalid, in both peer-ID directions.
- C authenticates and decrypts the forwarded ciphertext with the actual A/C key.
- Ordinary A-to-B data still encrypts and decrypts with the connection key.
- Invalid directly addressed sessions still reject data.
- Forwarded traffic and existing handshake/Ping/Pong exclusions remain untouched.

No ciphertext flag is forged, no filter/session mock is used, and no source
string assertion substitutes for behavior.

## Verification state and remaining gates

The maintainer explicitly authorized one unchanged-baseline full Test run and
one corrected-candidate full Test run, using the existing workflow without
adding a focused-test CI mode.

- Baseline source: `cd612a5546f2a1a6a5926c58b6061141acbe21c6`, consisting only
  of the new regression on parent `6e90dbf102e5c93d56c531b87eea1858c4a8e61e`.
- [Baseline Test](https://github.com/lovitus/EasyTier/actions/runs/36240736489)
  compiled successfully and failed the regression at runtime in 0.009 seconds:
  `relay payload must bypass the unrelated next-hop session`.
- The real next-hop session was invalidated; the old filter dropped the
  independently encrypted final-destination packet. The valid-next-hop case
  also reached the crypto backend's already-encrypted warning.
- The affected partition ran 452 tests: 451 passed and this regression failed.
  One passing test carried a LEAK annotation; it is not a clean-resource claim.
  Existing direct secure/legacy handshake tests were not changed to obtain red.
- The identical regression must now pass with the corrected ownership guard.
  Existing direct encryption and connection-local Stealth tests must also pass.
  Neither candidate green nor exact-artifact acceptance is claimed yet.

The isolated relay lab now enforces a 2 MiB per-Core log-file bound. Its
[negative control](https://github.com/lovitus/EasyTier/actions/runs/36238994113)
reused the old optimized artifact, hit that bound in the first secure/Stealth
relay case, retained the original error and removed all three namespaces while
leaving root routes unchanged. This is a resource failure, not a functional PASS.

The corrected optimized artifact must pass the bounded relay matrix with the
same secure/Stealth settings and logging level. Do not repeat the known-bad
bulk arm or the completed direct-TUN performance experiments merely to obtain
another baseline. A functional PASS without bounded logging is not acceptance.

Rollback boundary: the ownership condition and its regression only. No
ArcSwap/GC refactor, logging suppression, release, or merge is included.

## Additional static send-path audit

Inspected the actual fork call sites without changing another production file:

- `PeerConn::send_handshake` uses destination zero for legacy negotiation.
  `do_handshake_as_client` takes that path only without a secure handshake;
  the legacy server response does not install the Noise session/peer ID. The
  existing unknown-peer/no-session bypass therefore remains in effect.
- Noise Msg1/2/3 and relay handshake/Ping/Pong exclusions run before ownership
  checks, unchanged. Secure client/server attach the session and connected peer
  after the Noise handshake result is available.
- `Peer::send_msg` selects a connection (or the existing recovery connection)
  without rewriting the final destination header. The new guard changes neither
  selection nor retry behavior.
- Foreign-network encapsulation explicitly writes `my_node_id -> via_peer` in
  the outer peer header. Directly addressed outer traffic remains eligible for
  that peer's session; a relay next hop must not take ownership of the final
  destination session.
- The existing `derived_stealth_peer_sessions_are_connection_local` assertions
  keep the inner filter disabled in derived-Stealth-only mode. The log-storm
  experiment explicitly enables both secure mode and Stealth, not Stealth alone.

Existing handshake, secure data round-trip, legacy compatibility and
connection-local Stealth tests are untouched. This is source-level compatibility
reasoning, not a corrected-candidate PASS or proof that it is ready to merge.
