# Development build performance

Measured 2026-09-27 on Linux, stable Rust 1.98.1, two Cargo jobs, 6 GiB memory cap. UI-edit workload: change `CHART_GAP`
from 4 to 5 after warming the same target/features. Reports include instrumentation and exclude Nix shell startup,
packaging, application startup, and reload. Nested process spans overlap.

## Results

| Workload            |  Before | Optimized |
| ------------------- | ------: | --------: |
| Native UI edit      | 14.72 s |   2.877 s |
| Unchanged web build | 4.310 s |   1.460 s |
| Web UI edit         |  9.00 s |   6.770 s |

- **Native UI edit:** Incremental compilation, UI opt-level 2→0, and Mold. Measured costs: UI 0.925 s; host library
  0.554 s; executable including link 0.889 s.
- **Unchanged web build:** Bindgen cache enabled; Cargo and asset processing remain.
- **Web UI edit:** UI opt-level 2→0; Cargo 2.524 s, bindgen 2.941 s.

Unchanged-web numbers are medians of three alternating runs of the same patched Trunk; ranges were 4.28–4.31 s and
1.45–1.47 s. UI-edit numbers are individual captures. Bindgen is 0.2.126. Subsecond edits are not established.

Reproduction commands and cache contracts: [build system](../architecture/build-system.md#development-build-timing).

## HASS live serving

Measured 2026-10-13 on Linux with pinned Trunk 0.21.14, a warmed shared Cargo target, two Cargo jobs, and Trunk limited
to two CPUs and 6 GiB. The HASS demo backend, a temporary HTML/API bridge, and headless Chrome ran in separate capped
scopes. The browser used the app's own screenshot control API. A temporary copy of `garmin-ui/src` was bind-mounted over
the source directory inside Trunk's read-only sandbox; checked-out source did not change.

With Trunk's default watch set, changing the shared UI crate produced no rebuild or reload within 45 seconds. Watching
the web crate, shared crates, and vendor sources in `Trunk.toml` made directory-level edits trigger a build and browser
reload. Three alternating edits to the profile chooser's visible label took **16.79, 13.50, and 13.33 seconds** from the
temporary file write to receipt of an app-owned screenshot containing the new label. Median: **13.50 seconds**. These
values include watcher delay, compilation, bindgen, asset processing, browser reload, app reconnection, and screenshot
delivery. They exclude initial startup and the first cold build. Warm Cargo compile steps reported 3.07–3.93 seconds;
the rest of the time is not yet attributed to a single stage.

Trunk's raw HTML cannot start the app because HASS supplies canvas configuration values at serve time. The disposable
bridge fetches the current Trunk entrypoint on each request, supplies the demo values, and forwards `/remoc` and `/api/`
to the demo host. The [capture helpers](../architecture/developer-tools.md#automation-and-control) reproduce the visual
check. This establishes live serving and an observed latency, not a subsecond edit cycle or performance on hardware.

## Linker and compiler evidence

Two alternating executable-only rebuilds per linker, forced by touching `apps/garmin-desktop/src/main.rs`:

| Linker      | Whole build | Executable rustc, including linker |
| ----------- | ----------: | ---------------------------------: |
| LLD 22.1.8  | 1.77–2.00 s |                        1.01–1.40 s |
| Mold 2.42.1 | 1.47–1.61 s |                             0.95 s |

Mold is the Linux development default.

Compiler diagnostics used Rust 1.100.0-nightly (`5ceaf6608`, 2026-09-25), LLVM 23.1.1, measureme 12.0.3:

| Warm edit       | Compiler wall time | Profiling overhead |
| --------------- | -----------------: | -----------------: |
| UI library      |            1.136 s |             232 ms |
| Desktop library |            0.861 s |             204 ms |

Warm type checking was largely reused. Nightly instrumentation timings are not stable-build timings; changing profiler
flags also invalidates Cargo artifacts. The
[profiling wrapper](../architecture/build-system.md#development-build-timing) keeps flags stable and collects only fresh
profiles.

## Reload alternatives

Source review and Subsecond trial, 2026-09-28:

- Subsecond/Dioxus 0.7.10: stock replay included unrelated `garmin_hass_web`. A trial of
  [upstream #5837](https://github.com/DioxusLabs/dioxus/pull/5837) hit E0460 because the
  [ordering code](https://github.com/DioxusLabs/dioxus/blob/v0.7.10/packages/cli/src/build/link.rs#L563-L615) skipped
  the desktop library before rebuilding its executable. Launch succeeded; shared-UI patching and latency were
  unverified.
- [hot-lib-reloader](https://github.com/rksm/hot-lib-reloader-rs) 0.8.2: separate dylib, reload notifications, stable
  shared types required. Documented `tracing` conflicts affect our stack. Its egui example uses eframe 0.19; an
  [Windows crash report](https://github.com/rksm/hot-lib-reloader-rs/issues/31) concerns later versions.
- [Hotcode](https://github.com/alordash/hotcode), formerly `code_reload`: annotated functions/methods, `cdylib`, normal
  Cargo builds. Tests cover combined library/executable packages and separate libraries; CI covers Linux, macOS, and
  Windows. At commit `e69b3956` (2026-09-21), its
  [function guard](https://github.com/alordash/hotcode/blob/e69b3956e10a52a65917cd46248a3b3f75b93500/src/hotcode_core/src/fn_guard.rs)
  retains code during calls; the
  [library wrapper](https://github.com/alordash/hotcode/blob/e69b3956e10a52a65917cd46248a3b3f75b93500/src/hotcode_core/src/library_wrapper.rs)
  unloads on drop. Assessment: this does not cover egui state or WGPU callbacks retained after a call.
- [relib](https://github.com/xxshady/relib): explicit import/export interfaces and module cleanup, but macOS is
  unsupported/untested. Its ownership restrictions require more adaptation than our existing egui frame interface.

The [gallery loader](https://github.com/kubijo/rs-gallery/blob/v0.14.0/src/hot.rs) retains old mappings because egui
stores their vtables and drop code. Its scene-manifest API would need desktop integration; retained generations consume
memory and do not migrate changed state layouts.

These reviews did not establish transparent desktop reload. Further trials belong to the
[shared interface plan](../plans/shared-interface-workflows.md#deferred-gallery-hot-reload).

## Gallery reload measurement

Historical Linux/Wayland measurements used NVIDIA Vulkan, two Cargo jobs, and a 6 GiB cap; they do not qualify later
gallery versions.

With gallery 0.13.0 on 2026-09-28, ten alternating `CHART_GAP` edits after two warmups took median **5.28 s**
(**5.04–8.58 s**) from source write to replacement-library `openat`, measured with `strace -ttt`. Cargo reported
4.15–4.64 s. Startup was excluded and tracing overhead included; load completion and first presentation were unmeasured.
RSS grew from 237 to 474 MiB. A deliberate type error preserved the previous library and valid source resumed reload.
X11 startup failed during NVIDIA surface initialization; Wayland launched.

With gallery 0.13.1 on 2026-10-12, the map worker's egui lock timeout reproduced before any edit. Processing the
synthetic gallery surface on the UI thread preserved the headless capture pixel-for-pixel and passed windowed startup
and two swaps. The normal native map runtime stayed threaded.

Longer 0.13.1 checks edited an ignored watched gallery config and counted mapped libraries:

| Scene     | Completed swaps | RSS samples (MiB) | Outcome                         |
| --------- | --------------: | ----------------: | ------------------------------- |
| Workspace |              25 |         217–1,159 | Stalled; required KILL          |
| Workspace |              31 |           434–474 | Paused; resumed after `SIGCONT` |
| Select    |              30 |           208–446 | Closed normally on INT          |

The workspace pauses occurred despite newer successful builds, with its main thread waiting in `ep_poll`. The Select
control rules out a fixed generation limit. RSS samples have different baselines and include scene state retained per
library generation; they do not isolate loader overhead. A missed UI wakeup is a hypothesis, not an established cause.
Interaction, first-visible-frame timing, macOS, and repeatable long-run behavior remain unqualified.

Rejected experiments: disabling dependency debug information reduced WASM 143→80 MiB without improving bindgen time; one
no-demangle probe reduced bindgen 2.941→2.745 s, insufficient evidence to sacrifice symbols; moving desktop CLI parsing
into the library showed no clear gain.
