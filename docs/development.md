# Development workflow

Use the pinned Nix/uv tooling and repository Just entrypoints. Check commands and build ownership are described in
[the build system](architecture/build-system.md). UI changes require rendered and inspected gallery previews.

## Execution safeguards

- Do not commit, stage, unstage, reset, or otherwise alter the Git index unless explicitly requested. Existing staged
  work belongs to the user.
- Do not start or probe HASS unless that exact run is explicitly requested; production runs and their data directory
  belong to the user.
- Do not inspect, reset, mount, unmount, or probe physical devices without an explicit request. Do not infer that an
  attachment is fake or stale from a screenshot. Only demo, gallery, simulator, and synthetic-test devices use
  deliberately made-up identities.
- Use filtered flake/Just entrypoints, never `nix build path:.`.
- Cargo and heavyweight Rust/Nix builds require explicit permission. Scope authorized commands, use low parallelism, and
  on Linux run under a hard memory-limited cgroup; an unconstrained workload previously caused an OOM session loss.
  Apple Silicon macOS development has an approved exception: the maintained wrapper forces one Cargo job, one Nix build
  at a time, and one core per Nix builder. This is a parallelism limit, not a hard RAM limit. Run heavyweight commands
  serially through the Just recipes or `infra/just/memory-capped.sh`; do not run concurrent builds. On Linux the wrapper
  defaults Cargo to two jobs unless `CARGO_BUILD_JOBS` is set, avoiding memory throttling from excessive parallel
  workers.
- Keep diagnostics and analysis in maintained tooling, not disposable scripts. Temporary outputs belong under the
  repository's ignored `.tmp/` directories; gallery captures belong under `.tmp/gallery/`.
- Real-device manifest comparisons require a separately approved anonymization procedure. Never commit raw device
  identifiers or private captures; follow the fixture rules below.

## Tooling choices

`GARMIN_TOOLKIT_STATE_DIR` overrides the platform state-directory parent for CLI logs, automatic captures, and the
legacy map recovery registry. Tests that launch the CLI use a temporary directory through this application-level
setting; they do not depend on Linux XDG variables or write to the developer's normal state directory.

Prefer direct command composition and argument forwarding over shell wrappers and argument reconstruction.

Tooling uses typed CLI definitions (Tyro for Python) and template engines for generated documents. Ask before using
string concatenation or interpolation as a templating mechanism. JavaScript is a last-choice tooling language, reserved
for web work whose production code is already JavaScript.

Python uses Nix-pinned basedpyright in strict mode, with warnings failing the check. The shared configuration in
`infra/python/pyproject.toml` covers all Python tooling under `infra`, including tests and generators; the root
`pyrightconfig.json` exposes the same settings to editors. `just qa::preflight` and `just qa::full` check against the
pinned Python 3.14 environment. Ruff owns Python formatting and linting.

## Documentation ownership

Keep current contracts in architecture, consequential rationale in a small set of decisions, reproducible results and
uncertainty in research, and unresolved work in its owning plan. Retire superseded decisions after moving unique current
rules to their owners; Git retains the history. Do not create a second session checklist. After a review, preserve its
unique evidence, update incoming links, and remove the temporary review file. Keep session narratives, routine test
counts, command logs, and resolved checklists out of persistent documentation. Explain current behavior and design
constraints directly; keep open questions concrete and remove them when answered. Retain empirical measurements and
reproduction methods when they inform unresolved work.

Cite tracked fixtures, maintained reproduction commands, or durable published artifacts. Do not cite ignored local files
as evidence in committed documentation.

## Interface validation

Every TUI, GUI, or web UI change needs a gallery scene using production rendering and models. Compile the scene, capture
it, and inspect the image; compilation alone does not establish usable layout. Cover relevant loading, empty, success,
error, cancellation, and recovery states, supported viewport sizes and fonts, themes, languages, and long real-world
content. Check wrapping, clipping, contrast, focus, controls, and interaction in the live preview. Record scene IDs,
revision, viewport, font, locale, and capture results with the change. Reproducible capture recipes live in the repo;
rendered captures belong in local or CI artifacts. Runtime tests cover behavior but do not replace visual inspection.

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
