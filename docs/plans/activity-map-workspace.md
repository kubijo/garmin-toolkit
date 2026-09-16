# Activity map workspace

Replace the activity-detail spike with a dense shared workspace for desktop and HASS. Reuse the device explorer's pane,
row, toolbar, drawer, focus, and selection language. Render the same activity viewer inside device FIT previews. Garmin
shared links and FitFileViewer are behavioral references only; copy neither their implementation nor visual design.

The current transitional map proves the workspace behavior, but its loading and preparation path still contends with
eframe's event and render thread. Complete the rendering-isolation refactor below before closing this plan. Build on the
current staged GPU work in place; do not restore it merely to separate commits.

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
- Use [`walkers` 0.59][walkers] for MVT decoding and style interpretation behind an owned background map runtime. Do not
  use its interactive map widget or retain tile scheduling, tessellation, label placement, or response installation in
  the UI path. The runtime owns a 256-tile prepared LRU, missing/in-flight/empty/failed states, deduplicated requests,
  six-request concurrency, and bounded retry backoff.
- Add a separate `garmin-map-tiles` host crate. It discovers and validates OpenFreeMap TileJSON, fetches tiles, and owns
  a 256 MiB HTTP-aware disk cache with atomic writes, freshness metadata, and LRU pruning. Limit TileJSON to 1 MiB and a
  tile to 2 MiB; reject invalid coordinates, non-HTTPS templates, unexpected hosts, and malformed responses.
- Desktop fetches and prepares tiles on dedicated I/O, CPU, and GPU-upload workers. HASS exposes relative same-origin
  `map/tiles/{z}/{x}/{y}.pbf` routes with explicit ETags/freshness and excludes them from immutable asset handling; its
  web frontend fetches and prepares tiles in a dedicated Web Worker. Provider failure leaves the activity, charts,
  imports, and a neutral route canvas usable. Always show attribution.

## Rendering isolation refactor

### Landed boundary

- The activity UI owns one coherent `MapCamera`; `walkers::Map`, `MapMemory`, UI-side `Tiles::at` traversal, and
  destructor-triggered request dispatch have been removed. Camera projection, pointer-anchored hard-bounded zoom,
  panning velocity, inertia, and dateline wrapping now share that state machine.
- The UI submits a compact `MapViewDemand` camera snapshot. The surface runtime derives visible XYZ coverage and a
  one-tile prefetch ring, replaces stale unstarted demand, prioritizes visible tiles from the viewport center, and
  renders only the current visible set rather than every cached zoom level.
- Remaining work in this section is still real: the host contract exposes tile tasks and decoders, response installation
  still occurs at the surface boundary on the event-loop thread, scenes are not yet atomically published immutable
  `Arc`s, and browser GPU uploads still create a complete tile synchronously under an 8 MiB byte cap instead of the
  specified chunk and time budgets.

### Current contention and target boundary

- Native tile disk/network access, MVT decoding, styling, initial geometry tessellation, route preparation, and label
  preparation are already off-thread. The event-loop surface boundary still drains completions, derives a small tile
  coverage set, and admits browser uploads; browser uploads still create complete WGPU resources synchronously.
- Inertial movement is classified as camera motion and label work waits for the camera to settle. Route hover uses the
  persistent uniform-grid index instead of projecting and hit-testing the complete route; the remaining concern is
  scene publication and upload work at the event-loop boundary rather than route-query complexity.
- eframe performs application update, egui tessellation, and paint submission sequentially on its event-loop thread.
  After this refactor that thread may perform only input and camera integration, constant-time route queries, light
  controls and markers, one camera-uniform update, immutable-scene acquisition, and draw submission.
- Missing areas remain blank until complete resources are ready. Tile, label, or route publication must never block,
  reset, jump, or otherwise modify drag, wheel zoom, or inertial camera movement.

### Runtime and public interfaces

- Introduce a shared map-rendering core with `MapCamera`, `MapViewDemand`, `MapRouteSource`, immutable `MapScene`, and
  `MapPerfSnapshot` types. `MapCamera` alone owns projection, pointer-anchored zoom, hard zoom bounds, drag velocity,
  and inertial integration.
- Expose a cloneable `MapRuntimeHandle` and one `MapSurfaceHandle` per activity or FIT-preview map. The UI submits the
  latest view and route revisions through the surface and reads its latest ready scene and performance snapshot.
- Remove `MapTileRequest`, `MapTileResponse`, `MapTileDecoder`, `take_map_tile_requests`, and `resolve_map_tile` from
  the UI-facing host contract after both hosts migrate. Tile discovery, fetching, preparation, caching, and publication
  are runtime responsibilities rather than activity-workspace responsibilities.
- Publish complete immutable scenes atomically. Native uses a lock-free `Arc` scene swap; browser uses transferred
  buffers and a single-thread scene slot. Render callbacks never lock a mutable frame or expose a partially uploaded
  tile.

### Scheduling and rendering pipeline

- Coalesce camera demands so workers always process the newest generation. Calculate visible XYZ tiles outside the UI,
  add a one-tile prefetch ring, prioritize them from the viewport center outward, and discard stale queued work while
  retaining ready cache entries.
- Split native work into bounded asynchronous cache/network I/O, dedicated CPU preparation, and WGPU resource creation
  and upload using cloned thread-safe device and queue handles. The WGPU callback receives ready buffers and performs
  only scissored draws plus one shared camera-uniform update; it creates no tile or route resources.
- Add a dedicated HASS Web Worker which fetches, decodes, styles, tessellates, performs label layout, and returns
  transferable POD vertex, index, label, and atlas buffers. This requires neither shared WebAssembly memory nor
  cross-origin isolation.
- Keep the existing eframe browser canvas. Split uploads into chunks no larger than 256 KiB and admit at most 512 KiB
  within a measured 1.5 ms budget per frame. Publish a tile only after every chunk is uploaded. Prepared tiles and
  labels may appear live during movement without making the camera wait for them.
- Treat a map-only `OffscreenCanvas` renderer as a worthwhile future slice if profiling after this work shows that GPU
  upload or eframe painting still causes visible web stalls. Moving the entire eframe application into a worker is
  disproportionately expensive and is not necessary to fix map interaction. Native 4x antialiasing currently uses
  eframe's existing full-window multisampled WGPU target and resolve path, so it requires no private offscreen
  compositor but does charge every native UI frame for the multisampled target. Eframe's web painter remains
  single-sampled; implementing map-only web MSAA would therefore trigger the map-only `OffscreenCanvas` slice rather
  than moving the complete application.
- Render tile positions from immutable tile coordinates and the current camera in the shader. Do not rewrite per-tile
  transform buffers, scan the full cache, or retessellate geometry when the camera changes.

### Routes, labels, and input

- Prepare a route once per activity or selected-lap revision as world-space GPU geometry. Store normalized speed per
  route vertex and apply the speed colour ramp in the shader; camera movement must not retessellate or deform it. Build
  shared, miter-limited join offsets during route preparation and analytically antialias both lateral edges and exposed
  caps so independent segment rectangles cannot leave cracks or translucent overlap wedges at route bends.
- Apply an explicit map-rectangle scissor in the native and browser WGPU backends so tiles, routes, labels, and cursor
  overlays cannot paint into charts or sidebars. Project start, end, selected-sample, and highlighted-lap overlays from
  the same camera snapshot used by the map scene.
- Build a world-space uniform-grid index over route segments once. Convert the pointer to world coordinates and test
  only nearby segments while preserving exact original sample-index reporting and synchronized map/chart cursors.
- Perform label sizing, collision placement, and tessellation in workers using the bundled Noto Sans fonts and a spatial
  collision grid rather than the current quadratic occupied-area scan. Publish atlas changes and label meshes as
  camera-anchored batches: translate an existing batch during same-zoom movement and omit stale labels after a zoom
  change until a matching batch is ready.
- Clamp wheel intent before changing camera zoom. An outward wheel event at either bound is ignored and cannot produce a
  transient overshoot followed by a snap-back. Loading and texture publication remain independent from inertia.

### Activity-view isolation

- Make the chart scroll area viewport-aware. Offscreen chart cards reserve stable layout space without constructing an
  `egui_plot` widget, while intersecting cards retain the existing appearance and linked interaction behavior.
- Cache chart statistics, downsampled series, and layout inputs by activity, axis, lap, units, theme, and width. During
  map-driven repaint frames, only visible cards and their small cursor or lap overlays may perform chart work.
- Preserve the authoritative sample cursor, chart appearance, playback, selected-lap behavior, and index-based map/chart
  synchronization. This refactor changes ownership and scheduling, not those interaction contracts.

### Instrumentation and performance gate

- Expand the debug FPS readout with UI CPU p50/p95, scene generation, worker and upload backlogs, label time,
  route-query time, uploaded bytes, and stale-work count. Add trace spans around camera update, scene acquisition, chart
  construction, render preparation, and draw submission.
- On the bundled 67.92 km activity at roughly 1100 by 720 pixels, sustained dragging, wheel zoom, and inertial movement
  target 60 FPS with UI CPU p95 below 16.7 ms. No tile-arrival or label-publication stall may exceed 33 ms, and map
  loading must not alter the camera trajectory.
- Capture native interaction with `just desktop::profile demo 00-baseline`; close the application after representative
  panning, wheel zoom, and inertia, then inspect it with `just desktop::profile-load 00-baseline`. Captures require
  unrestricted Linux perf events rather than silently accepting incomplete samples. The first accurate profiling build
  recompiles the optimized dependency graph with frame pointers and line-table debug information, and may be pre-warmed
  with `just desktop::profile-build demo`; interrupted builds retain Cargo's completed work and create no report. Each
  named report retains the raw Samply profile, presymbolication data, runtime measurements, and provenance manifest
  unchanged, then derives a separate `combined.json.gz` with desktop frame timing, map UI/scene/query/label timing,
  render callback timing, worker backlog, tile state, upload pressure, and stale-work counters. Cancellation after
  capture starts is a terminal report state and preserves raw evidence without a traceback. The profiling Cargo profile
  retains release optimization, debug information, and frame pointers without changing shipped release artifacts.
- Summarize a capture headlessly with `just desktop::profile-analyze 00-baseline` and compare phases with
  `just desktop::profile-compare 00-baseline 01-antialiasing`. The analyzer resolves Samply's preserved symbol sidecar,
  including inline frames, reports per-stack inclusive and self CPU samples, verifies every artifact against the
  manifest, and refuses incompatible comparisons by default. Idle redraw gaps remain separate from uncapped intervals
  following dragging, wheel zoom, or inertial camera movement. At least 120 samples and two seconds of camera motion are
  required before evaluating the 16.7 ms/33 ms interaction gate.
- The user-captured `01-antialiasing` report measured 433 interaction intervals across 7.25 seconds: p95 was 17.544 ms,
  maximum was 34.638 ms, complete desktop UI p95 was 1.022 ms, and map UI p95 was 0.709 ms. It therefore narrowly failed
  the strict interaction gate despite remaining visually smooth. Its sampled CPU/core comparison with `00-baseline` is
  only directional because the captures used different user-driven durations.
- The clean `02-route-aa` capture after route joins and corrected interaction-tail telemetry covered 1,006 interaction
  intervals across 17.1 seconds. Interaction p95 was 21.086 ms and the maximum was 34.196 ms, while complete desktop UI
  p95 remained 1.145 ms, map UI p95 was 0.838 ms, render draw p95 was 0.049 ms, and sampled CPU averaged 0.291 cores.
  The strict cadence gate still fails, but the application-side measurements and the user's smooth subjective result do
  not implicate map CPU or draw submission as the limiting work. Retain native 4x MSAA and investigate window-system or
  presentation cadence only if interaction becomes visibly uneven.
- Keep the route stable, clipped, speed-coloured, and synchronized with charts at every zoom while tiles arrive. Verify
  missing-background behavior by leaving unprepared regions blank rather than falling back to synchronous work.
- Do not launch a GUI from unattended development or validation commands. Interactive performance evidence is user-run
  through the debug overlay; automated validation remains headless.

## Workspace and interaction

- Replace the stateless activity browser with a stateful workspace and reusable single-activity viewer. At 760 pixels of
  available activity width, show a resizable 232-pixel activity list, central map/charts, and a resizable 248-pixel
  summary/laps pane. Below it, retain the viewer and expose list and details as overlay drawers.
- Replace the import slab with a stable command bar using 32-pixel tertiary file/folder actions. The complete workspace
  remains a drop target with a non-shifting drag overlay. Use two-line 52-pixel activity rows, a compact two-column
  label-over-value metric grid, and 32-pixel lap rows.
- Give the map roughly 68 percent of the center height, clamped to 320--560 pixels. Place independently scrollable,
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
- Cover camera projection, pointer-anchored zoom, hard zoom limits, inertia, scene-generation coalescing, center-first
  tile priority, stale-work rejection, cache eviction, and atomic scene publication with unit tests.
- Compare uniform-grid route queries against brute-force nearest-segment results, including missing-coordinate segments
  and wrapped longitudes. Cover deterministic label placement, camera rebasing, and zoom invalidation.
- Cover browser upload chunking and budget accounting without wall-clock-sensitive CI assertions. Verify that partial
  uploads cannot become drawable and that the native and browser WGPU paths apply the same map scissor.
- Cover TileJSON validation, cache freshness and eviction, offline hits/misses, response limits, concurrent
  deduplication, retries, HASS relative routes, ETags, CSP, and cache-policy exclusions with local fixtures and servers.
- Maintain deterministic gallery scenes for wide/compact and light/dark layouts, synchronized hover, pinning, playback,
  selected laps, missing metrics, no GPS, loading, provider failure, and device FIT preview.
- Run formatting, workspace tests, Clippy, native builds, WASM/Trunk builds, and existing Nix validation without opening
  an application window. The user exercises the desktop and HASS demos for the interactive performance gate.

## Boundaries and deletion gate

Use one owned OpenFreeMap style family under [ADR 0025](../decisions/0025-proxied-online-vector-maps.md). Routing,
geocoding, heatmaps, free chart zoom, offline-region downloads, and graphical Garmin device-map management are separate
work. Delete this plan after the shared viewer passes desktop and HASS runtime checks, maintained interface evidence is
reviewed, and lasting map/chart architecture is documented.

[egui-plot]: https://docs.rs/egui_plot/0.37.0/egui_plot/
[walkers]: https://docs.rs/walkers/0.59.0/walkers/
