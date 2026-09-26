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

## Executed result: 0edb8894

Run `36213409344` completed SUCCESS on
`0edb8894aca9031628463a5e6054261a45704ffb`, without compiling Core.
Artifact `10896781424` is 24,477 bytes; its complete ZIP SHA-256 was verified:
`b91454e24fb3a42ce4ce755f30e6f885a892bec6148d1f14e94340320a094fdd`.
The archived source SHA matches the candidate.

All nine actual-adapter cases executed and passed:

| Family | Case | Observed result |
| --- | --- | --- |
| IPv4 | recover / cancel / shared | actual EAGAIN 1 / 1 / 2; timer ticks 192 / 191 / 301; ordered payloads |
| IPv6 | recover / cancel / shared | actual EAGAIN 1 / 1 / 2; timer ticks 195 / 194 / 305; ordered payloads |
| IPv4 | MTU rejection | raw GSO, adapter, ordinary send all errno 90; no downgrade; smaller group succeeds |
| IPv6 | MTU rejection | raw GSO, adapter, ordinary send all errno 90; no downgrade; smaller group succeeds |
| IPv4 | checksum rejection | raw errno 22; one persistent local downgrade; eight ordered datagrams; ordinary oversize remains errno 90; second writer sends four datagrams via GSO |

The checksum case reports exactly one capability fallback for the first writer
and one GSO call for the unaffected second writer. The two prior GSO calls on
the first writer are the explicit raw rejection control and the adapter's first
rejected group, not two successful submissions. Restoring checksum support does
not re-enable that writer within its lifetime. The second receiver verifies its
own full payload/sequence, not just the sender's counters.

All 12 receiver processes exited zero with no forced termination. Both namespaces
were removed with no residual PIDs. All nine qdisc snapshots report zero drops.
The existing harness's root-route invariance check passed. This proves bounded
adapter behavior on this runner, not full Core owner-drop/relay/endurance or
physical cross-host performance. No new production implementation was added.

The previously verified Core red/green pair remains the rejection-regression
control. This expanded fixture has a green execution; it was not itself separately
run against the old sender, and must not be described as a second independent
red/green pair. The historical MTU run remains FAIL under its incorrect assertion.
