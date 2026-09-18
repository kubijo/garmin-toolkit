# Activity map workspace

Replace the activity-detail spike with a dense shared workspace for desktop and HASS. Reuse the device explorer's pane,
row, toolbar, drawer, focus, and selection language. Render the same activity viewer inside device FIT previews. Garmin
shared links and FitFileViewer are behavioral references only; copy neither their implementation nor visual design.

The current transitional map proves the workspace behavior, but its loading and preparation path still contends with
eframe's event and render thread. Complete the rendering-isolation refactor below before closing this plan. Build on the
current staged GPU work in place; do not restore it merely to separate commits.

## Current-slice closing gate (2026-09-18)

Scope is frozen to correctness, security, performance, and maintenance blockers in the complete working-tree diff.
OffscreenCanvas and the subsequent semantic interaction runner remain follow-up work, not additions to this batch.

- [ ] Resolve the upload-latency tail in `Trace-20260919T000106.json.gz`: distinguish retained offscreen time from
  visible-tile waiting before treating the 2.669-second maximum as a loading regression or accepting throughput.

On 2026-09-19 the user confirmed a fresh demo data directory and disappearance of the duplicate tooltip, and approved
committing this slice with the upload-latency investigation retained as follow-up work.

Browser acceptance on 2026-09-18 used the current Nix demo package
`x5gc9r077g4kjsjj8aybi1sc7635bcj5-garmin-hass-demo-0.1.0`, with emitted asset hash `a149a11cfeeb1a33`. All eight
initializer/worker tests passed against that package with no skips. At 1440 by 1000, Chrome rendered complete maps after
cache-bypassed and warm reloads, repeated rapid zoom cycles, and world-wrap panning. Playback, speed-stop clicks, slider
dragging, and sustained hover worked without a panic. Morocco labels included Arabic and Tifinagh glyphs.
Screenshot-guided synthetic DOM input exercised these checks; it was not semantic automation or a trusted-input latency
measurement. The original 3389 by 1324 viewport was restored afterward.

Chrome used WebGL2 through ANGLE on the RTX 4090. The only captured warning was WebGPU adapter discovery reporting
`No available adapters.` No tile-decoder errors or missing-background warnings appeared during the checks. Sustained
speed-stop hover did expose two overlapping tooltips, recorded above.

MCP's raw-trace export remains unavailable because of its configured workspace roots. The user supplied
`Trace-20260919T000106.json.gz`, analyzed with the maintained tool on 2026-09-19; see performance evidence below. Its
mixed browser-cache results do not independently establish the server-cache state; the fresh-directory confirmation
comes from the user.

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

## Rendering isolation

### Landed

- `MapCamera` alone owns projection, bounded pointer-anchored zoom, panning, inertia, and dateline wrapping. Tile
  loading cannot alter camera state.
- The zoom-out floor keeps one world at least as large as the viewport's longest side. Buttons, gestures, route fitting,
  and resizing share that floor; shrinking the viewport does not force zoom-out.
- Tile placement enumerates visible horizontal world copies; GPU instancing reuses one upload per canonical tile.
  Native/browser labels and software painting share the placement calculation. Regressions cover wide views, fractional
  zoom, dateline crossings, fetch/upload deduplication, and actual GPU pixel readback.
- The UI submits a latest-only `MapViewDemand` and acquires one immutable `MapScene` per frame. That snapshot feeds GPU
  assembly, labels, status, and metrics; later publications cannot mutate an acquired scene.
- On native targets, a per-surface worker owns `TileStore`, installs completions, derives visible and prefetched XYZ
  coverage, schedules center-first requests, and publishes through `ArcSwap`. Prepared resources are shared rather than
  copied. The event loop retains input, route lookup, overlays, frame assembly, and draw submission.
- The browser uses the same surface driver and scene contract on its single thread. Its Web Worker already performs
  fetch, decode, styling, tessellation, route preparation, and label preparation with transferred owned buffers.
- Host and module worker share one envelope codec. `BrowserTileLimits` bounds encoded input, preparation complexity, and
  output. One bounded MVT decode checks geometry before styling; native and browser preparation share tessellation.
- Low-zoom multilingual dictionaries have an 8 MiB codec-element budget and 131,072 property-reference limit, calibrated
  against world, Europe/Africa, and Asia tiles. Encoded input and expanded-property bytes remain capped at 2 MiB and 8
  MiB. Real-tile regressions preserve both themes' styling and preparation; synthetic inputs still reject amplification.
- Bundled Arabic and Tifinagh font fallbacks cover the reported North African label boxes without changing map names.
  The main UI and label worker share these fonts; other Unicode scripts are not universally covered.
- Browser tiles cross one versioned four-buffer boundary: packed vertices, packed indices, fixed-width text records, and
  UTF-8 text storage. One admission state machine owns the transferred buffers, validation, cursors, scratch space,
  frame budget, and final task. Its message callback only validates and enqueues, without allocating destination
  buffers. The queue retains at most 64 packets and 64 MiB of accounted payload (sources, destinations, and
  reconstructed text; allocator and transport overhead are additional). Animation frames reserve one destination vector
  per step, copy at most 256 KiB per chunk and 4 MiB per frame, and stop starting steps after 1.5 ms. Allocations are
  indivisible and can exceed that soft time target; separate allocation timings and the total admission timer include
  their cost. Frames reconstruct text incrementally, validate every index, and publish only a complete valid packet. The
  packed geometry then enters the existing staged WGPU uploader without another geometry decode or whole-tile copy.
- Route/label replies retain JS buffers until animation-frame admission. The request ledger rejects stale ownership
  before copying, with at most one result completed per frame. Copy/decode/completion remains indivisible; the size caps
  and shared soft time budget do not guarantee a maximum frame duration.
- Browser tile uploads use one per-surface state machine in the WGPU preparation callback. Resource allocation is split
  into bounded stages, writes are at most 256 KiB, each frame admits at most 4 MiB within a measured 1.5 ms budget, and
  partial tiles remain unpublished. Hidden partial uploads are retained behind byte and entry limits so a flick does not
  discard useful work.
- Routes are immutable world-space GPU geometry with shader speed colours and antialiasing. Tile placement is derived
  from immutable coordinates and the current camera. Rendering is clipped to the map, hover uses a persistent spatial
  index, labels wait for camera motion to settle, and outward wheel input at a zoom bound is ignored.

### Remaining current-slice acceptance

- The user confirmed horizontal world repetition and the viewport zoom floor visually. Rebuild and recheck these,
  low-zoom coverage, and mixed-script labels against the final browser bundle. The low-zoom fixtures also run in the
  emitted-WASM worker test during the HASS build; native tests do not replace that check or visual acceptance. Native
  GPU pixel readback passes; the separate headless GL run could not create a device.
- Repeat throughput acceptance after restoring low-zoom coverage. The new lifecycle markers are present in
  `broken.json.gz`, but rejected tiles prevent a complete loading comparison. Upload latency ends at GPU resource
  publication, not screen presentation.
- Repeat the deterministic browser interaction trace against the bounded admission path. User Timing exposes transfer
  admission, allocation, text reconstruction, WGPU preparation, and WGPU draw as separate trace events. Analyze the raw
  trace with `just hass::profile-analyze TRACE`; the analyzer selects the HASS renderer process from the page URL
  instead of a browser or tool-provided headline. Require tile-admission and tile-allocation events and no
  worker-fallback events or main-thread tile requests before attributing results to worker execution.
- The analyzer also reports tile request latency/cache outcomes and worker task activity. Use
  `just hass::profile-analyze TRACE --timeline` for one-second loading windows, or `--json` for request-level data.
  Request latency includes event delivery; worker replies include route/label results. End-to-end per-tile
  time-to-visible still needs correlated application markers.

### Deferred renderer work (not part of this closing batch)

- Route and label ready state remains painter-owned. Move it into the immutable runtime scene, replace the quadratic
  label collision scan with a spatial grid, and retain same-zoom label translation plus zoom invalidation.
- Remove the public tile task/decoder completion API after desktop and HASS hosts move behind the runtime. Hosts should
  provide transport and cache services, not manipulate activity-view tile state.
- After closing the current browser slice, move map GPU uploads and drawing to a worker-owned `OffscreenCanvas`. Extract
  the renderer from egui callbacks first and verify parity, then transfer a map-only canvas and retain prepared geometry
  and GPU resources in the worker. Keep application UI on the main thread. Verify canvas placement, clipping, input
  alignment, resize/DPR changes, teardown, and device-loss reporting. Native 4x MSAA remains unchanged.
- Keep offscreen chart cards dormant and cache chart analysis by activity, axis, lap, units, theme, and width.
  Map-driven repaints must preserve the shared cursor, playback, lap selection, and map/chart sample-index
  synchronization.

### Deferred UI issues (not part of this closing batch)

- [ ] Keep chart endpoint tick labels inside the chart card's content bounds. Reported on 2026-09-18: the elevation
  chart's `0.0 km` label extends left beyond the plot/content edge. Check both endpoints, narrow layouts, and time and
  distance axes; add a visual regression check when fixing the layout.

### Next step after OffscreenCanvas: semantic interaction runner

Order: close the browser acceptance checks above, complete and validate the map-only OffscreenCanvas slice, then
implement this runner. It is not a prerequisite for OffscreenCanvas.

- Reuse egui's AccessKit semantics and `egui_kittest`/`kittest` queries. Complete custom-widget semantics and use stable
  identifiers where labels are ambiguous or translated; do not build a second widget lookup tree.
- Share one scenario runner between built-in demos and automated stress tests. Target controls, maps, and charts
  semantically; resolve current bounds for clicks, drags, flicks, wheel zoom, and scrubbing. Inject normal input rather
  than directly changing application or camera state. Include readiness waits, assertions, deadlines, cancellation, and
  explicit failure results. Restrict automation to explicitly enabled demo/test sessions.
- Provide a small browser control API for starting a named scenario, reading status/results, and cancelling it. Chrome
  DevTools MCP starts a trace with automatic stopping disabled, starts the scenario, observes completion or failure, and
  stops/saves the trace. Do not use per-gesture MCP round trips to pace the workload.
- Emit scenario/phase timing markers and record viewport, DPR, graphics backend, scheduled versus actual action timing,
  missed deadlines, and completed workload. Keep warm-cache interaction and tile-arrival stress separate. A stalled run
  must not pass by silently executing fewer actions. Measure the runner's overhead.
- Initial coverage: select a demo activity, pan/flick/zoom, operate playback, select/reset a lap, and scrub linked
  charts. Reuse scenarios as regression tests for subsequent renderer changes. Input injected inside egui does not
  validate browser event dispatch latency, trusted gestures, or native dialogs; test those separately.

### Performance evidence

- `Trace-20260919T000106.json.gz`: 368 interaction frame intervals, p95 16.71 ms, maximum 16.99 ms, none above 33 ms;
  one gap of 37.86 ms among all 1,428 intervals, outside the analyzer's interaction windows. No blocking microtasks
  above one 60 Hz frame and no worker-fallback markers. All 342 tile requests originated in the worker and returned HTTP
  200, with no transport failures or unfinished requests; 254 were browser-cached and 88 uncached. This confirms near-60
  Hz interaction, not a literal pass of the documented 16.7 ms p95 ceiling or proof of presentation FPS. Admission work
  p95/max was 1.50/1.90 ms, allocation max 0.20 ms, WGPU preparation max 1.50 ms, and draw max 0.20 ms.
  Request-to-finish p95/max was 277.53/611.37 ms. Upload queue-to-publication p95/p99/max was 34.00/1,467.50/2,669.00 ms
  across 145 publications. The upload timer keeps running while retained tiles are offscreen; these durations are not
  main-thread blocking time. Without per-tile visibility correlation, the trace cannot establish whether the tail is
  harmless retention or delayed visible coverage. The user separately confirmed a fresh demo data directory and post-fix
  tooltip appearance.
- `broken.json.gz` contains all three new lifecycle markers. Admission wait/total p95 is 16.6/22.0 ms; upload latency
  p95 is 17.5 ms. All 314 tile GETs returned HTTP 200, with no recorded transport failures. Interaction intervals have
  p95 16.73 ms, maximum 24.84 ms, and none above 33 ms. Across all frames, 215 intervals exceed 33 ms; the trace does
  not by itself attribute these gaps to tile work. Low-zoom decoder failures keep browser acceptance open.
- The user confirmed reliable coverage after the tile-admission and polygon-limit fixes. The accompanying trace,
  `Trace-20260918T145001.json.gz`, has 264 worker tile GETs (239 cached), 3.18 ms median request-to-finish time, and
  admission median/p95 of 0.2/0.5 ms. Interaction-frame intervals have p95 16.71 ms, maximum 21.61 ms, and none above 33
  ms. The user reported slow tile arrival under the former 512 KiB/frame ceilings. This is the baseline for the pending
  throughput comparison.
- The interaction target is 60 FPS, UI CPU p95 below 16.7 ms, and no tile or label publication stall above 33 ms on the
  bundled 67.92 km activity at roughly 1100 by 720 pixels. Missing resources leave blank map regions and never disturb
  drag, wheel, or inertia.
- Capture with `just desktop::profile demo NAME --gfx`; analyze with `just desktop::profile-analyze NAME`; compare with
  `just desktop::profile-compare BEFORE AFTER`. Reports preserve raw Samply data and provenance while adding runtime
  counters. The gate requires at least 120 interaction intervals and two seconds of camera motion.
- `01-antialiasing` recorded 433 interaction intervals over 7.25 seconds: 17.544 ms p95, 34.638 ms maximum, 1.022 ms UI
  p95, and 0.709 ms map UI p95. `02-route-aa` recorded 1,006 intervals over 17.1 seconds: 21.086 ms p95, 34.196 ms
  maximum, 1.145 ms UI p95, 0.838 ms map UI p95, 0.049 ms render-draw p95, and 0.291 sampled CPU cores. Both narrowly
  fail the cadence gate while application CPU and draw measurements remain low; investigate presentation cadence only if
  interaction becomes visibly uneven.
- The 2026-09-17 browser trace before bounded admission recorded smooth panning near 16.77 ms p95, then tile-completion
  bursts at 75.77--152.76 ms p95 with a 224.88 ms maximum. Forty-two blocking completion microtasks consumed 2.396
  seconds in total and reached 168.69 ms. Pointer and wheel dispatch stayed below 0.24 ms, GPU tasks below 14.53 ms, and
  compositor tasks below 0.54 ms. The completion callback, not input, compositing, or GPU execution, was the identified
  bottleneck. The repeat trace must keep interaction p95 at or below 16.7 ms, contain no tile-publication stall above 33
  ms, and leave GPU/compositor cost materially unchanged.
- Never launch a GUI from unattended validation. Interactive profiling is user-run; automated checks remain headless.
- The 27.2-second `longer.gz` capture had 245 main-thread tile requests, no tile-admission events, and completion
  microtasks reaching 70.97 ms. Its preceding startup capture shows the worker requesting the unhashed WASM filename and
  receiving HTTP 404. These captures exercised local fallback, not bounded admission. Worker initialization now receives
  the emitted JS and WASM URLs from the document's preload links.
- `Trace-20260917T220636.json.gz` confirms worker execution over 35.2 seconds: 343 worker tile requests, zero
  main-thread tile requests, and no fallback events. Reanalysis using separate pointer-contact and wheel-burst windows
  selects 457 overlapping frame intervals: 16.71 ms p95, 16.80 ms p99, and 20.53 ms maximum. Wheel windows include a 200
  ms tail; these are input-based windows, not measured camera motion or presentation FPS. Across all 2,047 frame
  intervals, the maximum was 31.57 ms. No main-thread task exceeded 33 ms and no microtask exceeded 16.7 ms. Tile
  admission measured 0.30 ms p95 and 0.80 ms maximum across 771 events, before destination allocation moved into that
  timer. A new capture is needed to validate the allocation-inclusive timing. The two manual captures are not identical
  workloads.
- `GARMIN_TEST_WEB_ROOT=/path/to/built/share/garmin-hass/web node infra/javascript/map-worker.test.mjs` checks the
  emitted WASM's ready handshake and four-buffer tile transfer headlessly, with fetch responses supplied by the test.
  Analyzer regression tests cover navigation, paired and zero-duration Chrome measures, and fallback diagnostics.

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
- Put quiet floating control clusters on the map: fit at top-left, vertical zoom at top-right, play/stop with a stepped
  0.5x/1x/2x speed slider at bottom-center, and full-activity reset at bottom-left when a lap is selected. Direct wheel
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
- Cover browser transfer versioning, malformed lengths, indices and UTF-8, empty and oversized packets, bounded
  admission, stale completions, and atomic publication. Cover browser upload chunking and budget accounting without
  wall-clock-sensitive CI assertions. Verify that native typed geometry and browser packed geometry expose identical
  WGPU bytes, partial uploads cannot become drawable, and both paths apply the same map scissor.
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
