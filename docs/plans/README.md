# Plans

Follow the [development workflow](../development.md) and
[single-path execution](../decisions/0037-single-execution-path.md).

Physical-device evidence is recorded in [device capabilities](../research/device-capabilities.md). Signed releases and
HAOS guest qualification are recorded in [publication readiness](../research/publication-readiness.md).

Other active plans: [shared interfaces](shared-interface-workflows.md),
[Garmin Connect and sharing](garmin-connect-and-sharing.md),
[device expansion and publication](device-expansion-and-publication.md).

A separate CLI distribution channel is intentionally not currently planned.
Flatpak is an idea outside the active roadmap.

Deferred: [security hardening](security-hardening.md).

Cross-cutting release work: profile bindgen after real WASM edits and recheck the nightly `egui_adapter::Paint: Sync`
recursion diagnostic before upgrading Rust.
