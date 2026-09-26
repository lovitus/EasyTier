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

### Current task cursor and remaining acceptance

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

Next: extend the existing package-only lab to IPv6-underlay and both placements
of mixed baseline/candidate peers, reusing these same immutable packages. Keep
the production source frozen and do not weaken existing assertions.
