# 0036: Persistent identity compatibility

## Decision

Change visible branding and build identities without changing persisted identity by accident.

Production desktop uses `io.github.kubijo.GarminToolkit`, but opens an existing database under `io.github.kubijo.nimrag`
when the new data root has no database. Home Assistant reads `GARMIN_TOOLKIT_HASS_DATA_BASE` first and accepts
`NIMRAG_HASS_DATA_BASE` as a compatibility fallback.

Existing UUID v5 and BLAKE3 domain bytes remain unchanged and are named `LEGACY_*_V1` in code. They are data-format
constants, not branding. A future domain version requires an explicit data migration and compatibility vectors.

On-device markers and transaction files retain their current names until their writers and recovery readers can move
through a versioned, interruption-safe transition.

## Why

Changing an app ID, data path, or deterministic-ID input can make valid user data disappear or produce duplicate
records. Renaming is not sufficient justification for that break.

## Consequences

Legacy strings remain only in documented compatibility code, fixtures, and historical records. The desktop does not copy
a live SQLite database; it continues using the old path until a later migration can acquire exclusive access and move
the database with its sidecars atomically.
