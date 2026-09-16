# Issue 4 outbound UDP ownership experiment

This is an authorized isolated CI diagnostic, not production source or a proposed shipping feature. Source is pinned to c6772dbf. The queued and inline arms use the same rebuilt diagnostic binary, with stock bracketing.

Inline changes the outbound ring enqueue/dequeue and executing task: framing and UDP submission occur in the caller through a bounded one-pending-datagram sink. The original unused ring/task are retained as dormant controls. Thus the experiment does not claim to measure their allocation/resource footprint. The backpressure boundary changes too; interpret this as a combined ownership/scheduling intervention, not a pure syscall or CPU-function ablation. Input/receive rings remain unchanged. No metrics, encryption, ACL, route or wire switch is removed.

Only the static two-peer isolated fixture is supported. Tokio 1.52.1 poll_send_to retains one send-side waker, so this direct implementation is not valid as a general multi-peer shared-socket replacement. Its registration.poll_write_io still consumes Tokio cooperative budget. Production adoption would require a per-socket writer owner or safe multi-waiter readiness plus full control/pressure/rotation coverage. No diagnostic binaries are published as release assets.

Five focused tests cover pending ownership, short submission rejection, errors, real UDP bytes/close, and cancellation. Integration covers three interleaved queued/inline repetitions, stock before/after, both directions, fixed load, saturation with ICMP progress, sparse UDP, content/EOF and scoped cleanup. A failed ping, process stop or transfer remains a failed gate. The new tests are not claimed passed before workflow completion.

## Deterministic packet fixture correction

The first ownership artifact (10428100940, source a658039a) contains successful
stock/probe builds followed by four passing tests and one failed wire test;
it contains no integration A/B results. ZIP SHA256 is
`1cf199cc2ae1dbfe270fa944537bb4a781ca85c0a3fb5864f3918555a4f0a648`.
The differing byte is UDP header offset 5 (`padding`): actual 0 versus expected
130. The test created two packets through `new_with_payload`, which leaves
header bytes uninitialized, and neither framing path sets padding.

Test packets now initialize the entire storage before constructing ZCPacket.
The expected plaintext datagram is a fixed independent wire vector, including
padding/reserved bytes; it no longer calls the candidate frame function.
Cancellation-test packets use the same initialized input helper. Full byte
comparison, deadline, closed-state and exactly-once notification assertions
remain unchanged. The production framing, security, sink behavior, workload,
features and CI pass/fail gates are unchanged. This corrects diagnostic inputs,
not the inherited production constructor; compilation/runtime evidence for the
corrected fixture must be recorded separately after execution.

The first local preparation check used the wrong local temporary root and failed with FileNotFoundError before changing a repository file; the corrected path passed the exact-source guards. Local Python AST and workflow YAML checks passed. No local Rust compilation or full repository pre-commit was available. Existing release workflows and production branches/hosts are untouched; only this exact branch/path workflow runs. This is one coherent test batch under the maintainer's later explicit CI authorization, not the old builder policy or release pipeline.
