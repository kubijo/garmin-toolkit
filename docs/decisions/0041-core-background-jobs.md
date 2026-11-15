# 0041: Library-backed background jobs in the core application

## Decision

The shared core owns a general background-job engine backed by Apalis and its SQLite backend. Hosts start it through
`Deployment::start_jobs()` and retain one lifetime guard. Desktop, HASS and future execution modalities share job
registration, persistence, dispatch and shutdown. Browsers issue application commands and read status; they never own
schedules. Short-lived read-only commands do not start background execution.

Use the maintained library for durable delayed work, claiming, acknowledgements, infrastructure retries and abandoned
worker recovery. Keep product policy in the application: profile identity, Garmin cadence, server retry deadlines,
credential pauses, manual-work priority and cancellation deferrals. Do not build another queue or dispatch scheduler
beside the library. The core's runtime supervisor only binds the library worker to the current deployment generation.

Jobs share the application database. Schedule changes and enqueue operations commit atomically. Payloads contain only
typed identities and eligibility, with no credentials or downloaded data. Delivery is at least once; handlers must be
idempotent and revalidate the current schedule before executing replaced or recovered work.

Deployment replacement closes application job admission and signals cooperative cancellation. Keep the backend driven
until handlers finish their durable writes, then stop the worker and flush acknowledgements before releasing its
application lease. Stopping immediately in Apalis 1.2 can strand an in-flight queue transaction while a handler waits
for the same SQLite writer. Pausing also strands an in-flight fetch transaction. The core uses the deployment admission
token and the library's task-count and stop APIs to avoid that ordering. Deliveries received after admission closes fail
without starting application work and remain retryable in the queue. Only then can restore replace storage. Reopen a
worker against the new generation and reconcile persisted schedule state.

## Why

A custom timer loop gives this repository responsibility for generic scheduling and recovery. A shared library engine
makes those mechanisms reusable across modalities and subjects them to upstream maintenance and testing. A persistent
SQLite queue fits the existing deployment and backup boundary without introducing an external service.

## Consequences

Pin Apalis and its backend. The application migration contains the backend's resulting schema; a regression test
compares it with the pinned upstream migrations. An upgrade that changes schema needs an ordinary application migration.
Keep our synchronous FULL configuration: the backend's convenience setup changes durability PRAGMAs and is not called.

The continuously driven worker runtime is necessary for idle desktop hosts. Hosts own its lifetime, while all execution
semantics remain in the core. There is no background execution after the hosting process exits.

The cooperative drain guarantee covers requested shutdown and deployment replacement. An Apalis backend polling error
can instead drop active handlers. Their registrations are cleaned up on drop; transactional writes and at-least-once
queue recovery preserve retryability, but an in-flight network download may need to be retried. Completion of that
transfer during an infrastructure fault is not guaranteed. Each worker instance has a fresh identity, and exclusive
deployment recovery invalidates old claims independently of heartbeat clocks, including future-dated restored values.

Tests use the real backend with disposable databases, synthetic jobs and loopback Garmin fixtures. They cover queue
reopen, deduplication, atomic rollback, acknowledgement draining, abandoned work, stale schedules and deployment
replacement. Passing these tests does not qualify the live Garmin service.

## Sources

- [Apalis worker implementation](https://docs.rs/apalis-core/1.2.0/apalis_core/worker/struct.Worker.html)
- [Apalis SQLite backend](https://docs.rs/apalis-sqlite/1.1.0/apalis_sqlite/)
