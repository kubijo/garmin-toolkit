# 0039: Virtual devices in demo and dry-run

## Decision

Use one stateful virtual MTP backend for runtime simulation and transaction tests. Directory backing is acceptable.
Real, demo, and dry-run use the same transaction engine for backup, upload, metadata checks, deletion, journaling, and
recovery. Garmin performs map-content acceptance after disconnect.

| Mode    | Read source                   | Mutation target         |
| ------- | ----------------------------- | ----------------------- |
| Real    | Selected device and Garmin    | Selected device         |
| Demo    | Synthetic device and services | Virtual MTP device      |
| Dry-run | Selected device and Garmin    | Isolated virtual shadow |

Dry-run keeps real discovery, inventory, authorization, capacity, and preflight. Its shadow contains affected originals
and storage bindings. Missing or ambiguous source state fails; preparation failure never falls back to physical writes.
Read-only source access is enforced by capability.

Journals bind to the mutation target. Source identity remains separate for authorization and diagnostics. A simulated
journal cannot authorize physical recovery.

## Artifact

Retain an inspectable shadow in the private session capture:

```text
simulation/
  README.md
  snapshot.json
  changes.json
  summary.md
  before/<storage-key>/...
  device/<storage-key>/...
```

The snapshot covers affected files, not the whole device. Before and shadow files must not share writable storage.
Observed changes come from before/after state and distinguish additions, replacements, removals, unchanged paths,
missing files, and incomplete runs. JSON retains exact bytes; the summary uses readable sizes.

## Validation

Run success, failure, cancellation, and restart through demo and dry-run. Verify artifacts, backups, journals, reports,
cross-target rejection, and zero source mutation. Render changed progress and completion states in the shared gallery.
Current hardware evidence is recorded in [USB synchronization](../research/usb-sync.md#map-maintenance-evidence).
