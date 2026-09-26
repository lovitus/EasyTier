# Bounded relay log regression

This is isolated-lab safety, not a production logging change. Core's logging
level, traffic checks, namespace topology and existing cleanup remain unchanged.

The previous exact relay matrix was functionally successful but generated about
2.76 GB of secure/Stealth Core logs. The repeated already-encrypted warning dumps
the payload. See `TUN_HEAD_PACKAGED.md`; that run is a resource failure, not an
overall PASS.

Every lab Core now inherits a 2 MiB regular-file limit and a zero core-dump limit.
These limits apply only to the lab's own child, not the runner or another host.
The kernel enforces the file cap while the parent waits for traffic. The existing
process and probe failure paths stop the case and clean its namespaces; resource
snapshots and final acceptance also reject a log at the cap. Each Core's actual
log size, SHA-256, limit state and exit status are recorded even on failure.

Do not truncate a log and continue, suppress warnings, or turn the negative
control into a functional PASS. A failure due to this cap is retained alongside
the original probe error and cleanup evidence. The expected negative control
uses the already built, unchanged `6e90dbf1` package; it must not rebuild Core.
The repaired package must pass the same bounded relay path and must not contain
the repeated already-encrypted warning. Windows/macOS/WAN performance is not
established by this Linux namespace fixture.

Status: implemented; the bounded negative control has not run yet.
