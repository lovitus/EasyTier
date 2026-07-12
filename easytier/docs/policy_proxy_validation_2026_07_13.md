# Optional Policy Proxy Validation - 2026-07-13

## Scope

This report records validation of the opt-in Linux `leaf-policy-proxy` spike.
It is not a release qualification for macOS, Windows, Android, or mobile
launchers. No public domain names or production endpoints are included.

Validation used the rolling `profiling-beta` x86_64-musl artifact on an
isolated Linux 3.10 network namespace. Every artifact was checked against its
outer and inner SHA-256 manifests, `BUILD_INFO.txt`, commit SHA, target, ELF
build ID, DWARF metadata, and unstripped symbol table before deployment.

## Validated architecture

- EasyTier remains the sole TUN owner. Mesh destinations retain the ordinary
  VirtualNic and PeerManager path; non-mesh packets are handed to a bounded
  Leaf packet bridge.
- Mesh SOCKS actors use EasyTier's existing TCP/UDP data plane to reach the
  selected peer, then a standards-compliant remote SOCKS5 server.
- Linux transparent capture uses main-table split defaults. EasyTier and Leaf
  native underlay sockets bypass capture through source/mark rules and private
  table 52000.
- A terminal `unreachable default` in table 52000 prevents a marked socket from
  falling through to the main table and recursing into the TUN while the
  physical default route is unavailable.
- One host-wide flock serializes ownership. Routes use protocol 99 and fixed,
  conflict-checked rule priorities 10899/10900.

## Functional results

### Routing and lifecycle

| Scenario | Result |
| --- | --- |
| Initial route/rule install | Pass |
| Graceful stop cleanup | Pass; policy rules, table routes, and capture routes removed |
| Core SIGKILL | Pass; worker exited through parent-death handling; stale owned state was cleaned on restart |
| Worker SIGKILL | Pass; supervised restart in about 3 seconds |
| Second policy process | Pass; rejected with `policy routing is owned by another process` without changing the first instance |
| Physical gateway `10.250.0.1 -> 10.250.0.254` | Pass; table 52000 converged within the 5-second refresh period |
| Physical source address add/remove | Pass; priority-10900 source rule added and removed |
| Physical default route removal | Pass; only terminal unreachable and connected route remained; marked lookup returned `No route to host` |
| Physical default route recovery | Pass; bypass default was restored automatically |

During the no-default-route interval the core reported approximately 0.1% CPU,
17 MiB RSS, ten threads, and 29 file descriptors. No retry or loop storm was
observed.

### Data plane

| Scenario | Result |
| --- | --- |
| Policy TCP through mesh SOCKS | Pass |
| Policy UDP through SOCKS5 UDP ASSOCIATE, 5 Mbit/s | Pass; zero measured loss |
| Policy UDP DIRECT, 10 Mbit/s | Pass; zero measured loss |
| Policy UDP DIRECT, 50 Mbit/s target | Functional at about 40 Mbit/s with 19% loss; high-PPS limitation remains |
| Magic DNS in policy mode | Pass; `100.100.100.101` returned the mesh peer record |
| Mesh writer load plus policy TCP | Pass; four mesh TCP flows sustained about 620 Mbit/s while 30/30 policy TCP connections succeeded |

After the mixed-load test without KCP encapsulation, core/worker file
descriptors settled from 58 to 33/11 within 20 seconds. RSS settled near 19/7
MiB. KCP encapsulation is tracked separately below because it exposed a
release-blocking connection-lifecycle defect.

### KCP encapsulation

Symbolized `perf` data from the exact `9d582e6d` beta proved that policy TCP
used the intended KCP path: `ikcp_flush` accounted for 13.54% of samples and
the profile also contained `ikcp_check`, `KcpConnection::run`, and KCP output
handling. Eight data streams plus the iperf control stream sustained 965
Mbit/s sender and 961 Mbit/s receiver throughput over 1.69 GiB.

That snapshot failed lifecycle qualification. More than seven minutes after
the load ended, the destination retained established target SOCKS connections
and `Connected/Kcp` RPC entries. One subsequent HTTP request added three more
entries with the same timestamp. This isolates the leak to multiple successful
connections created by the five-way, 200 ms KCP hedge: the winner was used,
but successful losers were dropped without a reliable explicit reset. The
ordinary 60-second pong cleanup could not reclaim them because both endpoints
continued exchanging pings.

The next candidate gives every `KcpStream` a local owner guard. Explicit
graceful shutdown retains the existing FIN and retransmission behavior; a
stream dropped by hedge cancellation, task cancellation, or panic synchronously
removes its local state and best-effort sends RST. A unilateral-drop regression
test requires the remote stream to observe EOF and both endpoint maps to return
to zero. This candidate remains unvalidated until a new beta shows that KCP
RPC entries, target sockets, file descriptors, and endpoint maps return to
baseline within two minutes.

The first owner-guard beta confirmed that successful hedge losers were
reclaimed, but it also separated a second lifecycle defect: one established
KCP winner remained for every Leaf-to-mesh-bridge TCP session. There was no
new SYN traffic on the policy TUN and policy routing remained correct. The
vendored smoltcp adapter's `poll_shutdown()` waited for the complete TCP state
machine to reach `Closed`, while the runner neither woke the shutdown waiter
when FIN was committed nor treated its `Closing` state as completion. That
held `copy_bidirectional()` and the winner indefinitely. The follow-up patch
wakes the waiter when the buffered write half is handed to smoltcp and returns
shutdown success at `Closing`; it does not shorten the TCP state machine or
discard buffered bytes.

## Defects found and disposition

1. The first transparent route used only a high-metric default on the TUN, so
   the physical default route won. Split-default capture fixed this.
2. Binding Leaf to a device did not override the split default. Source/mark
   policy routing and a private physical route table fixed the recursion.
3. STUN sockets lacked the policy socket mark and were captured. Applying the
   existing mark to STUN TCP/UDP fixed this.
4. A second process could clean another process's owned table. A host-wide
   nonblocking flock fixed this.
5. The private table was static. Periodic idempotent netlink reconciliation now
   follows physical gateway and source-address changes.
6. Magic DNS's exact fake address was absent from the mesh ownership snapshot.
   Adding its `/32` while Magic DNS is enabled fixed policy misclassification.
7. A biased TUN writer could starve Leaf responses under continuous mesh load.
   Removing the biased selection bounded policy latency without changing queue
   sizes.
8. The first endpoint-generation rebuild (`c008aff3`) stopped the old worker
   but did not recover after the peer returned. The speculative bounded child
   stop change (`d8f2d548`) did not fix the fault and was reverted by
   `26b59d8d`. Commit `26bc78ac` fixed the dormant-state route refresh and the
   worker restarted, but end-to-end SOCKS traffic still timed out. The second
   cause was in the shared SOCKS data-plane lifecycle: removing its last
   consumer unloaded the smoltcp net without invalidating the remembered IPv4,
   so a later consumer with the same IPv4 never rebuilt the net. The failed
   snapshot was explicitly reverted by `485e1395`; the combined lifecycle fix
   and its release/reacquire regression test are pending beta validation.
9. Strict HTTP validation found a fast-EOF data-loss bug in the pinned Leaf
   smoltcp backend. The remote SOCKS server emitted the complete 2500-byte
   response, and the private mesh bridge delivered it completely when tested
   directly, but transparent TUN traffic received FIN before the pending body.
   Holding the remote connection open for one second made the same transfer
   complete. The pinned `netstack-smoltcp` calls `socket.close()` before moving
   its pending adapter send buffer into the smoltcp socket. EasyTier now vendors
   that exact revision with one condition: FIN is requested only after the
   pending send buffer is empty. Immediate-close and delayed-close responses
   must both pass before the beta is accepted.
10. The `9d582e6d` beta passed immediate/delayed EOF, repeated data-plane
    release/reacquire, worker restart, exit restart, Magic DNS, and UDP
    recovery checks. It was nevertheless rejected because successful KCP
    hedge losers left remote target TCP connections established indefinitely.
    The failed snapshot was explicitly reverted by `09d73848`; its independently
    valid changes are being reapplied together with the KCP stream owner fix.
11. The `113b25b3` beta proved that KCP hedge losers were reclaimed, but each
    winner remained attached to a loopback Leaf SOCKS session because the
    smoltcp AsyncWrite shutdown future was never woken after FIN submission.
    The snapshot was explicitly reverted by `07f59ec2`; the next candidate
    adds the missing wake/completion transition and a focused regression test.
12. The `66f208eb` beta passed the focused smoltcp shutdown and unilateral KCP
    drop regressions. Real traffic confirmed that HTTP fast EOF preserves the
    complete body, canceled hedge streams are reclaimed, and established TCP
    winners close after the Leaf-side stream finishes. A 15-second policy-TUN
    capture then identified the remaining periodic KCP entries as SOCKS UDP
    ASSOCIATE controls created by EasyTier's own STUN DNS queries. STUN data
    sockets already had `SO_MARK`, but libc/Hickory DNS sockets used before
    those connections did not. The next candidate supplies a marked Hickory
    runtime for EasyTier control-plane DNS and routes STUN hostname lookups
    through it while policy mode is active. User DNS still enters Leaf, and
    Magic DNS remains mesh-owned; no blanket port-53 bypass is added.
13. The `e664d9a5` beta proved the DNS separation: a 25-second capture saw zero
    control-plane DNS packets on the policy TUN and 32 marked STUN lookups on
    the physical interface, while an application query still traversed Leaf
    and returned FakeDNS, and Magic DNS still returned the mesh peer address.
    It also passed 100 immediate-close HTTP transfers with exact 2333-byte
    bodies and no retained proxy entry. UDP testing exposed an independent
    bridge defect: a strict SOCKS5 server associates UDP with the control TCP
    source IP. KCP proxy changed that control source to the destination peer's
    own virtual IP while mesh UDP retained the caller's virtual IP, so the
    standards-compliant relay discarded every datagram. Merely disabling KCP
    in the data-plane connector was insufficient because the later deferred
    selector upgraded its SYN again; that failed beta was reverted. The next
    candidate keeps KCP for ordinary CONNECT sessions, uses native mesh TCP for
    UDP ASSOCIATE control, and carries a non-serialized packet marker across the
    local proxy interception pipeline. The deferred selector and legacy
    KCP/QUIC source filters read the marker without consuming it; serialization
    drops it before the packet reaches the peer. It never sets wire `no_proxy`
    and does not skip ACL or unrelated NIC filters.

## Static review disposition

- Dynamic policy-route refresh: real and fixed.
- Magic DNS staying on the mesh path: real and fixed.
- Biased writer starvation: real and fixed.
- Loopback DNS stub addresses are always unusable: false as a general claim.
  The worker shares the host namespace, so local stubs such as systemd-resolved
  and Docker DNS are valid. Missing/unreachable resolvers remain runtime
  readiness or DNS failures rather than a reason to reject loopback addresses.

## Cross-platform boundary

EasyTier already owns a TUN on supported platforms; a second TUN library is not
required. The reusable layer is the policy document, classifier, Leaf rule
compiler, mesh SOCKS bridge, bounded lifecycle, and fail-closed supervisor.
Platform-specific work is limited to packet-boundary IPC and native underlay
bypass:

- Linux: implemented with netlink, fwmark, source rules, and flock.
- Android: reuse the existing VpnService TUN and protect EasyTier/Leaf outbound
  sockets with `VpnService.protect()`.
- macOS/iOS: reuse the existing utun/NetworkExtension owner and platform route
  exclusions/socket ownership.
- Windows: reuse the existing TUN backend and add an equivalent WFP/route
  bypass plus packet-boundary local IPC.

The Rust `tun2proxy` project is a useful reference for external-TUN, UDP session,
Android, Apple, and Windows lifecycle behavior, but should not be introduced as
a second TUN owner or replace Leaf's rule/group engine without an isolated
compatibility spike.
