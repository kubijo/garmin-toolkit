# Security hardening options (parked)

Parked research from 2026-10-15 against the application boundaries at `6818d1c`. These are unaccepted options for a
future stage, not defaults, implementation commitments, or current protection claims.
[ADR 0023](../decisions/0023-deferred-security-hardening.md) remains the accepted contract;
[the deferred plan](../plans/security-hardening.md) owns future work.

The entire security stage is postponed: authentication, credential custody, database protection, and snapshot
encryption. The options, experiments, and policy choices below stay parked during current work. Current development
preserves compatible boundaries and trusted passwordless operation without implementing this stage.

## Assets and boundaries

Protect recorded locations, activity originals, route plans, profiles, full-deployment snapshots, and future connector
credentials. Authorization also controls device mutations, owner administration, imports, and restore. A pairing marker
is identification data, not a credential or proof that its reader owns a profile.

The deployment contains one SQLite database, including content-addressed artifact BLOBs. Profile UUIDs scope service
operations; they are currently selected by the caller. `UserContext::new` is not authentication. Snapshot operations
check the selected profile's owner role but do not prove who selected it. See [data](../architecture/data.md),
[storage](../architecture/storage.md), and [`UserContext`](../../crates/garmin-services/src/lib.rs).

HASS production ingress checks the network peer. The app does not currently map the authenticated HA user to an
application identity. HA documents `X-Remote-User-Id` on requests forwarded through Supervisor ingress; this supplies a
candidate authentication input without changing application UUIDs. Names and display names are presentation data. Only a
verified ingress connection may supply that identity; accepting the same header from a direct client would let the
client impersonate another user. [HA app security](https://developers.home-assistant.io/docs/apps/security/).

## Threat model

| Threat                             | Proposed defense                           | Limit                             |
| ---------------------------------- | ------------------------------------------ | --------------------------------- |
| Another visitor selects my profile | Validated actor and authorization          | Trusted mode has no isolation     |
| Forged UUID or ingress header      | Derive the actor at the host boundary      | The proxy remains trusted         |
| Stale session after logout/restore | Revoke sessions, jobs, tickets, and popups | Exports cannot be recalled        |
| Copied database or WAL             | Database encryption; separately held keys  | An unlocked host can read data    |
| Copied portable or HA backup       | Encrypt before publication; exclude keys   | Old plaintext copies stay exposed |
| Secrets in logs, caches, browser   | Minimize retention; test output boundaries | Activity exports reveal data      |
| Expensive unlock requests          | Bound KDF work and concurrency             | Host availability still matters   |
| Lost recovery key or credential    | Tested restore and reauthentication        | Lost keys may mean lost data      |

Host administrators, root, a compromised unlocked app process, and hostile code running as the same desktop account
remain outside the confidentiality guarantee. They can read process memory or use unlocked key stores. File permissions
and encryption do not establish isolation from those actors. Encryption also does not prove a backup is recent: an
authorized restore may intentionally select an older, valid snapshot.

Garmin devices, firmware, third-party cloud services, and deliberately exported FIT/GPX files remain separate trust
boundaries. Database encryption does not encrypt files written to a device. Previously exported data and retained
plaintext storage generations cannot become confidential through an application upgrade.

The deployment owner remains a trusted administrator with full-deployment snapshot access, including every profile's
data. Authenticated access would isolate ordinary members; it would not establish secrecy from that owner.

## Future design options

Keep the existing trusted, passwordless deployment. Evaluate encrypted storage and encrypted snapshots independently,
with distinct protection claims. Storage unlock provides encryption keys; it does not authenticate an application
profile. A later authentication feature would be independent of these storage choices and would need to remove
unrestricted profile selection throughout an authenticated deployment.

### Deferred authentication design

On HASS, prefer explicit owner-approved mappings from stable HA user IDs to application UUIDs. An unmapped HA user gets
no profile access. Changing a display name cannot change a mapping; an HA administrator is not automatically the app
owner. Enrollment must require an existing authenticated owner or a separately designed host-local bootstrap, never the
first remote visitor. Keep standalone application credentials available for desktop and deployments without ingress.

For local passwords, use an established Argon2id implementation with random salts and encoded parameters. Start
evaluation at OWASP's 19 MiB, two iterations, one lane minimum; benchmark and bound concurrent verification before
choosing release parameters. Rehash after successful login when the policy changes. Password verification is separate
from encryption-key custody: changing a login password must not silently rotate or destroy database keys.
[OWASP password storage](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html).

Create the service actor from a validated session at the host, not a request's UUID. Check ownership for reads, writes,
device mutations, exports, and owner-only operations. Shared profile metadata must not expose another user's activity
content. Bind HTTP downloads, Remoc connections, file popups, and background work to that actor and its authorization
generation. Revoke on logout, credential reset, mapping removal, profile deletion, and database restore. Reconnecting
after restore may retain only the existing restricted terminal-status behavior, never archive access.

Use maintained session mechanisms and OS randomness rather than a new token protocol. Decide transport/session
lifecycle, expiry, reauthentication for destructive owner actions, CSRF protection, and WebSocket origin handling before
implementation. Persist no bearer token or password in browser local storage, URLs, automation output, or screenshots.
Keep production control endpoints disabled; test control must not bypass authorization in authenticated fixtures.
Require protected transport for credentials and unlock material, except explicitly trusted host-local connections. Only
verified proxy requests may establish forwarded transport properties; enabling authentication cannot make an otherwise
exposed plaintext listener safe.

### Credential custody and unattended startup

Use a target-owned credential provider; persist only opaque references in application configuration and records. On
Linux desktop, evaluate Secret Service using an isolated test bus and keyring. Its specification permits locked
collections and does not mandate per-application access control, so it cannot support a claim of isolation from other
processes in the same login session. A missing or locked provider must not cause a plaintext fallback. Keep account
names and tokens out of provider lookup attributes, which may be stored unencrypted.
[Secret Service introduction](https://specifications.freedesktop.org/secret-service/latest/ch01.html),
[locking](https://specifications.freedesktop.org/secret-service/latest/unlocking.html), and
[access-control limits](https://specifications.freedesktop.org/secret-service/latest/ch10.html).

HASS has no assumed desktop keyring. Manual unlock after restart and a separately protected external key provider are
candidate approaches for encrypted storage. A locked service could expose key-unlock controls while withholding profile
data, jobs, and device mutations. The future design must establish how the deployment operator supplies keys without
depending on encrypted profile records. No startup or unlock policy has been selected.

Storing an automatic unlock key beside the database, in app options, or in the same Supervisor backup does not protect
against a copy containing both. Such a mode would need a narrower, explicit protection claim. Do not persist HASS
connector credentials until its provider, backup exclusion, unlock, and recovery behavior are proven. Encrypted portable
backups can ship independently of that provider.

### Database and snapshot encryption

Evaluate SQLCipher for the whole deployment database, not per-profile keys. This matches the existing shared BLOB and
snapshot boundaries. SQLCipher documents encrypted database and WAL pages, but also requires attention to unencrypted
temporary storage. It replaces SQLite rather than loading as an extension. No drop-in compatibility with the current
SQLx build is established. [SQLCipher design](https://www.zetetic.net/sqlcipher/design/).

The pinned `sqlx-sqlite` 0.9.0 uses `libsqlite3-sys` 0.37.0; the latter exposes SQLCipher features. Feature availability
alone does not prove migration, query checking, packaging, or snapshot correctness. A contained compatibility spike must
establish all of these before replacing storage. Include wrong/missing keys, WAL, temporary files, crashes, failed
migrations, and downgrade rejection. Avoid keys in SQL tracing, connection-string diagnostics, and core dumps.

For portable snapshots, evaluate the standard age envelope around the existing tar/Zstandard payload. The format
supports recipient encryption and streaming. A saved public recipient can allow unattended backup creation without
holding the private restore identity. Keep that identity outside deployment backups and test recovery on a fresh
deployment. [age specification](https://c2sp.org/age).

The Rust `age` 0.12.1 library documents streaming and interoperability with the reference implementation, but labels
pre-1.0 releases beta. Candidate selection still requires a review of implementation maturity, applicable independent
audits, dependencies, licensing, limits, and cross-tool fixtures. Neither the format name nor this proposal establishes
that the selected Rust dependency has been independently reviewed. [age crate](https://docs.rs/age/0.12.1/age/).

Current snapshot creation uses a plaintext `VACUUM INTO` file; restore stages a plaintext database. Adding encryption
only to the final download would leave those files exposed. Prove a bounded protected staging strategy and cleanup on
failure, cancellation, crash, and restart before claiming encrypted backup processing. Verify the authenticated end of
the envelope before approval or publication. Retain existing archive, migration, integrity, and size checks after
decryption; bound envelope parsing and password work before accepting untrusted input.

### Recovery and migration

Keep key recovery and cloud reauthentication separate. Losing the only database key requires a decryptable backup or
loss of that data. Losing every snapshot identity makes those snapshots unrecoverable. No hidden recovery key,
password-derived homegrown envelope, or automatic plaintext downgrade is proposed. Future application authentication
would add account recovery; resetting a login must not imply recovery of a missing decryption key.

Encrypting an existing deployment requires explicit approval and rollback planning. Verify the new storage generation
before selection, recover safely after interruption, and make retained plaintext originals and backups visible to the
operator. Deleting a file does not establish secure erasure on SSDs. Disabling protection likewise requires explicit
deployment-operator approval and a plaintext export/migration; older builds must not silently reopen protected state.
Portable snapshots continue to exclude connector credentials and target key-store entries. If application authentication
is added later, restored identity mappings and authentication state need a policy that prevents account takeover on
another deployment.

## Questions for the future stage

1. Choose key custody and unattended HASS startup behavior against an agreed copied-storage/backup threat model. Manual
   unlock and a separately protected external provider remain options, not selected policies.
2. Choose snapshot encryption and recovery material, including recipient keys versus passphrases, distribution,
   replacement, and loss. No recovery method has been selected.

When this stage resumes, revisit these options before recording accepted contracts or running compatibility work.
Key-store, SQLCipher, and snapshot-envelope experiments are also postponed. These questions do not block current work.
