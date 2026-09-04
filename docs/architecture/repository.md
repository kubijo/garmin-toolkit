# Repository architecture

Create a directory only when current work needs it.

| Path      | Owner                                                           |
| --------- | --------------------------------------------------------------- |
| `apps/`   | Runnable CLI, desktop, and Home Assistant deliverables          |
| `crates/` | Flat, precisely named Rust responsibilities                     |
| `infra/`  | Shared fixtures, policy, checks, and Nix plumbing               |
| `docs/`   | Current architecture, decisions, research, and disposable plans |

Rules:

- Target packaging and target-only inputs stay beside `apps/*`.
- Target-only assets and fixtures stay beside their owner.
- No target grouping or generic `common`, `shared`, `core`, or `utils` crate without a new decision.
- Root-level `src` and `scripts` are prohibited. Root `assets` contains generated legal metadata only.
- Wrapper-only directory levels and empty placeholders are prohibited.
- Ecosystem-required root files are exempt.

[workspace.md](workspace.md) defines crate ownership and dependency direction; lower layers never depend on apps.
