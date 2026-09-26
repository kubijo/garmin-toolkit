# Browser map evidence

Evidence for the bounded map introduced in `10234ef`, subsequent upload telemetry, and renderer extraction. Open work
lives in the [activity map plan](../plans/activity-map-workspace.md). Current renderer ownership and limits live in
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

On 2026-09-25, Vivaldi/ANGLE worker-GL remained at zero actions and input events when started hidden. After a
41.73-second pause it completed warm interaction; screenshots rejected the hidden canvas and recovered on visibility. A
separate 40-drag sequence recorded one interrupted drag during a 4.81-second hide, then completed all drags and the
final map fit. Main-GL completed the same sequence after a 5.77-second hide interrupted a drag; capture also recovered.
The reports excluded paused time and were marked ineligible for performance comparisons. This verifies gesture recovery
in both renderers. A main-GL hidden-start check on 2026-09-26 also held at zero actions/input events, then completed
warm interaction after a 100.85-second pause, using 13.33 seconds of active time. Capture recovered and the report
excluded the run from performance comparisons.

Opening either WebGL2 mode directly in a background tab reported a ready renderer while capture rejected the hidden
canvas. The first workload stayed at zero actions/input events, then completed after visibility returned. Worker-GL
excluded a 31.65-second pause from 12.28 seconds of active time; main-GL excluded 23.74 seconds from 12.27 seconds. Map
capture recovered in both. Both emitted one unsupported `SetTheme(SystemDefault)` warning; the rebuilt worker-GL check
below emitted none after disabling native window-theme synchronization on web. Evidence is in
`.tmp/renderer-automation-acceptance/browser-lifecycle-20260926/{background-startup,main-gl-background-startup}/`.

On 2026-09-25, the Linux desktop demo on Vulkan/RTX 4090 restored its 1234×812 viewport after success, failure, and
cancellation. A subsequent rebuilt run recovered from a fully scrolled workspace. Twelve cycles switching between
route/indoor recordings and leaving/reopening Activities, followed by six logout/login cycles, left the map ready
without new warnings. Post-warmup process RSS samples stayed between 415,708 and 415,964 KiB, with 24 threads. This
short native check does not measure GPU allocations or establish browser worker/callback retention bounds. Local reports
and the restored-window captures are in `.tmp/renderer-automation-acceptance/desktop-rebuilt/` and `desktop-35833/`.

On 2026-09-26, Vivaldi/ANGLE worker-GL at 1645×1291, DPR 1, completed twelve route/indoor and Activities/settings
cycles, then six logout/login cycles. The final map was ready, capture included the map and charts, and diagnostics
contained no warnings or errors. Vivaldi Task Manager showed tab memory falling from 213,980 K to 212,940 K, with the
same process and two dedicated workers before and after. This short sample shows no tab-memory or worker-count growth;
it does not establish callback, cache, or GPU allocation bounds. Reports and captures are in
`.tmp/renderer-automation-acceptance/browser-lifecycle-20260926/worker-gl/`.

A longer worker-GL check at a fixed 1440×900, DPR 1, completed four batches of six activity/map-removal cycles and six
logout/login cycles. Post-batch RSS varied between 390,976 and 400,192 KiB; private memory between 241,212 and 250,500
KiB, with two dedicated worker threads throughout. Memory fluctuated rather than growing across successive batches. With
the map removed, settled RSS/private memory was 393,532/243,764 KiB; the renderer used 0.56 CPU seconds over 39.10
seconds. Map restoration and capture then passed. An initial narrow-layout fixture timeout was excluded; all measured
batches used the fixed wide viewport. This passes the worker retention smoke gate, not exact GPU/cache allocation
accounting. Raw samples and reports are in the adjacent `retention-worker/` directory.

Main-GL completed the same replacement and logout/login cycles without warnings or errors, with the map ready and
capture working afterward. Replacement used 1645×1291; logout/login used 1219×838 after a window resize. Reports and
captures are in the adjacent `main-gl/` directory. No main-GL memory baseline was collected; these runs establish
functional recovery, not a matched performance or memory comparison.

The subsequent main-GL retention check matched the 1440×900 canvas and four measured batches above. All 24
activity/map-removal cycles and 24 logout/login cycles passed. RSS fluctuated between 326,032 and 331,752 KiB; private
memory between 168,208 and 173,928 KiB, with one dedicated worker throughout. Settled map-removed RSS/private memory was
325,660/167,924 KiB, with 0.75 CPU seconds over 52.74 seconds. Restoration and capture passed without warnings or
errors. Both modes pass the retention smoke gate; the samples do not measure shared GPU-process allocations. Evidence is
in `retention-main/` beside the worker samples.

Closing the main-GL tab removed its diagnostic state; status and workload-start requests returned HTTP 404 with no app
connected. Reopening registered a new browser source with idle automation, then completed a fresh arrival workload. The
rejected start did not replay. With two tabs connected, workload start returned HTTP 409 and neither tab emitted new
automation events. Closing the extra tab restored control without replay; a fresh warm-interaction workload completed.
Worker termination after page closure was not independently observed.

Reloading during a main-GL drag sequence removed a still-running workload last observed at 9/64; the new connection
started idle and accepted a fresh arrival workload. API cancellation also stopped a sequence during its seventh drag,
with six completed, and a subsequent map-fit action passed. Manual map dragging and page scrolling then responded
normally. These checks do not cover keyboard recovery or an HTTP request awaiting a reply at disconnect.

In both WebGL2 modes, manual scrolling and navigation clicks were ignored during a drag sequence. Escape cancelled
during the 34th drag in main-GL and the 37th in worker-GL, after which the user confirmed scrolling and navigation
worked again. Both reports record `stopped with Escape`. Later sequences in both modes completed all 64 drags across
repeated focus loss/return with no visibility pauses. The user confirmed Tab/arrow input was blocked during each run and
worked after normal completion; cancellation was not attempted in these checks. Reports and correlated window events are
in `.tmp/renderer-automation-acceptance/browser-lifecycle-20260926/keyboard-focus/`.

At Vivaldi menu zoom 130%, activity smoke completed in both WebGL2 modes with semantic targets scaled at 1.3 pixels per
point. Both captures returned 1490×1077 pixels for a 1146.15×828.46 logical viewport. Worker-GL's map looked softer than
main-GL's. Composition scaled a rounded bitmap into a fractional rectangle in both CSS and screenshot `drawImage`. The
correction aligns the surface to physical pixels, preserves the fractional origin inside the renderer, and redraws when
that origin changes. Regression tests cover placement, scrolling, and capture without resampling. Native monitor scaling
remains unverified. Original evidence is in
`.tmp/renderer-automation-acceptance/browser-lifecycle-20260926/{worker-gl-zoom,main-gl-zoom}/`.

Both rebuilt WebGL2 modes passed activity smoke at 130% with the same 1146.15×619.23 logical viewport and 1490×805
captures. Inspected stationary and scrolled captures no longer show general worker-map softness. Captures after 0.5- and
73.25-point downward scrolls preserve map alignment and clipping; returning to the top passed. Small label and marker
raster differences remain, so this establishes comparable visual sharpness, not pixel identity. Neither mode logged
warnings or errors during the checks. Evidence is in the adjacent `pixel-alignment-rebuilt/` directory.

Calling the existing composition host's failure handler in worker-GL with `Acceptance probe: renderer failure` preserved
that reason in diagnostics and the map's semantic state. Both dedicated workers terminated. Profile settings remained
usable and its screenshot succeeded; map capture rejected the failed renderer. This tests failure containment, not an
actual GPU crash. After rebuild/reload, worker-GL reported ready with no error; map fit and an inspected 1440×900
capture passed, with no new warnings. Evidence is in
`.tmp/renderer-automation-acceptance/browser-lifecycle-20260926/failure-recovery/`.

That check exposed a stale automation assertion: navigation succeeded, but the immediately following assertion read the
previous layout's value. The driver now allows a fresh layout after input delivery before reading the next action's
targets. Regression coverage checks both rejection of the old value and acceptance of the new one. The rebuilt browser
passed immediate selection assertions and navigation assertions in both directions, without inserted wait actions.

The rebuilt demo's emitted WASM also passed `map-worker.test.mjs` with no skipped checks: hashed-module initialization,
empty/nonempty tile transfer, worker log relay, malformed-input recovery, and bundled tile fixtures in both themes.

The acceptance evidence directory also contains worker-GL/main-GL scenario reports, cancellation/restoration captures,
and child window action/capture reports. Browser duplicate-command rejection and stale child handles were exercised.
Native root capture worked; immediate child viewports correctly rejected capture as unsupported. These checks do not
establish non-unit scale, teardown during a command, or reconnect/deadline behavior for in-flight requests.

Remaining input/lifecycle and performance questions live in the
[browser verification plan](../plans/activity-map-workspace.md#browser-verification).

## Matched renderer performance

The first stationary-arrival pair on 2026-09-26 passed all 11 actions in both WebGL2 modes at 1450×905, DPR 1, on the
same ANGLE/RTX 4090 adapter. Both requested the same 20 tile identities through the worker, all HTTP 200 and reported
uncached by the browser; this does not describe backend proxy caching. Neither run paused or resized.

| Workload measurement           | Worker-GL |    Main-GL |
| ------------------------------ | --------: | ---------: |
| Main-thread frame interval p95 |  17.29 ms |   17.34 ms |
| Maximum frame interval         |  30.39 ms |   32.54 ms |
| Frame intervals above 33 ms    |         0 |          0 |
| Main-thread busy wall time     | 941.53 ms | 1169.83 ms |
| Recorded main-thread task CPU  | 897.84 ms | 1118.63 ms |
| Maximum tile-admission work    |   1.90 ms |    2.00 ms |

Each 10.40-second trace window contains 622 frame intervals and excludes reload/operator waiting. CPU sums omit missing
samples and boundary tasks; frame intervals are not presentation FPS. This first pair shows no observed regression, but
does not establish a completed comparative benchmark. Both enabled telemetry, but only main-GL recorded correlated
upload lifecycles, so upload latency cannot be compared. Reports, analyzer output, and trace hashes are under
`.tmp/renderer-automation-acceptance/performance-20260926/`; source traces are `*-gl-stationary-1.json.gz` in Downloads.

Across three recorded worker stationary runs, frame-interval p95 ranged from 17.29 to 17.39 ms, with no interval above
33 ms. The second main-GL run had p95 17.37 ms and one 34.41 ms interval. No further A/B repetitions are required.

The final `worker-gl-warm-1.json.gz` capture passed all 27 actions at the same viewport/DPR without pauses, resizing, or
new warnings. Its 12.60-second trace window contains 754 frame intervals: p95 17.27 ms, maximum 30.93 ms, none above 33
ms. All 40 worker tile requests returned HTTP 200; none originated on the main thread. Admission work peaked at 1.70 ms.
Main/render-worker/preparation-worker busy wall times were 1288.61/440.41/695.24 ms; the longest preparation task was
63.99 ms. Correlated upload lifecycles remain absent in worker-GL. This closes the agreed performance smoke check,
without establishing presentation FPS, per-frame UI CPU, or end-to-end upload latency.

## Deferred acceptance coverage

The closing gate prioritizes retention, failure recovery, default-renderer performance, and final QA. Further renderer
A/B profiling is outside the gate; existing recordings remain evidence. It does not require deterministic hiding during
tile fetch/CPU preparation, the original rural-view comparison, native monitor scaling, every chart/theme/unit
permutation, or every child-window/reconnect/deadline/diagnostic-stream timing combination. Existing automated boundary
coverage and recorded runtime checks remain the evidence for those paths; untested permutations are not claimed as
passed. No tile-proxy fault-control API is required for closure.

Separate telemetry on/off benchmarking is deferred. Future measurements can use `--no-map-upload-telemetry` with matched
tile sets, cache state, and publication counts. Current renderer comparisons keep instrumentation settings identical;
they do not establish absolute telemetry overhead or exact GPU/cache allocation bounds from tab-memory samples.

## Browser rendering constraints

WebGL2 requires raster-only device limits; requesting compute limits prevents worker initialization. On the tested
Chrome/ANGLE setup, WebGPU exposed its API but returned no adapter. Renderer selection must report that failure rather
than substitute a backend.

On 2026-09-25, Vivaldi/ANGLE captures showed darker road edges in worker-GL than main-GL. The worker used the default
sRGB surface while eframe preferred an unorm surface. Using egui's framebuffer selection for both removed the visible
brightness difference at 1091×1291, DPR 1, on the bundled London activity. This was a visual comparison, not pixel-exact
parity or validation of other adapters.

Chrome DPR emulation returned inconsistent physical sizes: a 100-CSS-pixel element reported 100 physical pixels through
`ResizeObserver.devicePixelContentBoxSize` while `devicePixelRatio` was 2. Use native-DPR testing for canvas alignment
rather than compensating in application sizing.

Worker errors must stay local to the map. Terminate the failed worker, preserve the original error, and place recovery
controls above both canvases. A WASM panic may leave an object borrowed, so calling `free()` during failure handling can
mask the original cause. Applied egui texture deltas must be drained before their collections are dropped.

## Renderer projection

### Tile background seams

On 2026-09-25, the headless WGPU regression `adjacent_tile_backgrounds_have_no_seams_at_fractional_zoom_and_scale`
reproduced dark boundaries between uniformly filled tiles: expected RGB `(36, 104, 36)`, observed `(29, 83, 29)` at a
shared edge. Feathered background rectangles caused the discontinuity. Exact background quads pass at zoom 2/2.35, scale
1/1.25/2, and sample counts 1/4. Other geometry retains antialiasing. This establishes the rendering defect;
confirmation against the reported rural browser view still requires a rebuilt runtime.

### Projection and clipping

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
