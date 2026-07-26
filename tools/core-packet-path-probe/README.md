# Core packet-path probe

This Linux-only Rust probe evaluates experimental scheduling and buffer
mechanisms before they are allowed into EasyTier's production packet path. It
uses the same pinned `tun-rs 2.8.7` `GROTable` as
`LinuxTunOffloadSink`, valid sequential IPv4/TCP packets, the production mpsc
capacity of 32, and a current-thread Tokio runtime.

It is not a functional EasyTier test and it does not justify changes to legacy
TUN, Windows, macOS, BSD, Android, mesh routing, or transport selection.
Successful results only authorize a small Linux offload experiment; an exact
artifact still needs real-network functional, performance, and resource
validation.

Run on the dedicated builder:

```sh
cd /workspace/tools/core-packet-path-probe
cargo build --locked
target/debug/core-packet-path-probe \
  --packets=100000 \
  --payload-bytes=1360 \
  --route-work=64 \
  --rounds=2 \
  --scratch-capacity=0
target/debug/core-packet-path-probe \
  --packets=100000 \
  --payload-bytes=1360 \
  --route-work=64 \
  --rounds=2 \
  --scratch-capacity=65545
```

Validated on `192.168.2.160`:

- Production-like 4096-byte buffers coalesced a 32-packet input batch only in
  pairs; the largest GRO frame was 2770 bytes.
- One reusable 65545-byte scratch head coalesced the same ordered 32-packet
  batch into one 43570-byte frame.
- Current per-packet scheduling still delivered one packet per GRO call.
- A writer-side yield delivered only two packets per GRO call and is rejected.
- The scratch model adds one bounded buffer and copies only one TCP packet for
  batches containing at least three packets. It does not enlarge every packet.

The probe deliberately does not use `writev`: TUN preserves packet boundaries
per `write`, so generic vectored writes are not a valid batching mechanism.
