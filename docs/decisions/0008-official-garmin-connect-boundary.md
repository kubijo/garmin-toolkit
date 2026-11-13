# 0008: Official Garmin Connect boundary

## Decision

Use Garmin's official Personal API for an opt-in, read-only connector accessing the owner's account. Start with activity
listing and original-file downloads. Feed existing FIT validation, provenance and deduplication; cloud access never
gates local imports or USB operations.

Users create their own key on Garmin Connect. Exchange it for short-lived access tokens using the documented endpoint;
obtain a new token when needed. Never collect Garmin passwords or automate login, consent or key creation. Stop on
revocation, expiry or rate limits; no private-endpoint fallback or access-control evasion.

Store the owner-created key in a separate profile-scoped database table, as chosen for trusted local deployments in
[ADR 0023](0023-deferred-security-hardening.md). Settings may save, replace or remove the key; ordinary profile
responses report only whether one is saved. Keep secrets out of logs, captures, portable snapshots and exports.

## Why

Garmin now documents a Personal API offering activity JSON and original FIT downloads through owner-generated keys. The
phone can continue uploading normally while the toolkit imports cloud data independently of device pairing. Personal-use
eligibility, terms and service limits still need validation before release.

## Consequences

The initial connector has no cloud writes, sharing or commercial multi-user service. Garmin availability and device
uploads determine when new activities become accessible. Failed downloads create no acquisition. Confirmed imports
retain original bytes and normalization outcomes through the existing importer; rejected normalization creates no
activity. The separate Connect IQ and native Bluetooth experiments are not product dependencies.

See the [API evidence](../research/garmin-connect.md) and [implementation plan](../plans/garmin-connect-import.md).
