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
requires the device to match a committed or untouched transaction. A retained host receipt covers interruption before
portable publication. Cross-host recovery is not implemented: it still needs the originating retained payloads.

## Safety and progress

Recovery validates typed checkpoints once. It audits recorded writes before new uploads, removes only journal-authorized
objects, and records capacity changes. Cancellation after mutation leaves a recoverable failure. Failed-upload cleanup
and mandatory rollback use independent bounded cancellation.

Aggregate transaction progress and per-file byte progress are separate. Rates and ETAs disappear when samples are stale.
History rows contain status, completion time, duration, message, and path; repeated cached work is coalesced. Capacity
polling does not flood history, while mutation snapshots and reclamation remain visible.

## Evidence and limits

Tests cover multi-storage capacity, read-only media, backup policy, partial writes, cancellation, disconnection,
restart, rollback, marker collisions, exact completed-write reuse, failed-upload cleanup, portable-state detection, and
recovery ordering through production orchestration and directory-backed devices. Demo and dry-run execute the same
transaction engine against an isolated shadow.

This does not prove every desktop-MTP implementation, physical firmware acceptance, or recovery after a real cable
disconnect. Those remain in [production CLI map maintenance](../plans/mounted-device-updates.md).
