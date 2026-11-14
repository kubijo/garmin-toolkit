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
automatic import or periodic polling. Loading the cloud browser does traverse the account history to populate its cache.

Batch preparation returns only the activity ID, exact-byte digest and imported status. Interactive preview responses are
capped at 8 MiB of encoded data before entering the browser RPC transport; larger recordings can still be imported
through page import. A failed local-list refresh preserves committed imports and shows an error; the next cloud request,
including **Refresh**, retries the local list. Changing pages or refreshing clears the previous batch summary.

## Qualification

- Compare downloaded FIT bytes with a manual original export. Account/region differences, service limits and historical
  reach remain unqualified.
- Verify expiry/revocation against the live service. Explicit requests currently obtain fresh access tokens.

## Configurable auto-sync analysis

Proposed scope: opt-in, per-profile incremental cache refresh and original FIT downloads. Activity import stays
explicit; full-history reconciliation stays available through the Refresh menu. Configuration is **Off** or **Every n
hours / days / weeks**, backed by a domain type containing a positive count and unit, with checked duration conversion.
Preserve the chosen unit in storage and use ICU plural messages in the UI.

Timed incremental refresh inherits the overlap limit: backdated uploads and older edits or deletions still need full
reconciliation. An optional second full-refresh cadence would provide automatic reconciliation of older history;
incremental-only scheduling does not promise it.

Recommended timing semantics are elapsed intervals: a day is 24 hours and a week is seven days. Display due times in the
user's locale. This gives a stable duration across daylight-saving changes; recurrence at the same local clock time
would be a separate scheduling feature. Schedule from the last successful completion, including a successful manual
refresh. Enabling a previously unsynchronised profile runs once immediately. On restart or resume, perform at most one
overdue run, then calculate the next due time; accumulate no missed-run backlog.

Cancellation defers the next automatic attempt by one configured interval so a timer cannot immediately undo a user's
cancel action; it does not advance last successful completion. Changing the interval recalculates the next due time from
the last success while respecting a current cancellation deferral.

Persist the schedule, last attempt, last successful completion, next due time and redacted failure state with the
profile database. Keep failure retries separate from the configured cadence, with bounded backoff and any supported
server retry delay. Definitive credential rejection pauses attempts until credentials are repaired. An expired access
token during a long run should get one bounded fresh-session attempt before being treated as a credential problem.
Removing or replacing credentials must cancel or invalidate work tied to the previous account. Disabling auto-sync
cancels its current run at the same durable boundaries as manual refresh, preserving downloaded originals.

Rejected HTTP responses currently retain the operation and status code, so honoring a server-provided retry delay would
also require preserving that header in the client error contract.

Expose a structured refresh completion and failure classification before adding the runner. Today a stopped download can
return a cached page with a warning, and the cache layer reduces some errors to text. An `Ok` page therefore cannot be
used as the successful-sync marker. Distinguish completed, partial and cancelled runs, preserve authentication /
rate-limit / network classification, and record unavailable individual originals as coverage. Those individual files can
retry on the regular cadence without putting a completed index into a tight failure-retry loop.

The backend owns scheduling, so several HASS browser tabs cannot create duplicate jobs. HASS can sync while its server
is running with no browser open. Desktop sync runs while the application is running and catches up once when reopened;
running after application exit would require an additional daemon or OS integration. The current desktop worker uses a
current-thread Tokio runtime around individual commands and blocks on a command receiver while idle. It needs a
continuously driven scheduler runner tied to the active deployment, rather than a timer on the existing idle runtime.

Use the same per-profile exclusion for automatic and manual refreshes, imports and account changes. A busy profile skips
or defers a scheduled attempt instead of queuing duplicate jobs. Before background sync, allow cached list/search reads
to use an atomic database read snapshot without waiting for the long refresh mutex. Current list queries take that
mutex; simply adding a timer would block browsing during downloads. Define manual-operation priority so an arriving
preview or import can yield a background refresh at a durable boundary. Deployment replacement and restore must stop and
drain the old scheduler before releasing its application lease, then bind the runner to the new deployment epoch.

Expose last success, next attempt, current phase and an error/retry state through backend profile snapshots. A
background run should update cached pages when its generation changes and leave settings editing available. Keep the
cancellation signal tied to the job that started it; a browser closing must not own the lifetime of a scheduled backend
job. Scheduled jobs need a backend-owned identity and a profile-authorized cancellation operation, rather than ownership
by the lifetime of a browser request's watch channel.

Implementation sequence: introduce structured outcomes, interval/settings and persisted status; enable background-safe
snapshot reads and manual priority; add an injected-clock due-time planner and deployment-owned runner; expose
settings/status in both hosts. Test restart and suspend catch-up, clock changes, failed attempts, credential changes,
disable/cancel, deployment replacement, multiple clients and overlapping manual work using synthetic remotes and a
controllable clock.

## Future work

- Access-token reuse and renewal.
- Optional history selection and the configurable auto-sync design above.

Broader security remains deferred under [ADR 0023](../decisions/0023-deferred-security-hardening.md).
