# 0009: Local-only Connect IQ builds

## Decision

Publish companion source only. Users obtain Garmin's SDK, supply its path, and generate a private RSA-4096 key. Garmin
Toolkit never downloads or publishes the SDK, keys, `.prg`, `.iq`, or debug output.

Builds are explicit local commands writing under ignored `.tmp`, never flake packages, cache inputs, releases, or CI
requirements. Repository checks may validate project-owned text without Garmin tools.

Future automation must consume explicit user-owned SDK and key paths. It must never accept agreements or authenticate on
the user's behalf.

## Why

Garmin permits local developer signing and sideloading, but SDK use requires a personal, revocable agreement forbidding
redistribution. User-supplied inputs keep that agreement and proprietary material outside the repository build graph.

## Consequences

Connect IQ remains optional and source-only. Physical evidence records the SDK, device definition, firmware, revision,
and command because release CI cannot compile it. A lost key affects only that user's updates.

Probe after a HASS HTTPS endpoint can receive foreground, background, and Wi-Fi-sync requests. See
[Connect IQ research](../research/connect-iq-companion.md).
