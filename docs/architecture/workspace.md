# Repository and Rust workspace

Applications compose policy; crates own reusable capabilities.

## Layout and targets

Create a directory only when current work needs it. `apps/` owns runnable CLI, desktop, and Home Assistant targets;
`crates/` owns flat, precisely named Rust responsibilities; `infra/` owns fixtures, policy, checks, and Nix plumbing;
`docs/` owns current architecture, decisions, research, and plans. Target packaging, assets, and fixtures stay beside
their owner. Avoid wrapper-only directories, empty placeholders, root `src` or `scripts`, and generic `common`,
`shared`, `core`, or `utils` crates. Root `assets` contains generated legal metadata; ecosystem-required root files are
exempt.

The CLI composes native device, map-service, capture, update, and Ratatui adapters. Desktop embeds application services
and renders egui without a daemon. Home Assistant has a native host for storage and devices, serving the WASM client
over HTTP and typed WebSocket. Targets compose crates, never each other. Real, dry-run, and demo modes share workflows:
production permits confirmed mutation, dry-run uses real services and verification but skips commit, and demo supplies
isolated fake services and devices. One global indicator identifies demo or dry-run.

Discovery exposes transport candidates. Attaching or mounting a recognizable Garmin authorizes bounded local inspection;
file transfers, network contact, and mutation remain explicit actions. Automatic deletion is forbidden. Cloud connectors
stop on authentication or rate limits until explicit recovery.

HASS targets Linux `aarch64`, with `x86_64` for development. Mobile, Bluetooth, and new device tuples require separate
evidence. Connect IQ remains optional, source-only, and locally built. HASS serves the application UI and operational
health, without publishing Garmin Toolkit data as entities, events, recorder entries, or external statistics. Domain
publication needs a concrete automation use case and reviewed public contract.

## Crate ownership

| Application           | Responsibility                                  |
| --------------------- | ----------------------------------------------- |
| `apps/garmin-cli`     | Device and map-maintenance command line and TUI |
| `apps/garmin-desktop` | Native activity, route, and device application  |
| `apps/garmin-hass`    | Home Assistant native-process host              |

| Crate                | Responsibility                                                  |
| -------------------- | --------------------------------------------------------------- |
| `garmin-brand`       | Canonical visual assets and rasterization                       |
| `garmin-capture`     | Session evidence, logs, and HTTP exchanges                      |
| `garmin-color`       | Renderer-neutral colors and semantic themes                     |
| `garmin-device`      | Discovery, manifests, transports, capacity, and file operations |
| `garmin-fit`         | FIT decoding, normalization, and course encoding                |
| `garmin-fixtures`    | Inert publishable demo and test data                            |
| `garmin-gpx`         | GPX parsing and writing behind route types                      |
| `garmin-i18n`        | ICU MessageFormat catalogs                                      |
| `garmin-importer`    | Provenance-aware FIT, GPX, and avatar import                    |
| `garmin-map-service` | Garmin map catalog and authorization service adapter            |
| `garmin-map-tiles`   | OpenFreeMap tile validation, transport, and HTTP-aware cache    |
| `garmin-model`       | Source-neutral user, activity, route, and map models            |
| `garmin-progress`    | Operation observation and cancellation contracts                |
| `garmin-route`       | Route-plan transformations                                      |
| `garmin-service-api` | Host/client service contracts without host-only dependencies    |
| `garmin-services`    | User-scoped application use cases                               |
| `garmin-simulator`   | Fake device and map-service adapters                            |
| `garmin-storage`     | SQLite persistence and migrations                               |
| `garmin-ui`          | Shared egui components and colocated scenes                     |
| `garmin-update`      | Planning, download, backup, install, removal, and recovery      |

Apps may depend on crates; crates never depend on apps. Format and connector vocabulary stays behind its owning crate.
Dependencies use their full package names in manifests and Rust. There is no generic core, common, shared, or utilities
crate.

`garmin-model` is the canonical cross-platform domain vocabulary. Its serialized values must round-trip through
non-self-describing Postcard on native and WASM targets, so enums use the default external representation. Service
contracts embed these values instead of declaring transport copies. Source-shape rules reject incompatible enum tags and
unreviewed public types in `garmin-service-api`; round-trip and host/client tests guard the actual wire behavior. See
[ADR 0040](../decisions/0040-canonical-models-across-service-boundaries.md).

A small crate still needs an independent dependency boundary. `garmin-progress` is the leaf contract through which
device I/O, update transactions, services, and clients exchange operation state without depending on one another. It
owns no device policy, transaction decisions, persistence, or presentation.

`apps/garmin-cli/tui` is the CLI-owned Ratatui package. `infra/gallery` is a separate Cargo workspace that renders it
and colocated `garmin-ui` scenes without entering production binaries. Members inherit exact dependency versions and
lints from the root workspace.

The HASS browser client cannot depend on native device, SQL, or service implementations. `garmin-service-api` owns the
Remoc traits and RPC envelopes between that client and the native host. It reuses `garmin-model` values and owns no
device discovery, persistence, update policy, or UI. Native clients use a local connection; HASS serves binary
WebSockets through ingress. The wire representation is private and ephemeral.
