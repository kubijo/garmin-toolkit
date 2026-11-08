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

## Telemetry assessment

Reviewed against the [pinned dependencies](../../Cargo.toml), `mtp-rs` 0.32.0 and `gio` 0.22.9 bindings, and current
adapters, without physical-device queries.

- **Selected shared fields:** retain transport kind, storage capacity/writability, and measured operation rates.
  [`DeviceStateSnapshot`](../../crates/garmin-model/src/device.rs) represents unavailable capacity and unknown
  writability explicitly. Rates describe the current operation; stale samples disappear and upload excludes device
  finalization. These fields do not predict device performance or battery safety.
- **USB speed:** [`MtpCandidate`](../../crates/garmin-device/src/mtp.rs) already carries optional negotiated link speed.
  Keep it as a raw-transport diagnostic. It is neither achieved throughput nor a portable mounted-device property;
  [GIO volumes](https://docs.gtk.org/gio/iface.Volume.html) expose mount metadata, not a common USB speed API.
- **Battery:** the dependency's `ptp/session/properties.rs` has `BatteryLevel` access and mock descriptor/value tests.
  That proves protocol handling, not Garmin firmware support. The application's MTP adapter uses `MtpDevice`, whose
  public API does not expose those property operations; mounted and directory adapters have no shared battery source.
- **Charging:** no shared source exists in the current adapters. USB attachment, power descriptors, and historical FIT
  battery samples do not establish live charging state. Do not infer it or show an absent value as false or zero.

**Decision:** add no battery or charging fields to the shared model or UI. Keep the existing fields and speed
diagnostic; no new polling, raw sessions, property dumps, or identifiers are needed. The normalized measurements reveal
less than raw descriptors, which can include device serials, mount paths, and host topology. No telemetry history is
added. Optional battery/charging exposure would require an adapter-specific source, bounded reads within its owned
session, explicit unknown/stale handling, and observations tied to device, firmware, host, and adapter. This assessment
establishes no physical support and schedules no hardware probe.

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

Zero-byte interrupted uploads cannot establish a matching payload prefix and require explicit quarantine approval. Only
started, unapplied writes with verified host backups and retained payloads qualify. Approval binds the reviewed files,
portable transaction, and host capture, and all evidence is checked again before mutation. The host reads the empty
objects into a durable quarantine with an approval manifest before removing them by checked size and invoking ordinary
verified rollback. The recovered outcome retains the quarantine location. Back, refresh, stale approvals, cancellation,
and changed evidence cannot bypass the review. Simulation recovery operates on the retained copy and preserves the
source device. Desktop and packaged-HASS demo acceptance covered reviewed update/removal, cancellation, reconnect,
retained outcomes, exclusion of concurrent device writes, and recovery after a process stop. These demo runs do not
qualify zero-byte quarantine on physical media.

These checks do not qualify every desktop-MTP adapter or real cable-disconnect recovery. Physical fēnix and Edge
mounted-MTP update, firmware-restart, and process-interruption evidence lives in
[USB synchronization](usb-sync.md#map-maintenance-evidence).
