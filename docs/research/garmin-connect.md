# Garmin Connect Personal API

The pinned descriptor, Rust generator and offline HTTP verification live in
[`infra/garmin-connect`](../../infra/garmin-connect/README.md). Run `just connect-api::test` to verify the contract.

## Documented contract

Garmin's [program overview][overview] provides a Personal API for owners accessing their own data, including activity
JSON and original FIT files. Delivery is pull-only. It is not intended for commercial applications serving multiple
users; validate the applicable terms and intended distribution before release.

The [public Swagger reference][swagger] embeds an OpenAPI 3.0.1 document, version 1.0.0, with 26 endpoint paths and
response schemas. The production base is `https://personal.connectapi.garmin.com`; a separate `.cn` server is listed.
Relevant operations are:

- `GET /activitylist-service/activities/search/activities`: paginated summaries, offset/limit, date and sport filters.
  The documented default limit is 99; no maximum or guaranteed historical window is stated.
- `GET /activity-service/activity/{activityId}`: summary, metadata, measurements and split summaries.
- `GET /download-service/files/activity/{activityId}`: original-format activity download as a ZIP response.

Activity details include user-authored names/descriptions and summary fields `perceivedWorkoutEffort`,
`directWorkoutFeel` and `directWorkoutRpe`. Their rating scales are unspecified; previews show names and descriptions
without interpreting those ratings.

## Authentication and setup

The [Personal API guide][guide] documents manual key creation at the [Garmin Connect settings page][keys]. The key is
displayed once. The [FAQ] lists expiry choices of 30, 90, 180 days or one year; users can revoke keys there.

Exchange the key using `POST /personalapitoken-service/personal-api/personal-api-key/token` with an
`Authorization: ApiKey …` header. Access tokens last 24 hours or until key expiry, whichever occurs first. There are no
refresh tokens; obtain another token using the key. Authentication is explained separately from the Swagger document,
which does not declare security schemes.

## Live qualification

Key exchange, profile facts, activity listing/paging, original FIT download, preview, explicit import and duplicate
handling are confirmed on the owner's account.

### Owner acceptance

On 15–16 November 2026, the owner supplied live desktop and Home Assistant observations and explicitly confirmed that
the HASS browser remained closed during a successful scheduled run. Times below are the local times displayed by the
clients.

- Desktop cancellation cleared progress and deferred the next automatic attempt. A subsequent refresh encountered HTTP
  429, retained downloaded files and exposed a deferred retry instead of recording success.
- Desktop subsequently recorded a successful refresh at **15 November, 21:48**, with one unavailable FIT file and the
  next attempt at **22:48**. Both times persisted after restart, without an immediate repeated sync.
- Desktop continued scheduled execution overnight: the next day's view recorded success at **16 November, 08:49** and
  the next attempt at **09:49**.
- HASS displayed a rate-limited attempt and a deferred retry, then recorded success at **16 November, 21:11**, with one
  unavailable FIT file and the next attempt at **22:11**. The owner confirmed the browser stayed closed during that run,
  establishing that the server owns execution independently of the browser.
- Both clients distinguished an incomplete rate-limited attempt from a completed refresh with an unavailable original;
  successful completion advanced the persisted last-success time and the regular cadence.

These observations qualify the tested account and hosts, not other regions or a guaranteed historical coverage window.
They do not establish a numerical request quota or verify expiry/revocation. Original-file byte comparison with manual
exports remains outstanding. Overdue catch-up, disabling a running schedule and HASS process-restart behavior retain
automated coverage but were not confirmed in this live acceptance sequence.

[ADR 0008](../decisions/0008-official-garmin-connect-boundary.md) owns the connector boundary;
[the plan](../plans/garmin-connect-import.md) owns remaining implementation. The unofficial login clients and private
Bluetooth protocol are not the source contract for this connector.

[faq]: https://developer.garmin.com/gc-developer-program/personal/personal-faq/
[guide]: https://developer.garmin.com/gc-developer-program/personal/personal-api/
[keys]: https://connect.garmin.com/app/settings/personalApiToken
[overview]: https://developer.garmin.com/gc-developer-program/overview/
[swagger]: https://developer.garmin.com/gc-developer-program/personal/swagger-docs/
