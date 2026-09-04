# Device capacity and update recovery

## Implemented contract

`garmin-model::device` represents each storage independently: opaque ID, label, total and free bytes or an explicit
failure, and known writability. `DeviceRead::state` supplies the model through directory, raw-MTP, and mounted-GIO
adapters without walking device files. CLI, desktop, and HASS render separate volumes; unavailable capacity never
becomes a misleading empty bar.

Mounted updates bind every path to one storage before mutation. Preflight checks peak demand per affected volume and
capture-space headroom. The transaction retains payloads and verified originals, records intent before each write,
verifies uploaded objects, and records committed or rolled-back state. Recovery rejects a different device, changed
objects, damaged evidence, ambiguous storage, and insufficient restore space.

Demo and dry-run use the same service and transaction flow through a directory-backed MTP shadow. The shadow preserves
source capacity metadata, accounts for simulated writes, never shares writable files with the source, and remains in the
capture for inspection.

The disposable link benchmark also runs through a shared `DeviceLink` boundary. Raw-MTP payload transfer ends at source
EOF, while device acknowledgement has its own finalization stage. Garmin split transfers reserve one final source byte
as a short USB transfer, avoiding the missing packet-boundary terminator in `mtp-rs` 0.32.0 without changing content. A
content-addressed cache holds host artifacts; a durable receipt protects each raw-MTP probe object until verified
cleanup. After an ambiguous response timeout, bounded attempts reconcile the exact object while retaining the session.
Reopening is reserved for a disconnected or reset session. A busy reopen is reported as an unidentified interface owner,
never attributed to a desktop process without evidence.

The HASS host maps attachment data once into the canonical model. Remoc carries that model to the WASM client; no
transport-specific capacity type exists. [ADR 0040](../decisions/0040-canonical-models-across-service-boundaries.md)
records and mechanically guards this boundary.

Update-plan schema version 1 preserves captures written before the version field existed. Its compatibility fixture
locks the canonical digest, defaulted fields, and current serialization. Unknown versions, changed totals, and changed
digests fail validation.

The selected recovery-backup policy is part of that plan identity. Verified backups remain the default. Interactive
confirmation exposes a default-on backup choice, while automation must pass the policy explicitly. Choosing `skip`
removes backup reads and retained backup bytes from preflight and execution; progress, completion, journal state,
errors, and recovery instructions then state that automatic rollback is unavailable and a reinstall may be required.
Changing the choice changes the plan digest, so an approval for a backed-up update cannot silently authorize an unbacked
update.

## Evidence

The repository validation gate exercises the workspace, gallery, dependency policy, generated interfaces, and Nix
packages. A loopback WebSocket test carries inspected capacity through Remoc. Capacity and recovery captures cover both
TUI fonts, supported terminal sizes, narrow and wide GUI layouts, HASS host states, unavailable capacity, long metadata,
concurrent work, completion, insufficient space, and blocked recovery.

Directory-backed transaction tests cover per-storage rejection, read-only media, replacement, addition, authorization,
removal, backup and upload failures, failed readback, cancellation, disconnection, marker collision, process
termination, partial writes, reopening, and retry. Service-boundary tests run changed-source, unsupported-authorization,
backup disconnect, upload, readback, cancellation, and wrong-device recovery through production orchestration. They
assert preserved source state, rollback where safe, capacity evidence, failure diagnostics, and no false completion
markers. Backup-policy tests additionally prove that an explicit skip performs no backup reads and never claims that
rollback was available.

## Limits

This evidence does not prove GIO behavior on every desktop, nested HASS ingress, physical firmware acceptance, or
recovery after an actual device disconnect. Those remain in [device state](../plans/device-state.md),
[mounted-device updates](../plans/mounted-device-updates.md), and the
[HASS watch slice](../plans/hass-watch-vertical-slice.md).
