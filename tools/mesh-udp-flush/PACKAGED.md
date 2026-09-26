# Exact packaged Core comparison

This research-only entry reuses `lab.py` unchanged. It does not patch, rebuild,
or configure experimental switches in either Core. It lives outside the
production integration PR. `mesh-natural-cohort.yml` accepts an `artifact_pair`
JSON input with `baseline` and `candidate`, each containing `id` and `sha`.
The runner explicitly selects Python 3.12 and retains that interpreter for
the root-owned namespace lab; it does not rely on Ubuntu's default Python.

Both inputs must be matching musl/jemalloc-only comparator packages. Outer ZIP,
inner archive and actual Core bytes are checked against their manifests; source
SHA and toolchain must match the declared pair. The old base CLI is reused by
its independently verified digest because the UDP-only patch leaves RPC intact.
Missing or expired artifacts are blockers, not permission to rebuild silently.

The existing lab's `stock` mode means "no diagnostic overlay expected", not
"this is necessarily the baseline". `runs.json` and `provenance.json` identify
which real package is running. Assertions for TCP digest/half-close, UDP echo,
ICMP progress, TUN settings, direct UDP peer state, clean exit, namespace removal
and unchanged host routes are retained. No production counters are introduced.

Fixed load interleaves three samples per package, direction, inner IP family
and Stealth setting. Two namespaces on the same GitHub runner are the endpoints;
the underlay is IPv4 and the inner application traffic covers IPv4 and IPv6.
These results do not prove IPv6-underlay, relay, mixed-version or WAN behavior.
They do not establish idle CPU or long-duration leak freedom.

A separate traced low-rate round proves successful UDP_SEGMENT submission;
traced throughput/CPU numbers are never used as performance comparisons.
The final unpaced phase retains the existing zero-loss ICMP assertions. Known
saturation failures must remain failures; they cannot be waived to manufacture
an overall green result. Each phase stops on the first failure and preserves
the original logs and cleanup evidence. Results are pending until CI executes.

## Exact-package evidence: 2026-09-26

Run [36223095097](https://github.com/lovitus/EasyTier/actions/runs/36223095097)
completed SUCCESS. Harness SHA: `d7790e287c4cf2096fb6a7cf237e49e55cf33b71`.
Evidence artifact: `10900585354`, outer SHA-256
`50e159b81e334110396f83239a5f7b1552f77e06bc50575ce473fe4b862cd87b`.
The downloaded evidence digest was independently verified before aggregation.

| Identity | Baseline | Candidate |
| --- | --- | --- |
| Source | `c6772dbfef2395ff96b39bd4801945d92212dffb` | `3a1f3d9fc840a37a7649448a042bf44035bfaf7b` |
| Build run | `36222213728` | `36221363359` |
| Artifact | `10899308750` | `10898758495` |
| Core SHA-256 | `b59903879732a9f1b868e7a279aec076355877ff7b70a9372636aba13913fd23` | `b0d61a1911898412434fbbfb5bc15d8ca7dcd766f0710769de0c3f6968c8c1a6` |

Both are unmodified x86_64 musl, jemalloc-only Core packages built with Rust
1.95.0. There was no Core compilation or source overlay in the comparison run.
Candidate Test run `36219724313` separately passed the full existing workflow,
including all nine migrated UDP GSO contracts. One unrelated nextest partition
reported a leaky passing test; workflow success is not a blanket leak-free claim.

### Endpoints and measurement boundary

Both endpoints ran on the same GitHub Ubuntu 22.04 runner, Linux
`6.8.0-1064-azure`, AMD EPYC 7763, four vCPUs (two cores, two threads per core).
They were separate network namespaces joined by a veth pair, not separate
physical machines or a WAN path. Client: synthetic underlay `192.0.2.1`, overlay
`10.88.0.1` / `fd88::1`; server: `192.0.2.2`, overlay `10.88.0.2` / `fd88::2`.
These are isolated test addresses, not operational node identifiers.
The tunnel transport was direct UDP throughout. AES-GCM was enabled, compression
disabled, and original native-TUN settings retained. No Leaf/Mihomo process ran.

Each table cell is the median of three interleaved samples. Upload means
client to server; download means server to client. CPU s/GiB sums both Core
processes and uses completed application payload bytes, not offered load.
Fixed load requested 200 Mbit/s and delivered approximately 192 Mbit/s in both
arms; this is a CPU-cost measurement, not a bandwidth ceiling. The unpaced
transfers sent 1 GiB each and were not traced.

### Fixed-load CPU cost

| Inner IP | Stealth | Direction | Baseline CPU s/GiB | Candidate CPU s/GiB | Change |
| --- | --- | --- | ---: | ---: | ---: |
| IPv4 | off | upload | 32.96 | 24.48 | -25.7% |
| IPv4 | off | download | 33.60 | 24.16 | -28.1% |
| IPv6 | off | upload | 33.28 | 24.48 | -26.4% |
| IPv6 | off | download | 33.60 | 24.00 | -28.6% |
| IPv4 | on | upload | 36.48 | 26.56 | -27.2% |
| IPv4 | on | download | 35.84 | 26.56 | -25.9% |
| IPv6 | on | upload | 35.84 | 26.24 | -26.8% |
| IPv6 | on | download | 35.84 | 26.56 | -25.9% |

Throughput median changes were between -0.08% and +0.05%. Whole-host CPU/GiB
also fell by 18.7%-26.4%; the observed improvement was not simply transferred
from Core process accounting to kernel/other host CPU accounting.

### Unpaced results, Stealth off

| Inner IP | Direction | Baseline Mbit/s | Candidate Mbit/s | Rate change | Baseline CPU s/GiB | Candidate CPU s/GiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| IPv4 | upload | 1102.3 | 1381.8 | +25.4% | 24.75 | 17.68 |
| IPv4 | download | 1085.9 | 1369.8 | +26.1% | 24.83 | 17.75 |
| IPv6 | upload | 1078.7 | 1333.8 | +23.7% | 25.33 | 18.23 |
| IPv6 | download | 1072.6 | 1348.0 | +25.7% | 25.47 | 18.29 |

Core CPU/GiB fell 28.0%-28.6%; whole-host CPU/GiB fell 23.3%-24.8%.
IPv4 download had wider ranges: baseline 944.8-1095.9 Mbit/s, candidate
1143.7-1382.9 Mbit/s. Retain these ranges rather than treating medians as exact
hardware limits. This is one runner and three samples, not a universal gain.

### Functional and resource evidence

- 24 fixed-load rounds, 12 unpaced rounds and one separate low-rate trace round.
- 74 digest/half-close integrity transfers, 1110 checked UDP echo datagrams,
  and 1480/1480 ICMP replies across all rounds.
- 74 Core process exits returned zero without forced killing. All 74 namespaces
  were removed; root routes were unchanged in all 37 rounds.
- All 148 before/after endpoint peer observations reported UDP transport.
- The separate trace observed 6467 successful UDP_SEGMENT submissions. Trace
  CPU and rates are excluded from every performance comparison above.
- Paired process RSS snapshots: fixed baseline 52.44-54.20 MiB, candidate
  52.05-54.46 MiB; unpaced baseline 52.63-56.79 MiB, candidate 52.25-55.36 MiB.
  These overlapping short-run ranges do not establish a memory optimization,
  bounded long-term RSS, or absence of a leak.

### Scope of the first measurement

Deployed scope: immutable packages only in disposable GitHub namespaces.
No operational host replacement, merge, tag or release has occurred. The
production draft remains PR #9 at `3a1f3d9f`; this document is research evidence.
The Linux direct-UDP throughput/CPU benefit has now been reproduced with the
actual integration package, not merely the earlier experimental overlay.

The previous high-load simultaneous mixed-flow receive-ring loss remains open.
This run used one bulk direction at a time with concurrent ICMP, so its green
result does not supersede the different mixed-flow failure. Idle CPU, original
hardware/WAN, IPv6 underlay, relay, mixed-version, sustained memory and non-Linux
compatibility acceptance are also incomplete. The optimization does not claim
to resolve every Core performance bottleneck or reach 10 Gbit/s.

IPv6-underlay and mixed-version follow-up evidence is recorded below. The
production source stays frozen; the follow-up reuses the same packages.

## Compatibility-only input extension

`artifact_compatibility_only=true` reuses the same verified packages and skips
the already completed performance/trace phases. It adds no Core build and no
new product configuration. The existing lab now accepts an optional server
package directory and an IPv6-only underlay; all previous defaults and assertions
remain intact. IPv6 underlay assigns only `2001:db8:88::1/64` and `::2/64` to the
isolated veths and uses explicit bracketed IPv6 listener/peer URLs. Namespace
IPv6 is enabled independently of the inner application address family.

The 24-case functional matrix covers both inner families and Stealth states:
both mixed-version placements over IPv4, plus baseline/baseline,
baseline/candidate, candidate/baseline and candidate/candidate over IPv6.
Pure-version IPv4 cases already measured above are not repeated. The binary
record includes the independently selected server; the run record identifies
both package labels. No rate gain is claimed from this compatibility matrix.
This extends the existing E2E inputs, not a new mock/test framework or a claim
that baseline compatibility is defective. Results remain pending execution.

Run `36224324021` at harness `c0f883f3` failed after all eight IPv4-underlay
mixed-version cases passed. The first IPv6-underlay baseline/baseline case
successfully connected, but the inherited IPv4-only CLI assertion expected
`udp` while the real CLI correctly returned `udp6`. This is a harness omission,
not a candidate Core failure. Both Core processes exited zero, both namespaces
were removed, and root routes remained unchanged for that failed case.

Source contract: `easytier-cli.rs` populates `tunnel_proto` using
`PeerRoutePair::get_conn_protos()`; `proto/common.rs::display_tunnel_type()`
normalizes its display scheme using the resolved endpoint address family.
Therefore the corrected assertion requires exactly `udp` for IPv4 underlay
and exactly `udp6` for IPv6 underlay. It does not allow both indiscriminately
or accept another transport. Other assertions and both Core packages are
unchanged. The original failed run remains failed; IPv6 traffic acceptance
is pending the corrected run, not inferred from a successful handshake.

## Compatibility result

Corrected run [36224742826](https://github.com/lovitus/EasyTier/actions/runs/36224742826)
at harness `50d9091f44f32ed08eefb2fb85c713e7d49cea1e` completed SUCCESS.
Artifact `10900597139` has independently checked outer SHA-256
`3d3ec15ff1f939c651632d945d4e9de5146fd483fed320bd96f39be3fe9b6935`.
The baseline/candidate artifacts and their actual Core hashes are unchanged.

All 24 cases passed. Source identity was checked separately for client and
server from the lab's binary records, including the optional server override.
The matrix covers eight mixed-version IPv4-underlay cases and sixteen
IPv6-underlay cases across both inner families and both Stealth configurations.
There were 32 exact `udp` observations and 64 exact `udp6` observations.

The run completed 48 integrity/half-close transfers, 720 checked UDP echo
datagrams and 960/960 ICMP replies. All 48 Core processes exited zero without
forced killing, all 48 namespaces were removed, and root routes were unchanged
in all 24 cases. This is real packaged-Core traffic evidence, not just handshake
or CLI-state evidence. The historical failure above remains a harness failure;
it is not relabeled as a passing run or a fixed production regression.

Current deployed scope remains disposable GitHub namespaces. Production PR #9
is still `3a1f3d9f`, with no operational host deployment, merge, tag or release.
Direct UDP compatibility now covers IPv4 and IPv6 underlays, both inner families,
Stealth configurations and both placements of mixed baseline/candidate peers.
The earlier rate/CPU table still measures IPv4 underlay only; no IPv6-underlay
speedup is inferred from these functional checks.

Remaining: relay acceptance, high-load simultaneous mixed-flow receive-ring
loss, idle CPU and sustained resources, original-hardware/WAN performance,
and non-Linux/other-architecture build acceptance. The bounded three-peer relay
follow-up using these same packages is recorded below.

## Bounded relay extension

`artifact_relay_only=true` reuses the same artifacts with no Rust compilation.
The existing lab gains one optional relay package. Its two endpoint namespaces
have disjoint veth subnets, with no direct L3 route to one another; only the
relay namespace has both links. IPv4 and IPv6 forwarding are explicitly
disabled inside that namespace. Both endpoints connect only to the relay,
and automatic P2P remains disabled. No host firewall or root route is changed.

Route JSON is checked using the exact baseline CLI contract: on both endpoints,
the other endpoint must have `path_len=2` and next-hop virtual IPv4 `10.88.0.3`;
the relay must report both endpoints at `path_len=1`. These checks run before
and after real traffic, in addition to the existing transport, integrity,
half-close, UDP echo, ICMP and cleanup checks. Every role has its own recorded
binary digest. All three Cores participate in bounded lifetime and cleanup.

The 24 functional cases cover IPv4 and IPv6 underlays with the matching inner
family, both Stealth configurations, and six client/relay/server version
combinations: BBB, CCC, BCB, CBC, BCC, CBB. Both transfer directions are checked;
the last two combinations also cover the reverse endpoint orientation. This
is not a relay performance claim or exhaustive cross-family relay matrix.
Results remain pending. Production source and existing package identities stay
unchanged; prior failures and direct-path performance records remain preserved.

### Relay result: exact packages, not inferred from peer status

Run [36225644719](https://github.com/lovitus/EasyTier/actions/runs/36225644719)
at harness `8a1de37eb5efe3dc2d5a42f1ff46990c8beb18ca` completed SUCCESS.
Evidence artifact `10900851933` has independently verified SHA-256
`723f9d5e579a24123299ecde96bd2bceb8517263fe12b96ab4bb54f1beddd50b`.
Client, relay and server hashes match the same previously verified baseline
and candidate packages. No Core source was changed or rebuilt.

All 24 cases passed. The recorded evidence includes 48 checks that endpoints
have no direct underlay L3 route, 96 endpoint route observations with two hops
through `10.88.0.3`, and 96 relay route observations with one hop to the endpoints.
Before/after protocol observations include 72 IPv4 and 72 IPv6 peer queries,
covering 96 direct link entries per family. IPv4/IPv6 kernel forwarding was
disabled in the relay namespace during setup; the independent no-direct-route
and Core route observations also prevent attributing a direct path to relay.

Actual traffic completed 48 integrity/half-close transfers, 720 checked UDP
echo datagrams and 960/960 ICMP replies. All 72 Core processes exited zero
without forced killing, all 72 namespaces were removed, and all 24 root-route
snapshots were unchanged. Each of BBB, CCC, BCB, CBC, BCC and CBB completed
four family/Stealth cases with traffic in both directions. This is functional
relay acceptance for this matrix, not relay throughput or WAN acceptance.

## Current task cursor

Production candidate: PR #9, `3a1f3d9f`, still unmerged and not deployed to an
operational host. Its exact packages now have direct-path CPU/rate evidence,
IPv4/IPv6-underlay mixed-version acceptance, and three-peer relay acceptance.
Evidence-only commits and research harness changes do not change that Core SHA.

Still open: high-load simultaneous mixed-flow receive-ring loss, idle CPU and
sustained resource behavior, original-host/WAN performance, and non-Linux /
other-architecture build acceptance. The short runs do not prove leak freedom;
the earlier full Test workflow's leaky passing test remains a separate caveat.
Next action: reuse the symbol-bearing packages for bounded idle/resource samples
and actual Core CPU profiles to identify the remaining cost, rather than add
another speculative production optimization or rebuild the same Core.

## Active cursor: exact-package CPU and idle-resource observation

The 24-case relay milestone is complete. The next observation reuses the same
baseline `c6772dbfef2395ff96b39bd4801945d92212dffb` and candidate
`3a1f3d9fc840a37a7649448a042bf44035bfaf7b` packages. No Core rebuild, source overlay,
policy engine, operational-host deployment, or production edit is part of it.

- Run one baseline pair and one candidate pair on an isolated hosted Linux
  runner: direct UDP, IPv4 underlay/overlay, AES-GCM, Stealth off, native TUN.
- Before and after traffic, record two idle intervals (30 and 60 seconds),
  including per-process CPU time, RSS/high-water RSS, FD count and thread count.
  These are bounded observations, not proof of long-term leak freedom.
- Reuse the existing perf attachment/acknowledgement and per-PID sample checks.
  Sample both Cores at 99 Hz during each 2 GiB upload/download using the frame
  pointers already enabled in the exact artifact build; retain raw perf data,
  flat reports and call graphs. Inline symbol expansion is disabled for bounded
  reporting; this does not remove or change recorded instruction-pointer samples.
- Preserve integrity/half-close, UDP echo, ICMP progress, clean process exit,
  namespace cleanup and unchanged root-route assertions. Profiled throughput is
  diagnostic only and must not replace the completed unprofiled comparison.
- At each idle observation, abort above the existing smoke guard's bounds:
  1 GiB total RSS, 512 FDs or 128 threads per Core, 16 MiB log output, or 180%
  aggregate Core CPU. These are safety bounds, not claimed acceptance SLOs.
- Install Ubuntu's HWE 6.8 userspace tools without installing/changing a kernel;
  select the actual perf executable, record its package/build metadata and
  require the cpu-clock preflight to succeed. Unsupported sampling is a blocker,
  never a reason to fabricate a profile or weaken functional assertions.

References: [Ubuntu package](https://packages.ubuntu.com/jammy/linux-tools-generic-hwe-22.04)
and [perf record call-graph options](https://man7.org/linux/man-pages/man1/perf-record.1.html).

Status: run [36227357275](https://github.com/lovitus/EasyTier/actions/runs/36227357275)
failed in the new perf-tool preparation step, before any Core was launched.
The existing packages passed their identity checks. Ubuntu installed
`linux-hwe-6.8-tools-6.8.0-138`, but the harness searched only regular files under
`/usr/lib/linux-tools-*`; it produced no `perf-path.txt`. This is a harness
tool-discovery failure, not a Core regression or a completed profile.

Failure evidence artifact `10900384496` was downloaded and verified against
SHA-256 `50c5264df430be8b6f8d4f9c84079c7d2dc4bee18d3a4259940b406cb058c4b5`.
The original failure log is preserved privately. No CPU profile or idle result
is claimed. The proposed correction is to ask the installed HWE package for
its file list and require the resulting perf executable, rather than assume
an installation directory. Core remains frozen at `3a1f3d9f`.

The first correction, harness `41048205`, also failed before Core startup in
[run 36227743112](https://github.com/lovitus/EasyTier/actions/runs/36227743112):
`dpkg-query -L` rejects package-name wildcards (exit 2). The failure log is
preserved; no runtime result is claimed. The corrected command uses `-W` for
pattern expansion, then calls `-L` with each exact package name, as required by
the [dpkg-query contract](https://manpages.debian.org/bookworm/dpkg/dpkg-query.1.en.html).

Run [36227931747](https://github.com/lovitus/EasyTier/actions/runs/36227931747),
harness `094293f0`, passed perf preparation and actually sampled the baseline.
Two before-traffic idle intervals measured aggregate Core CPU of 0.2667% and
0.2333%, paired RSS about 51 MiB, 13 threads per Core at the interval ends and
declining FD counts. Upload captured 2232/2341 samples from the two Core PIDs.
The subsequent `perf report` exceeded its 60-second analysis timeout; this is
not an application timeout, completed paired comparison, or after-load result.
The raw profile is preserved in artifact `10901790217`, verified SHA-256
`40ad5860076722ffe8408c3c605922a9708c125b36c34a5b58e07498aa6c3ccd`.

The exact source workflow already sets `-C force-frame-pointers=yes`; the extra
DWARF recording was unnecessary. The next observation returns to existing
frame-pointer recording, disables inline expansion and callchain display in
the flat report, and retains a separate callgraph report. It first replays the
preserved baseline data without a new Core execution. Kernel symbols in this
cross-run replay may be unavailable and must not be invented or treated as a
fresh profile. Actual paired profiling still occurs on one runner.

The unique next step is this bounded report/observation. No already-completed
acceptance matrix or Core build needs rerun; no functional assertion changes.
