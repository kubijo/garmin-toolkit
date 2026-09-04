# Connect IQ companion

## Feasible role

A signed Connect IQ app can send HTTPS through phone or device Wi-Fi to a configured endpoint.

It is not a tunnel: background work gets 30 seconds every five minutes, with no listener, UDP, mDNS, or FIT-history
access. Obtain history elsewhere.

Sources: [communications](https://developer.garmin.com/connect-iq/api-docs/Toybox/Communications.html),
[Wi-Fi sync](https://developer.garmin.com/connect-iq/core-topics/downloading-content/),
[backgrounding](https://developer.garmin.com/connect-iq/core-topics/backgrounding/),
[signing](https://developer.garmin.com/connect-iq/core-topics/security/), and
[activity recording](https://developer.garmin.com/connect-iq/core-topics/activity-recording/).

## Toolchain boundary

Publish only project-owned source, manifest, and docs. Users supply the SDK path and private key.

Compilation stays local and outside packages, caches, releases, and CI. Outputs and keys stay under ignored `.tmp`.

The local workflow needs:

1. `bootstrap`: check prerequisites and start interactive setup without accepting agreements or authenticating.
2. `keygen`: create or reuse an owner-readable external key.
3. `doctor`: verify SDK, definitions, Java, key permissions, exclusions, and compiler.
4. `build`: consume explicit SDK/key paths and write under `.tmp`.
5. `sideload`: target a resolved device outside validation and release.

## Probe

On the fēnix, test foreground, background, and sync requests to HASS with and without the phone. Record transport, DNS,
certificates, limits, scheduling, available data, and `.local` resolution.
