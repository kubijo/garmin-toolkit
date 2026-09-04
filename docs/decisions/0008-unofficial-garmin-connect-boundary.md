# 0008: Unofficial Garmin Connect boundary

## Decision

Ship a built-in, opt-in, unofficial Garmin Connect adapter. Support evidenced reads and explicit non-destructive writes.
Destructive or in-place mutations need separate live evidence and confirmation and never run in background sync.

Persist and refresh tokens. Stop on authentication failure until reauthentication and on rate limits until the supplied
or conservative cooldown. Never bypass CAPTCHA/WAF controls, rotate identities or addresses, impersonate TLS clients, or
try alternate login flows after refusal.

The replaceable adapter never gates local features. Preserve responses and downloads through immutable-artifact and
secret-scrubbing boundaries.

## Why

Cloud routing improves coverage, especially for Index S2, but Garmin offers no suitable personal API. Its terms prohibit
automation not purposely exposed by the service; code licensing cannot sanction or stabilize this path.

Independent projects provide evidence, not a wholesale dependency. `python-garminconnect` has broad current coverage;
Garth is deprecated; Rust-Garmin is obsolete. Adaptation requires exact provenance.

## Consequences

Users accept breakage and account-blocking risk. Claims name the tested region, operation, and date. Service changes may
disable cloud access but never trigger evasion or endanger local data.

The Garmin Connect stage selects endpoints from fresh evidence after security hardening resolves secret storage. An
official adapter may share the boundary.

See [Garmin Connect research](../research/garmin-connect.md).
