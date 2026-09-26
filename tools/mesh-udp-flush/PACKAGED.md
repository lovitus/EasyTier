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
