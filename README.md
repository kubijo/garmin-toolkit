# Garmin Toolkit

Experimental, unofficial Linux tools for Garmin devices and data:

- `garmin-cli`: device inspection and guarded map maintenance;
- `garmin-desktop`: local activity and route management;
- `garmin-hass`: Home Assistant host under development.

Applications compose the reusable crates under `crates/`.

## Run

```sh
just cli::run
just cli::run --help
just cli::demo
just cli::run --dry --capture=/safe/new/session
just cli::run benchmark link --size=256MB
```

Nix pins the toolchain and runtime dependencies. `cli::run` is production; `cli::demo` substitutes disposable adapters.
`--dry` uses real discovery, services, downloads, capture, and verification, but skips the device commit. `--json` is
pretty and colored on a terminal; redirected output stays plain. `FORCE_COLOR` and `NO_COLOR` provide environment
overrides; `--color auto|off|always` provides explicit control. Failures use structured, width-aware reports on stderr
after any TUI closes, while redirected diagnostics remain plain and copyable.

Verified map payloads are reused from a content-addressed cache. Installed binaries default to
`~/.cache/garmin-toolkit/maps`; `GARMIN_TOOLKIT_CACHE_DIR` relocates it and `--cache-dir` takes precedence. Repository
recipes keep it at `.tmp/app-cache`. The link benchmark uses the normal device picker, asks before writing one
disposable file, and separately reports payload transfer, device finalization, SHA-256 read-back, and cleanup. Raw-MTP
probes keep recovery receipts below the configured cache until the exact disposable object is verified and removed.

## Safety

Garmin contact and device writes require separate confirmation. Downloads require HTTPS, reject redirects, and verify
size and MD5. Device paths reject traversal and symlink escapes. Writes use verified backups and a durable journal;
cancellation waits for a safe checkpoint.

Production writes require a new capture directory. Captures may contain device identity, authorization, licensed maps,
logs, and backups. Keep them private.

## Provenance and interoperability

This project is not affiliated with, sponsored by, or endorsed by Garmin. Garmin product names are trademarks of their
owner. Garmin supplies its maps, software, and services; this project does not redistribute them.

Map interoperability came from publicly distributed Garmin Express binaries, traffic from developer-owned devices, and
openly available Garmin-hosted forum posts from [2012], [2016], and [2019]. Public visibility neither makes these
services supported APIs nor implies permission, endorsement, or continued access.

The repository contains no Garmin binaries, decompiler output, private captures, credentials, or maps. It implements
observed service and download-authorization behavior behind a replaceable adapter.

## Development

```sh
just qa::preflight
just qa::full
just qa::audit
```

See [architecture](docs/architecture/), [decisions](docs/decisions/), and [active plans](docs/plans/). The gallery
renders terminal and desktop states without entering a shipped binary.

## License

Licensed under [AGPL-3.0-or-later](LICENSE-AGPL). Earlier grants and third-party attributions remain in
[COPYRIGHT.md](COPYRIGHT.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

[2012]: https://forums.garmin.com/de/strabennavigation/f/motorrad/32822/zumo-220-kartenupdate
[2016]: https://forums.garmin.com/apps-software/mac-windows-software/f/garmin-express/113071/edge-1000-won-t-update-eu-cycle-map
[2019]: https://forums.garmin.com/apps-software/mac-windows-software/f/garmin-express-windows/167747/garmin-express-v6-15-0-0-cannot-install-map-2020-10-on-a-computer
