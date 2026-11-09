# 0009: Local-only Connect IQ builds

## Decision

Publish companion source only. Users obtain Garmin's SDK through owner-operated SDK Manager and use a private RSA-4096
key encoded as PKCS#8 DER. Garmin Toolkit never fetches the SDK as a Nix input or publishes SDK files, keys, `.prg`,
`.iq`, or debug output.

Builds are explicit local commands retaining compiler work under ignored `.tmp` and copying the sideloadable probe to
`dist/connect-iq/probe.prg`. Neither directory is published. These builds are never flake packages, cache inputs,
releases, or CI requirements. Repository checks may validate project-owned text without Garmin tools.

Local setup may provide a Nix-managed runtime for an owner-downloaded SDK Manager, discover its selected SDK, and retain
SDK/key paths in ignored local state. Only public runtime dependencies may enter its compatibility image. Setup must
never accept agreements or authenticate on the user's behalf.

## Why

Garmin permits local developer signing and sideloading, but SDK use requires a personal, revocable agreement forbidding
redistribution. User-supplied inputs keep that agreement and proprietary material outside the repository build graph.

## Consequences

Connect IQ remains optional and source-only. Physical evidence records the SDK, device definition, firmware, revision,
and command because release CI cannot compile it. A lost key affects only that user's updates.

Qualify the selected transport before claiming device support. The foreground BLE probe avoids HTTPS but requires a host
peripheral adapter and physical safety evidence; see [Connect IQ research](../research/connect-iq-companion.md).
