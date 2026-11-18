# Official Garmin Connect imports

## Outcome

Import uploaded activities through the official Personal API while the device remains paired with its phone. Retain
original FIT bytes, source account/activity identity and existing deduplication behavior. Local and USB workflows remain
independent. [ADR 0008](../decisions/0008-official-garmin-connect-boundary.md) defines scope;
[API research](../research/garmin-connect.md) records the documented contract and uncertainties.

Configured profiles expose **Garmin activities** in navigation. The first load enumerates all account summaries and
caches original FIT archives in the profile database, independently of activity acquisition and import. Subsequent loads
and **Refresh** read recent pages until a page overlaps the cached index, update summaries from those pages, preserve
older cached history and retry missing originals throughout the account. **Full refresh** enumerates the complete
history to reconcile older edits and deletions. Backdated additions beyond the overlap require a full refresh. One
authenticated session serves either refresh; already downloaded originals are reused. Individual unavailable files
remain searchable. Connection or rate-limit failures stop downloading, retain completed files and show a warning; cached
metadata remains browsable.

Live progress reports connection, indexed pages and discovered summaries while the total remains unknown, then original
file completion with new downloads, reused cache entries, unavailable files and remaining work. Desktop events and a
coalescing browser watch channel carry the same profile-scoped updates; completion clears transient progress state.

Refresh cancellation is request-scoped. It stops before another remote request or metadata publication, retains the
previous published index when summary loading is cancelled, and finishes persisting an original already in flight. RPC
disconnection signals cancellation to an owned refresh task, which retains its deployment lease through cleanup.
Cancellation returns the currently published page as a normal outcome; cancelling while queued leaves the client's
current view intact without waiting for another job's lock. Page imports keep their separate cancellation control and
explicit-import boundary.

Fuzzy search matches unordered words across the whole cached account's names, descriptions, activity type keys,
local/UTC start times and IDs. Alphabetic words tolerate minor insertions, deletions and transpositions; short terms and
numbers use literal substrings. Type, inclusive local-date and **Not yet imported** filters apply before pagination,
which reports the exact matching total. Applying or clearing filters starts at page one. Paging and filtering read the
cache without contacting Garmin. Results retain chronological order.

Cache publication is atomic: interrupted summary enumeration retains the previous searchable snapshot. The cache is
scoped to the local profile and Garmin account, and its active index is bound to the credential fingerprint. Switching
accounts cannot reuse another account's originals. Cached summaries and originals persist across restarts, separately
from imported observations. The cache is part of the database and its backups; credentials remain excluded from backups.

Opening an activity previews cached metadata and its original FIT. **Import FIT** explicitly persists observations
through the existing importer. Confirmation binds the profile, activity ID, account and exact cached FIT bytes; an exact
previously imported file has import disabled. Importing does not download the original again.

**Import all on this page** snapshots up to 20 visible activities, skips imported rows and processes the rest
sequentially through the same FIT checks. Cancellation stops between files and retains completed imports. The summary
reports imported, skipped and failed counts; access, connection and rate-limit failures stop the batch. There is no
automatic import. Optional auto-sync refreshes the account cache and downloads original FIT files on a per-profile
schedule. Loading the cloud browser also populates that cache.

Batch preparation returns only the activity ID, exact-byte digest and imported status. Interactive preview responses are
capped at 8 MiB of encoded data before entering the browser RPC transport; larger recordings can still be imported
through page import. A failed local-list refresh preserves committed imports and shows an error; the next cloud request,
including **Refresh**, retries the local list. Changing pages or refreshing clears the previous batch summary.

## Qualification

Owner acceptance on 15–16 November 2026 confirmed desktop scheduling, restart persistence, rate-limit recovery and HASS
execution with the browser closed. These are completed checks, recorded in the
[live evidence](../research/garmin-connect.md#owner-acceptance).

Remaining owner qualification:

- Compare downloaded FIT bytes with a manual original export.
- Verify expiry/revocation against the live service. Explicit requests currently obtain fresh access tokens.

Other accounts/regions, a guaranteed historical coverage window and numerical request quotas remain limits of the
available evidence, rather than completed checks inferred from this account's acceptance.

As of 16 November 2026, a CI rerun of the HASS browser backup/restore workflow with the core job runner enabled remains
pending after the deployment-lease deadlock fix. Native regression tests cover the fix; successful live account sync
does not qualify that separate browser workflow.

## Configurable auto-sync

Auto-sync is opt-in, per-profile incremental cache refresh and original FIT downloads. Activity import stays explicit;
full-history reconciliation stays available through the Refresh menu. Configuration is **Off** or **Every n hours / days
/ weeks**, backed by a domain type containing a positive count and unit, with checked duration conversion. The chosen
unit is preserved in storage and the UI uses ICU plural messages.

Timed incremental refresh inherits the overlap limit: backdated uploads and older edits or deletions still need full
reconciliation. An optional second full-refresh cadence would provide automatic reconciliation of older history;
incremental-only scheduling does not promise it.

Timing uses elapsed intervals: a day is 24 hours and a week is seven days. Due times use the user's locale and local
time zone. This gives a stable duration across daylight-saving changes; recurrence at the same local clock time would be
a separate scheduling feature. Schedule from the last successful completion, including a successful manual refresh.
Enabling a previously unsynchronised profile runs once immediately. On restart or resume, perform at most one overdue
run, then calculate the next due time; accumulate no missed-run backlog.

Cancellation defers the next automatic attempt by one configured interval so a timer cannot immediately undo a user's
cancel action; it does not advance last successful completion. Changing the interval recalculates the next due time from
the last success while respecting a current cancellation deferral.

The profile database persists the schedule, last attempt, last successful completion, next due time and redacted failure
state. Failure retries use bounded backoff separately from the configured cadence. Server retry deadlines survive
restarts and interval edits. Definitive credential rejection pauses attempts until credentials are repaired; changing
the interval does not unpause rejected credentials. Scheduled refreshes retry an expired session once with fresh
authentication before treating another rejection as a credential problem. Removing or replacing credentials must cancel
or invalidate work tied to the previous account. Disabling auto-sync cancels its current run at the same durable
boundaries as manual refresh, preserving downloaded originals.

The client preserves a parsed `Retry-After` delay from rejected responses, supporting seconds and HTTP dates without
retaining response headers or bodies in diagnostics.

Scheduled work consumes typed refresh failures directly rather than treating an `Ok` cached page with a warning as
successful completion. Authentication, rate-limit and network failures retain their classification. Individual
unavailable originals are coverage on a completed refresh and retry on the regular cadence. User cancellation defers the
next attempt; shutdown or deployment replacement interrupts the job and leaves it due for one catch-up after restart.
Both preserve the last successful completion.

The backend owns scheduling, so several HASS browser tabs cannot create duplicate jobs. HASS can sync while its server
is running with no browser open. Desktop sync runs while the application is running and catches up once when reopened;
running after application exit would require an additional daemon or OS integration. The shared core owns an Apalis
worker and its continuously driven runtime, independently of desktop command processing and browser connections. Desktop
and HASS use the same `Deployment::start_jobs()` entry point and retain its lifetime guard. Other hosting modalities can
use this entry point without implementing scheduling. Short-lived read-only commands do not start jobs.

Apalis SQLite provides persisted delayed dispatch, task claiming, acknowledgement, infrastructure retries and orphan
recovery. Garmin-specific cadence, credential pauses, cancellation deferrals and server retry deadlines remain explicit
application policy. Saving that policy and enqueueing its next job share one SQLite transaction. Jobs contain profile
identities and an expected due time, never credentials or FIT payloads. Changing credentials, resetting schedule state
and enqueueing are also one transaction, so failed account changes preserve the previous credential and timing. Replaced
schedules produce harmless stale jobs; handlers revalidate the current due time under the profile mutex before any
network request. Delivery is at least once, so application handlers remain idempotent.

The queue shares the application database and its backup/restore boundary. The application migration embeds the pinned
Apalis schema; a parity test checks it against upstream migrations. It preserves synchronous FULL writes instead of
calling the library's convenience setup, which changes durability PRAGMAs. Library upgrades must retain schema parity or
add a normal application migration. On startup and after restore the core reconciles saved schedules, including
interrupted attempts. Shutdown closes application job admission and drains active handlers while backend transactions
remain driven, then stops and flushes the worker before releasing its application lease. Restore then reopens a worker
against the new database epoch. Apalis supplies timed queue polling; the outer runtime loop only supervises deployment
replacement and worker recovery, with no periodic scan of profile due times.

Automatic and manual refreshes, imports, account changes and schedule changes share per-profile exclusion. A busy
profile makes scheduled work wait and revalidate eligibility after manual work finishes. Cached lists and searches use
an atomic SQLite read snapshot without waiting for that mutex. An arriving manual refresh, preview or import cancels a
background job at its next durable boundary. Deployment replacement pauses scheduling, cancels old jobs and waits for
their application leases before replacing storage; the runner then binds new work to the current epoch.

Backend profile snapshots expose last success, next attempt, current phase and redacted failure state. Both hosts poll
that status without initiating refreshes. Cached pages update when the publication/completion revision changes; failed
or stale page reads retry without overwriting a newer filter or page selection. Settings stay editable during a
background run. Cancelling scheduled work is profile-scoped and independent of browser request watch channels.

Qualification uses explicit-clock planner tests, the real Apalis SQLite backend, and synthetic loopback remotes.
Coverage includes transactional enqueue rollback, queue deduplication, delayed work surviving reopen, acknowledgement
drain, abandoned-worker recovery, cancellation, cache reads and deployment replacement. No timer test contacts Garmin or
imports activities.

## Future work

- Access-token reuse and renewal.
- Optional history selection and a second cadence for full-history reconciliation.

Broader security remains deferred under [ADR 0023](../decisions/0023-deferred-security-hardening.md).
