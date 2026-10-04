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
