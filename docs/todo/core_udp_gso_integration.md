# Linux Core UDP GSO integration draft

Status: DRAFT PR #9 / INTEGRATION VALIDATION INCOMPLETE / NOT DEPLOYED.
Base: c6772dbfef2395ff96b39bd4801945d92212dffb, checked against origin/codex/current.
Parent research: issues #4 and #8, experiment PR #7.

## Scope and activation

This draft automatically uses the private GSO sender on Linux, with writer-local
negative-submission fallback to existing async send_to. It adds no runtime flag,
public sender interface or dependency. Activation is a reviewable candidate policy,
not an approved production rollout. Non-Linux retains the original writer and
128-slot send ring. Receive queues and the shared MPSC are unchanged everywhere.

Only udp.rs's two send-ring/sink constructions and Linux writer dispatch change.
UdpConnection's AbortOnDrop task and close-event ownership are unchanged. No TUN,
routing/relay selection, protocol header, policy, GUI, TCP, QUIC or KCP changes.

The new private udp_gso.rs carries the measured staged sink and sender, without
experiment environment variables, namespace guards, Drop logging or file metrics.
Production writer state contains one disable bit; counters are test-only. The
32 staging + 64 ring + 32 writer packet bound replaces the old 128-slot ring.
No timer is added to wait for a larger batch. Each datagram is independently framed
and sealed; control/gate traffic is not combined into GSO groups.

## Failure checklist

- EAGAIN waits for Tokio readiness; EINTR retries without a busy-spin policy.
- Reviewed negative GSO rejection disables only this writer for its lifetime.
- MTU EMSGSIZE and ordinary-send failures propagate through existing ownership.
- Positive short/ambiguous submissions are errors, never whole-batch replay.
- Cancellation/owner drop adds no retransmission or guaranteed-delivery promise.
- Closed consumers retain existing error behavior; pending flush is bounded.
- Non-Linux code must not reference the Linux-only module.
- Test-only counter removal and experiment-code extraction need fresh compilation;
  past green experiment results do not validate this draft.

## Evidence and remaining gates

Prior actual-Core green: b0d88e17 / run 36209030843, nine contracts plus IPv4
fixed-load lab. Same unchanged regression with old sender: f2123fd3 / run
36211303677, eight PASS and one EINVAL failure. Lightweight actual-adapter:
0edb8894 / run 36213409344, nine cases covering real IPv4/IPv6 readiness,
writer-local fallback, second-writer isolation, MTU and ordinary-send errors.

Existing nine experimental contract tests are carried into the private module,
with experiment mode arguments/counters removed or test-gated. They have NOT
been executed against this integration draft. The imported regression's historical
red/green pair is evidence for the bug, not fresh integration acceptance.

Integration run 36214089572 at 738d48d8 failed the unchanged `-D warnings`
gate: Linux production retained an unused StreamExt import and a test-only
send_group counter parameter. The follow-up scopes the import to its consumers,
marks the test-only parameter accordingly, and places helpers before the test
module. No send behavior, fallback condition or test assertion changes. Local
formatting and the maintained pre-commit gate passed; fresh CI remains required.
The failed run remains failure evidence, not an infrastructure retry or a pass.

Next: one coherent integration review and focused GitHub compilation/tests before
any candidate deployment. Then one frozen exact-Core candidate for direct/relay,
mixed-version, lifecycle and dual-stack acceptance plus real performance evidence.
Do not infer original-host performance or cross-platform gains from namespace labs.
The TUN scratch optimization and known saturation receive-ring loss remain separate.
