# 0001: Initial application targets

## Decision

Start with Home Assistant and desktop. HASS combines a native backend with an ingress-served egui/WASM UI; desktop
embeds the services and renders egui natively. There is no standalone web app or universal daemon.

## Why

Persistence and native connectors preclude browser-only operation. Desktop needs no network boundary. Mobile and Connect
IQ wait for shared contracts.

## Consequences

HASS needs a native-host/WASM service boundary; desktop does not. Shared lower layers must depend on neither host. See
[target architecture](../architecture/targets.md).
