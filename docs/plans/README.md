# Active plans

Unfinished work only. On completion, move lasting contracts/evidence to their owners and delete the plan. Follow
[development safeguards and interface validation](../development.md) and
[single-path execution](../decisions/0037-single-execution-path.md). Close each working slice before advancing; run full
QA and audit, plus package evidence for deployment claims.

Current implementation focus: [HASS route-plan editing](hass-watch-vertical-slice.md). Device pairing and shared map
workflows have demo acceptance; add-on and owned-hardware checks remain open.

Other active plans: [shared interfaces](shared-interface-workflows.md), [portable data](data-foundation.md),
[security](security-hardening.md), [Garmin Connect and sharing](garmin-connect-and-sharing.md),
[device expansion and publication](device-expansion-and-publication.md), and [CLI distribution](distribution.md).

Cross-cutting release work: verify production/demo IDs, data roots, populated-database startup, and HASS environment
precedence; replace the global coverage floor with per-owner behavioral floors; audit crate boundaries after transaction
work; and resolve the Walkers → Bincode [unmaintained advisory](https://rustsec.org/advisories/RUSTSEC-2025-0141.html)
without suppression. After a cache-stack upgrade, recheck root and gallery locks, RustSec, native/WASM builds, and
focused map tests. Close release claims with packaged artifacts, audit results, and behavioral evidence.

[Build-performance evidence](../research/build-performance.md) records the deferred native hot-reload trial; desktop
restart and gallery scene reload remain the working tools.
