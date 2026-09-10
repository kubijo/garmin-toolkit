# 0033: Garmin Toolkit identity and license

## Decision

Name the repository and product family Garmin Toolkit. Use precise `garmin-*` Cargo package names, with `garmin-cli`,
`garmin-desktop`, and `garmin-hass` as the executable packages.

License the combined workspace as `AGPL-3.0-or-later`. Preserve the MIT and Apache-2.0 notices covering source that was
previously offered under those terms, plus every third-party notice and asset license.

State the project's provenance directly. Its Garmin map-service interoperability was developed by inspecting publicly
distributed Garmin Express binaries, traffic from developer-owned devices, and cited public material. The repository
must not contain Garmin binaries, decompiler output, private captures, credentials, or map files.

This supersedes ADR 0003's project identity and license choice and ADR 0029's `nr-*` package namespace.

## Why

The repository now combines device maintenance with activity, route, desktop, and Home Assistant capabilities. A toolkit
identity describes that scope, while precise package names make ownership legible outside the workspace. The AGPL is the
conservative common license for the combined source.

## Consequences

Public artifacts use Garmin Toolkit branding. `garmin-cli` remains the command name because it accurately names that
application. Release checks retain provenance and license evidence. Before the first release, persisted identifiers use
only the current Garmin Toolkit domains; no compatibility names are retained for development artifacts.
