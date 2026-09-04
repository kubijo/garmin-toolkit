# 0025: Proxied online vector maps

## Decision

Render [walkers](https://github.com/podusowski/walkers) on native egui and WASM using MVT tiles from
[OpenFreeMap](https://openfreemap.org/). Keep provider metadata, style, attribution, and overlays behind an owned map
boundary.

The native host discovers TileJSON, fetches, and caches. HASS exposes same-origin Axum routes; desktop calls the same
component in-process. Do not use walkers' built-in OpenFreeMap URL.

Use HTTP-aware caching with upstream freshness rules and storage bounds; exclude it from snapshots. Offline regions and
prefetch are unsupported. Cache misses fail explicitly when offline.

## Why

Walkers provides shared egui rendering, an MVT styling subset, and overlays. OpenFreeMap needs no account, publishes
open components and attribution, and can be replaced or self-hosted. The proxy shares cache and centralizes policy.

Map rendering does not calculate routes; routing, elevation, and geocoding remain separate.

## Consequences

Always show attribution. Validate responses and coordinates before caching or serving. Provider failure disables maps,
not activity data. Full MapLibre style compatibility is not promised.
