# 0023: Deferred security hardening

## Decision

Current deployments target trusted local operation: passwordless profiles and plaintext database storage. HASS ingress
and the desktop OS gate access but never identify an application user.

Application authentication, database protection and snapshot encryption remain postponed. The optional Garmin Connect
key is saved in a separate profile-scoped database table. This deliberately uses the deployment's existing plaintext
storage boundary; it makes no claim of encryption at rest. Local workflows acquire no new security prerequisites.

Profile settings conceal key entry and support replacement and removal. Profile responses contain only a saved-key flag.
Snapshot creation removes credentials from its private database copy and compacts it before archiving, so deleted key
bytes cannot remain in the exported SQLite pages. Raw deployment database copies contain credentials.

When broader hardening resumes, use independently reviewed implementations; invent no cryptography or key protocol. Keep
passwordless operation supported, storage replaceable and snapshots envelope-able.

## Why

The optional connector uses the deployment's existing storage boundary. A separate credential table keeps keys out of
portable user models and backups while preserving local and USB workflows.

## Consequences

Current releases make no application-level confidentiality claim. API validation, token renewal and expiry/revocation
handling remain connector work. Broader authentication, database protection and snapshot encryption remain separate.
