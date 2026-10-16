# Security hardening (deferred)

This entire stage is postponed, including authentication, credential custody, database protection, and snapshot
encryption. Preserve architectural compatibility with later choices. Do not run feasibility work or implement this stage
now, and do not make its policy decisions a prerequisite for current work. See
[ADR 0023](../decisions/0023-deferred-security-hardening.md).

Current operation remains trusted and passwordless, with plaintext storage/snapshots and no persisted connector
credentials or application-level confidentiality claim.

## When resumed

1. Revisit the [parked threat model and options](../research/security-hardening.md). None of its candidate mechanisms or
   suggested policies is an accepted decision.
2. Resolve shared access, host compromise, copied storage, snapshots, recovery, password/key changes, and unattended
   HASS startup before migrations or credential persistence.
3. Choose independently reviewed implementations separately for authentication, target credential custody, database
   protection, and snapshot envelopes. Invent no cryptography or key protocol.
4. Prove compatibility, key custody, protected staging, recovery, and failure behavior with isolated fixtures.
5. Implement behind existing boundaries with migration and downgrade tests while preserving passwordless operation.

Before making a protection claim, test that secrets cannot reach plaintext storage, exports, browser state, fixtures, or
logs, and that passwordless local use and recovery still work.
