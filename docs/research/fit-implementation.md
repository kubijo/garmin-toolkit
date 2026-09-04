# FIT implementation

## Boundary

Preserve imports, keep parser types inside `garmin-fit`, and encode only courses/workouts. Tests use publishable data
with independent expectations.

## Treatment

### `rustyfit` 0.10.2: depend

The BSD-3-Clause package handles FIT v2 decode/encode, streaming, and unknown/developer fields without restricted
generator inputs or SDK fixtures. Do not vendor; preserve notices and originals.

- Package SHA-256: `f100a37f0672f038671fdd8d983a03d6e5223ee5850dcea7477413798cc69bfc`
- Packaged VCS revision: `5cd21f4227591069de5663664b650427eec8046f`
- Sources: [package](https://docs.rs/crate/rustyfit/0.10.2), [API](https://docs.rs/rustyfit/0.10.2/rustyfit/),
  [repository](https://github.com/muktihari/rustyfit)

Three owned fēnix activities—two runs and a ride—decoded under protocol 1.0/profile 21.201. They contained session, lap,
record, event, activity, and unknown data but no developer fields; all report product `4532` (`fenix8_solar`) and
firmware `22.44`. The ride repeats activity start in lap timestamps, so summary end uses start plus elapsed.

### Chained sequences

`garmin-fit` classifies every sequence in source order. Activities remain separate; settings remain only in the
original. No activity, unknown type, or malformed sequence rejects normalization. Synthetic tests cover mixed,
unsupported, and corrupt chains.

### Rejected

- `fitparser` 0.11.0 is decode-only, SDK-profile-derived, and bundles insufficiently sourced fixtures. It is not an
  independent oracle. Package SHA-256: `56a834aed7c01a500afb06ebcfde59a4dcfd1de7ce15ae501462934d59655376`. Sources:
  [API](https://docs.rs/fitparser/0.11.0/fitparser/), [repository](https://github.com/paulja/fitparser).
- Garmin SDK code, generated output, fixtures, and automated oracles are excluded. Its license restricts redistribution,
  source-disclosure terms, and benchmarking. Sources: [protocol](https://developer.garmin.com/fit/protocol/),
  [license](https://raw.githubusercontent.com/garmin/fit-sdk-tools/main/LICENSE.txt).

See [the decision](../decisions/0006-fit-implementation-boundary.md).
