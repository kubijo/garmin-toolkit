# Activity map workspace

Close the shared activity viewer's telemetry and rendering-isolation work for desktop and HASS. Current recording,
interaction, and rendering contracts live in [activity map architecture](../architecture/activity-map.md).

The bounded browser map is implemented. GPU upload and map draw submission still run on the event/render thread;
complete the rendering-isolation work below before closing the overall workspace plan.

## Current telemetry slice (2026-09-19)

The bounded-browser-map implementation was committed as `10234ef`. This slice adds correlated upload telemetry and
regression coverage; OffscreenCanvas and the semantic interaction runner remain separate follow-ups.

- [ ] Measure telemetry overhead with a controlled comparison before claiming it is negligible. Existing captures do not
  isolate instrumentation cost. Keep production upload budgets unchanged.

The user reported the requested full QA, audit, and sandboxed Rust verification gates green on 2026-09-19. This is
user-reported acceptance, not a second assistant-run verification; see the
[evidence record](../research/browser-map.md).

[Browser map evidence](../research/browser-map.md#telemetry-review-resolution) records the resolved review and CI
license failure. The exact sandboxed license check passes; the entire workflow has not been rerun.

### Unresolved historical observation

The 2.669-second upload tail in `Trace-20260919T000106.json.gz` remains unreproduced, not fixed. New stationary and
pan/return recordings have short visible upload lifetimes but no pending-upload visibility transitions. The real-GPU
regression verifies retention/resumption, not the cause of that historical spike. Do not request another ordinary manual
pan or increase upload budgets on this evidence. Attribute a future reproduction using correlated visibility markers.

Browser acceptance, trace measurements, telemetry semantics, test results, and implementation context are maintained in
[Browser map evidence](../research/browser-map.md). Completed checks are not additional work items here.

## Rendering isolation

The [renderer architecture](../architecture/activity-map.md#rendering-boundary) records the existing boundaries.

### Verification rules for subsequent changes

- Rebuild and repeat emitted-WASM and visual checks when runtime changes invalidate the recorded browser evidence.
  Native tests alone do not establish browser parity.
- Analyze raw traces with `just hass::profile-analyze TRACE --timeline` or `--json`. Require admission/allocation
  markers, no worker fallback, and no main-thread tile requests before attributing results to worker execution.
- Keep map and tab visible, record before enqueue, and wait for loading to settle. Request duration includes event
  delivery. First draw is command encoding, not screen presentation; incomplete or malformed lifecycles must not support
  successful completion claims.

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

### Performance acceptance

- Target 60 FPS, UI CPU p95 below 16.7 ms, and no tile/label publication stall above 33 ms on the bundled 67.92 km
  activity at roughly 1100 by 720 pixels. Require at least 120 interaction intervals and two seconds of camera motion.
  Input-window frame intervals are not presentation FPS or measured camera-motion windows.
- Profile native runs with `just desktop::profile demo NAME --gfx`; analyze with `just desktop::profile-analyze NAME`
  and compare with `just desktop::profile-compare BEFORE AFTER`.
- Keep workload, viewport, backend, cache conditions, and instrumentation comparable. See the
  [recorded baselines and limitations](../research/browser-map.md) rather than treating historical captures as open
  verification tasks.
- Never launch a GUI from unattended validation. Interactive profiling is user-run; automated checks remain headless.
- Run emitted-WASM worker checks using
  `GARMIN_TEST_WEB_ROOT=/path/to/built/share/garmin-hass/web node infra/javascript/map-worker.test.mjs`.

## Boundaries and deletion gate

Use one owned OpenFreeMap style family under [ADR 0025](../decisions/0025-proxied-online-vector-maps.md). Routing,
geocoding, heatmaps, free chart zoom, offline-region downloads, and graphical Garmin device-map management are separate
work. Delete this plan after the shared viewer passes desktop and HASS runtime checks, maintained interface evidence is
reviewed, and its architecture and evidence are updated.
