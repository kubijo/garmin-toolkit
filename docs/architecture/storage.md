# Storage architecture

Each deployment owns one local SQLite database behind `garmin-storage`; callers receive no connection. Opening enables
foreign keys, WAL, full synchronous durability, a five-second busy timeout, and embedded migrations.

The schema stores identities, sources, content-addressed artifacts, provenance, observations, associations, activity
projections, avatars, and route plans. Domain UUIDs remain stable as labels change.

One transaction writes acquisition through projection and fingerprint association. Every activity observation requires
one projection; immutable conflicts abort, exact retries do nothing, and partial reads never become artifacts.

Avatar import atomically stores the original, acquisition, thumbnail, derivation, and selection. Only that user's linked
avatar may be selected; replacement retains prior artifacts.

GPX import atomically stores its source and one selected initial revision. Candidates may share an artifact but remain
separate plans. Exact retries do nothing; immutable conflicts abort.

Public reads rebuild validated model and FIT types from private rows. Complete activity reads use one owner-scoped
snapshot and verify semantics against the observation fingerprint.

SQL lives under `queries`; migrations under `migrations`. SQLx checks a fresh migrated database and commits offline
metadata under `.sqlx`.

Both targets use `storage.sqlite3`. HASS stores it under `/data`; desktop uses platform application data. Live copies
are unsupported: external tools consume a completed snapshot or stopped store.

The future portable snapshot API remains actor-aware and opaque for later encryption. Restores require device pairing
and cloud authentication; plaintext export is separate.

See [single SQLite storage](../decisions/0014-single-sqlite-storage.md),
[relational reconciliation](../decisions/0019-relational-reconciliation.md), and
[deferred security](../decisions/0023-deferred-security-hardening.md).
