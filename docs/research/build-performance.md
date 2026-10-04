# Development build performance

Measured 2026-09-27 on Linux, stable Rust 1.98.1, two Cargo jobs, 6 GiB memory cap. UI-edit workload: change `CHART_GAP`
from 4 to 5 after warming the same target/features. Reports include instrumentation and exclude Nix shell startup,
packaging, application startup, and reload. Nested process spans overlap.

## Results

| Workload            |  Before | Current |
| ------------------- | ------: | ------: |
| Native UI edit      | 14.72 s | 2.877 s |
| Unchanged web build | 4.310 s | 1.460 s |
| Web UI edit         |  9.00 s | 6.770 s |

- **Native UI edit:** Enabled incremental compilation, changed UI opt-level 2→0, and used Mold. Remaining costs: UI
  0.925 s; host library 0.554 s; executable including link 0.889 s.
- **Unchanged web build:** Enabled the bindgen cache. Remaining costs: Cargo and asset processing.
- **Web UI edit:** Changed UI opt-level 2→0. Remaining costs: Cargo 2.524 s; bindgen 2.941 s.

Unchanged-web numbers are medians of three alternating runs of the same patched Trunk; ranges were 4.28–4.31 s and
1.45–1.47 s. UI-edit numbers are individual captures. Bindgen is 0.2.126. Subsecond edits are not established.

Reproduction commands and cache contracts: [build system](../architecture/build-system.md#development-build-timing).

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

UI emitted 2/256 codegen units: one type-check invocation and 4,285 cache hits; 19.24 ms type-check self time, mostly
incremental loading. Desktop spent 65.50 ms decoding generic symbols, 64.11 ms on trait queries, and 12.49 ms type
checking.

Warm type checking is largely reused. Nightly instrumentation timings are not stable-build timings. Earlier runs were
excluded because changing the profiler output path changed Cargo artifact identities. The profiler now keeps flags
stable and collects only fresh profiles.

## Native hot-reload trial

Subsecond/Dioxus 0.7.10 was rejected on 2026-09-28; its integration and CLI patches were removed. Stock replay included
unrelated `garmin_hass_web`. Applying [upstream #5837](https://github.com/DioxusLabs/dioxus/pull/5837) exposed Rust
E0460: replay rebuilt `garmin_ui` but skipped the desktop library before its executable. The
[ordering code](https://github.com/DioxusLabs/dioxus/blob/v0.7.10/packages/cli/src/build/link.rs#L563-L615) excludes the
tip package. Launch worked; shared-UI patching and save-to-visible latency were not established.

## Reload alternatives

Source review on 2026-09-28; no new candidate was integrated or benchmarked.

- [hot-lib-reloader](https://github.com/rksm/hot-lib-reloader-rs) 0.8.2: separate dylib, reload notifications, stable
  shared types required. Documented `tracing` conflicts affect our stack. Its egui example uses eframe 0.19; an
  [open Windows crash report](https://github.com/rksm/hot-lib-reloader-rs/issues/31) concerns later versions. Last
  repository push: 2025-08-11.
- [Hotcode](https://github.com/alordash/hotcode), formerly `code_reload`: annotated functions/methods, `cdylib`, normal
  Cargo builds. Tests cover combined library/executable packages and separate libraries; CI covers Linux, macOS, and
  Windows. At commit `e69b3956` (2026-09-21), its
  [function guard](https://github.com/alordash/hotcode/blob/e69b3956e10a52a65917cd46248a3b3f75b93500/src/hotcode_core/src/fn_guard.rs)
  retains code during calls; the
  [library wrapper](https://github.com/alordash/hotcode/blob/e69b3956e10a52a65917cd46248a3b3f75b93500/src/hotcode_core/src/library_wrapper.rs)
  unloads on drop. Assessment: this does not cover egui state or WGPU callbacks retained after a call.
- [relib](https://github.com/xxshady/relib): explicit import/export interfaces and module cleanup, but macOS is
  unsupported/untested. Its ownership restrictions require more adaptation than our existing egui frame interface.
- [Dexterous](https://github.com/lee-orr/dexterous_developer) is archived.
  [dynamic_reload](https://github.com/emoon/dynamic_reload) and [Reloady](https://github.com/anirudhb/reloady) last
  received repository pushes in 2023 and 2021; neither establishes compatibility with our current stack.
- Our pinned [gallery loader](https://github.com/kubijo/rs-gallery/blob/v0.13.1/src/hot.rs) retains old mappings because
  egui stores their vtables and drop code. Its API loads scene manifests, not desktop frame functions. Existing gallery
  reload supports shared-UI iteration; desktop reuse would need new integration. Retained generations consume memory and
  do not migrate changed state layouts.

No reviewed replacement establishes transparent reload of the desktop. Another desktop trial must first address callback
lifetime and `tracing` without dependency patches; see the [acceptance criteria](../plans/native-hot-reload.md).

## Gallery reload measurement

Measured 2026-09-28 with gallery 0.13.0, Linux/Wayland, NVIDIA Vulkan, two Cargo jobs and a 6 GiB cap. Run
`just gallery::run --hot --scene '^garmin_gallery::activity::Workspace$'`; warm twice, then alternate ten `CHART_GAP`
edits between 4 and 5. Timestamp source modification and the replacement-library `openat` with `strace -ttt` on the
gallery host. Startup is excluded; tracing overhead is included.

- Save to replacement-library opening: median **5.28 s**, range **5.04–8.58 s**. This excludes load completion and first
  presentation; save-to-visible was not measured. Cargo reported **4.15–4.64 s** for these builds.
- The gallery's displayed reload timer starts after compilation, excluding the build and 500 ms save debounce.
- RSS rose from **237 to 474 MiB** across ten reloads; retained library generations rose from 3 to 13. Threads returned
  to 14. This is process growth, not an isolated measurement of loader overhead.
- The workspace map worker repeatedly panicked on an egui RwLock timeout, including before the first edit. Interactive
  correctness is therefore unverified. X11 startup also failed in NVIDIA surface initialization; Wayland launched.
- A deliberate type error preserved the process and previous library. Restoring valid source rebuilt and loaded again.
- Scene state is owned by library-local `WORKSPACES` in `activity.scene.rs`, initialized through `SceneState` in
  `infra/gallery/lib.rs`. It has no migration into a replacement generation; desktop state preservation is not provided.

The existing gallery path misses the latency target and has a workspace correctness failure. It is not a validated
desktop reload solution. First-frame timing, interactive state checks, 30-generation resource testing, and macOS remain
unverified.

## Remaining work

- Resolve the gallery workspace lock timeout before using it for reload acceptance tests.
- Profile bindgen after real WASM edits. Samply was blocked by host `perf_event_paranoid=4`; no CPU capture exists.
- Measure HASS live serving and save-to-visible latency.
- Recheck nightly `recursion_depth_exceeding_limit` for `egui_adapter::Paint: Sync` through WGPU before upgrading Rust.

Rejected experiments: disabling dependency debug information reduced WASM 143→80 MiB without improving bindgen time; one
no-demangle probe reduced bindgen 2.941→2.745 s, insufficient evidence to sacrifice symbols; moving desktop CLI parsing
into the library showed no clear gain.
