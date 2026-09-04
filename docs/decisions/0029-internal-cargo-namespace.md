# 0029: Internal Cargo namespace

## Decision

Use `nr-*` for Cargo package identity. It is an opaque workspace namespace, weakly mnemonic of Nimrag, and is not public
branding. Package directories exactly match package names; dependency aliases and Rust modules omit the prefix when
their context is already clear.

Rename existing packages before adding the data-foundation storage crate. A future public brand may replace the prefix
in one deliberate migration; do not encode it in domain, storage, or protocol identifiers.

## Why

Cargo package names benefit from an unambiguous workspace prefix, while repeating the full provisional application name
in imports and symbols adds noise.

## Consequences

Internal packages use names such as `nr-model`, `nr-usb`, and `nr-storage`. Types rely on crate and module namespaces
instead of repeating project or domain prefixes.
