# Browser map evidence

Synthetic demo-route evidence, 2026-09-17–26. Raw traces are private local inputs. Reproduce summaries with
`just hass::profile-analyze TRACE --json`; [activity map](../architecture/activity-map.md) owns renderer contracts,
[developer tools](../architecture/developer-tools.md) owns automation.

## Semantic interaction findings

- **Chrome 153, Apple M3 Max, DPR 2, 1200×999:** Worker/main WebGL2 passed arrival, pan/zoom/fit, playback, speed, laps,
  charts, indoor replacement and restoration. Main-GL also passed Escape during drag and a subsequent run.
- **Vivaldi/ANGLE, Linux RTX 4090:** Both WebGL2 modes emit no input on hidden starts; hide/resume interrupts and
  retries drags; captures reject hidden canvases and recover. Pointer/keyboard input is blocked during runs and restored
  afterward; focus changes preserve runs; Escape cancels.
- **Vivaldi menu zoom 130%, 1146.15×619.23 logical / 1490×805 capture:** Both modes pass activity smoke. Rebuilt
  physical-pixel alignment preserves sharpness/clipping at 0.5- and 73.25-point scroll offsets. Minor label/marker
  raster differences remain.
- **Linux desktop Vulkan/RTX 4090:** 1234×812 viewport restored after success/failure/cancellation; route/indoor
  replacement, navigation and logout/login recover.
- **HASS control, main-GL:** Closed tab: 404; two root tabs: 409; no rejected command replay. Reload during drag
  reconnects idle. API cancellation permits fresh actions and normal input.
- **Worker-GL injected renderer failure:** Original error retained; both workers terminate; settings and its capture
  remain usable; map capture fails; reload restores map. This was not a GPU crash.

AccessKit pixel-scale removal fixed HiDPI target misses. A fresh layout between delivered input and assertions fixed
stale navigation assertions; rebuilt browser checks passed without inserted waits. Emitted-WASM tests passed module
initialization, tile transfer, logs, malformed-input recovery, and both-theme fixtures.

Retention at 1440×900, DPR 1: four batches totaling 24 activity/map-removal and 24 logout/login cycles per renderer:

| Renderer  | RSS range (KiB) | Private memory (KiB) | Dedicated workers | Removed-map CPU / observation |
| --------- | --------------: | -------------------: | ----------------: | ----------------------------: |
| Worker-GL | 390,976–400,192 |      241,212–250,500 |                 2 |                0.56 / 39.10 s |
| Main-GL   | 326,032–331,752 |      168,208–173,928 |                 1 |                0.75 / 52.74 s |

Memory fluctuated without successive growth; restoration/capture passed without warnings. These are retention smoke
checks, not GPU/cache accounting. Native's shorter check held RSS at 415,708–415,964 KiB and 24 threads.

## Matched renderer performance

2026-09-26, ANGLE/RTX 4090, 1450×905, DPR 1; telemetry enabled, no pause/resize. First stationary pair: 20 identical
worker-requested tile IDs, HTTP 200, browser-reported uncached; backend proxy cache state is unknown. Each workload
window is 10.40 s / 622 main-thread frame intervals.

| Measurement                   |        Worker-GL |          Main-GL |
| ----------------------------- | ---------------: | ---------------: |
| Frame interval p95 / max      | 17.29 / 30.39 ms | 17.34 / 32.54 ms |
| Intervals >33 ms              |                0 |                0 |
| Main-thread busy wall time    |        941.53 ms |       1169.83 ms |
| Recorded main-thread task CPU |        897.84 ms |       1118.63 ms |
| Maximum admission work        |          1.90 ms |          2.00 ms |

Three worker stationary runs had p95 17.29–17.39 ms and no intervals >33 ms. A second main run had p95 17.37 ms and one
34.41 ms interval. Final warm worker run: 27 actions, 12.60 s / 754 intervals, p95/max 17.27/30.93 ms, no intervals >33
ms, 40 successful worker tile requests and none on the main thread. Admission peaked at 1.70 ms;
main/render/preparation-worker busy times were 1288.61/440.41/695.24 ms; longest preparation task 63.99 ms.

This closes the agreed default-renderer performance smoke check. Frame intervals are not presentation FPS or per-frame
UI CPU; CPU sums omit missing samples and boundary tasks. Worker-GL lacked correlated upload lifecycles, preventing
upload-latency comparison.

## Deferred acceptance coverage

Unmeasured: presentation/input latency, telemetry overhead, exact GPU/cache retention, native monitor scaling,
deterministic hiding during tile preparation, rebuilt rural-view seams, and exhaustive chart/theme/window/reconnect
permutations. Further renderer A/B profiling was dropped from the closing gate. Hidden or resized runs establish
functional recovery only. Child capture/control checks do not establish teardown or deadline behavior in flight.

## Browser rendering constraints

- WebGL2 needs raster-only device limits. Tested Chrome exposed WebGPU but returned no adapter.
- Matching egui's preferred unorm framebuffer removed the worker's darker road edges; this is visual, not pixel parity.
- Chrome DPR emulation reported inconsistent physical sizes; use native DPR for alignment checks.
- Worker panic cleanup must preserve the original error: WASM `free()` can encounter a retained borrow. Drain applied
  texture deltas before dropping their collections.

## Renderer projection

The headless `adjacent_tile_backgrounds_have_no_seams_at_fractional_zoom_and_scale` regression reproduced edge RGB
(29,83,29) instead of (36,104,36). Exact background quads pass zoom 2/2.35, scale 1/1.25/2, MSAA 1/4. Other geometry
retains antialiasing. Projection and immutable draw-binding contracts live in
[activity map](../architecture/activity-map.md#rendering-boundary).

### Post-review browser capture

September 19, asset `815b37b00ce1ca6d`: 399 successful worker tile requests, no main-thread requests or fallback; 347
interaction intervals, p95/max 18.47/23.49 ms. Settled map inspection and user confirmation passed smoke acceptance.
Scroll/resize inspection did not establish a changed rendered state.

## Upload latency evidence

[Upload analyzer](../../infra/python/browser_upload_analysis.py) distinguishes visible queue lifetime, measured CPU,
offscreen retention, publication-to-first-draw command encoding, and incomplete/released/invalid cohorts. Visibility
means viewport demand; first draw is not presentation. Invalid cohorts never enter completed distributions.

- **September 19, initial run:** 145 publications; queue lifetime p95/p99/max 34/1467.5/2669 ms. No per-tile visibility
  correlation; the 2669 ms tail remains unexplained.
- **Stationary and pan-return, `7e0c922b68ffbc68`:** 6/18 complete lifecycles; visible queue maxima 18.3/19.6 ms, upload
  CPU maxima 2.7/3.6 ms, draw delay maxima 17.7/17.7 ms. No hidden transitions or invalid/incomplete uploads. Slowest
  request 1.814 s, upstream of admission.
- **September 19, afternoon, `3cc8fd55eb581c95`:** A 1819.5 ms publication tail included 1784.2 ms hidden time. Visible
  queue max 100.5 ms; draw delay max 2120.2 ms, with post-publication visibility unknown.
- **September 19, evening, `815b37b00ce1ca6d`:** A 1079.7 ms tail included 1051.2 ms hidden time; visible queue max 95.9
  ms; draw delay max 20.4 ms.

Hidden retention explains the identified September 19 tails, not the earlier uncorrelated 2669 ms observation. The
production-queue regression retains an upload through 2.6 s hidden time and publishes once after resumption.

Telemetry-off and telemetry-on runs used the same `b16287233d847358` asset but different viewports (1761×1324 /
2367×1324) and tile counts (20/28); they cannot measure telemetry overhead.

Earlier failing baselines: September 17 completion microtasks reached 168.69 ms before bounded admission; another run
used main-thread fallback after unhashed WASM returned 404; low-zoom decoding also failed. Later recordings no longer
reproduced these failures.
