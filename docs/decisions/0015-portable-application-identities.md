# 0015: Portable application identities

## Decision

Create portable users with immutable random IDs and mutable profiles. External accounts, hosts, and devices never
identify users; connectors are user-owned sources.

Each deployment has an owner. Trusted mode permits passwordless profile selection and provides no privacy boundary among
people with application access.

Storage and services carry ownership and actor context. Plaintext export requires authorization and remains distinct
from snapshots.

Defer passwords and encryption to independently reviewed implementations; invent no cryptography or key protocol. Until
then, persist no connector credentials and claim no application-level encryption.

## Why

Application identities keep USB, restore, and sharing independent of Garmin, HASS, and host identities. Early ownership
avoids a single-user rewrite; unreviewed cryptography adds risk outside the trusted deployment's threat model.

## Consequences

HASS ingress gates access but does not select a user. Passwordless profiles are convenient, not secure. Authentication,
encryption, recovery, unattended unlock, and credential custody await reviewed implementations.
