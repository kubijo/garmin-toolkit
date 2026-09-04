# Rust workspace

Applications compose policy; crates own reusable capabilities.

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
device discovery, persistence, update policy, or UI.
