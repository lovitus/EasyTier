# AtomicU64 32-bit Target Release Gate

Status: permanent release-blocking gate from v3.0.9 onward.

## Why this gate exists

Five EasyTier Core runs failed on MIPS or MIPSel in July 2026 because x86_64
preflight did not exercise the 32-bit implementation of 64-bit atomics:

| Date | Core run | SHA | Target | Failure |
| --- | --- | --- | --- | --- |
| 2026-07-02 | [28558306376](https://github.com/lovitus/EasyTier/actions/runs/28558306376) | `83081bb9` | MIPSel | `gateway/proxy_failover.rs` imported `std::sync::atomic::AtomicU64` (`E0432`). |
| 2026-07-11 | [29147596757](https://github.com/lovitus/EasyTier/actions/runs/29147596757) | `8b212f2f` | MIPS | `tunnel/stealth.rs` imported `std::sync::atomic::AtomicU64` (`E0432`). |
| 2026-07-28 | [30373790096](https://github.com/lovitus/EasyTier/actions/runs/30373790096) | `1d999fd7` | MIPS | `common/network.rs` imported `std::sync::atomic::AtomicU64` (`E0432`). |
| 2026-07-28 | [30407863378](https://github.com/lovitus/EasyTier/actions/runs/30407863378) | `273085da` | MIPS | The same code used `atomic-shim`, but its non-native-64-bit implementation did not provide `fetch_max` (`E0599`). |
| 2026-07-30 | [30503244034](https://github.com/lovitus/EasyTier/actions/runs/30503244034) | `37fef1b4` | MIPSel | `common/p2p_endpoint_retry.rs` imported `std::sync::atomic::AtomicU64` (`E0432`). |

The recurring mistake was reviewing each new counter in isolation. Native
64-bit targets provide `std::sync::atomic::AtomicU64` and the full standard
atomic API, so normal macOS, Windows, Linux x86_64, and `.160` builds could all
pass while Core still failed. Replacing the import with `atomic-shim` fixes
type availability but does not make every standard-library method available.

The v3.0.8 formal Core run
[30510279753](https://github.com/lovitus/EasyTier/actions/runs/30510279753)
proved the corrected implementation on both targets:

- MIPS job
  [90768894610](https://github.com/lovitus/EasyTier/actions/runs/30510279753/job/90768894610):
  success;
- MIPSel job
  [90768894628](https://github.com/lovitus/EasyTier/actions/runs/30510279753/job/90768894628):
  success.

## Mandatory source audit

Run this against the exact intended release SHA before the first formal
workflow:

```bash
rg -n 'AtomicU64|fetch_max' easytier/src --glob '*.rs'
```

Record every result in the candidate manifest. Each `AtomicU64` occurrence must
be one of:

1. `atomic_shim::AtomicU64` in code reachable by MIPS/MIPSel;
2. test-only code that is not compiled by the Core release build; or
3. code excluded from both MIPS targets by an explicit, inspected `cfg`.

An unclassified result blocks dispatch. MIPS/MIPSel-reachable code may not
import `std::sync::atomic::AtomicU64`.

For every operation used through `atomic_shim::AtomicU64`, inspect the exact
version locked in `Cargo.lock` and confirm that its non-native-64-bit
implementation provides that operation. The current locked version is
`atomic-shim 0.2.0`. Do not infer support from the standard library API or from
the crate's native-64-bit branch.

`fetch_max` is not available through the current shim fallback. A max update
must use the supported load/compare-exchange loop, as
`InterfaceSnapshotCache::invalidate_generation` does, with memory ordering
chosen for the original synchronization contract.

## Mandatory workflow and release evidence

1. Freeze the exact validated release SHA.
2. Complete the source audit above and record its classification.
3. Dispatch formal Core before the other formal release workflows.
4. Require both `mips-unknown-linux-musl` and
   `mipsel-unknown-linux-musl` jobs to succeed for that exact SHA.
5. Audit the two produced artifacts for the expected 32-bit architecture and
   big-/little-endian target.
6. Record the Core run ID, both job IDs and conclusions, SHA, and artifact
   audit in the version's validation matrix.
7. Only then dispatch formal GUI, Mobile, OHOS, and Test and later
   `EasyTier Release`.

This gate cannot be waived for a release. None of the following is equivalent
evidence: an x86 build, `.160` preflight, a successful static grep, merely
using `atomic-shim`, compiling only one endianness, a previous-SHA workflow, or
a successful unrelated platform matrix.
