# Native hot reload

Target: warm UI edits visible within one second, preserving navigation and interaction state. Keep shared production UI
and avoid maintained patches to reload tooling. Subsecond was rejected; see the
[trial and replacement review](../research/build-performance.md#native-hot-reload-trial).

The measured native UI build takes 2.877 s, excluding startup. Gallery takes a median 5.28 s from save to replacement
library opening and its workspace map worker hits an egui lock timeout. Neither establishes subsecond save-to-visible
latency. [Measurement and limits](../research/build-performance.md#gallery-reload-measurement).

## Before another trial

Resolve the gallery workspace lock timeout before extending its runtime measurements. Its library-local scene state does
not provide desktop state preservation. Keep desktop rebuild/restart as the working baseline.

Before trying another desktop loader, identify the exact adapter and its lifetime contract. Gallery exposes scenes;
Hotcode and hot-lib-reloader do not automatically retain code referenced by egui state after a frame returns. Do not
extract application code until a maintained API covers that ownership requirement.

## Acceptance criteria

- A real `garmin-ui` function edit appears without restarting or requiring pointer input.
- Profile/activity selection, camera, lap/pin selection, and scroll survive; workers do not restart or duplicate.
- egui stored state, WGPU callbacks, tracing callsites, and child windows remain valid across reloads and shutdown.
- Incompatible type/interface edits require restart before replacement; compilation success is insufficient.
- Compilation failure leaves the current UI usable; a subsequent valid edit recovers.
- Thirty reloads have measured memory, thread, and renderer-resource costs. Linux and Apple Silicon are verified
  separately.
- Release behavior and the workspace unsafe-code policy remain unchanged. Use existing resource caps and root temporary
  outputs under the [development safeguards](../development.md).

If no maintained integration meets these criteria, keep desktop restart and gallery reload for scenes verified to work.
Do not introduce a custom reload engine to satisfy this plan.
