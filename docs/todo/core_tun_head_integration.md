# Bounded Linux mesh TUN head integration

Status: IMPLEMENTED, NOT YET COMPILED OR EXECUTED. Not merged or released.

Base: `3a1f3d9fc840a37a7649448a042bf44035bfaf7b` (frozen UDP GSO candidate,
PR #9). This separate branch adds no UDP sender changes and does not relabel
that candidate's existing evidence as evidence for this new tree.

## Evidence and scope

- Exact packaged profiles put Linux TUN delivery, including kernel work, at
  approximately 33-34% of candidate samples. This is not a new throughput claim.
- Previous actual-Core matched-rate experimentation found an additional 18.4%
  CPU/GiB reduction with one 8 KiB head on top of GSO. It used a different
  artifact/workload; integration must reproduce the gain before acceptance.
- [Locked head contract run 36230603039](https://github.com/lovitus/EasyTier/actions/runs/36230603039)
  passed 40 IPv4/IPv6 packet-byte cases, allocation-identity controls and 1000
  reuses using Core's bytes 1.9.0 and tun-rs 2.8.7. Full evidence is in
  [issue #4](https://github.com/lovitus/EasyTier/issues/4#issuecomment-5844719667).
- Upstream tun-rs 2.8.11 changes read-side GSO output validation, not this
  write-side head-capacity constraint. No dependency upgrade is bundled.

Only the existing Linux offload sink changes. It owns one reusable 8192-byte
head, promotes at most one eligible already-queued TCP frame, and recovers the
same allocation after the existing stored write future returns. The future
type, send_multiple call, error propagation and queue capacity are unchanged.
There is no timer, wait-for-batch, global pool, new config, protocol/route change,
or change to legacy/non-Linux TUN, the reader, Leaf, Mihomo, KCP or QUIC.

The bound is intentional: at 1320-byte payloads, 128 input packets still require
123 library emission entries. This is a small-cohort optimization, not a claim
that one head eliminates the entire bandwidth/CPU bottleneck.

## Failure modes and tests

- Corrupted/lost/duplicated packet content: real GRO emission list and GSO
  reversal compare complete IPv4/IPv6 bytes and multiplicity.
- Incorrect head recovery after prepend or equally large buffers: allocation
  identity, not original slot/capacity, selects the head.
- Growth and aliasing: capacity/pointer assertions, repeated reuse and a live
  shared receive-slab tail cover the new ownership boundary.
- GRO failure: retain InvalidInput and recover the head after the error.
- Partial kernel write then EINVAL: real namespace-isolated TUN accepts the
  valid cohort before the invalid IP frame; subsequent flush/close must not
  replay it. Disabling only the head is the original-capacity negative control.
- Caller cancels a pending flush: real down TUN keeps the stored future pending;
  bringing it up must finish once, and dropping the entire sink must release
  the device without a detached task.
- Unexpected missing allocation identity: disable promotion for that sink,
  retaining ordinary packet-buffer behavior without repeated allocations.

Pure contract tests include the no-head and wrong-slot negative controls.
They do not claim that every preserved behavior is newly broken on the old
baseline. The new production tests, including their actual failure sensitivity,
must be executed before any passing-test claim. Real-TUN tests require Linux
CAP_NET_ADMIN/CAP_SYS_ADMIN and use a new thread-owned anonymous network
namespace; missing permission is a failure, not a silent skip.

## Validation and current cursor

- Local Rust formatting/pre-commit: pending.
- Existing GitHub Test workflow, exact new SHA: pending; no workflow edits.
- Exact optimized artifact A/B against unchanged `3a1f3d9f`: pending.
- Recheck CPU/GiB at matched rates, natural cohort/write counts, throughput,
  IPv4/IPv6, integrity/half-close, UDP/ICMP, mixed-flow overload, idle/lifecycle
  and cleanup. Existing research tools/artifacts are reused, not rebuilt blindly.
- Original-host/WAN and long-duration resource evidence remain open. No claim
  that namespace tests prove every platform or solve cluster-wide performance.
- Unique next step: format/pre-commit and run the existing Test workflow once
  for the complete production/test/documentation batch. Preserve the first
  failure, diagnose it, and do not relax assertions or rebuild PR #9.
