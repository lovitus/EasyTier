# Release Candidate Validation: b019c33c

## Scope

- Commit: `b019c33ca85abd8d2359cca6d12d3f721181c700`
- Artifact: `easytier-profiling-beta-linux-x86_64-musl.tar.gz`
- Artifact SHA-256: `9da13c71f5c6c24e45a2e2fc791b1790ea4b4a3453714fda9bb7bade745161a3`
- Core build ID: `fec864c819273cbb1f44d9cea66598e061f4e048`
- Target: `x86_64-unknown-linux-musl`
- Build: optimized profiling build with symbols, not stripped
- CI: `EasyTier Test` and `EasyTier Linux Profiling Beta` passed for the exact commit.

The validation uses isolated network names, virtual addresses, TUN names, RPC ports,
SOCKS ports, and listener ports. Public host names and addresses are intentionally not
recorded in this document.

## Test Hosts

| Role | Kernel | CPU | RAM | Network |
|---|---:|---:|---:|---|
| LAN-A | Linux 3.10 | 2 cores | 3.7 GiB | high-speed LAN |
| LAN-B | Linux 3.10 | 2 cores | 3.7 GiB | high-speed LAN |
| Builder/Lab | Linux 3.10 | 8 cores | 32 GiB | LAN |
| WAN-A | Linux 4.19 | 1 core | 1 GiB | public network |
| WAN-B | Linux 5.4 | 1 core | 1 GiB | public network |
| WAN-C | Linux 5.15 | 1 core | 2 GiB | public network |

The outer bundle size, outer SHA-256, inner `SHA256SUMS.txt`, `BUILD_INFO.txt`,
commit, target, binary type, and build ID were verified before deployment. All six
hosts reported `easytier-core 2.6.9-b019c33c`.

## LAN Full-Stealth Baseline

Configuration:

```text
stealth_mode=true
stealth_protocols=udp,tcp,faketcp,quic,wg,ws,wss
transport_priority=global:quic,faketcp,ws,wg,udp,tcp
enable_kcp_proxy=true
enable_quic_proxy=true
```

Both nodes advertised explicit UDP, TCP, QUIC, WG, WS, FakeTCP, and WSS listeners.
LAN-B bootstrapped through TCP only. About four seconds after bootstrap, direct
discovery established QUIC and the peer data plane reported `quic,tcp`, route cost
`1`, RTT about `0.46 ms`, and zero observed loss. This confirms that Stealth
capability negotiation and QUIC preference work without requiring a QUIC bootstrap
URL.

### Throughput and Resources

| Path | Streams | Receiver throughput | Retransmits |
|---|---:|---:|---:|
| Physical LAN | 1 | 11.105 Gbit/s | 0 |
| Physical LAN | 4 | 14.914 Gbit/s | 0 |
| EasyTier QUIC + full Stealth | 1 | 518.1 Mbit/s | 2637 |
| EasyTier QUIC + full Stealth | 4 | 531.4 Mbit/s | 1748 |

During the four-stream overlay run, LAN-B consumed about `169-172%` CPU on a
two-core host. Its RSS high-water mark was about `53 MiB`, then fell back to about
`26 MiB`; FD count remained `31`. LAN-A consumed about one core, with RSS below
`19 MiB` and FD count `31`. The result is CPU-bound on the two-core sending side;
adding streams does not materially improve throughput.

## SOCKS KCP-Only

LAN-B exposed a SOCKS5 portal while both KCP Proxy and QUIC Proxy were enabled. A
64 MiB random file was fetched through SOCKS from LAN-A's virtual IP:

| Result | Value |
|---|---:|
| HTTP status | 200 |
| Bytes | 67,108,864 |
| Duration | 1.185 s |
| Average payload rate | 56.6 MB/s (about 453 Mbit/s) |
| SHA-256 | matched source |
| FD before / 8 s after | 31 / 31 |
| RSS before / 8 s after | 19.5 / 21.3 MiB |

No persistent FD growth was observed after the transfer.

## Failover and Failback

With QUIC active, both QUIC listener paths were blocked at the firewall while a
5 Hz overlay ping was running. The peer remained P2P and eventually changed from
`quic,tcp` to `tcp,faketcp_linux_bpf`. The complete ping result was 150 transmitted,
103 received, and 31% loss. Forty-seven missing samples correspond to an outage of
about 9.4 seconds before the existing TCP backup carried traffic. RTT after recovery
was approximately `0.4-0.6 ms`. FakeTCP was also established after QUIC failed.

This is a high-availability concern: retaining a TCP backup does not currently make
underlay failure switching immediate because the failed default QUIC connection is
not invalidated until its liveness timeout expires.

QUIC had not returned after more than 75 seconds, but did return after the existing
300-second direct-candidate blacklist expired. Code inspection confirmed that a
normal reachability failure and a high-confidence self-loop signal shared the same
300-second timeout. This is a failback defect rather than a permanent loss of QUIC.
The follow-up candidate keeps the 300-second self-loop safety timeout while reducing
only the ordinary reachability-failure cooldown to 30 seconds. The same firewall
fault must be repeated against the follow-up artifact before release.

## Pending Matrix

- QUIC failback within the new ordinary-failure cooldown after firewall recovery.
- Sequential QUIC, FakeTCP, WS, WG, UDP, and TCP fallback/failback.
- QUIC Proxy to KCP Proxy to Native behavior.
- Wrong-secret strict listener behavior and mixed-version compatibility.
- Process kill/restart, stale TUN cleanup, connection recovery, and resource baseline.
- Public-network three-node relay to P2P upgrade and WAN protocol preference.
- Longer SOCKS/KCP short-connection and sustained-transfer resource tests.
- Plain, derived Stealth, explicit Secure, and Stealth + Secure performance matrix.
