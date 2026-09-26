# Activity map workspace

Close worker rendering and automation together before the Activities layout redesign. Current recording, interaction,
and rendering contracts live in [activity map architecture](../architecture/activity-map.md).

Acceptance covers native, worker-GL, and main-GL. Worker-WebGPU remains experimental. Completed checks and measurement
limits live in [renderer and automation evidence](../research/browser-map.md#semantic-interaction-findings).

## Browser verification

The agreed runtime checks are complete. The dependency audit remains a deferred follow-up, described below.

Retention passed in both WebGL2 modes; worker failure containment and reload recovery passed. The rebuilt browser also
passed immediate post-click navigation assertions. Core interaction, screenshot/zoom, input isolation, focus, and
hide/resume evidence is already recorded. Stationary and final warm-interaction traces close the default-renderer
performance smoke check. No further recordings or renderer A/B profiling are planned. Narrow timing and platform
permutations are explicitly [deferred coverage](../research/browser-map.md#deferred-acceptance-coverage), not completed
checks. Presentation FPS, browser input latency, and a completed comparative benchmark are not established.

The [shared interface plan](shared-interface-workflows.md) owns device operations and platform decoration checks. Use
[responsive-layout.json](../../infra/automation/responsive-layout.json) when testing with files open; built-in scenarios
log out during reset.

## Deferred dependency audit

[RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) reports discontinued maintenance of
`bincode 1.3.3`, not a specific known vulnerability; there is no patched version. It enters through Walkers 0.59 →
http-cache-reqwest 0.16 → http-cache 0.21. Ordinary `cargo update` cannot change our exact Walkers pin; the Nix and
Cargo lockfile refreshes do not resolve this finding.

The published Walkers 0.60.0 manifest selects http-cache-reqwest 1.0.0-alpha.9 and requires Reqwest 0.13.5; our Reqwest
pin is 0.13.4. Its cache backend uses Postcard, with Bincode confined to optional legacy features and upstream tests.
This is a candidate upgrade, not a validated fix. The HTTP-cache stack is still prerelease, and Walkers also changes its
MVT dependencies. See the [Walkers manifest](https://raw.githubusercontent.com/podusowski/walkers/main/Cargo.toml) and
[cache release](https://docs.rs/crate/http-cache-reqwest/1.0.0-alpha.9).

Before adopting it, update both workspace and gallery lockfiles, verify Bincode is absent, rerun the audit, and check
native/WASM builds and focused map tests. Keep the advisory visible until then; no vendored patch or suppression is
planned.

## Performance acceptance

- Target 60 FPS, UI CPU p95 below 16.7 ms, and no tile/label publication stall above 33 ms on the bundled 67.92 km
  activity at roughly 1100 by 720 pixels. Require at least 120 interaction intervals and two seconds of camera motion.
  Input-window frame intervals are not presentation FPS or measured camera-motion windows.
- Profile native runs with `just desktop::profile demo NAME --gfx`; analyze with `just desktop::profile-analyze NAME`
  and compare with `just desktop::profile-compare BEFORE AFTER`.
- Keep workload, viewport, backend, cache conditions, and instrumentation comparable. See the
  [recorded baselines and limitations](../research/browser-map.md) rather than treating historical captures as open
  verification tasks.
- Require admission/allocation markers, no fallback, and no main-thread tile requests before attributing work to a
  worker. Keep the map/tab visible and record before enqueue. Submission timing does not measure presentation latency.
- Never launch a GUI from unattended validation. Interactive profiling is user-run; automated checks remain headless.
- Run emitted-WASM worker checks using
  `GARMIN_TEST_WEB_ROOT=/path/to/built/share/garmin-hass/web node infra/javascript/map-worker.test.mjs`.
- Repeat emitted-WASM and visual checks after runtime changes; native tests do not establish browser parity.

## Boundaries and deletion gate

Use one owned OpenFreeMap style family under [ADR 0025](../decisions/0025-proxied-online-vector-maps.md). Routing,
geocoding, heatmaps, free chart zoom, offline-region downloads, and graphical Garmin device-map management are separate
work. Delete this plan after the shared viewer passes desktop and HASS runtime checks, maintained interface evidence is
reviewed, and its architecture and evidence are updated.
