# Account activity trends

The shared Trends page implements the design explored in the temporary HTML proposal. The HTML is a visual reference
only; aggregation, interaction tests, and maintained captures belong to the Rust implementation.

## Data contract

- Read the profile's current credential-bound, published summary-cache generation. A read never starts a network
  request, original-file download, or import. An explicit **Sync activities** action uses the existing cache refresh.
- Count each remote activity ID once, independently of whether its original FIT is downloaded, unavailable, or imported.
  Local-only imported activities are not added to these account totals.
- Bucket by each summary's local date. Do not guess a date from GMT when the local date is absent or invalid; report
  excluded summaries explicitly.
- Retain counts when distance or duration is missing. Known-value totals carry missing-field counts. Unknown chart
  samples have a separate treatment from measured zero; cumulative values remain unknown after a gap.
- Duration means the duration field supplied by the Garmin activity summary. It is not reconstructed from FIT samples or
  substituted with a different elapsed/moving-time statistic.
- Active days count distinct local dates, including multiple activities and different sports on the same date once.
  Preserve running, cycling, swimming, walking, hiking, and uncategorized activities.

## Periods and preferences

- Day, calendar week, month, and year share one aggregation path. ICU supplies the client's locale week start; Jiff
  performs civil-date arithmetic. Date labels follow the client's locale.
- Compare the same elapsed calendar window with the preceding period; clamp dates to shorter months. Past periods are
  complete, while the current period ends today. Legends include the actual dates and years.
- Profile units affect displayed distance. Optional monthly distance targets for running, cycling, and swimming are
  persisted in profile preferences, canonically as whole kilometres. Zero means no configured target.
- Rings represent explicitly configured monthly targets. Other periods show sport totals without invented goals. Editing
  language, theme, units, or file-window preferences preserves targets.

## UI and verification

Desktop and HASS use the same page, chart renderer, cache revision handling, and preference-saving paths. Cache read
failures retain the last successful report within the same published generation. A generation change clears the previous
report and pinned date before rendering, including when another client replaces credentials. Stale responses cannot
overwrite a newer requested revision. Sync progress, cancellation, and completion feedback reuse existing shared
components and backend operations.

The maintained scenes live beside the implementation in `crates/garmin-ui/src/trends.scene.rs`; captures are configured
in `infra/gallery/captures/trends.capture.toml`. Tests cover aggregation and calendar semantics, cache ownership and
credential replacement, preference persistence, stale reads, and explicit sync versus local browsing.

The deterministic history includes rest days, one to three activities per day, varied training durations, a known
zero-distance session, independent previous-month activity, and a seasonal history with an empty month. The
missing-field variant removes one distance and one duration while preserving activity counts.

## Remaining scope

Custom date ranges, local-only history combined with account history, cross-source deduplication, activity-to-activity
comparisons, and FIT-derived elevation/performance metrics remain separate work. They must not be inferred from missing
summary fields or FIT download coverage.
