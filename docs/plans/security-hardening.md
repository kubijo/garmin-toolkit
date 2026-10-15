# Security hardening

Add optional authentication and protection for credentials, storage, and snapshots without removing trusted passwordless
operation or binding users to external identities. Until then, local storage is plaintext, credentials are not
persisted, and no application-level confidentiality is claimed.

## Dependency updates

Upgrade `rustls` from `0.23.45` to `>=0.23.46` in the workspace and gallery lockfiles to resolve
[RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html),
[RUSTSEC-2026-0347](https://rustsec.org/advisories/RUSTSEC-2026-0347.html),
[RUSTSEC-2026-0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html), and
[RUSTSEC-2026-0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html). Regenerate dependency licenses and rerun
`just qa::full` and `just qa::audit`.

## Work

1. Model shared access, host compromise, copied storage, snapshots, recovery, and unattended HASS startup.
2. Choose reviewed implementations independently for passwords, target credential custody, database protection, and
   snapshot envelopes. Decide key-store versus manual unlock, recovery, and unattended HASS startup; preserve
   passwordless use.
3. Use only established, independently reviewed cryptography; create no primitives or protocols.
4. Define recovery, loss, password changes, passwordless users, and unattended operation before migration.
5. Implement behind existing boundaries with downgrade and failure tests.

Before making a protection claim, test that secrets cannot reach plaintext storage, exports, browser state, fixtures, or
logs, and that passwordless local use and recovery still work.
