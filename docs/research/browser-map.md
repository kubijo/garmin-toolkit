# Browser map evidence

Recorded 2026-09-19. Scope: the bounded map committed as `10234ef` and the subsequent working-tree upload telemetry.
This is an evidence record, not a completion checklist. Open work lives in the
[activity map plan](../plans/activity-map-workspace.md). Current renderer ownership and limits live in
[activity map architecture](../architecture/activity-map.md).

Raw traces remain private local inputs, not committed fixtures. Measurements concern the bundled synthetic demo route.
Use the maintained `infra/python/browser_trace_analysis.py` via `just hass::profile-analyze` to reproduce summaries.

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

Closing verification: the missing `UPLOAD_TELEMETRY_PLACEHOLDER` test import was fixed, and assistant-run preflight
passed with no formatting changes. The user then reported `just qa::full` green for the comparison switch. This is
user-reported full-gate acceptance, not an independent rerun or an overhead measurement.

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

Focused verification on 2026-09-19 added a real-WGPU regression through the production browser upload queue. A tile
larger than the per-frame byte budget is partially written, hidden for ten upload frames with a test-only 2.6-second
clock advance, and returned with duplicate visible copies. Hidden frames write no bytes and emit no progress; the
remaining byte count and upload identity survive unchanged. Resumption publishes once, and total written bytes equal the
original mesh size (no restart or duplicate upload). The test requires an adapter and passed without skipping on this
host. The adversarial review identified missing headless adapter provisioning; the follow-up below records the fix. All
202 UI tests (including 42 renderer tests) and 28 trace-analysis tests passed, including the existing analyzer test that
separates multiple hidden intervals from visible waiting. Strict UI Clippy, Rust formatting, and diff whitespace checks
passed. No sleeps, production scheduling changes, or budget changes were introduced. The hidden-duration offset is
simulated, but upload advancement still uses the production wall clock; this is not a wholly virtual-clock scheduler
test. Code inspection confirms progress marks are batched per pending upload per frame and browser retention is bounded
to one mark; this is an allocation/history bound, not a measurement of CPU overhead.

Follow-up verification on 2026-09-19 passed all 42 renderer tests with the pinned Mesa software Vulkan ICD and no
display. Linux Nix test/coverage environments now provision this driver. Analyzer payload/timestamp hardening passed 55
Python tooling tests, including report-level rejection of damaged cohorts; valid zero-duration work and relative
timestamps remain supported. Both supplied recordings still have 6/18 valid completed uploads with no malformed marks.
The sandboxed license check passed after restoring vendored license files to its source closure; bundles were unchanged.
The full sandboxed Rust test build was stopped during its cold dependency build after dependency checking, so it is not
recorded as passed. Analyzer Ruff/type checks, three license-generator tests, and repository validation also passed.

Subsequently, on 2026-09-19, the user reported the requested full QA, audit, and sandboxed Rust verification gates green
and authorized committing this slice. No command output from that run was supplied or independently rerun. This updates
the earlier verification limit by user report; it does not measure telemetry overhead or establish a new CI run.

On 2026-09-19 the user confirmed a fresh demo data directory and disappearance of the duplicate tooltip, and approved
the bounded-map commit `10234ef` with the upload-latency investigation retained as follow-up work. This does not imply
that the subsequent telemetry diff has been committed or accepted.

Browser acceptance on 2026-09-18 used the current Nix demo package
`x5gc9r077g4kjsjj8aybi1sc7635bcj5-garmin-hass-demo-0.1.0`, with emitted asset hash `a149a11cfeeb1a33`. All eight
initializer/worker tests passed against that package with no skips. At 1440 by 1000, Chrome rendered complete maps after
cache-bypassed and warm reloads, repeated rapid zoom cycles, and world-wrap panning. Playback, speed-stop clicks, slider
dragging, and sustained hover worked without a panic. Morocco labels included Arabic and Tifinagh glyphs.
Screenshot-guided synthetic DOM input exercised these checks; it was not semantic automation or a trusted-input latency
measurement. The original 3389 by 1324 viewport was restored afterward.

Chrome used WebGL2 through ANGLE on the RTX 4090. The only captured warning was WebGPU adapter discovery reporting
`No available adapters.` No tile-decoder errors or missing-background warnings appeared during the checks. Sustained
speed-stop hover initially exposed overlapping tooltips; the user subsequently confirmed the fix.

The user supplied `Trace-20260919T000106.json.gz`, analyzed with the maintained tool on 2026-09-19; see performance
evidence below. Its mixed browser-cache results do not independently establish the server-cache state; the
fresh-directory confirmation comes from the user.

## Telemetry review resolution

The 2026-09-19 staged and unstaged telemetry review against `10234ef` found three implementation issues, now addressed:

- Linux Nix test applications, sandboxed Rust tests, and coverage select the pinned Mesa software Vulkan ICD. The
  retention regression remains mandatory; the 42 headless renderer tests passed. The assistant's full sandboxed run was
  incomplete; the user's subsequent green-gate report is recorded above.
- Malformed identifiable events invalidate that upload ID's cohort; unidentifiable gaps invalidate every upload cohort
  in the recording. If IDs repeat after navigation, uncertain malformed events invalidate every occurrence rather than
  guessing an epoch. Publication requires nonzero uploaded bytes; zero-duration CPU work remains valid. Invalid cohorts
  cannot enter completed distributions.
- Finite numeric, non-boolean timestamps are validated before sorting. Invalid timestamps are excluded from frame and
  network analysis but retained for upload invalidation and diagnostics. Signed finite timestamps remain valid for
  relative traces. Regression tests cover parsing and report output.

[Actions job 105767514106](https://github.com/kubijo/garmin-toolkit/actions/runs/35396838899/job/105767514106) failed
because the sandboxed license check's Cargo-only source filter omitted `vendor/fast-mvt/LICENSE-MIT` and
`LICENSE-APACHE`. Including the vendored source fixed the local/sandbox mismatch without regenerating or weakening the
committed bundle. The exact sandboxed license check passed; package-level mismatch diagnostics and a notice-comparison
regression were added to the maintained generator. This does not establish success of the entire CI workflow.

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
