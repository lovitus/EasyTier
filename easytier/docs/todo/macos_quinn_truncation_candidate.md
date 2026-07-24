# macOS Quinn truncated-datagram spin candidate

## Pre-build candidate manifest

Base commit: `6d5fa34fd9d5cefc79459b1b05645771c0faf26`

Intended build snapshot:

- patch the exact locked `quinn-udp 0.5.15` source through
  `third_party/quinn-udp`;
- in the Darwin/BSD single-datagram receive path, reset `msghdr` before every
  `recvmsg`, handle syscall errors before inspecting output flags, and discard
  `MSG_TRUNC` only after a successful receive;
- add a macOS regression test which queues a truncated datagram and uses a
  delayed rescue packet so the historical loop fails deterministically without
  hanging the test process;
- remove the superseded EasyTier macOS Quinn runtime/backoff from
  `easytier/src/tunnel/quic.rs` and restore Quinn's standard runtime.

Root-cause evidence:

- installed `3.0.5-6d5fa34f` on macOS reproduced two full-CPU Tokio workers;
- a bounded three-second sample placed both workers in `recvmsg`;
- offline disassembly at the sampled return address showed
  `recvmsg -> test MSG_TRUNC bit -> retry -> test syscall error`, matching the
  locked `quinn-udp 0.5.15` slow Apple receive loop exactly;
- removing and recreating the QUIC Brutal listener destroyed the affected
  sockets and temporarily cleared the spin; starting the instance reproduced
  it;
- EasyTier's outer backoff could not run because the dependency receive call
  never returned EAGAIN.

Remote `.160` gate:

- synchronize this complete worktree with `scripts/leaf-remote-preflight.sh`;
- run its locked no-run EasyTier library build and focused QUIC/policy suite;
- additionally run a locked no-run build and the `quinn-udp` integration tests
  available on Linux;
- inspect `Cargo.lock`, the crates.io path patch, platform cfgs, generated
  files, and the complete candidate diff.

Completed pre-push evidence:

- the final complete snapshot passed `scripts/leaf-remote-preflight.sh`;
- the locked no-run build compiled EasyTier, Policy, SOCKS egress, netstack,
  Quinn, and the vendored `quinn-udp`;
- every focused test selected by the standard preflight passed;
- `cargo test --locked --no-run --package quinn-udp --test tests` succeeded;
- the exact Linux integration-test binary passed all eight available tests;
- the macOS-only truncated-datagram regression is compiled and executed by the
  required macOS ARM64 workflow with a five-minute step timeout.

Required workflow:

- manually dispatch only `.github/workflows/gui-macos-aarch64-test.yml`
  (`EasyTier GUI macOS ARM64 Test`) for the exact pushed commit;
- do not dispatch Core, GUI matrix, Mobile, OHOS, full Test, tag, or Release.

Planned macOS evidence from the immutable artifact:

- verify the workflow's macOS-only truncated-datagram regression;
- verify arm64 architecture and signatures from the workflow;
- after artifact deployment, reproduce the previous QUIC Brutal lifecycle and
  malformed/truncated UDP trigger while observing bounded CPU and mesh
  continuity.

Wait-time tasks:

- during `.160`, review the vendored crate diff against registry
  `quinn-udp 0.5.15` and confirm no unrelated dependency resolution;
- during GitHub build, prepare the bounded real-device trigger and cleanup
  commands without changing the in-flight snapshot.

## Post-build evidence

Record workflow run ID, exact commit, artifact checks, and real-device results
here only after they exist. Updating this section must not trigger another
build.
