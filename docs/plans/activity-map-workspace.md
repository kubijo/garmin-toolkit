# Activity map workspace

Close the shared activity viewer's telemetry and rendering-isolation work for desktop and HASS. Current recording,
interaction, and rendering contracts live in [activity map architecture](../architecture/activity-map.md).

The bounded browser map is implemented. GPU upload and map draw submission still run on the event/render thread;
complete the rendering-isolation work below before closing the overall workspace plan.

## Telemetry comparison control (2026-09-19)

The bounded-browser-map implementation was committed as `10234ef`; correlated upload telemetry followed in `11dcba8`.
HASS now provides a comparison switch. Renderer extraction has passed scoped checks and browser smoke acceptance; the
next implementation slice is OffscreenCanvas, followed by the semantic interaction runner.

Overhead measurement is deferred until the semantic interaction runner after OffscreenCanvas; it is not a blocker for
renderer extraction. Do not claim negligible instrumentation cost or change production upload budgets on the existing
evidence.

The comparison control is HASS's `--no-map-upload-telemetry` flag, forwarded by `just hass::run`. It disables upload
observers and their clocks/locks/serialization, not existing render/latency timings or upload budgets. The no-store
entrypoint supplies the startup choice to the browser; reload after changing the host flag. A
`garmin.map.upload-telemetry` mark records the choice, viewport, DPR, and graphics backend. The user rebuilt HASS; live
verification confirmed the disabled flag and a fully rendered map. The user reported `just qa::full` green after the
missing test import was fixed. No overhead measurement is claimed. Manual on/off captures validate both modes but have
different viewports and workloads, so they cannot isolate instrumentation cost. No further manual comparison captures
are requested. MCP raw export remains unavailable. See the
[capture record](../research/browser-map.md#telemetry-overhead-comparison).

The user reported full QA, audit, and sandboxed Rust verification gates green for the preceding slice on 2026-09-19,
before the comparison switch. This is user-reported acceptance, not verification of the current changes; see the
[evidence record](../research/browser-map.md).

[Browser map evidence](../research/browser-map.md#telemetry-review-resolution) records the resolved review and CI
license failure. The exact sandboxed license check passes; the entire workflow has not been rerun.

### Unresolved historical observation

The 2.669-second upload tail in `Trace-20260919T000106.json.gz` remains unreproduced, not fixed. The initial stationary
and pan/return recordings had no pending-upload visibility transitions. The post-extraction capture does record them,
attributing particular long queue tails to offscreen retention; it does not retrospectively explain the historical
spike. Visible queue lifetime still reaches 100.50 ms, and post-publication visibility is not measured. See the
[capture evidence](../research/browser-map.md#post-extraction-interaction-capture). Do not request another ordinary
manual pan or increase upload budgets on this evidence; retain these observations for scripted validation.

Browser acceptance, trace measurements, telemetry semantics, test results, and implementation context are maintained in
[Browser map evidence](../research/browser-map.md). Completed checks are not additional work items here.

## Rendering isolation

The [renderer architecture](../architecture/activity-map.md#rendering-boundary) records the existing boundaries.

### Extraction boundary and closing verification

GPU preparation and draw submission are extracted from egui callbacks without changing upload budgets, sample counts, or
worker ownership. The renderer accepts a frame, full physical-pixel projection, and target-bounded viewport/scissor; the
egui adapter owns logical-coordinate conversion. Callback preparation and drawing retain the same frame snapshot.
Pipelines belong to the device handle, and uniforms remain per map surface. This is not yet an OffscreenCanvas
implementation.

The closing review's clipping defect and missing production-ordering test are addressed. The shader now preserves the
full projection at target edges, and regressions cover translated pixel output and current-frame UI assembly. See the
[fix evidence](../research/browser-map.md#extraction-review-fixes).

Browser smoke acceptance is closed with the rebuilt capture, live settled-map inspection, and the user's confirmation
that it still works fine. The assistant did not independently verify the targeted scroll/resize case; deterministic
pixel regressions cover clipping and late transforms. See the
[capture evidence](../research/browser-map.md#post-review-browser-capture). Controlled performance validation remains
deferred to the scripted interaction runner.

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
- Next, move map GPU uploads and drawing to a worker-owned `OffscreenCanvas`. Use the accepted renderer boundary to
  transfer a map-only canvas and retain prepared geometry and GPU resources in the worker. Keep application UI on the
  main thread. Verify canvas placement, clipping, input alignment, resize/DPR changes, teardown, and device-loss
  reporting. Native 4x MSAA remains unchanged.
- Keep offscreen chart cards dormant and cache chart analysis by activity, axis, lap, units, theme, and width.
  Map-driven repaints must preserve the shared cursor, playback, lap selection, and map/chart sample-index
  synchronization.

### Deferred UI issues (not part of this closing batch)

- [ ] Keep chart endpoint tick labels inside the chart card's content bounds. Reported on 2026-09-18: the elevation
  chart's `0.0 km` label extends left beyond the plot/content edge. Check both endpoints, narrow layouts, and time and
  distance axes; add a visual regression check when fixing the layout.

### Next step after OffscreenCanvas: semantic interaction runner

Order: extract the renderer, complete and validate the map-only OffscreenCanvas slice, then implement this runner. It is
not a prerequisite for OffscreenCanvas.

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
- [ ] Resume the deferred upload-telemetry overhead comparison using scripted stationary tile arrival. Use one built
  package and data/cache directory, warm the cache, and fix activity, viewport, theme, DPR, backend, and capture
  settings. Repeat alternating on/off blocks with equal workload windows; require matching tile sets, cache status, and
  publication counts without failures or fallback. Analyze with `just hass::profile-analyze TRACE --json`, comparing
  prepare/draw CPU distributions and run variation, not queue wall time or presentation FPS. The disabled baseline
  retains option checks and existing profiling; it is not instrumentation-free. Re-establish both baselines after
  renderer changes.
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
