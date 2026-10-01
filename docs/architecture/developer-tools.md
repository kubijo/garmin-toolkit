# Developer tools and application logs

Tools opens/focuses a [shared child window](application-windows.md). Logs/debug work without automation. Closing tools
leaves runs active; Stop/Escape cancels. Reopen tools after parent reload.

## Automation and control

Demo accepts `--ui-automation`; `--control-server` also enables localhost HTTP. Production rejects both. Desktop prints
its random loopback port; HASS uses its listener. Without control enabled, desktop has no listener and HASS routes are
404\. Access requires loopback peer/Host; forwarded and browser-origin commands are rejected. Same-origin diagnostic
reads are allowed. No token; ingress cannot access control.

```sh
just hass::run demo "$PWD/.tmp/hass-ui-automation" --control-server
just hass::control command status
just hass::control check
```

The checker writes `.tmp/hass-control-runtime`; `--url URL` selects another running listener.

| Endpoint                | Result                                  |
| ----------------------- | --------------------------------------- |
| `GET /api/capabilities` | Supported operations                    |
| `GET /api/debug`        | Desktop debug information               |
| `POST /api/control`     | JSON `value`/`error`, or PNG screenshot |

Command shape: `{"operation":"start","argument":"activity-smoke"}`. Start/action/sequence acknowledge admission with
`value: null`; poll status/result. HASS requires exactly one root tab: none →404, multiple →409; children do not count.
Disconnect revokes that connection; commands never replay or migrate.

Limits: 16 connections, one pending command per connection, 16 KiB requests, 1 MiB JSON replies. Request IDs increase;
explicit IDs must exceed the last admitted ID. Busy →429; reused ID →409. Five-second deadlines report unknown outcomes.

### Child windows

`windows` returns `id`, `kind`, `title`, `ready`, `focused`, `screenshots`. Command `window` defaults to root. Handles
expire on close, logout, popup reload; stale handles fail. `window.focus`/`window.close` require a child handle and no
argument. Focus is subject to platform policy. Each child has independent targets/actions/status/cancellation; named
scenarios run in root only.

```sh
just hass::control command targets --window HANDLE
just hass::control command action --window HANDLE --argument '{"kind":"click","target":"files.refresh"}'
just hass::control screenshot --window HANDLE --output .tmp/files.png
```

Section targets: `developer.section.{automation,control,logs,debug,log-time}`, plus
`developer.section.record.<sequence>`; values are `open`/`closed`. Stop IDs: `automation.stop`,
`developer.automation.stop`.

### Screenshots

No argument. PNG response metadata `x-garmin-capture`: dimensions, pixels-per-point, requested/received UI frames; HASS
adds `x-garmin-request-id`. Limits: 8 Mi pixels, 8 MiB PNG, one pending capture, command deadline. Captures cover one
canvas, maps and overlays; exclude browser chrome/system dialogs. Hidden tabs, pending map views,
resize/navigation/closure invalidate capture. HASS supports children; native immediate children advertise unsupported;
native root capture works.

### Semantic actions

`window.garminAutomation` and HTTP use the same Rust/AccessKit driver:

| Operation                                     | Contract                                                        |
| --------------------------------------------- | --------------------------------------------------------------- |
| `list`, `start`, `status`, `result`, `cancel` | Discover/run/inspect/stop scenarios                             |
| `targets`                                     | IDs, labels, roles, enabled/value state, clipped logical bounds |
| `action`                                      | `kind`, `target`: click, drag, scroll, key, text, resize        |
| `sequence`                                    | 1–64 actions; also wait, assert_available, assert_value         |

Drag uses normalized coordinates; scroll logical points; key egui names; text a `text` string; assertions a `value`
string. Resize uses logical `width`/`height` (1–8192), subject to native minima; browser resizes canvas only. Targets
resolve at execution; pointer bounds must stabilize across two frames within two seconds. Key/text establishes focus.
Missing/ambiguous/disabled/clipped targets fail. Each window allows one workload.

Profile creation exposes `profile.create` and `profile.create.{name,submit,cancel}`. File choosers expose
`files.chooser.{name,confirm,cancel,replace,keep}`; directory and file rows use `files.entry.<catalog-relative-path>`.
Select a directory row and send Enter to navigate; selecting a file fills the chooser's filename field. These targets
remain stable across language changes.

Named scenarios log out if needed and restore initial viewport on every exit; explicit sequences retain final size.
Resize settles for 50 ms within five seconds; status exposes `resize_request`. Focus loss continues; hidden tabs pause
active-time deadlines and release input. Geometry changes release/retry gestures after layout settles; prior movement is
retained. Real pointer/keyboard/wheel/touch input is blocked during runs and restored afterward.

For functional testing without keeping the browser in front, opt in when launching each automation run:

```sh
just hass::control command start --argument '"activity-smoke"' --run-in-background
```

The flag applies to `start`, `action`, and `sequence`. Browser calls accept a second options argument, for example
`window.garminAutomation.start('activity-smoke', {run_in_background: true})`. HTTP callers wrap the usual command
argument as `{"argument": ..., "run_in_background": true}`. The option applies only to that run; subsequent runs default
to visibility pausing. Opted-in runs use a timer fallback when animation callbacks stop, including while Chrome still
reports the page as visible. The fallback ends after input release and viewport restoration complete
(`needs_background_frames` in control reports). Hidden browser windows continue UI passes without being focused. Browser
timer throttling still applies, so the two-second frame/action timing checks are disabled in this mode; readiness and
overall run timeouts remain enforced. These runs have `performance_eligible: false` and must not be used for performance
comparisons.

Native desktop automation still requires a visible window: on Wayland, fully covered windows can stop processing control
commands before a run is admitted. The browser fallback does not bypass the native redraw gate.

The runner removes AccessKit's root pixel-scale transform before ordinary egui input injection. Assertions read fresh
layout after delivery. Deadlines, renderer errors, and readiness failures fail runs. Reports retain target bounds,
pointer position, viewport/DPR, renderer, readiness, pauses and input/tree CPU. The target/crosshair overlay has no
input region. CPU excludes AccessKit generation/status/overlay. Browser phase marks poll at 100 ms; Rust timestamps are
authoritative.

### Scenario runs and profiling

| Scenario             | Workload                                                                       |
| -------------------- | ------------------------------------------------------------------------------ |
| `stationary-arrival` | Readiness, eight seconds stationary                                            |
| `warm-interaction`   | Readiness, four pan/zoom/fit cycles, returned-view readiness                   |
| `activity-smoke`     | Warm workload, playback/speed, laps, scrubbing, indoor replacement/restoration |
| `responsive-layout`  | Activity/archive/playback at narrow/wide sizes                                 |

[responsive-layout.json](../../infra/automation/responsive-layout.json) tests resizing without logout after selecting an
activity. Chrome: record before enqueue, poll completion, save trace/result together; reload to the same starting
screen. Analyze with `just hass::profile-analyze TRACE --json`. Injected gestures have no DOM input events. Keep
viewport, cache, telemetry and workload fixed. Paused/resized runs establish functional recovery only; frame/task
timings and readiness do not establish presentation FPS or end-to-end input latency.

```sh
just desktop::profile build demo
just desktop::profile record demo NAME -- --gfx
just desktop::profile analyze NAME
just desktop::profile compare BEFORE AFTER
just desktop::profile load NAME
```

Reports: `.tmp/profiles/NAME`; `finalize NAME` rebuilds derived output. Recording is explicitly interactive. Targets on
the 67.92 km demo at ~1100×720: 60 FPS, UI CPU p95 \<16.7 ms, publication stalls \<33 ms, ≥120 interaction intervals and
≥2 seconds camera motion. [Measured coverage](../research/browser-map.md).

## Read-only diagnostics

Control access rules apply; 32 additional streams. `/api/logs-{get,stream}` serves application logs;
`/api/events-{get,stream}` serves state/history for connections, windows, automation, renderers. Reads work without
tabs. History: 2,048 records/8 MiB; progress may coalesce. Window closure removes current state. Subscription failure
clears stale telemetry; source overflow reports incomplete state.

Filters: both use `source,since_ms,until_ms,after,limit`; logs add `minimum,component,text`; events add exact
`kind,window`. Limits default 100/max 256; inclusive Unix-ms bounds affect history. Invalid/unknown/duplicate filters
→400. Without `after`, return newest matches chronologically. Replies contain `cursor,more,gap`; events add
`state,snapshot_revision`. Cursors cross nonmatches; restart/retention/source loss signals gaps. SSE prefers
Last-Event-ID.

Browsers receive HTML, curl text. `format=text|ansi|html|json` (get) or `sse` (stream) overrides negotiation.
Rust/Askama renders; Axum owns SSE/keepalives. HASS closes diagnostic streams before HTTP shutdown drains.

```sh
just hass::diagnostics logs --minimum warn --follow
just cli::run diagnostics --url http://127.0.0.1:35435 events --json
```

CLI `--json` selects JSON/SSE; `--after` resumes; `--output` saves; Ctrl-C stops. ANSI follows `--color`, `NO_COLOR`,
`FORCE_COLOR`; files/pipes default plain.

## Logs

`garmin-logging::LogService` owns history, subscription, ingestion, export, persistence, and browser/worker records.
Backend paths stay private; a failed transport does not log through itself. `garmin_logging::console` is the shared
stderr/diagnostic formatter: cyan keys, green strings, orange booleans, purple numbers; plain output keeps
layout/escaping. Gallery `console::All variants` compares outputs; capture set `captures/logging.capture.toml` covers
both terminal fonts.

| Bound                     | Limit                   |
| ------------------------- | ----------------------- |
| Record including metadata | 16 KiB                  |
| Browser queue / batch     | 512 / 128               |
| Backend history           | 16,384 records / 32 MiB |
| Developer panel           | 2,048 records           |
| Persistent segments       | Four ×8 MiB             |

Retries retain record IDs; permanent rejections count/drop, unacknowledged records queue. Deduplication lasts through
history retention; overflow/resume gaps are explicit. Store epochs distinguish restarts. Persistence failure stops disk
writes until reopening while live collection continues. Concurrent writers lock separate `writer-N` stores; drop drains.
Desktop/HASS export filtered JSONL over fixed cursor ranges; gaps/storage errors fail visibly until another attempt.
[Remaining runtime checks](../plans/shared-interface-workflows.md#runtime-acceptance).
