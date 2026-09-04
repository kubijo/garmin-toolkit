# Garmin Connect

## Support policy

Garmin Connect is built-in, opt-in, unofficial, and independent of USB. Reuse tokens; stop on authentication/rate-limit
failures; never evade controls. Reads need evidence; writes are explicit; destructive changes need separate validation.

Garmin's [Terms of Use](https://www.garmin.com/en-US/legal/terms-of-use/), reviewed 2026-09-04, prohibited automated or
manual access, copying, or scraping through unexposed means. Warn about blocking and recheck terms/authentication before
implementation and release.

## Existing implementations

| Candidate                                                                                       | Treatment                                                                                                                                                                                                                                   |
| ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`python-garminconnect`](https://github.com/cyberjunky/python-garminconnect)                    | The pinned MIT-licensed audit covers broad reads and writes. Adapt only provenance-recorded material whose behavior fits ADR 0008; exclude its TLS fingerprint rotation, randomized client identity, anti-WAF, and fallback-login behavior. |
| [Garth](https://github.com/matin/garth)                                                         | MIT-licensed authentication history only. Upstream deprecated it after Garmin changed the mobile authentication flow.                                                                                                                       |
| [Rust-Garmin](https://github.com/poster515/Rust-Garmin)                                         | Rejected implementation. GPL-3.0 compatibility is not the problem; its obsolete flow, secret handling, failure behavior, and absent tests are.                                                                                              |
| [Garmin Connect Developer Program](https://developer.garmin.com/gc-developer-program/overview/) | Reconsider only if Garmin offers a suitable personal/public agreement; it is not the baseline for this project.                                                                                                                             |

Before enabling an operation, record region, purpose, response/mutation, retries, and date.

See the [pinned source audit](prior-art.md) for exact revisions and evidence.
