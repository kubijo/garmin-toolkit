# Security hardening

Add optional authentication and protection for credentials, storage, and snapshots without removing trusted passwordless
operation or binding users to external identities. Until then, local storage is plaintext, credentials are not
persisted, and no application-level confidentiality is claimed.

## Work

1. Model shared access, host compromise, copied storage, snapshots, recovery, and unattended HASS startup.
2. Resolve OQ-018 independently for passwords, credential custody, database protection, and snapshot envelopes.
3. Use only established, independently reviewed cryptography; create no primitives or protocols.
4. Define recovery, loss, password changes, passwordless users, and unattended operation before migration.
5. Implement behind existing boundaries with downgrade and failure tests.

Delete this plan when every claim names its threat model and reviewed implementation; secrets cannot reach plaintext
storage, exports, browser state, fixtures, or logs; failure and recovery paths are tested; and passwordless local use
still works.
