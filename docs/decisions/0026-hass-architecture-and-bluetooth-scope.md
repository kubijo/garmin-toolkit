# 0026: HASS architecture and Bluetooth scope

## Decision

The first real deployment and primary HASS release gate is the owned Raspberry Pi 5 running Linux `aarch64`. Also build
an `amd64` artifact for development and x86-64 installations. Support no 32-bit architecture and keep none in the
execution queue.

Exclude Garmin Bluetooth in every form, including native adapters and HASS Bluetooth proxies. Ship no related crates,
permissions, configuration, or discovery. ADR 0010 remains dormant design research; ADR 0011 records the safety gate,
but no milestone is tasked with clearing it. Reconsideration requires new evidence and a superseding decision.

## Why

The owned HASS host requires `aarch64`, while `amd64` covers development and common installations. No owned use requires
32-bit artifacts. A HASS proxy cannot make the unproven and apparently unusable native Garmin Bluetooth path useful.

## Consequences

The HASS target flake builds both 64-bit artifacts. A real Raspberry Pi 5 installation is mandatory for the first slice
and every HASS release; `amd64` requires a package smoke test. Apple Silicon shares the ARM architecture but remains a
separate macOS desktop target.
