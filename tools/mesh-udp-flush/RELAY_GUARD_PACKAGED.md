# Secure relay destination guard: exact-package acceptance

Status: B01 red/green and bounded same-version relay acceptance passed. The
whole-project performance objective, original-host acceptance and B02 session
GC are not closed by this report. Related: issues #2, #4, #8 and draft PR #11.

## Exact revisions and artifacts

| Item | Identity |
| --- | --- |
| Unchanged production baseline | `6e90dbf102e5c93d56c531b87eea1858c4a8e61e` |
| Test-only negative source | `cd612a5546f2a1a6a5926c58b6061141acbe21c6` |
| Corrected candidate | `3166ab672d347cdcc5a6768bc77056cd8ec38323` |
| Unchanged bounded harness | `deb750fb0fac1907a5a526e61ef6e8952c41a7fb` |
| Optimized artifact | `10911199419`, 79,740,412 bytes |
| Core binary SHA-256 | `46ab646adbac5eaea1c008f23b1b2340b25ac6ffb80018b1d5b41d6ec404fb86` |
| Core Build ID | `068335a387251ac823e8bacdc61ba5c82067a14c` |
| Core archive SHA-256 | `cf19f8fc26a00eb42ec0cd194c954663a1415d2a05eee8519dd82fdbea98f545` |
| Evidence artifact | `10911962719`, 1,599,184 bytes |
| Evidence archive SHA-256 | `6ba1813181539c0c9f37ba563630299e1469f1e024a262bde705606ee25bb1db` |

BUILD_INFO, ELF metadata and provenance agree on the candidate: Rust 1.95.0,
`x86_64-unknown-linux-musl`, jemalloc, no Leaf/policy features, static PIE with
debug information. The comparator was built with `audit_comparator=true`, not
published as a rolling beta. CI consumed the Core archive; the small evidence
archive was downloaded and its full SHA-256 and ZIP integrity checked locally.
All three running roles reported the expected Core binary hash.

## Scope and red/green evidence

The production delta is only the source/destination ownership check in
`PeerSessionTunnelFilter::before_send`, moved before the existing session lock.
Final-destination ciphertext owned by `RelayPeerMap` must not use a next-hop
connection's unrelated session. No wire, route, crypto, logging, GC, ArcSwap,
configuration or dependency change is included.

- [Baseline Test](https://github.com/lovitus/EasyTier/actions/runs/36240736489)
  compiled and failed the real regression at runtime in 0.009 seconds with
  `relay payload must bypass the unrelated next-hop session`. The unrelated
  next-hop session was invalid; ordinary direct/control behavior was not
  weakened to produce the failure.
- [Corrected Test](https://github.com/lovitus/EasyTier/actions/runs/36256718846)
  succeeded on the exact candidate SHA. Its log records the identical regression
  passing in 0.007 seconds. No workflow modification or test expectation change
  was used. There was one baseline Test run and one corrected Test run, explicitly
  authorized by the maintainer.
- The [old-artifact bounded negative control](https://github.com/lovitus/EasyTier/actions/runs/36238994113)
  hit the 2 MiB per-Core log bound in its first secure+Stealth relay case. The
  source logged 520 already-encrypted warnings before SIGXFSZ; cleanup completed.
  This old failing bulk arm was not repeated in the acceptance run.
- [Optimized build](https://github.com/lovitus/EasyTier/actions/runs/36258711567)
  and [fixed-artifact relay acceptance](https://github.com/lovitus/EasyTier/actions/runs/36259577427)
  both succeeded. Observation timeouts did not trigger duplicate builds/runs.

## Topology and measurement boundary

Both endpoints and the relay were three separate network namespaces on one
GitHub-hosted Ubuntu 22.04 runner: AMD EPYC 7763, four logical CPUs, Linux
6.8.0-1064-azure. The path was application A -> Core A -> Core relay -> Core B
-> application B, and the reverse direction. It used UDP mesh over isolated
veth underlay, with one mesh TUN per endpoint and MTU 1360. No Mihomo or Leaf
was involved. Endpoint-to-endpoint underlay routes were unavailable and kernel
forwarding at the relay was disabled.

The fixture's `baseline` and `candidate` labels both point to the **same fixed
artifact**. The 24 cases are six repetitions of each IPv4/IPv4 or IPv6/IPv6
underlay/inner-family combination, with the fixture's secure+Stealth setting
off/on. They are not mixed-version coverage or old/new A/B samples. The
secure+Stealth case is not a claim about derived-Stealth-only mode.

Traffic was paced at 200 Mbit/s. Each direction transferred 64 MiB in the
non-Stealth fixture or 32 MiB with secure+Stealth. Approximately 192 Mbit/s
achieved goodput is a paced workload result, **not a bandwidth ceiling**.

## Audited functional and resource results

| Check | Actual result |
| --- | --- |
| Completed cases | 24/24, all exit zero |
| Bulk transfers | 48/48, 2,415,919,104 bytes total (2.25 GiB) |
| Independent integrity transfers | 48/48; each 1,048,595 bytes, expected SHA-256 matched |
| UDP echo | 720/720 datagrams, payload sizes 1, 64 and 1200 bytes |
| Relay route observations | 192; endpoint paths via relay length 2, relay-to-endpoint paths DIRECT length 1 |
| Negative direct-underlay checks | 48 expected unreachable results |
| Core exits | 72/72 exit zero, no forced kill |
| All tracked child exits | 240/240 exit zero, no forced kill |
| Namespace cleanup | 72 removed, no remaining PIDs |
| Root route snapshots | 24/24 unchanged |
| Per-Core log hard limit | 2,097,152 bytes, unchanged |
| Actual Core logs | 72 files, 288,248 bytes total; largest 7,050 bytes |
| Secure+Stealth Core logs | 36 files, 123,451 bytes total; largest 5,329 bytes |
| Already-encrypted warnings / encrypt failures / panics | 0 / 0 / 0 in all Core logs |

Every recorded Core log size and SHA-256 matches its archived file. No file
reached the cap. The fix removes the incorrect encryption call rather than
suppressing its warning. The previous bounded failure and current success use
the same harness, logging level and security settings.

The 288 per-process before/after snapshots show RSS 25.63-27.88 MiB per Core,
78.88-82.05 MiB for the three Cores together, 26-29 FDs and 13-14 threads per
Core. These are short transfer snapshots, not an idle baseline, long-duration
leak test or a memory-reduction comparison. ICMP processes participated in the
fixture, but packet/loss counts were not independently recounted in this audit.

## Absolute paced CPU cost, not an improvement claim

Each row is the median of six same-version samples. CPU is the **sum of all
three Core processes**, including the relay. CPU percent uses one logical CPU
as 100%; it is not the percentage of the four-CPU host.

| Underlay / inner | Fixture | Direction | Goodput Mbit/s | Core CPU seconds/GiB | Aggregate Core CPU % |
| --- | --- | --- | ---: | ---: | ---: |
| IPv4 / IPv4 | Non-Stealth | Upload | 192.44 | 31.36 | 70.26 |
| IPv4 / IPv4 | Non-Stealth | Download | 192.31 | 32.24 | 72.18 |
| IPv4 / IPv4 | Secure+Stealth | Upload | 192.31 | 35.04 | 78.46 |
| IPv4 / IPv4 | Secure+Stealth | Download | 192.25 | 34.72 | 77.66 |
| IPv6 / IPv6 | Non-Stealth | Upload | 192.54 | 31.52 | 70.65 |
| IPv6 / IPv6 | Non-Stealth | Download | 192.49 | 31.36 | 70.27 |
| IPv6 / IPv6 | Secure+Stealth | Upload | 192.45 | 34.24 | 76.69 |
| IPv6 / IPv6 | Secure+Stealth | Download | 192.35 | 34.56 | 77.39 |

This establishes bounded relay operation and an absolute CPU/resource sample.
It does not quantify this guard's speedup. Do not add it to the separately
measured UDP/TUN percentages, compare three-process relay cost directly with
two-process direct cost, or infer WAN/original-host performance.

## Remaining work and rollback boundary

B01's reproduced drop and log-storm acceptance are closed for this exact Linux
artifact. PR review/merge is separate. Original-host acceptance, a defensible
combined-performance baseline and any remaining common Core hot paths still
belong to the overall performance goal. B02 activity-aware session GC is not
implemented or implicitly accepted. A passing Test workflow also does not erase
the earlier passing-test LEAK annotation; it is not long-term resource proof.

Rollback is limited to the ownership guard, its regression and its design note.
The isolated harness safety cap stays in place independently.
