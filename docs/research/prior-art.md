# Prior art

Audits used ignored shallow clones; no project touched a Garmin account or device.

## Pinned treatments

| Project                                                                                                                           | Revision                                   | License       | Treatment                                                                                                                                     |
| --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| [Gadgetbridge](https://codeberg.org/Freeyourgadget/Gadgetbridge/commit/d5961f0e2bcde00aa201e34d11571ee005f58d86) 0.93.0           | `d5961f0e2bcde00aa201e34d11571ee005f58d86` | AGPL-3.0      | Direct-Garmin behavioral reference. Selective adaptation is legally compatible but remains prohibited while ADR 0011's safety gate is closed. |
| [garmin-bridge](https://github.com/wh1le/garmin-bridge/commit/5b849c7b81c005765560b7193d28c580ac80ca2c) 0.1.0                     | `5b849c7b81c005765560b7193d28c580ac80ca2c` | AGPL-3.0-only | Reject as a dependency. Retain as a small Linux/BlueZ cross-check if active Bluetooth is reconsidered.                                        |
| [Rust-Garmin](https://github.com/poster515/Rust-Garmin/commit/81c115e1003139d51deeb17b0d43652684f9c328)                           | `81c115e1003139d51deeb17b0d43652684f9c328` | GPL-3.0       | Reject implementation and dependency; its license is not the rejection reason.                                                                |
| [python-garminconnect](https://github.com/cyberjunky/python-garminconnect/commit/981d150caeda7d632224a75f3895c08df27a2a34) 0.3.12 | `981d150caeda7d632224a75f3895c08df27a2a34` | MIT           | Pinned endpoint and response-shape oracle only. Selected non-authentication material may later be adapted with provenance.                    |
| [Garth](https://github.com/matin/garth/commit/f99159a15c4c9463ce215a60ba9f7cb21f94a3b7) 0.8.0                                     | `f99159a15c4c9463ce215a60ba9f7cb21f94a3b7` | MIT           | Historical authentication and token-lifecycle reference only; upstream is deprecated.                                                         |
| [nix-tools](https://github.com/kubijo/nix-tools/commit/6f826ac80ca775e99b4f3be5858a83d5a140ab17) 0.5.0                            | `6f826ac80ca775e99b4f3be5858a83d5a140ab17` | Unlicense     | Depend through the pinned root input for repository formatting, linting, checks, and GritQL structural policies.                              |
| [rs-gallery](https://github.com/kubijo/rs-gallery/commit/23503e52b8c7bc170c9273181e1843dc8fe7dedc) 0.9.0                          | `23503e52b8c7bc170c9273181e1843dc8fe7dedc` | Unlicense     | Pin as the isolated component gallery and headless capture framework.                                                                         |

Copied or modified GPL/AGPL material retains upstream terms, notices, and provenance.

## Direct-device findings

Gadgetbridge covers GFDI, framing, MultiLink, negotiation, legacy/protobuf sync, and FIT transfer. Venu 3S is named;
fēnix 8 Solar and Edge 850 are experimental, not hardware evidence.

Its sync paths archive or acknowledge downloads, and “Keep activity data” defaults false. Do not inherit those actions.

`garmin-bridge` pairs through BlueZ and handles device info plus phone services, but lacks history sync and only tested
fēnix 6 Pro. It proves neither phone coexistence nor portability.

## Cloud findings

`python-garminconnect` covers reads, mutations, refresh, MFA, and 401/429 errors. Its login rotates identities and
strategies after refusal, so ADR 0008 permits only dated endpoint evidence, never that authentication behavior.

Rust-Garmin has no tests, obsolete auth, pervasive panics, unsafe token handling, and no failure pause. Reject it.

Garth reports broken login; existing sessions last until OAuth1 expiry. Retain only historical token-layer evidence.

## Project patterns

The root flake consumes pinned `nix-tools`; target flakes define no quality policy. `rs-gallery` supplies shared scenes,
deterministic capture, and Nix validation under recorded provenance.

## Related treatments

- [Bluetooth adapters](bluetooth-adapters.md)
- [USB adapters](usb-sync.md)
- [Garmin Connect policy](garmin-connect.md)
- [FIT implementation](fit-implementation.md)
- [Connect IQ companion](connect-iq-companion.md)
