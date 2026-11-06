# Development workflow

Use the pinned Nix/uv tooling and repository Just entrypoints. Check commands and build ownership are described in
[the build system](architecture/build-system.md). UI changes require rendered and inspected gallery previews.

Close implementation slices with full QA and audit; deployment claims also require packaged behavioral evidence. Just
lists the available commands.

## Execution safeguards

- Do not commit, stage, unstage, reset, or otherwise alter the Git index unless explicitly requested. Existing staged
  work belongs to the user.
- Do not start or probe HASS unless that exact run is explicitly requested; production runs and their data directory
  belong to the user.
- Do not inspect, reset, mount, unmount, or probe physical devices without an explicit request. Do not infer that an
  attachment is fake or stale from a screenshot. Only demo, gallery, simulator, and synthetic-test devices use
  deliberately made-up identities.
- Use filtered flake/Just entrypoints, never `nix build path:.`.
- Agent-run Cargo and heavyweight Rust/Nix builds require explicit permission. Run them serially through
  `bash infra/just/memory-capped.sh just qa::full` or wrap the focused command. Manual Just commands inherit the user's
  settings. On Linux, verify the active worker's `memory.max` is at most 6 GiB and `cpu.max` at most two CPUs. The
  wrapper defaults Cargo to two jobs and requires a working systemd user manager. Nix daemon workers do not inherit the
  client scope: verify separate worker limits before heavy builds. Do not request or change daemon-wide limits. Prefer
  cached toolchains and direct Cargo/Trunk in an agent-owned scope; reuse `.tmp/cargo-target` and keep Cargo
  home/distribution outputs in `.tmp`. The Apple Silicon macOS exception uses one Cargo job, one Nix build, and one core
  per builder; these are parallelism limits rather than a hard RAM cap.
- Keep diagnostics and analysis in maintained tooling, not disposable scripts. Temporary outputs belong under the
  repository's ignored `.tmp/` directories; gallery captures belong under `.tmp/gallery/`.
- An isolated tooling shell can hide `/dev/kvm` even when the host has it. Check through a transient host user service
  before declaring HAOS acceptance unavailable. Run only the requested disposable harness there, with its explicit tool
  paths, four-CPU/six-GiB VM limits, and runtime deadline; collect the service and verify cleanup afterward. The service
  must not use a real HASS instance or inherit a personal browser profile.
- Real-device manifest comparisons require a separately approved anonymization procedure. Never commit raw device
  identifiers or private captures; follow the fixture rules below.

## Tooling choices

Use compatible published dependency releases. Do not add dependency source patches, forks, or unpublished revisions to
resolve external blockers. Document each blocked finding as a narrowly scoped exception with its upstream blocker and a
condition for revisiting it; keep unrelated findings fatal.

`GARMIN_TOOLKIT_STATE_DIR` overrides the platform state-directory parent for CLI logs, automatic captures, and the
legacy map recovery registry. Tests that launch the CLI use a temporary directory through this application-level
setting; they do not depend on Linux XDG variables or write to the developer's normal state directory.

Prefer direct command composition and argument forwarding over shell wrappers and argument reconstruction.

Tooling uses typed CLI definitions (Tyro for Python) and template engines for generated documents. Ask before using
string concatenation or interpolation as a templating mechanism. JavaScript is a last-choice tooling language, reserved
for web work whose production code is already JavaScript.

Python command runners use `infra/python/cli_output.py` for Rich panels, syntax highlighting, and traceback handling.
Force child-process colors and retain their original output in logs. Display colors only when the terminal supports them
and no agent environment is detected, or when explicitly forced with `FORCE_COLOR` or `CLICOLOR_FORCE`; strip ANSI
otherwise. Explicit forcing wins over automatic detection and `NO_COLOR`. Catch expected failures, report them in a
panel, and preserve subprocess exit codes (124 for timeout, 126/127 for permission/missing executable, 128 + signal for
interruption). Unexpected failures use Rich tracebacks. Use a known language lexer when displaying code or structured
data.

Python tooling uses strict basedpyright, deptry, and Ruff through nix-tools. Configuration lives in
`infra/python/pyproject.toml` and root `ruff.toml`; warnings fail QA. Root and nested `pyrightconfig.json` expose the
same settings to editors. Preflight and full QA use the pinned Python environment.

HAOS browser tooling uses `.mts`, run directly by the pinned Node version. QA checks it with strict TypeScript and
locked Node/CDP types from `infra/javascript/package-lock.json`; no browser bundle is built. For editor types, run
`npm ci --ignore-scripts --prefix infra/javascript`. Node strips types at runtime but does not check them.

The online `just dev::outdated` report excludes both Rust workspaces: cargo-outdated's temporary copy cannot resolve
patched `vendor/fast-mvt`, and gallery path dependencies leave its project root. Local Nix inputs and UV packages
without a reported latest version stay visible as unresolved findings.

For editor import resolution, populate `infra/python/.venv` from the locked tooling dependencies:

```bash
uv sync --frozen --project infra/python
```

The root type-checker configuration selects this environment; `infra/python/pyrightconfig.json` inherits it for editors
that start the language server at the nested Python project. In Zed, select its Python interpreter through
`toolchain: select` to use the same environment for editor tasks.

## Documentation ownership

Architecture owns current contracts, decisions consequential rationale, research reproducible evidence and limits, and
plans unresolved work. Move unique content and update incoming links before deleting superseded decisions, completed
plans, or temporary reviews; Git retains history.

Exclude session narratives, routine test counts, command logs, and resolved checklists. Keep open questions concrete;
remove them when answered. Retain measurements and methods that inform unresolved work. Cite tracked fixtures,
maintained commands, or durable published artifacts, never ignored local files.

## Interface validation

Every TUI, GUI, or web UI change needs a gallery scene using production rendering and models. Compile the scene, capture
it, and inspect the image; compilation alone does not establish usable layout. Cover relevant loading, empty, success,
error, cancellation, and recovery states, supported viewport sizes and fonts, themes, languages, and long real-world
content. Check wrapping, clipping, contrast, focus, controls, and interaction in the live preview. Record scene IDs,
revision, viewport, font, locale, and capture results with the change. Reproducible capture recipes live in the repo;
rendered captures belong in local or CI artifacts. Runtime tests cover behavior but do not replace visual inspection.

For authorized runtime UI checks, keep app data and browser profiles under `.tmp`, bind control/debugging listeners to
loopback, and apply the execution limits above to actual workers. Stop owned processes and verify their listeners are
gone. Prefer the app's semantic control API; record missing capabilities in the owning plan. Capture visual fixes before
and after at the same viewport, link the app-owned images, and explain the change. A full-desktop capture cannot replace
an app screenshot.

## Headless hardware rendering probe

The opt-in desktop `render-probe` feature runs the real demo UI and map callbacks with 4x MSAA into an offscreen Vulkan
texture. It selects the sole AMD hardware adapter by default, or an unambiguous `--adapter` name substring; software
adapters are rejected. It uses temporary demo data and starts the usual loopback control API. Without `--scenario`,
control the UI through that API until the time limit; repeated `--scenario` arguments run built-in workloads
sequentially and return a failure for failed or timed-out workloads. The time budget includes initialization. An
independent watchdog terminates the process with exit code 124 after the budget plus ten seconds for cleanup, even if a
frame or shutdown blocks. Forced termination can leave partial captures and no final summary. Native file dialogs and
folder launches are rejected and fail the probe before they open; use the native application to verify those
integrations.

After approval to build and run:

```sh
just desktop::render-probe --output .tmp/amd-render-probe \
    --scenario activity-smoke --scenario responsive-layout --seconds 120
```

The output directory must not exist. `summary.json` records the selected adapter, driver, limitations, and recording
errors; `scenario-*.json` contains the automation results. `frames/` contains sampled PNGs and `frames.jsonl` records
their original frame numbers, timestamps, and sizes. Use those timestamps for later video assembly: resize workloads
change frame dimensions, and a busy writer drops captures instead of blocking rendering. Compression and disk I/O use a
two-frame worker queue. `--capture-every 1` requests every frame, `--capture-every 0` disables periodic readback, and
`--max-captures` caps files (default 120). Explicit control screenshots still perform readback.

Ordinary frames remain on the GPU. Captured frames still incur GPU readback; GPU completion is synchronized each frame
to bound queued work. Results are functional evidence, not performance benchmarks. The probe covers only the screens and
interactions exercised by its scenarios or control commands. It embeds child windows and does not validate native
presentation, window management, file dialogs, or physical-device access. It does not replace Deck/Bazzite acceptance.

## Fixtures

Committed FIT fixtures are synthetic and project-owned. Garmin samples, private files, traces, responses, and uncertain
material stay in ignored private storage. Third-party material needs an exact source/revision, license, hash, notices,
and publication review; regenerate device cases synthetically instead of redacting private originals. Shared generated
corpora belong under `infra/fixtures/<domain>/<case>/` with `fixture.toml` recording purpose, origin, generator,
license, input hashes, hand-authored expectations, and approval. Commit deterministic recipes, not generated FIT bytes
or databases. Small owner-local synthetic test vectors may stay beside their tests.

Review raw and decoded fields for names, identifiers, serials, secrets, real times, and locations, including unknown FIT
fields. Automation checks shape and secrets but does not replace semantic review. Demo artifacts use separate roots and
seed through the production importer; production starts empty or continues existing user data.
