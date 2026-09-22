# Activity map workspace

Close the shared activity viewer's telemetry and rendering-isolation work for desktop and HASS. Current recording,
interaction, and rendering contracts live in [activity map architecture](../architecture/activity-map.md).

HASS uses worker-GL by default; `--no-map-render-worker` restores the original renderer. The semantic interaction runner
is demo-only and opt-in. This plan owns the remaining lifecycle, resource, and performance questions.

## Browser verification

Use the [run contract](../architecture/build-system.md#opt-in-browser-interaction-runs) and
[development safeguards](../development.md). Renderer ownership is described in the
[map architecture](../architecture/activity-map.md); empirical findings live in
[browser map evidence](../research/browser-map.md#semantic-interaction-findings).

### Open questions

- Does a fresh HASS launch without renderer flags select worker-GL and display the activity map?
- Do real pointer, wheel, and keyboard events remain isolated during a run, with ordinary input restored after Stop?
  Exercise the Automation menu and Stop button through browser input, including cancellation during a held drag.
- Do focus loss and hidden tabs cancel promptly, release held input, and recover correctly when the page returns?
- Are worker resources released across activity replacement, map removal, page teardown, and renderer failure?
- Does worker-GL reduce main-thread work without worse visible responsiveness or unacceptable memory growth?
- What is the CPU cost of upload telemetry and of the scenario runner itself?

### Comparison procedure

Resolve the input/lifecycle questions before collecting timed comparisons. Run the real activity viewer, not the
composition fixture. Use the same build/server, warmed data/cache directory, activity, viewport, DPR, theme,
instrumentation, and tile identities for both modes. Compare worker-GL with `?map-render-mode=main-gl`; worker-WebGPU
changes both thread ownership and graphics backend and is a separate experiment.

Collect five alternating matched pairs, keeping stationary arrival and warm interaction separate. Reload to the profile
chooser before each run. Start a Chrome trace before launching the scenario, with automatic stopping disabled. The Rust
runner owns gesture pacing; browser tooling only starts, observes, and stops the run. After the terminal result, save
the trace and JSON report together, labelled by renderer, scenario, pair, and revision.

Analyze traces with `just hass::profile-analyze TRACE --json`. Compare completed workloads, CPU distributions, memory,
and run variation. Readiness records preparation/upload completion; compositor presentation and browser input latency
require separate measurements. Reported driver/tree-hook timing excludes AccessKit generation and overlay painting.

## Telemetry overhead

Measure upload-telemetry overhead with alternating on/off runs using `--no-map-upload-telemetry`. This switch disables
upload observers and their clocks, locks, and serialization; existing render timings and upload budgets stay enabled.
The startup mark records the setting, viewport, DPR, and backend. Reload after changing the host option.

Use scripted stationary arrival with matching tile sets, cache status, and publication counts. Compare prepare/draw CPU
distributions and run variation. Re-establish both baselines after renderer changes.

Separate visible queue time from hidden retention before attributing upload delays to GPU work. The post-extraction
capture observes hidden retention directly; its visible queue maximum is 100.50 ms. See
[capture evidence](../research/browser-map.md#post-extraction-interaction-capture).

## Rendering isolation

The [renderer architecture](../architecture/activity-map.md#rendering-boundary) records the existing boundaries.

### Renderer constraints

Keep native 4x MSAA and existing upload budgets unchanged. Transfer route data once per revision and bound dynamic
updates to one in flight plus one replaceable pending update. Keep renderer failures explicit. Do not introduce
per-frame image transfers, whole-UI paint-list transfers, or shared-memory deployment requirements.

### Verification rules for subsequent changes

- Rebuild and repeat emitted-WASM and visual checks when runtime changes invalidate the recorded browser evidence.
  Native tests alone do not establish browser parity.
- Analyze raw traces with `just hass::profile-analyze TRACE --timeline` or `--json`. Require admission/allocation
  markers, no worker fallback, and no main-thread tile requests before attributing results to worker execution.
- Keep map and tab visible, record before enqueue, and wait for loading to settle. Request duration includes event
  delivery. First draw is command encoding, not screen presentation; incomplete or malformed lifecycles must not support
  successful completion claims.

### Deferred renderer work

- Route and label ready state remains painter-owned. Move it into the immutable runtime scene, replace the quadratic
  label collision scan with a spatial grid, and retain same-zoom label translation plus zoom invalidation.
- Remove the public tile task/decoder completion API after desktop and HASS hosts move behind the runtime. Hosts should
  provide transport and cache services, not manipulate activity-view tile state.
- Keep offscreen chart cards dormant and cache chart analysis by activity, axis, lap, units, theme, and width.
  Map-driven repaints must preserve the shared cursor, playback, lap selection, and map/chart sample-index
  synchronization.

### Deferred UI issues

- [ ] Investigate the faint rectangular grid reported over rural map fills in worker-GL on 2026-09-20. Compare the same
  view in main-GL and worker-GL at fractional zoom/DPR; add an adjacent-tile pixel regression before changing coverage.
  Source inspection found feathered tile-background rectangles and no per-tile clipping of buffered geometry; these are
  candidates, not a confirmed diagnosis of the screenshot. Do not mask the defect with overlapping tiles or globally
  disable antialiasing.
- [ ] Keep chart endpoint tick labels inside the chart card's content bounds. Reported on 2026-09-18: the elevation
  chart's `0.0 km` label extends left beyond the plot/content edge. Check both endpoints, narrow layouts, and time and
  distance axes; add a visual regression check when fixing the layout.

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
