# Activity map workspace

Replace the activity-detail spike with a dense shared workspace for desktop and HASS. Reuse the device explorer's pane,
row, toolbar, drawer, focus, and selection language. Render the same activity viewer inside device FIT previews. Garmin
shared links and FitFileViewer are behavioral references only; copy neither their implementation nor visual design.

## Recording boundary

- Replace coordinate-only detail payloads with one portable recording snapshot containing ordered samples, laps, and
  timer events. Samples retain timestamp and optional coordinate, elevation, distance, speed, heart rate, cadence,
  power, and temperature values. Laps retain time, totals, and aggregate metrics.
- Embed the same recording snapshot in stored activity details and device FIT previews. Project it once from the
  normalized activity model and remove the separate desktop, HASS, and preview path projections.
- Select activities by observation ID and preserve stale-result protection. Derive display paths and chart series from
  the complete recording without discarding missing values.
- Keep one authoritative sample-index cursor with idle, hover, pinned, and playback modes. Maps, charts, readouts, and
  lap highlighting consume that cursor; none owns a parallel selection or playback state.

## Libraries and host boundary

- Use [`egui_plot` 0.37][egui-plot] for chart axes, grids, formatting, lines, areas, transforms, clipping, and pointer
  coordinates. It resolves against egui 0.36, runs in native and WebAssembly egui, links plot axes and cursors, and
  exposes vertical lines and points for an externally selected sample. Do not implement a chart painter.
- Convert a hovered plot X coordinate to the nearest original sample index. Render the shared external cursor as a
  vertical line plus the corresponding point in each chart. Split missing measurements into contiguous lines and keep
  the vertical guide when the selected sample has no value for a particular chart.
- Downsample only render series with width-sensitive first/minimum/maximum/last buckets and borrowed plot points. Retain
  the original samples for inspection, laps, and playback.
- Use [`walkers` 0.59][walkers] with MVT support behind an owned UI tile adapter. The adapter owns a 256-tile decoded
  LRU, missing/in-flight/empty/failed states, deduplicated requests, six-request concurrency, and bounded retry backoff.
- Add a separate `garmin-map-tiles` host crate. It discovers and validates OpenFreeMap TileJSON, fetches tiles, and owns
  a 256 MiB HTTP-aware disk cache with atomic writes, freshness metadata, and LRU pruning. Limit TileJSON to 1 MiB and a
  tile to 2 MiB; reject invalid coordinates, non-HTTPS templates, unexpected hosts, and malformed responses.
- Desktop fetches tiles on a dedicated asynchronous worker. HASS exposes relative same-origin
  `map/tiles/{z}/{x}/{y}.pbf` routes with explicit ETags/freshness and excludes them from immutable asset handling.
  Provider failure leaves the activity, charts, imports, and a neutral route canvas usable. Always show attribution.

## Workspace and interaction

- Replace the stateless activity browser with a stateful workspace and reusable single-activity viewer. At 760 pixels of
  available activity width, show a resizable 232-pixel activity list, central map/charts, and a resizable 248-pixel
  summary/laps pane. Below it, retain the viewer and expose list and details as overlay drawers.
- Replace the import slab with a stable command bar using 32-pixel tertiary file/folder actions. The complete workspace
  remains a drop target with a non-shifting drag overlay. Use two-line 52-pixel activity rows, a compact two-column
  label-over-value metric grid, and 32-pixel lap rows.
- Give the map roughly 55 percent of the center height, clamped to 260--420 pixels. Place independently scrollable,
  aligned chart cards below it in this order when data exists: elevation, running pace or cycling speed, heart rate,
  cadence, power, and temperature. Each card has a quiet inset plotting field, subtle grid, filled series, current
  value, and minimum/average/maximum summary above its 112-pixel plot.
- Default the shared chart X domain to distance when it has at least two monotonic anchors and non-zero span,
  interpolating internal gaps. Otherwise use elapsed time and disable distance. Respect profile units.
- Plot hover snaps to an original sample index and highlights that point on every chart, the corresponding map position,
  metric readouts, and active lap. Route hover performs the inverse lookup. Clicking pins; Escape or empty analysis
  space clears. Missing measurements omit only their point marker.
- Lap hover highlights its interval without moving the viewport. Lap click selects and fits its map/chart range and
  restricts playback. A full-activity action resets the range.
- Put quiet floating control clusters on the map: fit at top-left, vertical zoom at top-right, play/pause with a cycling
  0.5x/1x/2x speed control at bottom-center, and full-activity reset at bottom-left when a lap is selected. Direct wheel
  zooms the map. Playback drives the shared sample index over 30 seconds at 1x, skips timer-stopped intervals, begins at
  the pinned/paused sample, and restarts after completion. Passive hover cannot override playback; clicking or scrubbing
  pauses and pins. Keep the fitted viewport stable and interpolate the map marker only between adjacent valid
  coordinates in one route segment.
- Reuse the complete viewer and global tile cache inside device FIT previews. Recordings without coordinates retain
  charts, laps, inspection, and playback with an explicit no-route state.

## Acceptance evidence

- Cover recording projection, gaps, domains, units, active-time calculation, lap ranges, playback scaling, and stale
  results with unit tests.
- Cover plot-X/index conversion, external cursors, linked X bounds, missing points, and render downsampling
  independently of map behavior.
- Cover fitting, dateline and no-coordinate paths, route hit testing, hover/pin/play transitions, lap reset, and
  activity changes in shared UI tests.
- Cover TileJSON validation, cache freshness and eviction, offline hits/misses, response limits, concurrent
  deduplication, retries, HASS relative routes, ETags, CSP, and cache-policy exclusions with local fixtures and servers.
- Maintain deterministic gallery scenes for wide/compact and light/dark layouts, synchronized hover, pinning, playback,
  selected laps, missing metrics, no GPS, loading, provider failure, and device FIT preview.
- Run the repository preflight, tests, lints, gallery checks and captures, then exercise desktop demo and HASS demo.

## Boundaries and deletion gate

Use one owned OpenFreeMap style family under [ADR 0025](../decisions/0025-proxied-online-vector-maps.md). Routing,
geocoding, heatmaps, free chart zoom, offline-region downloads, and graphical Garmin device-map management are separate
work. Delete this plan after the shared viewer passes desktop and HASS runtime checks, maintained interface evidence is
reviewed, and lasting map/chart architecture is documented.

[egui-plot]: https://docs.rs/egui_plot/0.37.0/egui_plot/
[walkers]: https://docs.rs/walkers/0.59.0/walkers/
