# Official Garmin Connect imports

## Outcome

Import uploaded activities through the official Personal API while the device remains paired with its phone. Retain
original FIT bytes, source account/activity identity and existing deduplication behavior. Local and USB workflows remain
independent. [ADR 0008](../decisions/0008-official-garmin-connect-boundary.md) defines scope;
[API research](../research/garmin-connect.md) records the documented contract and uncertainties.

Configured profiles expose **Garmin activities** in navigation, with manual paging and persistent imported markers.
Opening an activity previews its name, description and original FIT without importing. Names and descriptions are
preview-only; **Import FIT** persists the file through the existing importer. Confirmation binds the profile, activity
ID, account and exact FIT bytes; an exact previously imported file has import disabled.

**Import all on this page** snapshots up to 20 visible activities, skips imported rows and processes the rest
sequentially through the same FIT checks. Cancellation stops between files and retains completed imports. The summary
reports imported, skipped and failed counts; access, connection and rate-limit failures stop the batch. There is no
automatic import, history traversal or polling.

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
