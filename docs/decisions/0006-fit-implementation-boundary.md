# 0006: FIT implementation boundary

## Decision

Pin `rustyfit` 0.10.2 behind `fit`. Preserve imports, expose only Nimrag types, and encode only new courses and
workouts. Exclude Garmin SDK material; use synthetic or publishable project-owned tests.

## Why

`rustyfit` provides the needed operations under declared BSD-3-Clause terms. `fitparser` is decode-only; Garmin's SDK
license is unsuitable.

## Consequences

Originals remain authoritative. Preserve notices and rerun corpus checks on upgrades; real-file and upload compatibility
remain target evidence.

See [FIT implementation research](../research/fit-implementation.md).
