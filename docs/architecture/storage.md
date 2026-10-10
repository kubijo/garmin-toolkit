# Storage architecture

Each deployment owns one local SQLite database behind `garmin-storage`; callers receive no connection. Opening enables
foreign keys, WAL, full synchronous durability, a five-second busy timeout, and embedded migrations.

The schema stores identities, sources, content-addressed artifacts, provenance, observations, associations, activity
projections, avatars, and route plans. Domain UUIDs remain stable as labels change.

One transaction writes acquisition through projection and fingerprint association. Every activity observation requires
one projection; immutable conflicts abort, exact retries do nothing, and partial reads never become artifacts.

Avatar import atomically stores the original, acquisition, thumbnail, derivation, and selection. Only that user's linked
avatar may be selected; replacement retains prior artifacts.

GPX import atomically registers the source and stores original bytes, acquisition, one selected initial revision, and
its durable receipt. The receipt records the original track/segment or route locator and parser identity/version. Each
confirmed selection has its own host-issued acquisition operation; imports of identical files share a byte blob while
retaining distinct artifact and plan identities.

The receipt binds the owner, source, source filename, exact input digest, candidate, name, and sport. Retry lookup runs
before parsing and returns the original result after restart or subsequent route edits; changed arguments conflict. Host
timestamps do not change a committed retry. Concurrent confirmations recheck the receipt under SQLite's immediate write
transaction. A failed receipt write rolls back the source and every import record. Legacy routes remain readable; their
unavailable candidate locators are not inferred or fabricated during migration.

Activities support running, cycling, swimming, walking, and hiking; route revisions support all except swimming. Sport
expansion preserves existing activity details, route revision chains, cues, transformations, and current heads.

Public reads rebuild validated model and FIT types from private rows. Complete activity reads use one owner-scoped
snapshot and verify semantics against the observation fingerprint. Route detail reads also use one SQLite snapshot;
`route_plan` follows the current head, while `route_revision` returns the requested immutable revision within its
owner's scope, including historical revisions.

## Generated FIT Courses

`Application::generate_course` resolves an owned, exact route revision and encodes only its persisted geometry. Each
generation has its own identity, artifact, version within that revision, FIT serial, encoder identity/version, and
generation timestamp. FIT encoding leaves the source geometry unchanged; coordinate and elevation quantization belongs
to the encoder. Unresolved control points cannot generate a Course.

Storage first reuses the newest surviving artifact with the same owned revision and encoder identity/version. Repeated
requests and concurrent generation therefore return the same bytes and serial. A changed revision or encoder creates a
new artifact; deleting all matching artifacts allows a fresh generation. Older artifacts remain available as history.
Paged reads put the current encoder's newest matching artifact first, even after an encoder rollback, followed by the
remaining history in descending version order.

Storage holds a SQLite immediate write transaction while checking reuse and encoding the selected revision. It allocates
the revision version and database-wide serial, then commits the immutable bytes and generation receipt together. Failed
encoding, dropped work, and failed writes roll back both allocations. The serial counter is independent of generation
history deletion and rejects exhaustion instead of wrapping. The transaction serializes concurrent writers; callers must
handle the normal database busy timeout for unusually long work.

A host-issued generation operation ID binds its owner and exact revision. Reused artifacts retain a receipt for each
operation, including after deletion and snapshot restore. A committed retry returns the original receipt before
encoding, including after a restart, encoder upgrade, or later route edit. Reusing it for another revision conflicts; a
new operation reuses matching output or creates the next version when no match remains. Identical bytes from distinct
encoder versions may share a content blob, while artifact and generation identities remain distinct.

Deleting a generated Course removes only that owned version and its artifact, reclaiming the content blob when no other
artifact references it. The route and original GPX remain. A deletion tombstone retains the operation ID and
revision/version allocation, so deletion retries are idempotent, old generation requests cannot resurrect the artifact,
and later versions and FIT serials are never reused. Tombstones survive portable snapshots. Already prepared downloads
may finish with their retained bytes; new download requests cannot obtain a deleted artifact.

`generated_courses` lists versions within the owning profile. `course_artifact` authorizes through the owning route,
verifies length and digest, and returns saved bytes without regeneration. Portable snapshots include original GPX, route
revisions, Course bytes, receipts, and the serial counter. Host adapters must hold the deployment lease through
generation and delivery; these services do not themselves issue download tickets or write to devices.

## Route client workflow

Desktop and HASS use one `RouteOperations` registry per host. A session binds an existing profile and the deployment
epoch captured at request time. Uploads accept contiguous, repeatable chunks of at most 64 KiB; each file is limited to
16 MiB. Four uploads and sixteen generation intents may be retained for fifteen minutes. Cancellation, expiry, or a
database epoch change drops retained input and aborts an active parser. Confirmation retains the approved preview
through its transaction and cannot be cancelled halfway through publication.

The [contained GPX helper](../research/gpx-route-plans.md#native-parser-containment) produces the retained preview.
Clients receive candidate summaries in pages of sixteen and exact geometry in chunks of at most 2048 points. Route lists
and Course history use bounded pages as well. The client must select a candidate, name, and one of walking, hiking,
running, or cycling. Unresolved controls can be saved but cannot generate a Course. Shared map input accepts planned
coordinates directly; it does not create an activity recording or fabricate recorded metrics.

Downloads authorize the artifact through an owned route and verify the persisted bytes. A prepared download retains its
original epoch but does not hold a database lease while waiting for the user. Delivery acquires an owned lease; restore
drains active deliveries, while unused tickets become stale. At most eight route downloads are retained per host. HASS
wraps them in the existing bounded, single-use download tickets. Desktop writes a temporary file beside the chosen
destination, syncs it, and publishes without replacing an existing file. Neither adapter regenerates Course bytes during
download or writes to a device.

## Database integration

SQL lives under `queries`; migrations under `migrations`. SQLx checks a fresh migrated database and commits offline
metadata under `.sqlx`.

HASS uses a managed deployment under `/data`; desktop uses platform application data. Live copies are unsupported:
external tools consume a completed snapshot or stopped store.

## Portable snapshots

`Application::snapshot` and `prepare_restore` require an existing owner profile. Storage creates a `VACUUM INTO` copy
under an exclusive application borrow. A snapshot contains `manifest.json`, then `storage.sqlite3`, in one checksummed
Zstandard frame. The manifest records format/app versions, creation time, migration checksums, database length, and
SHA-256. No host directories or device pairing files are included.

Creation and restore stage privately and verify length, digest, SQLite integrity, foreign keys, and migration
compatibility before publication. A snapshot must match a nonempty prefix of the embedded migrations, including their
checksums; opening the restored database applies any remaining migrations. Unknown or changed migrations are rejected.
Defaults cap compressed input and database size at 1 GiB and the decoding window at 8 MiB. Dropping a restore removes
its stage. Low-level publication refuses existing destinations.

`Deployment` owns the process lock and application leases. Every host job must hold a lease with its database epoch;
restore drains leases and revokes queued work before closing the old store. Verified databases publish under
`storage-generations/<id>/storage.sqlite3`. An atomic selection record chooses the live generation and retains the
previous one. The original `storage.sqlite3` remains intact.

A durable journal precedes selection changes. An interrupted switch selects the previous database on startup; a
completed switch opens the restored database. Failed reopening rolls back, and failed recovery leaves the application
unavailable. Startup removes abandoned upload stages and unreferenced generations. The original and previous selected
databases are retained. Hosts must open through `Deployment` to honor selection and recovery.

`SnapshotOperations` binds the shared Remoc service to a host-supplied owner actor. Requests carry opaque operation IDs,
while the host-local file adapter accepts explicitly selected server paths. Transfers use contiguous chunks of at most
64 KiB; the registry holds at most 16 operations. Verification produces a preview and single-use approval bound to the
upload, connection, and database epoch. Reconnect uses `List` to discover operations and `Resume` to rotate approval and
revoke the previous connection. Status and transfer offsets survive reconnect; operation IDs do not survive process
restart.

The browser discovers retained operations when the owner opens the backup page after a reload. Discovery includes the
original source and connection ownership, without exposing approval tokens. Explicit `Recover` only claims abandoned
work and rotates its approval; a live client or HTTP download retains ownership. `Resume` remains the reconnect path for
an existing runner. Incomplete browser uploads require discard and reselection; server file operations and verified
uploads can continue. Discard cancels and releases the stage, while an approved switch remains non-cancellable.

Progress reports phases and transferred bytes. Cancellation discards pre-approval work and its eventual result; an
approved database switch completes independently of client disconnection. Clients release completed downloads and
terminal operations. Desktop uses this service through a native file adapter, with atomic save publication and explicit
restore approval. Its background runtime keeps progress and cancellation responsive independently of ordinary jobs.
Desktop clears profile, activity, playback, avatar, and pending-import state on epoch change, then reloads profiles.
HASS uses the same deployment and operation engine. Browser uploads use bounded chunks; downloads use single-use HTTP
streams. Device files and snapshots share one download ticket, registry, and `/download/{token}` endpoint. Tickets
expire after 60 seconds; the registry allows eight pending or active downloads and at most 512 MiB of staged device
files. Reservations remain held until delivery and source cleanup finish. Completion, expiry, and abandoned responses
release the source; duplicate snapshot tickets cannot cancel the original transfer. Workers and download handles remain
bound to the operation they started with; reusing a released request ID cannot redirect old reads, updates, or cleanup.
Server file selection reuses the application explorer with directory-at-a-time RPC, open/save controls, and explicit
overwrite confirmation. Host-local files pass through the same verification and approval lifecycle. After a database
switch, old host connections reject database calls and the browser discards stale request results. An operation's
original actor can reconnect to read its terminal outcome even if restore removed that profile; this restricted session
cannot start operations or read archive contents.

Desktop and HASS demo initialization runs under the deployment lock before the first selection record is published. It
is idempotent for interrupted startup and is skipped once a database is selected, including after restore.

The `tar` and `zstd` libraries own format parsing. Only the two named regular entries are accepted; links, extension
records, extra entries (including after tar end markers), trailing compressed data, and multiple frames are rejected.
Library validation accepts tar streams without end blocks and Zstandard frames without a checksum. Our writer emits
both; restore always verifies the database SHA-256. Strict rejection of those omissions is not claimed.

The HASS portable snapshot acceptance test transfers an archive containing a deterministic 64 MiB opaque artifact from a
native file through upload, approval, and HTTP download, then reopens the resulting database and verifies its profiles
and artifact digest. Native controller tests cover file publication, approval, cancellation, and restart separately.

Snapshots remain plaintext pending the encryption decision. Restore does not change device markers; a marker whose user
UUID is absent from the restored profiles needs review. Cloud authentication must be reestablished; open export is
separate.

See [relational reconciliation](../decisions/0019-relational-reconciliation.md) and
[deferred security](../decisions/0023-deferred-security-hardening.md).
