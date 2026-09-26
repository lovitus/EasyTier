# Non-shipping GSO fallback negative control

This isolated branch is based on green candidate
`b0d88e17c84d56736cc9f87d4bd1cae533e0b8f3`.
Only `send_frames` is restored to the implementation inspected at
`0cb5c1fbc0090fcad683ca52535336dd61c2a6f2`.
The nine tests, including the new regression, remain unchanged. Added diagnostic
fields remain so compilation does not fail merely because the test references
new fields. No workflow, assertion, expected result or production source changes.

Expected useful red result: the real-kernel rejection regression executes and
fails because the old sender propagates EINVAL rather than submitting remaining
individual datagrams. A compiler, dependency, artifact or infrastructure failure
is NOT an acceptable red result. The other eight contracts should still pass.
The already verified green run is `36209030843`.

This branch must not be merged, deployed, or released. It exists only to close
the one-time behavioral red/green check under the approved experimental staging
exception, using the existing workflow without a new CI mechanism.
