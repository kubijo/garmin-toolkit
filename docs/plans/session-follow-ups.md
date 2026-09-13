# Session review follow-ups

This is the durable recovery ledger for unfinished work raised during the HASS Web, gallery, notification, and device
review. Keep only unresolved work here; remove an entry after its implementation and acceptance evidence have a durable
owner elsewhere.

## Device metadata and files

- [ ] Keep physical-device evidence honest. The Edge 1050 and Venu 3S shown together in production were both connected;
  only demo, gallery, simulator, and synthetic-test devices use deliberately made-up names.
- [ ] Capture real-device manifest differences only through a separately approved anonymization procedure. Do not commit
  raw device identifiers or manifests copied directly from hardware.

## Notifications and diagnostics

- [ ] Implement the structured-logging work in [shared interface workflows](shared-interface-workflows.md): configurable
  startup verbosity, typed/redacted fields, bounded live history, filtering, rotation, and explicit UI download.
- [ ] Verify that `Ctrl+C` through `just hass::run` exits cleanly after application-owned SIGINT/SIGTERM shutdown,
  without Just printing `error: interrupted by SIGINT`.

## Web and gallery acceptance

- [ ] Verify a normal browser reload receives the favicon and current entrypoint without DevTools cache bypass. Keep
  HTML and the initializer `no-store`; keep content-hashed static assets immutable.
- [ ] Remove or justify duplicate/unused WASM preload warnings, and keep loader prose tied to real download/startup
  phases rather than vague text such as `Preparing download`.
- [ ] Complete the keyboard-only and focus-style audit already owned by
  [shared interface workflows](shared-interface-workflows.md), including the explorer window and reconnect overlay.
- [ ] Complete the translation-adoption gate in [toolkit integration](toolkit-integration.md): mechanically reject
  production UI copy that bypasses typed FormatJS messages, separately from catalog parity.

## Working constraints

- Do not commit, stage, unstage, reset, or otherwise alter the Git index unless the user explicitly requests that exact
  operation. Existing staged work belongs to the user.
- Do not start or probe the HASS server unless the user explicitly requests that exact run; the user owns production
  runs and their data directory.
- Do not inspect, reset, mount, unmount, or otherwise probe connected physical devices without an explicit request.
  Never classify an attachment as fake or stale from a screenshot when the host state has not been checked.
- Never use a direct `nix build path:.` invocation; use the repository's filtered flake/Just entrypoints.
- Do not run Cargo or another heavyweight Rust/Nix build without explicit permission. A prior unconstrained development
  workload contributed to an OOM session loss. When authorized, scope the command, use low parallelism, and place it in
  a hard memory-limited cgroup.
