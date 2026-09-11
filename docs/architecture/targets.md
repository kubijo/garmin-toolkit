# Application targets

| Target         | Process boundary                                                                   |
| -------------- | ---------------------------------------------------------------------------------- |
| CLI            | Native process composes device, map service, capture, update, and Ratatui adapters |
| Desktop        | Native process embeds application services and renders egui                        |
| Home Assistant | Native host owns storage and devices; HTTP serves WASM and typed WebSocket         |

Targets compose crates, never each other. Real, dry-run, and demo share workflows; modes select adapters and
capabilities. Production permits confirmed mutation. Dry-run uses real services and verification but skips commit. Demo
supplies isolated fake services and devices.

Discovery exposes transport candidates. Attaching or mounting a recognizable Garmin authorizes bounded local inspection;
file transfers, network contact, and mutation remain explicit actions. Automatic deletion is forbidden. Cloud connectors
stop on authentication or rate limits until explicit recovery.

HASS targets Linux `aarch64`, with `x86_64` for development. Desktop needs no daemon. Mobile, Bluetooth, and new device
tuples require separate evidence. Connect IQ remains optional, source-only, and locally built.
