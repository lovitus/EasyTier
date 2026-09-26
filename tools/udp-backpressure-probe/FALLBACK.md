# Writer-local fallback compatibility batch

This fixture imports the actual experimental `group_end`, `send_group`, and
`send_frames` functions verbatim through its existing build script. No production
Core or workflow is changed by this batch. It does not simulate the sender.

The adapter at b0d88e17 added writer-local downgrade fields and a tracing call;
the fixture containers and pinned tracing dependency now match those requirements.
Rust/Tokio remain 1.95/1.52.1. This is a small test binary, not a new Core build.

## Failure contracts and unchanged coverage

- Keep the six IPv4/IPv6 real EAGAIN, pending-future cancellation and shared-socket
  cases unchanged, including timer progress, bytes/order and cleanup checks.
- Characterize raw GSO rejection independently through `send_group`.
- For MTU rejection, expect EMSGSIZE on this Ubuntu runner. This intentionally
  corrects the old EINVAL assertion: diagnostic-only run 35699982270 preserved
  actual errno 90, and the distribution includes upstream fix
  235174b2bed88501fda689c113c55737f99332d8. The old failing run remains FAIL.
  Do not accept arbitrary errno values or add EMSGSIZE to the fallback allowlist.
- Require both adapter GSO and ordinary oversized sends to preserve EMSGSIZE,
  without disabling GSO. A smaller group on that socket must still succeed.
- For IPv4 checksum-disabled EINVAL, require one writer-local downgrade, exact
  ordered delivery and no repeated GSO attempt even after checksums are restored.
- An oversized ordinary datagram after downgrade must still return EMSGSIZE and
  must not increment delivered-packet accounting.
- A new writer to the second destination on the same socket must still send a
  GSO group, with no inherited disabled state or fallback count.

The two receiver processes in the checksum case validate independent sequences
and full payloads. Existing bounded namespace/qdisc/route cleanup assertions stay.
The nine cases remain nine cases; this expands their explicit behavioral checks,
not a mock framework or source-string-matching test.

## Evidence status

The actual-Core regression has already been checked with an unchanged test:
negative control f2123fd3/run 36211303677 executed eight passing contracts and
one EINVAL failure; fixed b0d88e17/run 36209030843 passed all nine. This does NOT
pre-validate the expanded lightweight cases. Their execution remains pending.

Local work is limited to formatting, lockfile generation and pre-commit checks;
compilation/execution uses the existing push-triggered
`udp-backpressure-probe.yml`. Do not attempt workflow_dispatch on that workflow,
or rebuild Core to obtain these fixture results. No merge, deployment, release,
physical-host performance or full Core lifecycle acceptance is implied.
