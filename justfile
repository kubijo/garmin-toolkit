import 'infra/just/common.just'

# Development tasks for builds, documentation, and dependency maintenance.
mod dev 'infra/just/dev.just'

# Development-only recipes for rendering and inspecting the TUI.
mod gallery 'infra/gallery'

# Garmin Toolkit host for Home Assistant.
mod hass 'apps/garmin-hass'

# Repository quality gates and focused checks.
mod qa 'infra/just/qa.just'

# Native Garmin Toolkit desktop application and its packages.
mod desktop 'apps/garmin-desktop'

# CLI application and its packages.
mod cli 'apps/garmin-cli'
