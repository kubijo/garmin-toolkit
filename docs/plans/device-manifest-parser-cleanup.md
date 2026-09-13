# Device-manifest parser cleanup

Create an explicit parser extension point while retaining exactly one supported format: GarminDevice v2. Adding a future
v1, v3, or other-manufacturer parser must not require mixing its wire structures or validation rules into the v2
implementation. Metadata-file discovery remains Garmin-specific until another real format supplies concrete filenames
and locations.

The final scoped run passed all 56 `garmin-device` library tests plus warnings-denied production and demo Clippy for the
affected shared API, UI, desktop, and HASS crates.

Do not implement another manifest format or generalize metadata-file discovery as part of this cleanup. Verification
uses only explicitly authorized, scoped, memory-capped commands. Do not change the existing Git index or commit.
