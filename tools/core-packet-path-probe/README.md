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

Run through the existing `Mesh natural-cohort kernel mechanism` workflow with
`capacity_only=true`. Builds/tests do not run on the maintainer's Mac or private
builders. The following commands describe the historical scheduling experiment,
not a current instruction to use a private builder:

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

Historical synthetic results (not an exact-Core receive-buffer measurement):

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

## Bounded head contract

`--capacity-contract` also exercises the existing 8 KiB head mechanism using
the Core's exact `bytes 1.9.0` and `tun-rs 2.8.7`. It calls the real `handle_gro`
and `gso_split`, checks the actual emission list and compares all reconstructed
IP bytes, including checksum/flags/sequence and multiplicity. Out-of-order
cohorts are compared as packet multisets; this does not claim cross-flow order
preservation beyond the library's existing GRO behavior.

The same one-write predicate is false without promotion and true with the
bounded head. A prepend fixture has two equally large allocations and proves
that recovering the original slot would select the wrong allocation. IPv4/IPv6,
cohort sizes through 128, shared receive slabs, flags, invalid checksums, short
tails, oversized packets, a GRO error and 1000 head reuses cover the selected
ownership boundary. No buffer allocation is enlarged during GRO.

This is a standalone API/mechanism contract, not a production regression test,
kernel partial-write test, runtime cancellation test, memory-leak proof or new
performance result. The async sink's existing partial-write/error/cancellation
semantics are audited separately in `../mesh-udp-flush/PACKAGED.md`. A future
production patch still needs exact-artifact acceptance.
