# 0023: Deferred security hardening

## Decision

The data-foundation, HASS-watch, and shared-interface stages target trusted local deployment: passwordless profiles,
plaintext storage and snapshots, and no persisted connector credentials. HASS ingress and the desktop OS gate access but
never identify an application user.

Security hardening separately resolves passwords, target credential custody, database protection, and snapshot
encryption. Use independently reviewed implementations; invent no cryptography or key protocol. Garmin Connect requires
proven target custody before persisting credentials.

Keep users independent of authentication, storage replaceable, and snapshots envelope-able. Until security hardening,
secrets stay outside databases, snapshots, and exports. An evidenced design may supersede ADR 0014; plaintext output
stays secret-free and passwordless operation remains supported.

## Why

USB-only slices need none of these protections. Separation avoids an unreviewed all-purpose scheme while preserving
later choices.

## Consequences

Early releases make no application-level confidentiality claim. Define threats, recovery, loss, password changes, and
unattended HASS startup before migration or credential persistence.
