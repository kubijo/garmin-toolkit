# Plans

Follow the [development workflow](../development.md) and
[single-path execution](../decisions/0037-single-execution-path.md).

[HASS hardware acceptance](hass-watch-vertical-slice.md) covers physical devices. Signed releases and HAOS guest
qualification are recorded in [publication readiness](../research/publication-readiness.md).

Other active plans: [shared interfaces](shared-interface-workflows.md), [portable data](data-foundation.md),
[Garmin Connect and sharing](garmin-connect-and-sharing.md),
[device expansion and publication](device-expansion-and-publication.md), and [CLI distribution](distribution.md).

Deferred: [security hardening](security-hardening.md).

Cross-cutting release work: replace the global coverage floor with per-owner behavioral floors and audit crate
boundaries after transaction work. Profile bindgen after real WASM edits and recheck the nightly
`egui_adapter::Paint: Sync` recursion diagnostic before upgrading Rust.
