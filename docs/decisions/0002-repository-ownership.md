# 0002: Repository ownership

## Decision

Use shallow owner-based directories: `apps/` for deliverables, `crates/` for flat Rust responsibilities, and `infra/`
for repository tooling, policy, and shared evidence. Keep target packaging beside its app.

## Why

Target grouping obscures ownership, centralized packaging separates target inputs, and empty scaffolding implies work
that does not exist.

## Consequences

New paths need an explicit owner. Material structural changes require a new decision and an update to
[repository architecture](../architecture/repository.md).
