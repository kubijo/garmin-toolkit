# Garmin Connect

## Support policy

Garmin Connect is built-in, opt-in, unofficial, and independent of USB. Reuse tokens; stop on authentication/rate-limit
failures; never evade controls. Reads need evidence; writes are explicit; destructive changes need separate validation.

Garmin's [Terms of Use](https://www.garmin.com/en-US/legal/terms-of-use/), reviewed 2026-09-04, prohibited automated or
manual access, copying, or scraping through unexposed means. Warn about blocking and recheck terms/authentication before
implementation and release.

## Existing implementations

- [`python-garminconnect` 0.3.12][gc-audit]: The pinned MIT-licensed audit covers broad reads and writes. Adapt only
  provenance-recorded material consistent with [ADR 0008](../decisions/0008-unofficial-garmin-connect-boundary.md).
  Exclude TLS fingerprint rotation, randomized client identity, anti-WAF, and fallback-login behavior.
- [Garth 0.8.0](https://github.com/matin/garth/commit/f99159a15c4c9463ce215a60ba9f7cb21f94a3b7): MIT-licensed
  authentication history only. Upstream deprecated it after Garmin changed the mobile authentication flow.
- [Rust-Garmin](https://github.com/poster515/Rust-Garmin/commit/81c115e1003139d51deeb17b0d43652684f9c328): Rejected
  implementation. GPL-3.0 compatibility is not the problem; its obsolete flow, secret handling, failure behavior, and
  absent tests are.
- [Garmin Connect Developer Program](https://developer.garmin.com/gc-developer-program/overview/): Reconsider only if
  Garmin offers a suitable personal/public agreement; it is not the baseline for this project.

Before enabling an operation, record region, purpose, response/mutation, retries, and date.

These revisions are historical audit evidence, not current dependency pins.

[gc-audit]: https://github.com/cyberjunky/python-garminconnect/commit/981d150caeda7d632224a75f3895c08df27a2a34
