# 0034: Application composition and runtime modes

## Decision

Keep composition roots under `apps/` and reusable capabilities under `crates/`. The three applications are the CLI,
native desktop, and Home Assistant host. Crates never depend on applications.

Real, dry-run, and demo execution use the same application workflows and presentation. A mode selects adapters and
capabilities only:

- real uses Garmin services and real device mutation after confirmation;
- dry-run uses real discovery, services, capture, download, and verification with device commit disabled;
- demo supplies disposable device and service adapters while retaining production orchestration.

Only one global presentation element identifies a non-production mode. “Demo” names the user-facing synthetic build;
“mock” is reserved for fake adapters, fixtures, and tests.

This refines ADR 0001 and replaces the production-versus-mock terminology in ADR 0032.

## Why

Parallel workflows can pass while production fails. Adapter substitution exercises substantially the same code and makes
behavioral differences explicit. App-shaped crates retain target policy without contaminating low-level APIs.

## Consequences

`just cli::run` is production and `just cli::demo` is disposable. Desktop and Home Assistant demo packages have separate
IDs and data roots. New mode branches require a capability that cannot be expressed by an adapter; frontend-specific
copies of a workflow are rejected.
