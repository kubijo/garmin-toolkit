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
  run under a hard memory-limited cgroup; an unconstrained workload previously caused an OOM session loss.
- Keep diagnostics and analysis in maintained tooling, not disposable scripts. Temporary outputs belong under the
  repository's ignored `.tmp/` directories; gallery captures belong under `.tmp/gallery/`.
- Real-device manifest comparisons require a separately approved anonymization procedure. Never commit raw device
  identifiers or private captures; follow the [fixture policy](research/fixture-policy.md).

## Documentation ownership

Keep current contracts in architecture, accepted choices in decisions, reproducible results and uncertainty in research,
and unresolved work in its owning plan. Do not create a second session checklist for an existing owner. After resolving
a review, preserve its unique evidence, update incoming links, and remove the temporary review file. Historical test
counts establish only their recorded scope and revision, not acceptance of later changes.
