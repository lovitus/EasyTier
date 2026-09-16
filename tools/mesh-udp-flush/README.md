# UDP feed/flush diagnostic candidate

This is an exact-source, runner-only experiment for issue #4, not production
code or a shipping feature. The source checkout is pinned to c6772dbf. No
PeerManager, routing, TUN, receive, global MPSC or generic RingSink code changes.

The existing Core MPSC driver already feeds ready packets and then flushes.
TCP preserves this boundary in FramedWriter; UDP publishes every ring item.
This experiment compares legacy, staging-only, and staging plus UDP_SEGMENT
in one binary, with unmodified stock brackets. It neither sleeps nor yields
to manufacture a batch. Actual Core batch histograms are required.

For staged modes the outbound budget is 32 staged + 64 ring + at most 32
writer-owned packets, instead of the old 128 ring + one writer packet. The
unchanged MPSC adds 32 in either case. Framing releases plaintext ownership
as each wire frame is formed; sent payloads are cleared before idle waits.
The inbound ring and its reserved control slots are untouched. Stage-only
has lower active buffering than the full candidate; do not call it a pure
single-instruction ablation or claim identical backpressure timing.

The original UDP send task/close-event owner is retained. Control/gate-phase
frames use single sends. Each data packet keeps its own framing and seal;
eligible equal-sized datagrams may end with one short tail. UDP_SEGMENT uses
scatter/gather, without concatenating payloads or changing MTU/PMTU settings.
Raw C pointers exist only inside a synchronous Tokio async_io closure, not
across await. This retains asynchronous shared-socket readiness rather than
the single-waker poll_send_to pattern. Unsupported GSO or a short GSO return
fails the diagnostic; there is no hidden fallback that could fake activation.

The tests cover independent initialized wire bytes, ready-group publication,
backpressure/order/close, consumer closure, control/tail boundaries, per-packet
Gate/Outer sealing, IPv4/IPv6 kernel output and two destinations on a shared
socket. They do not by themselves prove forced kernel EAGAIN, key rotation,
full handshake timing, all multi-peer/relay paths, or production acceptance.
Those boundaries remain explicit runtime/review gates.

No test, compilation or gain is claimed until the corresponding evidence
exists. Full-Core performance, memory, control progress and cleanup must be
measured against the same-binary legacy arm and stock bracket. Keep all old
failed ring-drain/inline/GRO results; this tests a different publication point.

The dedicated workflow uses Ubuntu 22.04 GNU builds so the artifacts can also
run on the authorized older-glibc lab machine. It builds stock, applies only
the exact-source udp.rs diagnostic overlay, builds the same-binary arms, and
runs all seven contract tests before runtime. No release workflow is involved.
The lab uses stock brackets, three interleaved repetitions per diagnostic arm,
both directions, real CLI transport checks, independent TCP digest/half-close,
UDP echo, ICMP progress, native-TUN/MTU/device-feature evidence, process and
whole-host CPU, fresh configuration/HOME directories, and exact route cleanup.
Additional explicit secure-mode/Stealth rounds must report actual outer-phase
activation; setting an environment variable is not sufficient evidence.

Each diagnostic Core receives an otherwise empty, per-round directory under its
temporary `--config-dir` through `ET_ISSUE4_FLUSH_METRICS_DIR`. Sink and writer
destructors write unique JSON records there, while stderr retains the same
human-readable records. `lab.py` accepts only the dedicated files as activation
evidence: a missing, unexpected, or malformed record is an observation failure.
This removes shutdown stderr truncation from the evidence path without adding a
runtime facility, persistent state, or production logging behavior.

For a narrowly scoped external observation, `lab.py --order gso` runs only the
requested comma-separated non-Stealth arms while retaining the same functional
checks and cleanup. It writes each round's two Core PIDs before the startup
wait so an external tracer can attach only to those processes. A tracer may
prove that the candidate reached the `sendmsg` GSO path, but its CPU and rate
figures are not performance evidence because ptrace changes scheduling and
syscall cost.

Kernel EAGAIN forcing, saturation/relay/mixed-version acceptance and a shipping
fallback policy remain outside this first experiment, not silently PASS. The
fixed-load result must first justify further work. All unsupported-GSO errors,
missing batch counters, failed control checks or cleanup errors fail the run.
