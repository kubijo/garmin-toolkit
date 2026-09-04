# 0039: Virtual devices in demo and dry-run

## Decision

Use the same stateful virtual MTP backend for runtime simulation and transaction tests. Directory backing is acceptable.
One transaction engine performs backup, upload, readback, deletion, journaling, and recovery through device traits.

| Composition | Services and source state           | Transaction destination |
| ----------- | ----------------------------------- | ----------------------- |
| Real        | Selected device and Garmin services | Selected device         |
| Demo        | Synthetic device and local services | Virtual MTP device      |
| Dry-run     | Selected device and Garmin services | Isolated virtual shadow |

Dry-run reads the selected device and retains real discovery, inventory, authorization, and preflight. Populate its
shadow with the affected original files and storage bindings; do not substitute an empty fixture. Missing or ambiguous
source state must fail explicitly. Capacity checks use real measurements, not the shadow's host filesystem.

Only the shadow accepts mutations. Enforce read-only access to the source through capabilities, not a late mode check.
Failure to prepare the shadow must never fall back to writes on the selected device.

Bind journals and recovery to the execution target. Preserve source identity separately for service authorization and
diagnostics. A simulated journal must not authorize recovery against physical hardware, even when source manifests
match.

This refines ADR 0037: dry-run disables physical-device commit, not transaction execution. Remove skipped-stage branches
and canned outcomes as adapters converge. Keep the global mode indicator; results distinguish simulation from physical
installation. A successful simulation does not prove native transport support or device acceptance.

## Inspectable artifact

Retain the shadow inside the session capture after success, failure, or cancellation. Human and automated inspection
must work without rerunning the application:

```text
simulation/
  README.md                    # Scope and inspection guidance
  snapshot.json                # Source identity, scope, and storage mapping
  changes.json                 # Planned changes and observed results
  summary.md                   # Readable outcome and observed changes
  before/<storage-key>/...     # Affected originals, preserved for comparison
  device/<storage-key>/...     # Actual virtual-device backing tree after execution
```

Preserve device-relative paths beneath stable, safe storage keys. This is an affected-files snapshot, not a complete
device image; list its scope explicitly. Originals and the source must not share writable files with the shadow.

Generate observed changes from captured before/after state, not the plan alone. Record additions, replacements,
removals, and unchanged targets with paths, byte counts, hashes, and outcome. Distinguish absent files from unobserved
state. Failed or interrupted runs must remain identifiable as incomplete, with partial files available for inspection.

The summary uses readable sizes; JSON retains exact bytes. Show the artifact path in the retained completion screen.
Keep captures private: device files, identifiers, and authorization data may be sensitive. Inspection does not imply
permission to publish or upload the artifact.

## Validation

Run the same success, failure, cancellation, and restart cases against demo and dry-run adapters. Verify shadow
contents, backups, and journals. Prove zero source mutations, including failure and recovery paths; reject cross-target
journals. Verify change reports against retained file contents, including incomplete runs. Validate changed progress and
completion screens through the shared gallery renderer.

Implementation and native-adapter evidence remain in the [mounted-update plan](../plans/mounted-device-updates.md).
