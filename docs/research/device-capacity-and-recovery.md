# Device capacity and update recovery

## Contract

Each storage exposes identity, label, capacity or an explicit error, and writability. Update paths bind to one storage;
preflight checks peak demand and backup headroom per affected storage. Capacity refreshes before and after mutations and
every two seconds during long uploads.

A mounted-MTP artifact completes this lifecycle before the next starts:

1. verify the local payload size and SHA-256;
2. copy it and wait for desktop-MTP finalization;
3. require its exact device path, regular-file type, and size;
4. record the checkpoint.

The application does not reread complete map files through MTP. Garmin validates map content after disconnect. An
ambiguous partial file may be prefix-matched to transaction evidence before size-checked deletion.

Verified recovery backups are the default. Skipping them is explicit and part of plan identity. Backup-free recovery
cannot roll back old content; it verifies retained payloads, reconciles recorded paths and sizes, replaces proven
partials, completes journal-authorized writes and removals, then commits the original journal.

## Portable state

Before mutation, the CLI writes a bounded, versioned transaction under the visible `GARMIN-TOOLKIT` device directory and
indexes the richer host capture. Documents carry a magic value, kind, and exact schema version. Owning
`PortableTransaction` and `DeviceIdentityState` newtypes contain parsing, validation, serialization, and mutation; wire
structs remain private. Unsupported versions fail closed.

After device selection, unresolved portable state blocks a new update and offers recovery or proven clear. Clearing
requires the device to match a committed or untouched transaction. A host receipt created before the transaction was
prepared may be discarded; once a prepared journal exists, recovery or proven clear is required. Cross-host recovery is
not implemented because the originating payloads remain host-local.

## Safety and progress

Recovery validates typed checkpoints once. It audits recorded writes before new uploads, removes only journal-authorized
objects, and records capacity changes. Cancellation after mutation leaves a recoverable failure. Failed-upload cleanup
and mandatory rollback use independent bounded cancellation.

Aggregate transaction progress and per-file byte progress are separate. Rates and ETAs disappear when samples are stale.
History rows contain status, completion time, duration, message, and path; repeated cached work is coalesced. Capacity
polling updates the live header. Successful transactions retain only their initial and final capacity; reclamation and
refresh failures remain explicit history events.

## Evidence and limits

Tests cover multi-storage capacity, read-only media, backup policy, partial writes, cancellation, disconnection,
restart, rollback, marker collisions, exact completed-write reuse, failed-upload cleanup, portable-state detection, and
recovery ordering through production orchestration and directory-backed devices. Demo and dry-run execute the same
transaction engine against an isolated shadow.

A zero-byte interrupted upload is deliberately not accepted as a matching payload prefix. The 2026-10-01 packaged HASS
crash test reproduced this limit: restart found the transaction, but recovery preserved the empty object and retained
evidence instead of deleting it. Quarantining that object in the disposable simulation allowed verified rollback to
finish; this assisted result does not establish automatic recovery for empty uploads.

The shared map workflow now offers an explicit review of empty uploads. Only started, unapplied writes with verified
host backups and retained payloads qualify. Approval is bound to the reviewed files, portable transaction, and host
capture, and all evidence is checked again before mutation. The host reads the empty objects into a durable quarantine
with an approval manifest before removing them by checked size and invoking ordinary verified rollback. The recovered
outcome retains the quarantine location. Back, refresh, stale approvals, cancellation, and changed evidence cannot
bypass the review. Simulation recovery operates on the retained copy and preserves the source device. The
[shared acceptance summary](../plans/shared-interface-workflows.md#map-workflow-acceptance) records the demo coverage.

This does not prove every desktop-MTP implementation or recovery after a real cable disconnect. Current fēnix and Edge
mounted-MTP update, firmware-restart acceptance, and process-interruption recovery evidence lives in
[USB synchronization](usb-sync.md#map-maintenance-evidence).
