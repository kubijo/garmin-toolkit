# Activity workspace and map rendering

The shared `garmin-ui` workspace presents stored activities and device FIT previews in desktop and HASS. It owns the
activity list, reusable viewer, map, charts, laps, and playback. The host exposes owned recording snapshots, not its
storage objects. [Browser map evidence](../research/browser-map.md) records validation;
[the active plan](../plans/activity-map-workspace.md) owns remaining isolation work and acceptance gates.

## Recording and interaction

One recording snapshot carries ordered samples with optional coordinates and measurements, laps, and timer events.
Stored details and device FIT previews use the same projection. Activity selection is observation-ID based with stale
result protection. One authoritative original-sample cursor drives idle, hover, pinned, and playback state across the
map, charts, metric readouts, and lap highlighting.

The wide workspace uses resizable activity and summary/lap panes around the map and independently scrollable charts.
Narrow views retain the viewer and expose subordinate panes as drawers. Missing coordinates do not remove charts,
inspection, laps, or playback. The shared visual language applies; external activity viewers are behavioral references,
not sources of copied implementation or design.

`egui_plot` owns chart axes, grids, lines, fills, transforms, clipping, and pointer coordinates. Chart hover maps X to
the nearest original sample. Missing measurements split rendered series and omit only the affected point marker, not the
shared vertical guide. Width-sensitive first/minimum/maximum/last buckets reduce rendered series without discarding
inspection data. Distance is the default domain only with monotonic anchors and nonzero span; otherwise elapsed time is
used. Profile units apply.

Route hover performs the inverse sample lookup. Clicking pins; Escape or empty analysis space clears. Lap hover
highlights without moving the camera; selection fits the lap range and restricts playback until reset. Playback uses a
stepped 0.5x/1x/2x slider, skips timer-stopped intervals, and runs the activity over 30 seconds at 1x. Passive hover
cannot override playback; clicking or scrubbing pauses and pins. Marker interpolation stays between adjacent valid
coordinates in one continuous segment.

## Host tile service

`garmin-map-tiles` validates OpenFreeMap TileJSON and fetches tiles through a 256 MiB HTTP-aware disk cache with atomic
writes, freshness metadata, and LRU pruning. TileJSON is capped at 1 MiB and a tile at 2 MiB. Coordinates, HTTPS
templates, provider hosts, and response shape are validated. HASS exposes relative same-origin
`map/tiles/{z}/{x}/{y}.pbf` routes with ETags/freshness, separate from immutable application assets. Provider failure
leaves the activity and neutral route canvas usable; attribution remains visible.

`walkers` supplies map style/types behind the owned runtime, not the interactive map widget. Prepared tile caching,
deduplicated requests, scheduling, and retry state belong to that runtime.

## Rendering boundary

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

GPU uploads and map draw submission use a renderer-owned prepare/draw boundary. Its host supplies one frame snapshot and
full physical-pixel projection and target-bounded viewport/scissor, plus a WGPU queue and render pass. The shared shader
remaps clip coordinates into the bounded viewport, preserving geometry scale and alignment when the map extends beyond
the render target. Device handles own pipelines; map surfaces own uniforms and upload controllers. The egui adapter
converts logical placement and invokes this boundary on the event/render thread, retaining the same frame through
preparation and drawing. It no longer stores pipelines in egui callback resources. Route and label ready state remains
painter-owned in this native/main-thread path. HASS instead defaults to a worker-owned map-only OffscreenCanvas using
WebGL2. The main thread retains egui, camera/input, and chart state; the render worker owns map scheduling, uploads,
labels, markers, and presentation. A separate preparation worker transfers bulk results directly to the render worker.
The egui composition callback supplies final placement and a transparent replacement-blend hole over the worker canvas.
`?map-render-mode=main-gl` retains the matched WebGL2 baseline; `--no-map-render-worker` restores the original host
path. Worker initialization failures are explicit, with no silent backend substitution. Native rendering is unchanged.
The browser integration lives in the `map_composition` Rust module and `map-composition.js`; the host's
`map_render_worker` setting selects the default worker path. The composition host exposes `window.garminMapComposition`
for status and disposal. Worker-WebGPU is an experimental alternative, selected explicitly with
`?map-render-mode=worker-webgpu`. Functional native-DPR scenario evidence and remaining lifecycle/performance limits are
recorded in [browser evidence](../research/browser-map.md#semantic-interaction-findings).

If egui transforms a callback rectangle after construction, painting uses the final placement with an immutable
corrected camera binding. It does not rewrite the prepared surface buffer, which earlier draws may still reference.
Unchanged placement keeps the reusable surface buffers; late-transform allocation is included in draw CPU timing.
Rebuilt-browser smoke acceptance and its visual-verification limits are recorded in the
[capture evidence](../research/browser-map.md#post-review-browser-capture).

## Regression contracts

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

Routing, geocoding, heatmaps, free chart zoom, offline-region downloads, and Garmin device-map management are separate
capabilities; the background map uses one owned style family under
[ADR 0025](../decisions/0025-proxied-online-vector-maps.md).
