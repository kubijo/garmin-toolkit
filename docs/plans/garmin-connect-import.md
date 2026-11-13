# Official Garmin Connect imports

## Outcome

Import uploaded activities through the official Personal API while the device remains paired with its phone. Retain
original FIT bytes, source account/activity identity and existing deduplication behavior. Local and USB workflows remain
independent. [ADR 0008](../decisions/0008-official-garmin-connect-boundary.md) defines scope;
[API research](../research/garmin-connect.md) records the documented contract and uncertainties.

Configured profiles expose **Garmin activities** in navigation. Loading and **Refresh** enumerate all account summaries
and cache original FIT archives in the profile database, independently of activity acquisition and import. One
authenticated session serves the load. Refresh updates metadata, removes deleted cloud entries from the cache and
retries missing originals; already downloaded originals are reused. Individual unavailable files remain searchable.
Connection or rate-limit failures stop downloading, retain completed files and show a warning; cached metadata remains
browsable.

Live progress reports connection, indexed pages and discovered summaries while the total remains unknown, then original
file completion with new downloads, reused cache entries, unavailable files and remaining work. Desktop events and a
coalescing browser watch channel carry the same profile-scoped updates; completion clears transient progress state.

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

## Future work

- Access-token reuse and renewal.
- Optional history selection and incremental polling, separate from manual browsing.

Broader security remains deferred under [ADR 0023](../decisions/0023-deferred-security-hardening.md).
