# 0003: License and provenance

## Decision

License Nimrag as `AGPL-3.0-or-later` with [the canonical text](../../LICENSE-AGPL). Before importing third-party
material, record its source, revision, license, reused content, notices, modifications, and validation.

## Why

The AGPL provides the desired openness and warranty terms. Reference, dependency, adaptation, and copying carry
different obligations.

## Consequences

Preserve upstream terms and notices. Provenance is required before import, not reconstructed during release cleanup. Nix
pins non-Cargo asset licenses; `cargo-bundle-licenses` harvests linked runtime dependencies. `just qa::licenses` writes
the committed desktop and HASS bundles, and validation rejects stale output.
