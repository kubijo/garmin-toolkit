# Development workflow

Use the pinned Nix/uv tooling and repository Just entrypoints. Check commands and build ownership are described in
[the build system](architecture/build-system.md); UI changes require
[validated interface previews](decisions/0038-validated-interface-previews.md).

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
  serially through the Just recipes or `infra/just/memory-capped.sh`; do not run concurrent builds.
- Keep diagnostics and analysis in maintained tooling, not disposable scripts. Temporary outputs belong under the
  repository's ignored `.tmp/` directories; gallery captures belong under `.tmp/gallery/`.
- Real-device manifest comparisons require a separately approved anonymization procedure. Never commit raw device
  identifiers or private captures; follow the [fixture policy](research/fixture-policy.md).

## Documentation ownership

Keep current contracts in architecture, accepted choices in decisions, reproducible results and uncertainty in research,
and unresolved work in its owning plan. Do not create a second session checklist for an existing owner. After resolving
a review, preserve its unique evidence, update incoming links, and remove the temporary review file. Keep session
narratives, routine test counts, command logs, and resolved checklists out of persistent documentation. Explain current
behavior and design constraints directly; keep open questions concrete and remove them when answered. Retain empirical
measurements and reproduction methods when they inform unresolved work.
