# 0031: Carbon gray theme baselines

## Decision

Derive the dark and light themes from Carbon Gray 100 and Gray 10. Profile choices are Auto, Dark, and Light. Auto is
the default, follows the system, and falls back to dark. `nr-color` exposes typed swatches and app-owned semantic roles.

Keep the semantic API smaller than Carbon's token catalog. Add roles only for concrete component needs. User accents
remain profile data and require an explicit contrast-tested resolver. Components do not name or assume the action hue.

## Why

The upstream palettes provide coherent dark and light baselines. App-owned roles keep Carbon names out of the UI API.

## Consequences

Alpha values use the nearest eight-bit representation. Updating Carbon requires reviewing changed values, extending
provenance, and validating every gallery scene.

See [third-party notices](../../THIRD_PARTY_NOTICES.md) for the exact source and modifications.
