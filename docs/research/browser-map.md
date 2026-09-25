# Browser map evidence

Recorded 2026-09-19. Scope: the bounded map committed as `10234ef`, subsequent upload telemetry, and renderer
extraction. This is an evidence record, not a completion checklist. Open work lives in the
[activity map plan](../plans/activity-map-workspace.md). Current renderer ownership and limits live in
[activity map architecture](../architecture/activity-map.md).

Raw traces remain private local inputs, not committed fixtures. Measurements concern the bundled synthetic demo route.
Use the maintained `infra/python/browser_trace_analysis.py` via `just hass::profile-analyze` to reproduce summaries.

## Semantic interaction findings

The runner resolves AccessKit author IDs to current clipped bounds and injects ordinary egui input. AccessKit applies
its root pixel-scale transform to bounding boxes; input coordinates are logical points. Removing that transform before
clipping fixes the HiDPI profile-selection miss. The coordinate contract and diagnostic overlay are documented in the
[run contract](../architecture/build-system.md#opt-in-browser-interaction-runs).

On Chrome 153 / Apple M3 Max at native DPR 2 and a 1200x999 logical viewport, stationary arrival, warm pan/zoom/fit, and
activity smoke complete in both worker-GL and main-GL. Activity smoke exercises playback, speed selection, laps, charts,
indoor-activity replacement, and map restoration. Main-GL also cancels a drag with Escape and completes a subsequent run
from an active profile.

Remaining input/lifecycle and performance questions live in the
[browser verification plan](../plans/activity-map-workspace.md#browser-verification).

## Browser rendering constraints

WebGL2 requires raster-only device limits; requesting compute limits prevents worker initialization. On the tested
Chrome/ANGLE setup, WebGPU exposed its API but returned no adapter. Renderer selection must report that failure rather
than substitute a backend.

Chrome DPR emulation returned inconsistent physical sizes: a 100-CSS-pixel element reported 100 physical pixels through
`ResizeObserver.devicePixelContentBoxSize` while `devicePixelRatio` was 2. Use native-DPR testing for canvas alignment
rather than compensating in application sizing.

Worker errors must stay local to the map. Terminate the failed worker, preserve the original error, and place recovery
controls above both canvases. A WASM panic may leave an object borrowed, so calling `free()` during failure handling can
mask the original cause. Applied egui texture deltas must be drained before their collections are dropped.

## Renderer projection

Clipping must preserve the full map projection. Remap it into the target-bounded viewport with a scale/offset uniform
shared by tiles, routes, and highlights. Build the scene before constructing callbacks, and retain the same frame for
preparation and drawing.

Egui can transform a callback rectangle after preparation. Use the final paint rectangle to create an immutable camera
binding for that draw; rewriting a shared uniform would alter earlier draws in the same submission. Unchanged placement
reuses the surface buffer. Late-transform allocation contributes to draw CPU timing.

### Post-review browser capture

`Trace-20260919T194959.json.gz` records emitted asset `815b37b00ce1ca6d`, also confirmed in the live browser. All 399
worker tile requests completed with HTTP 200 (299 cached, 100 uncached), with no main-thread tile requests or
worker-fallback timing. Across 347 interaction frame intervals, p95 was 18.47 ms and maximum 23.49 ms; none exceeded 33
ms. Across the entire recording, 31 of 1,753 intervals exceeded 33 ms, with a maximum of 46.40 ms. These are main-thread
frame intervals, not presentation FPS or a matched performance comparison. Preparation peaked at 1.60 ms, draw
submission at 0.20 ms, and tile admission at 1.70 ms. Worker tasks peaked at 65.00 ms.

Upload lifecycles contain 174 drawn, 10 published-but-not-drawn, 2 released, and 8 incomplete entries, with no malformed
or partial events. Among published/drawn entries, visible queue lifetime has p95 35.50 ms and maximum 95.90 ms; measured
upload CPU work peaks at 1.80 ms. The 1,079.70 ms publication tail consists of 1,051.20 ms hidden and 28.50 ms visible,
with no recorded first draw for that tile. Recorded publication-to-first-draw delay peaks at 20.40 ms. Incomplete and
released entries do not establish successful completion. The older unexplained tail and deferred controlled performance
validation remain open.

The startup configuration mark is absent from the trace, although correlated upload events are present. Live inspection
independently confirms enabled telemetry, backend `Gl`, and DPR 1. Its current viewport/canvas is 2367 by 1268; the
startup mark records 3389 by 1324, so neither is assumed to be the unchanged capture viewport. No warning/error console
messages were returned. Eleven sampled trace screenshots show initial loading and subsequent map navigation. A live
screenshot shows the settled route, labels, controls, and map background without visible tile gaps. The attempted
scroll/resize check did not establish a changed rendered state: a resize briefly reported a narrower viewport, but
subsequent inspection returned the original dimensions and unchanged appearance. Consequently this is ordinary browser
smoke evidence, not independent visual verification of partially offscreen projection or late callback transforms.

The user subsequently confirmed that it still works fine. Together with the scoped GPU regressions, rebuilt trace, and
live settled-map inspection, this closes browser smoke acceptance for the review fixes. It does not establish that the
user exercised every clipping/transform case, or replace deferred controlled performance validation. No further manual
recording is requested.

### Post-extraction interaction capture

The user supplied `Trace-20260919T162515.json.gz`, recording emitted asset `3cc8fd55eb581c95`. The maintained analyzer
found 411 completed worker tile requests, all HTTP 200 (284 cached and 127 uncached), no main-thread tile requests, and
no worker-fallback timing. Across 351 interaction frame intervals, p95 was 18.67 ms and maximum 24.26 ms; none exceeded
33 ms. These are main-thread frame intervals, not presentation FPS. Preparation peaked at 1.50 ms, draw submission at
0.30 ms, and tile admission at 1.70 ms. Worker tasks peaked at 65.05 ms.

Correlated telemetry contains 173 drawn, 14 released, 11 published-but-not-drawn, and 4 incomplete uploads, with no
malformed or partial events. All four incomplete lifecycles end with a hidden marker, not a successful completion. There
are 25 pending-upload visibility transitions. Among published/drawn lifecycles, visible queue lifetime has p95 34.30 ms
and maximum 100.50 ms; measured upload CPU work peaks at 2.40 ms. One drawn tile spends 1,385.00 ms hidden within a
1,482.40 ms enqueue-to-publication lifetime. The longest upload-latency timing, approximately 1,819.50 ms, correlates
with an upload spending 1,784.20 ms hidden, publishing, then being released without a recorded first draw. This
establishes offscreen retention as the dominant contributor to these particular tails, not to the historical
2.669-second observation.

Publication-to-first-draw delay reaches 2,120.20 ms. Pending-upload visibility accounting stops at publication, so the
recording does not establish whether that delay represents an offscreen tile or a visible rendering delay. Neither this
tail nor the 100.50 ms visible queue maximum is a performance acceptance. The startup configuration mark is absent;
upload events establish active instrumentation during capture, but do not establish its initial viewport, DPR, or
backend. Live inspection independently confirms the same `3cc8fd55eb581c95` JS/WASM asset, enabled telemetry, backend
`Gl`, and DPR 1. The current canvas and viewport are 2367 by 1324; the startup mark records 3389 by 1324, so they must
not be treated as an unchanged capture viewport. No warning/error console messages were returned. Screenshot capture
timed out, so the assistant did not independently inspect appearance. The user subsequently reported that interaction
looked good. Combined with the scoped tests, live build check, and trace, this closes browser smoke acceptance for the
extraction. The latency observations and deferred controlled performance measurements remain open.

## Telemetry overhead comparison

The user rebuilt HASS and started the demo with `--no-map-upload-telemetry`, using the dedicated `.tmp/hass-telemetry`
data directory. Live inspection confirmed asset `b16287233d847358`, backend `Gl`, DPR 1, and `enabled: false` in the
startup configuration mark. After warming the map, a fresh load at 1440 by 1000 selected Alex Rider's first cycling
activity using screenshot-guided DOM pointer events. Playback remained paused; the completed map had no visible gaps.
The console reported `No available adapters.` while the configuration identified `Gl`; no console errors were listed.

MCP reported starting and stopping a recording, but refused raw export to both repository and Downloads paths. The user
confirmed no recording was available in their DevTools. No raw artifact was obtained from that attempt; the earlier
instruction to save an existing recording was incorrect. Its live checks establish only the flag and visual state, not
raw-trace evidence.

The user instead supplied `Trace-20260919T132634.json.gz`. The maintained analyzer found 112 worker tile requests, all
HTTP 200 and completed, with no main-thread tile requests or worker-fallback timing. Of these, 107 were browser-cached
and 5 uncached. Interaction frame intervals had p95 18.75 ms and maximum 21.97 ms across 206 intervals; none exceeded 33
ms. These are main-thread frame intervals, not presentation FPS. Upload-latency timings had 43 samples, p95 24.10 ms and
maximum 51.10 ms. Preparation callbacks peaked at 1.60 ms; worker tasks peaked at 138.83 ms.

This replacement includes 14 wheel events and requests across multiple zoom levels, not the stationary comparison
workload. Neither the startup telemetry configuration nor correlated upload lifecycles were captured. Their absence is
consistent with the previously inspected disabled session, but does not independently establish that mode for this
recording. It is useful interaction evidence, not an accepted overhead baseline.

The subsequent `telemetry-off-01.json.gz` is a usable first stationary disabled capture. Its startup mark confirms
`enabled: false`, backend `Gl`, DPR 1, and viewport 1761 by 1324; the emitted asset is still `b16287233d847358`. This
differs from the earlier MCP viewport, so enabled comparisons must match this manual capture's layout instead. It
includes one click and no wheel events. All 20 tile requests originate in the worker, are browser-cached, and complete
with HTTP 200; no worker fallback or correlated upload lifecycle marks are recorded. The requested tile set is zoom 11,
x 1020 through 1024, y 679 through 682. Six upload-latency samples complete with maximum 19.00 ms.

Across 28 preparation/draw callbacks, measured preparation totals 5.20 ms (p95 1.20 ms, maximum 1.50 ms); drawing totals
0.199 ms (maximum 0.10 ms). These short measurements include many zero-valued samples and are limited by clock
resolution. The last preparation marker is 2.078 seconds after the configuration mark; renderer events continue to 4.906
seconds, so the capture contains a settled tail but is shorter than the requested ten-second workload. Two main-thread
microtasks exceed 16.67 ms, with maximum 64.07 ms; whole-trace frame gaps include startup and idle time and are not a
map-animation FPS result. This single disabled run establishes neither instrumentation overhead nor its variability. An
overhead conclusion would require matched workloads and repeated controlled blocks.

`telemetry-on-01.json.gz` confirms enabled telemetry with the same asset, backend, and DPR, but its startup viewport is
2367 by 1324 rather than 1761 by 1324. It requests 28 cached worker tiles instead of 20, adding x columns 1019 and 1025
at the same zoom and y range. All requests complete with HTTP 200; no worker fallback is recorded. Ten valid upload
lifecycles reach first draw, compared with six upload-latency completions in the disabled capture. Preparation totals
9.90 ms over 46 callbacks, with maximum 1.60 ms; maximum visible upload lifetime is 68.80 ms. The unequal viewport and
workload invalidate this pair for overhead attribution. Neither the higher preparation total nor the longer queue
lifetime measures telemetry cost. Both captures remain functional evidence. Further manual comparison captures are
deferred: resume the measurement after OffscreenCanvas with the planned semantic interaction runner controlling the
viewport, workload, and repeated trials. This unresolved measurement does not block renderer extraction and does not
support a negligible-overhead claim. New renderer code will require fresh on/off baselines.

## Correlated telemetry and current evidence

The upload investigation adds versioned `garmin.map.upload` marks with a unique upload ID and XYZ coordinates: queued,
hidden/visible, first work, batched CPU work/bytes, published, first draw, and released without drawing. Only one mark
is retained in the browser Performance timeline; the active trace retains the history. The maintained analyzer separates
viewport-visible queue lifetime, that lifetime excluding measured CPU work, offscreen retention, time from latest return
to publication, and publication-to-first-draw delay. Visibility means membership in the map's viewport upload demand,
not browser-tab visibility or screen occlusion. First draw is command encoding, not presentation. Incomplete, released,
and invalid uploads are reported separately and excluded from completed-upload percentiles. The old trace cannot
retroactively provide these correlations; its 2.669-second tail remains unexplained.

Live verification on 2026-09-19 confirmed rebuilt browser asset `7e0c922b68ffbc68` at 1515 by 742 and a fully rendered
London map. The user supplied private `stationary.gz` and `pan-return.gz` recordings; both were analyzed with the
maintained analyzer and both record that asset hash.

| Upload evidence                                            | Stationary | Pan/return |
| ---------------------------------------------------------- | ---------: | ---------: |
| Complete enqueue-to-first-draw lifecycles                  |          6 |         18 |
| Maximum visible enqueue-to-publication                     |    18.3 ms |    19.6 ms |
| Maximum visible waiting excluding measured upload CPU work |    16.6 ms |    17.0 ms |
| Maximum accumulated upload CPU work per tile               |     2.7 ms |     3.6 ms |
| Maximum publication-to-first-draw command encoding         |    17.7 ms |    17.7 ms |
| Hidden/visible transitions                                 |          0 |          0 |

Neither recording has malformed, partial, invalid, released, or unfinished upload lifecycles. Recording continues 6.76
seconds and 3.37 seconds respectively after the last upload mark. All 20/60 tile requests completed with HTTP 200,
without transport failures or worker fallback. Stationary requests were all browser-cached; pan/return had 52 cached and
8 uncached requests. The slowest pan/return request was uncached and took 1.814 seconds from request to finish, upstream
of upload admission; this includes browser/worker delivery delay and does not isolate network or server time.

Pan/return interaction frame intervals had p95 18.25 ms and maximum 24.10 ms (115 intervals, none above 33 ms); these
are not presentation FPS. Long main-thread microtasks occurred before the first upload in both captures, including
92.09/104.20 ms application callbacks; the traces do not establish their internal cause. Worker tasks reached 169.83 ms
during pan/return. Upload preparation callbacks reached 3.10 ms. These observations do not establish instrumentation
overhead, nor do they justify larger upload budgets. The 2.669-second historical tail remains unreproduced: neither
capture exercised retained pending uploads moving offscreen and back, so offscreen retention is still a hypothesis.

Hidden uploads retain their identity and remaining bytes without writing progress. The production-queue regression
simulates a 2.6-second hidden interval and verifies a single publication after resumption. Upload advancement still uses
the production clock. Progress marks are batched per frame and browser retention is bounded to one mark.

## Trace validity rules

- Malformed identifiable events invalidate that upload ID's cohort; unidentifiable gaps invalidate every upload cohort
  in the recording. If IDs repeat after navigation, uncertain malformed events invalidate every occurrence rather than
  guessing an epoch. Publication requires nonzero uploaded bytes; zero-duration CPU work remains valid. Invalid cohorts
  cannot enter completed distributions.
- Finite numeric, non-boolean timestamps are validated before sorting. Invalid timestamps are excluded from frame and
  network analysis but retained for upload invalidation and diagnostics. Signed finite timestamps remain valid for
  relative traces. Regression tests cover parsing and report output.

## Earlier profiling evidence

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
  not by itself attribute these gaps to tile work. Low-zoom decoder failures invalidated that capture's coverage
  acceptance; later checks above supersede it.
- The user confirmed reliable coverage after the tile-admission and polygon-limit fixes. The accompanying trace,
  `Trace-20260918T145001.json.gz`, has 264 worker tile GETs (239 cached), 3.18 ms median request-to-finish time, and
  admission median/p95 of 0.2/0.5 ms. Interaction-frame intervals have p95 16.71 ms, maximum 21.61 ms, and none above 33
  ms. The user reported slow tile arrival under the former 512 KiB/frame ceilings. This records the earlier 512
  KiB/frame baseline; later manual workloads are not controlled throughput comparisons.
- `01-antialiasing` recorded 433 interaction intervals over 7.25 seconds: 17.544 ms p95, 34.638 ms maximum, 1.022 ms UI
  p95, and 0.709 ms map UI p95. `02-route-aa` recorded 1,006 intervals over 17.1 seconds: 21.086 ms p95, 34.196 ms
  maximum, 1.145 ms UI p95, 0.838 ms map UI p95, 0.049 ms render-draw p95, and 0.291 sampled CPU cores. Both narrowly
  fail the cadence gate while application CPU and draw measurements remain low; investigate presentation cadence only if
  interaction becomes visibly uneven.
- The 2026-09-17 browser trace before bounded admission recorded smooth panning near 16.77 ms p95, then tile-completion
  bursts at 75.77--152.76 ms p95 with a 224.88 ms maximum. Forty-two blocking completion microtasks consumed 2.396
  seconds in total and reached 168.69 ms. Pointer and wheel dispatch stayed below 0.24 ms, GPU tasks below 14.53 ms, and
  compositor tasks below 0.54 ms. The completion callback, not input, compositing, or GPU execution, was the identified
  bottleneck. Later bounded-admission recordings supersede that failing baseline; they do not measure identical
  workloads.
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
  timer. The newer captures above include allocation timing. These manual captures are not identical workloads.
