# Release operator

The release process has one normal path. The operator owns synchronization, retries,
exact-SHA reuse, fail-fast cancellation, and workflow dispatch.

## Normal path

1. Set the version in project files, then initialize compact release records:

    scripts/release-operator.sh init-release

Fill the generated scope and user-facing notes before continuing.

2. Run local gates and the automatically selected remote preflight:

    scripts/release-operator.sh prepare

PREFLIGHT_SCOPE=full, rust, or frontend is an explicit override. Auto mode skips remote
compilation only for documentation, workflow, and release-tool-only changes; unknown
build paths fail safe to the full gate.

3. Commit and push the immutable SHA, then run validation:

    scripts/release-operator.sh validate

Linux Profiling Beta and required Android candidate run first. After they pass, Core,
GUI, Mobile, OHOS, and Test start together. Validate candidate artifacts and real
devices in parallel with those five workflows. Existing active/successful runs are
reused, so the command is safe to resume.

4. After artifact/device validation succeeds, publish:

    EXACT_ARTIFACT_VALIDATED_SHA="$(git rev-parse HEAD)" \
      scripts/release-operator.sh publish "v$(scripts/release-operator.sh version)"

Release remains blocked on candidate runs, all five formal runs, MIPS/MipSel, exact
artifact attestation, a clean pushed SHA, and an unpublished matching version.

## Failure path

    scripts/release-operator.sh status
    scripts/release-operator.sh retry

The first failed candidate/formal run cancels active peers in that group. Use retry
only after proving an infrastructure or flaky failure with unchanged source; it reruns
failed jobs and resumes canceled peers. Product/source failures require a fixed new SHA
and a fresh validate.

## Expected wall time

The v3.0.16-1 evidence was 21 minutes for Linux candidate and 77 minutes for the formal
critical path. The intended successful path is therefore about 100 minutes plus remote
preflight, with device/artifact checks overlapped. Multi-hour delays indicate repeated
SHAs, runner scarcity, or a failed/retried run, not the normal release design.

Advanced recovery commands remain available through
scripts/release-operator.sh help-advanced; they are not the normal procedure.
