# Plans

Unfinished work only. On completion, move lasting contracts/evidence to their owners and delete the plan. Follow
[development safeguards and interface validation](../development.md) and
[single-path execution](../decisions/0037-single-execution-path.md). Close each working slice before advancing; run full
QA and audit, plus package evidence for deployment claims.

Current implementation focus: the
[installable HASS experimental release](device-expansion-and-publication.md#hass-experimental-release), with
[HASS hardware acceptance](hass-watch-vertical-slice.md) tracked separately.

Other active plans: [shared interfaces](shared-interface-workflows.md), [portable data](data-foundation.md),
[Garmin Connect and sharing](garmin-connect-and-sharing.md),
[device expansion and publication](device-expansion-and-publication.md), and [CLI distribution](distribution.md).

Deferred stage: [security hardening](security-hardening.md). Keep its future options possible without requiring
decisions or implementation during current work.

Cross-cutting release work: replace the global coverage floor with per-owner behavioral floors and audit crate
boundaries after transaction work. Profile bindgen after real WASM edits and recheck the nightly
`egui_adapter::Paint: Sync` recursion diagnostic before upgrading Rust. Close release claims with packaged artifacts,
audit results, and behavioral evidence.
