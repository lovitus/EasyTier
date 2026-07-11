# Release Candidate Validation: 63ed38f0

## Result

Commit `63ed38f0d7715fa6739b7952caa2aac5ef723b99` passes the tested functional,
compatibility, proxy, SOCKS, veth, throughput, and resource-retention checks. It is
approved for formal release. Failover to an already-live backup can interrupt
traffic for approximately 7-15 seconds while the existing five-loss PeerConn
liveness threshold confirms the fault. The maintainer accepts this inherited
anti-flap tradeoff for v2.6.9: a short outage is preferable to repeated protocol
switching on lossy or high-latency links.

Public host names and public addresses are intentionally omitted. All tests used
isolated network names, virtual addresses, TUN names, listener ports, SOCKS ports,
and RPC ports.

## Artifact Provenance

| Item | Value |
|---|---|
| Workflow | EasyTier Linux Profiling Beta run `29145434544` |
| Commit | `63ed38f0d7715fa6739b7952caa2aac5ef723b99` |
| Target | `x86_64-unknown-linux-musl` |
| Outer SHA-256 | `df3b83dcca64496c0555f1cc08458379427eaf09826931ac3b6fcfe7824c6701` |
| Core SHA-256 | `45870898bda18ba93f82c6bd837c5ef95b08192b77137346c560b8610a943b94` |
| CLI SHA-256 | `1f012e375176aa53b2f7c62d2dcae9412ffde214cfa5277eef731bcc65dfc01c` |
| Core build ID | `a844fd2e17e2361b3e369bc89fdbb109d7dccd33` |
| Runtime version | `2.6.9-63ed38f0` |

The outer checksum, inner checksums, `BUILD_INFO.txt`, commit, target, static PIE
format, debug symbols, and build ID were verified before deployment. The artifact
was then verified again on each validation host.

## Test Topologies

| Role | Kernel | CPU | RAM | Purpose |
|---|---:|---:|---:|---|
| LAN-A | Linux 3.10 | 2 cores | 3.7 GiB | listener, target service, throughput receiver |
| LAN-B | Linux 3.10 | 2 cores | 3.7 GiB | connector, SOCKS, throughput sender |
| Lab | Linux 3.10 | 8 cores | 32 GiB | strict-listener, mixed-version, veth |
| WAN-A | Linux 4.19 | 1 core | 1 GiB | public bootstrap and throughput receiver |
| WAN-B | Linux 5.4 | 1 core | 1 GiB | public near-path sender |
| WAN-C | Linux 5.15 | 1 core | 2 GiB | public cross-region sender |

Unless a case states otherwise, new nodes used:

```text
stealth_mode=true
stealth_protocols=udp,tcp,faketcp,quic,wg,ws,wss
transport_priority=global:quic,faketcp,ws,wg,udp,tcp
```

## Transport Preference and WAN Mesh

- TCP bootstrap upgraded to QUIC in about four seconds without a QUIC bootstrap URL.
- Three public nodes bootstrapped through one node and converged to a full P2P mesh.
- Every public edge established QUIC; one edge also retained UDP and TCP backups.
- Route cost was `p2p`/`1`, with zero idle control-plane loss.
- A single near-path QUIC transfer sustained `394 Mbit/s` for 20 seconds on one-core
  nodes. One earlier run lost its QUIC PeerConn after six seconds and recovered nine
  seconds later; an immediate 20-second repeat was stable and did not reproduce it.
- Cross-region physical TCP reached `101 Mbit/s`; full-Stealth QUIC overlay reached
  `63.8 Mbit/s` in the same direction. The one-core endpoints and approximately
  `212-215 ms` overlay RTT are material constraints.

## Failover and Failback

Both LAN nodes initially had TCP plus two QUIC PeerConns. QUIC listener traffic was
blocked in both directions for 30 seconds while a 5 Hz overlay ping and two-second
PeerConn snapshots ran.

| Event | Observed time relative to block |
|---|---:|
| First side removed failed QUIC | about 8.2 s |
| Second side removed selected failed QUIC | about 15.2 s |
| FakeTCP established | about 20.6 s |
| Firewall restored | 30.0 s |
| QUIC re-established | about 36.3 s |

The failback fix is effective: QUIC returned about `6.3 s` after path restoration,
instead of waiting for the old 300-second ordinary-failure blacklist. The 300-second
self-loop safety timeout remains unchanged.

Failover behavior is accepted for this release. The initial ping run sent 400 packets and
received 326, with a contiguous outage of about `14.8 s`. A follow-up reproducibility
matrix repeated the same bidirectional QUIC firewall failure five times against the
same artifact:

| Workload | Direction | Trials | Contiguous zero-delivery window |
|---|---|---:|---:|
| ICMP at 5 Hz | LAN-A to LAN-B | 3 | `8.6 s`, `7.8 s`, `7.8 s` |
| Saturated TCP | LAN-A to LAN-B | 1 | about `15 s` |
| Saturated TCP | LAN-B to LAN-A | 1 | about `14 s` |

All five trials recovered through an already-live backup while QUIC was still
blocked, and both saturated TCP application connections survived. Therefore the
existence of a substantial failover stall is reproducible (`5/5`), while its exact
duration is workload- and pinger-phase-dependent rather than fixed at 15 seconds.

The two sides detect a dead selected connection independently. The active sender
closes it first; an initially idle reverse side only accelerates its pinger after
traffic arrives through the backup. Existing anti-flap behavior requires five
consecutive failed ping rounds and allows two seconds per round. Shortening those
constants directly would risk false closure on high-latency or lossy links. A later
fix should mark a connection locally degraded for selection before applying the
existing five-loss close threshold.

### Upstream release comparison

The same fault was reproduced with the official upstream `v2.6.4-8428a89d` Linux
x86_64 release, without this fork's transport-priority code:

- With all listeners configured normally, upstream converged to one TCP PeerConn.
  Blocking it removed the peer after about 8.6 seconds, but no QUIC/UDP/FakeTCP
  backup was created. Ping remained unavailable for the full 20-second firewall
  interval and resumed about 0.6 seconds after TCP was allowed again.
- To isolate the shared liveness code, the upstream client was then given explicit
  TCP and QUIC bootstrap URLs. Both PeerConns remained live before the fault.
- Blocking selected QUIC with TCP already live still caused 36 consecutive 5 Hz
  ping losses, about 7.2 seconds, before TCP carried traffic.
- A saturated TCP flow over the same dual-connection setup had about 12 seconds of
  zero throughput. The two endpoints removed failed QUIC after approximately 12
  and 14 seconds. The application TCP connection survived and continued over TCP.
- After restoring the path, the explicit upstream QUIC connector re-established
  QUIC in about four seconds.

Therefore delayed hot-backup selection is inherited from upstream's existing
PeerConn pinger/default-connection lifecycle, not introduced by transport priority.
The fork's automatic multi-protocol retention improves eventual availability over
the upstream default, but it currently inherits the same detection delay before a
live backup is selected.

## Proxy Selection

The Proxy selector was tested independently from the QUIC underlay by changing only
the destination's advertised Proxy input capability.

| Destination capability | Selected path | Throughput |
|---|---|---:|
| QUIC + KCP input | QUIC Proxy | `501 Mbit/s` |
| KCP input only | KCP Proxy | `512 Mbit/s` |
| Neither input | Native | `640 Mbit/s` |

RPC observations showed both iperf control and data flows as `Connected / Quic` or
`Connected / Kcp` in the corresponding cases. Native produced no Proxy entry. The
fixed `QUIC -> KCP -> Native` order worked and did not alter the QUIC underlay.

## SOCKS KCP-Only

A 64 MiB random file was downloaded through the built-in SOCKS service and KCP path:

| Check | Result |
|---|---:|
| HTTP result | success |
| Bytes | 67,108,864 |
| Duration | 1.412 s |
| Payload rate | about 380 Mbit/s |
| SHA-256 | exact match |
| FD before / 8 s after | 33 / 33 |

Short-connection stress used a 1 KiB target and concurrency 20:

| Check | Result |
|---|---:|
| Connections | 1,000 / 1,000 successful |
| Completion time | 10.32 s |
| FD baseline / peak / 60 s / 120 s | 30 / 50 / 30 / 30 |
| RSS baseline / peak / 60 s / 120 s | 17.0 / 18.7 / 17.7 / 17.6 MiB |
| Proxy entries after 120 s | 0 |
| Residual target TCP connections | 0 |

No persistent FD, task-visible connection, Proxy entry, or RSS growth was found.

## Stealth and Secure Performance

The same two Linux 3.10 hosts, QUIC preference, one TCP stream, and 15-second test
duration were used for all three modes. KCP/QUIC Proxy was not involved.

| Mode | Throughput | TCP retransmits | Relative to plain |
|---|---:|---:|---:|
| Plain | `813 Mbit/s` | 0 | baseline |
| Derived Secure for Stealth | `751 Mbit/s` | 0 | -7.6% |
| Explicit Secure + Stealth | `656 Mbit/s` | 0 | -19.3% |

Derived Secure is not silently plain: wrong-secret and plain clients were rejected
by the strict listener, while the derived pair exchanged traffic successfully. The
historical near-10x explicit-Secure slowdown was not reproduced on this commit and
topology. Explicit Secure still measured about 12.7% below derived Stealth, so the
existing known-bug entry should remain until profiling covers relay/foreign-network
topologies as well.

## Strict and Mixed-Version Compatibility

- A wrong-secret new client was reset by the strict TCP listener and never appeared
  in its peer table.
- A correct-secret client with Stealth disabled was also reset and never appeared.
- An upstream release `v2.6.4` plain listener rejected the new client's first
  Stealth attempt (`InvalidPacket("body too long")`). The new client then performed
  its unknown-capability legacy fallback and connected over plain TCP, followed by
  UDP. Both peer tables agreed.
- This compatibility behavior is outbound-only. A strict new listener was not
  relaxed to accept an old/plain inbound connection.

## Restart and veth

- A TUN node killed with `SIGKILL` and restarted after 200 ms reused the configured
  interface and restored overlay ping with about 1.2 seconds of loss.
- Linux 3.10 veth created one main interface and one private peer, established TCP
  and QUIC, and passed 10/10 IPv4 pings.
- veth `SIGKILL` plus 200 ms restart left one visible main interface, 28 FDs, about
  17.9 MiB RSS, and no duplicate named interface. TCP returned in about one second
  and QUIC in about 4.6 seconds.
- End-to-end ping to the restarted veth node recovered after about 8.4 seconds. The
  delay is associated with replacement of the old PeerId/route for the same virtual
  IP, not veth setup or a leaked interface.

## Release Gate

Approved for v2.6.9 after maintainer review. The measured 7-15 second hot-backup
selection delay is documented as an accepted anti-flap tradeoff, not a release
blocker. A future optimization may mark a selected PeerConn locally degraded before
the existing five-loss close threshold, but only if loss/RTT testing proves that it
does not introduce protocol flapping. No such behavioral change is required for
this release.
