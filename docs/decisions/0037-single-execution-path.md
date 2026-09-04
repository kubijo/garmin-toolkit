# 0037: Single execution path

## Decision

Each use case has one execution path shared by Linux, macOS, Windows, real, demo, and dry-run. Composition selects trait
implementations; workflows and presentation consume owned models and capabilities. Platform and mode differences belong
behind those traits, including service access, device I/O, host storage, clock/timing, and commit authority. Platform
errors are normalized at the adapter boundary.

Dry-run shares preparation and preflight, with device commit disabled. Demo injects synthetic adapters into production
orchestration. Neither may substitute a transaction engine or canned success. Adapter internals implement
transport-specific primitives without duplicating workflow policy.

OS/build selection stays at factories and composition roots. Unsupported capabilities fail explicitly. The global
indicator identifies demo or dry-run; operation results describe what actually happened.

This supersedes ADR 0034's allowance for mode-specific workflow branches. ADR 0035 retains device ownership. Existing
duplicated paths are implementation debt, not exceptions.

## Enforcement

Review workflow changes for OS/mode dispatch, copied sequencing, and leaked external types. Exercise the same workflow
with injected adapters; compare preparation, approval, progress, cancellation, and recovery behavior. Dry-run must
produce no device mutations. A simulator result must name the real adapter layers it bypassed.

## Why

Tests must exercise the behavior users run. Platform support and simulation change dependencies, not application logic.
