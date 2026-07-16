# [SUSPECT] Raw UDP Underlay Queue Drops and Packet-Rate Bottleneck

**Status**

- Suspect known bug. A code-level intentional drop point is confirmed; runtime attribution of all
  observed loss is not yet closed because neither the ring drop nor kernel UDP receive-buffer drop
  is counted in current production metrics.
- First recorded against exact profiling candidate `c48816f4300f5853525b62d5793d9778923aed80`
  on CentOS 7 / Linux 3.10 validation hosts.
- This issue is in the ordinary EasyTier mesh data plane. It reproduces with the no-Leaf binary and
  therefore is not, by itself, evidence of a Leaf, HEV, SOCKS, or policy-routing regression.
- Do not treat the existence of a QUIC peer connection as proof that a tested flow used QUIC. The
  selected `PeerConn` must be fixed or observed for each protocol-specific result.

**Observed evidence**

The strongest isolated sample used `--disable-p2p true` on both peers and one explicit
`udp://...` peer, while still assigning explicit non-default ports to every listener protocol.
The exact no-Leaf artifact showed:

- `50 Mbit/s` iperf3 UDP receiver loss of `17%`, `6.9%`, and `12%` across three runs.
- Two TCP iperf3 control-connection resets among six short runs on the same raw UDP underlay.
- Successful TCP runs around `685-721 Mbit/s`, so the failure is not a simple physical-link
  bandwidth ceiling.

Earlier `100 Mbit/s` samples reported roughly `19-29%` UDP loss, but those runs had multiple UDP
and QUIC peer connections and are retained only as a symptom. They are not protocol-isolated
evidence.

**Confirmed code-level drop path**

All ordinary TUN traffic is encoded as `PacketType::Data`. `ZCPacket::is_lossy()` returns true for
that packet type without distinguishing the inner IP protocol, so both inner TCP and inner UDP are
treated as lossy by the raw UDP tunnel:

- [`packet_def.rs`](../../src/tunnel/packet_def.rs#L740)

Each raw UDP connection creates two rings with capacity 128:

- [`udp.rs`](../../src/tunnel/udp.rs#L1188)
- [`ring.rs`](../../src/tunnel/ring.rs#L25)

Four entries are reserved. Once occupancy reaches 124, `RingSink::try_send()` rejects another lossy
packet:

- [`ring.rs`](../../src/tunnel/ring.rs#L120)

`UdpConnection::handle_packet_from_remote()` uses that non-blocking path for every ordinary Data
packet. A full ring logs only at trace level and drops the packet while returning success to the
outer receive loop:

- [`udp.rs`](../../src/tunnel/udp.rs#L397)

At the measured datagram sizes, 124 entries absorb only about 26 ms at `50 Mbit/s` and about 13 ms
at `100 Mbit/s`. A scheduler pause or downstream backpressure longer than that interval can create
an immediate burst of end-to-end sequence gaps.

**Drain-path bottlenecks**

The raw UDP listener and connector each use one `recv_buf_from().await` loop. The transmit side uses
one `UdpSocket::send_to().await` per packet. There is no raw-underlay `recvmmsg`, `sendmmsg`, or UDP
GSO batching in this path:

- [`udp.rs`](../../src/tunnel/udp.rs#L330)
- [`udp.rs`](../../src/tunnel/udp.rs#L774)

After the receive ring, one task per `PeerConn` reads one packet and awaits a send into the shared
NIC channel:

- [`peer_conn.rs`](../../src/peers/peer_conn.rs#L1542)

That shared NIC channel has capacity 128:

- [`peers/mod.rs`](../../src/peers/mod.rs#L82)

The NIC writer then drains the channel. Linux TUN offload can batch writes, but the PeerConn and
route pipeline still deliver packets through bounded, serial async stages:

- [`virtual_nic.rs`](../../src/instance/virtual_nic.rs#L1489)
- [`linux_tun_offload.rs`](../../src/instance/linux_tun_offload.rs#L130)

The reverse TUN-to-peer direction also processes one packet through classification, route lookup,
compression/encryption, and peer send before polling the next item:

- [`virtual_nic.rs`](../../src/instance/virtual_nic.rs#L1444)

Every `PeerConn` adds another capacity-32 MPSC queue serviced by one forwarding task:

- [`mpsc.rs`](../../src/tunnel/mpsc.rs#L59)

These bounded stages normally provide backpressure, but the raw UDP receive ring deliberately
converts that backpressure into packet loss for `PacketType::Data`.

**Socket-buffer boundary**

The generic UDP `bind()` path configures nonblocking mode, address reuse, interface binding, and an
optional mark, but does not set `SO_RCVBUF` or `SO_SNDBUF`:

- [`common.rs`](../../src/tunnel/common.rs#L544)
- [`common.rs`](../../src/tunnel/common.rs#L707)

The raw UDP tunnel therefore depends on host defaults. Kernel receive-buffer overflow is a second
plausible loss point, but current EasyTier metrics do not distinguish it from the confirmed
application-ring drop.

**QUIC underlay boundary**

QUIC is also an EasyTier underlay transport. It accepts/opens a reliable bidirectional QUIC stream
and wraps it in `FramedReader` / `FramedWriter`:

- [`quic.rs`](../../src/tunnel/quic.rs#L1352)

The QUIC writer buffers up to 64 frames before flushing the reliable stream:

- [`common.rs`](../../src/tunnel/common.rs#L349)

QUIC underlay does not use the raw UDP tunnel's 128-entry lossy ring. Under congestion it should
primarily expose retransmission, flow-control, and head-of-line latency. It still shares the
PeerConn, NIC channel, route pipeline, and TUN boundaries, so inner UDP can still be lost outside
the QUIC stream if the local TUN/kernel path overflows.

When multiple connections exist, `Peer` selects and caches a default connection based on transport
preference and latency. Tests must not infer the chosen underlay from connection-list presence:

- [`peer.rs`](../../src/peers/peer.rs#L267)

**Leaf boundary**

When policy routing is disabled, the policy context is absent and mesh packets fall through to the
ordinary `do_forward_nic_to_peers()` path:

- [`virtual_nic.rs`](../../src/instance/virtual_nic.rs#L1451)

The current evidence therefore supports a pre-existing raw UDP mesh bottleneck. It does not yet
show that the disabled Leaf feature materially changes this bottleneck.

**Runtime attribution update — 2026-07-16**

An isolated follow-up used the exact no-Leaf `c48816f4` artifact, one explicit raw UDP PeerConn,
`--disable-p2p true`, and distinct listener/test ports on `192.168.1.37` and `192.168.1.38`.
No production or unrelated validation process was reused.

The direct physical-underlay baseline from `.37` to `.38` was:

- `10 Mbit/s`: `0/8548` lost; receiver `RcvbufErrors` unchanged.
- `20 Mbit/s`: `0/17098` lost; receiver `RcvbufErrors` unchanged.
- `50 Mbit/s`: `42/42751` lost (`0.098%`); receiver `RcvbufErrors +42`.
- `100 Mbit/s`: `4163/85514` lost (`4.9%`); receiver `RcvbufErrors +4163`.

The direct `100 Mbit/s` result therefore has an independent iperf/kernel receive-buffer ceiling and
must not be used as a zero-loss EasyTier baseline. The physical path is effectively clean at
`20 Mbit/s` and nearly clean at `50 Mbit/s`.

With the same average rate carried through the isolated EasyTier raw UDP overlay:

- `20 Mbit/s`: iperf lost `433/18926` (`2.3%`); system `RcvbufErrors` increased by exactly 433.
  The EasyTier underlay socket's `ss -m` drop counter increased by about 55; the remaining roughly
  378 drops occurred at the final iperf UDP socket after TUN delivery.
- `50 Mbit/s`: iperf lost `4763/47321` (`10%`); system `RcvbufErrors` increased by about 4747.
  The EasyTier underlay socket accounted for about 965 drops; roughly 3780 occurred at the final
  iperf UDP socket.
- `50 Mbit/s` with iperf `--fq-rate 50M`: iperf still lost `4205/47321` (`8.9%`). System
  `RcvbufErrors` increased by about 4191; the EasyTier underlay socket accounted for about 540 and
  the final iperf socket for roughly 3650.

Both hosts reported `net.core.rmem_default=212992` and `net.core.rmem_max=212992`. In these runs the
iperf loss is almost completely explained by kernel UDP receive-buffer overflow. There is no large
unexplained remainder requiring an application-ring drop, so the 128-entry ring is not the primary
runtime cause for this candidate and topology.

The stronger root-cause hypothesis is **burst amplification**:

1. paced UDP enters TUN;
2. the generic PeerConn MPSC forwarding task drains queued packets in a tight loop;
3. the raw UDP send ring can accumulate another burst and its forwarding task emits back-to-back
   `send_to()` calls;
4. the receiver queues packets again and the Linux offload writer drains up to 64 packets before a
   TUN flush;
5. a smooth average rate is therefore delivered to both the EasyTier underlay socket and the final
   application socket as microbursts large enough to overflow their 212,992-byte receive buffers.

The locked TUN dependency is crates.io `tun-rs 2.8.7` with checksum
`ea75f145e8f32c72b1afdf137f2181810b0232be9930519e8d82071b4a3b3bdf`. Its Linux
`send_multiple()` performs GRO evaluation and then writes the resulting buffers back-to-back. On
the Linux 3.10 hosts UDP GSO is not expected to coalesce this flow, so batching primarily reduces
syscall scheduling overhead while preserving a burst of individual UDP packets into TUN.

This changes the fix priority. Enlarging the raw UDP ring alone is not a root fix and may increase
burst size. Enlarging only the EasyTier underlay socket cannot prevent the dominant final
application-socket overflow. The first implementation candidate should measure and bound burst
release at the raw UDP sender and TUN writer while preserving existing mesh routing and wire
semantics.

**Preferred minimal implementation candidate — not yet implemented**

The first candidate must address burst amplification rather than hide it behind larger queues. Its
scope should remain inside the raw UDP tunnel adapter and the Linux peer-to-TUN writer. It must not
change PeerManager routing, connection selection, mesh packet headers, Leaf, HEV, SOCKS, KCP,
smoltcp, or QUIC behavior.

1. Split the raw UDP send and receive ring capacities instead of using the same hard-coded 128 for
   both directions.
   - Keep the receive ring bounded and lossy with the existing reserved control-packet capacity.
   - Use an initial send-ring candidate of 16 or 32 packets. The send side uses the async `Sink`
     backpressure path, so a smaller send ring bounds the burst released by the dedicated
     `send_to()` task rather than intentionally dropping Data.
   - Do not increase the receive ring as part of this candidate. A larger ring can accumulate and
     later release a larger burst without increasing the sustained service rate.
2. Bound Linux peer-to-TUN UDP release independently from TCP.
   - Identify plaintext inner IPv4/IPv6 UDP after normal PeerConn processing and before TUN write.
   - Limit one tight UDP write burst to an initial candidate of 16 packets.
   - Preserve the existing 64-packet/GRO path for TCP so the UDP fix does not create a broad TCP
     throughput regression.
   - The bound must be implemented in the existing writer task; do not spawn one task per packet or
     introduce cross-flow reordering.
3. Tune only the raw UDP underlay sockets.
   - Apply explicit `SO_RCVBUF` and `SO_SNDBUF` to listener and connector sockets, including sockets
     supplied through the existing shared-socket path.
   - Reuse the existing project pattern that logs the kernel-accepted values. Start with a bounded
     2-4 MiB diagnostic range, subject to the host's `rmem_max/wmem_max`.
   - Treat this as protection for the minority of loss observed at the EasyTier underlay socket,
     not as the fix for final application-socket overflow.
4. Add low-overhead attribution counters before performance claims.
   - raw UDP datagrams received and sent;
   - raw UDP receive-ring full Data drops;
   - raw UDP socket receive/send errors;
   - peer-to-TUN UDP packets and bounded-burst yields;
   - TUN write failures.
   Counters should use atomics and power-of-two or interval-limited reporting. Per-packet tracing is
   not valid performance instrumentation.
5. Replace the current loss-blind UDP benchmark behavior.
   - Add a deterministic unit test that fills the receive ring and asserts the exact drop counter.
   - Add a bounded packet-count test that compares sent and received packet counts instead of only
     printing bytes per second.
   - Keep high-rate host performance tests outside ordinary unit-test timing assumptions.

The initial candidate constants are deliberately conservative and must be selected together:

```text
raw UDP receive ring: keep 128 for the first attribution candidate
raw UDP send ring:    test 16 and 32
Linux UDP TUN burst:  test 16
TCP TUN burst/GRO:    keep existing 64 behavior
UDP socket buffers:   test 2 MiB and 4 MiB, recording accepted values
```

**Candidate validation gates**

- Use exact no-Leaf and same-SHA Leaf-disabled artifacts; policy routing remains disabled.
- Establish one explicit raw UDP PeerConn with automatic P2P disabled and verify no QUIC/WG/TCP
  PeerConn participates.
- Run direct physical-underlay and overlay UDP at 10, 20, 50, and `100 Mbit/s` with the same
  datagram size.
- At 20 and `50 Mbit/s`, require overlay loss to approach the direct baseline and require final
  application-socket `RcvbufErrors` to stop showing multi-percent burst overflow.
- At `100 Mbit/s`, report the direct iperf/kernel receive-buffer loss separately; do not demand an
  impossible zero-loss overlay result from a receiver whose direct baseline already drops 4.9%.
- Record per-socket `ss -u -m` drops for the EasyTier underlay socket during the run and system
  `Udp: InErrors/RcvbufErrors` before and after each interval.
- Confirm TCP throughput remains within the established noise band because its 64-packet/GRO path
  is intentionally preserved.
- Confirm ordinary mesh ICMP/TCP/UDP behavior, shutdown cleanup, FD/thread baseline, and peer
  connection selection are unchanged.

**Explicitly rejected first fixes**

- Do not merely increase the raw UDP ring to 512/1024; this can enlarge the emitted microburst.
- Do not make the queue unbounded; this turns overload into memory growth.
- Do not make the shared listener await one slow connection's receive ring; this creates
  head-of-line blocking across peers and only moves loss back into the kernel socket.
- Do not add `recvmmsg/sendmmsg` as the first change. Batch syscalls can improve sustained PPS but
  may worsen the confirmed burst-overflow mechanism if release is not bounded first.
- Do not globally disable Linux TUN offload; TCP must retain its existing GSO/GRO performance path.
- Do not force QUIC/KCP/UoT, or route ordinary mesh UDP through Leaf/HEV/SOCKS, as a workaround.

**Conditional follow-up after the minimal candidate**

Only if counters show a sustained service-rate ceiling after burst loss is controlled should a
second candidate add platform-specific `recvmmsg/sendmmsg` or equivalent batching. That batching
must sit behind the raw UDP adapter, preserve packet order, retain a cross-platform Tokio fallback,
and keep an explicit burst bound. It must not widen the first candidate before the smaller fix is
measured.

**Fixed-underlay comparison on `c48816f4` (2026-07-16)**

The no-Leaf artifact was run between the isolated `.37` and `.38` instances with automatic P2P
disabled, exactly one explicit PeerConn, 1300-byte iperf3 UDP datagrams, and 10-second intervals.
Only the selected underlay changed. This is diagnostic evidence, not a release benchmark.

| Underlay | 20 Mbit/s | 50 Mbit/s | 100 Mbit/s | Receiver attribution |
| --- | ---: | ---: | ---: | --- |
| direct physical UDP | 0% | 0.098% | 4.9% | At 100 Mbit/s the host itself added 4163 `RcvbufErrors`. |
| EasyTier raw UDP | 2.3% | 10% | previously observed 25-29% range | At 20 and 50 Mbit/s nearly all loss matched receiver `RcvbufErrors`; both the EasyTier underlay socket and final iperf socket overflowed. |
| EasyTier QUIC | 1.1% | 3.0% | 26% | Kernel `RcvbufErrors` deltas were 202, 1412, and 2636. The QUIC socket recorded no drops at 20/50 Mbit/s and 241 cumulative drops by the end of 100 Mbit/s. At 100 Mbit/s, 22048 of 24684 sequence gaps were not explained by kernel receive-buffer overflow. |
| EasyTier WG | 0.97% | 3.6% | 34% | Kernel `RcvbufErrors` deltas were 184, 1727, and 11363. The WG underlay socket accumulated 3064 drops across all three runs. At 100 Mbit/s, at least 21256 of 32619 sequence gaps occurred outside the receiver kernel overflow count. |

QUIC and WG therefore reduce the 20/50 Mbit/s loss relative to the current raw UDP path, probably
because their transport processing changes packet release and pacing. They do not remove the
receiver burst boundary. At 100 Mbit/s both are substantially worse than the 4.9% direct baseline,
and most of their sequence gaps are not attributable to receiver socket overflow. WG is not a
performance workaround; QUIC is at most a conditional low/mid-rate mitigation and cannot be made
the default on this evidence.

This comparison strengthens, rather than replaces, the minimal candidate above. The raw UDP send
burst, Linux peer-to-TUN UDP burst, and socket buffers remain the narrowest first scope. QUIC/WG
need their own counters before their high-rate loss can be assigned to congestion control,
userspace queues, decrypt/dispatch service rate, or TUN release. Ordinary mesh routing and underlay
selection must not be changed merely to hide the raw UDP result.

**Unresolved questions**

- What fraction of loss occurs in `RingSink::try_send()` versus the kernel UDP receive queue?
- Does TUN GSO/GRO initialize successfully on both validation hosts for the tested artifact?
- What is the stable packet-rate ceiling for raw UDP and QUIC underlay at 10, 20, 50, and
  `100 Mbit/s` with one fixed connection?
- Does increasing only the ring/socket buffers reduce transient loss while leaving the sustained
  packet-rate ceiling unchanged?
- Is the TCP control reset caused by dropping inner TCP Data at the raw UDP ring, or by another
  downstream TUN/socket boundary?
- Are stealth AEAD and normal per-packet encryption significant contributors once syscall batching
  and drop counters are controlled?

**Required follow-up evidence**

1. Add separate counters for raw UDP receive-ring full drops, parse/auth drops, kernel UDP receive
   errors where available, and TUN write failures. Do not use trace logging as a performance
   counter.
2. Record `Udp: InErrors` and `RcvbufErrors`, socket queue state, TUN offload status, CPU, packet
   rate, and EasyTier ring-drop counters in the same bounded interval.
3. Run fixed-underlay matrices for raw UDP, QUIC, and WG at 10, 20, 50, and `100 Mbit/s`; use one exact
   no-Leaf artifact first, then the same-SHA Leaf-disabled artifact.
4. Compare the current path with larger socket/ring buffers only as a diagnostic. A larger queue is
   not a complete fix for a sustained per-packet processing ceiling.
5. Evaluate raw UDP `recvmmsg`/`sendmmsg` or equivalent batching separately from queue sizing.
6. Preserve existing mesh protocol semantics. Do not route ordinary mesh traffic through Leaf,
   HEV, SOCKS, KCP proxy, or QUIC proxy as a workaround.

**Temporary interpretation rule**

Until the counters above exist, any high-rate UDP result must report the exact selected underlay and
must be labeled inconclusive for Leaf regression if the no-Leaf baseline also loses packets. Loss
from a saturated raw UDP baseline cannot be assigned to policy routing or a sidecar by subtraction.
