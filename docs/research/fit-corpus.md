# FIT corpus

## Initial cases

The data foundation creates these deterministic synthetic fixtures:

| Case                    | Required content                                                          |
| ----------------------- | ------------------------------------------------------------------------- |
| `activity-basic`        | File/device identity, one session/lap, timestamped GPS and sensors        |
| `activity-multisport`   | Multiple sessions and laps with distinct sports                           |
| `wellness`              | Timestamped monitoring and body measurements; some optional values absent |
| `sleep-hrv`             | Interval sleep data and HRV samples spanning a synthetic day boundary     |
| `course`                | Route points and course metadata                                          |
| `workout`               | Nested steps, targets, repetitions, and rest                              |
| `developer-unknown`     | Described developer fields plus an unknown message/field                  |
| `developer-undescribed` | A developer field without its required description                        |
| `truncated`             | A valid case cut at defined offsets                                       |
| `bad-checksum`          | A valid case with one controlled mutation                                 |

Basic activity normalization must match the synthetic definition exactly; multisport hierarchy and ordering must
survive. Absent wellness values stay absent. Unknown fields preserve supported values and original bytes; undescribed
developer fields fail cleanly. Every truncation returns an error without panic or committed partial data; checksum
failure is reported without a commit.

Course and workout cases compare semantics, not bytes. Expectations come from the synthetic definition, never Garmin SDK
fixtures or parser output.

## Shared invariants

- Imported bytes and their SHA-256 hash remain unchanged.
- Parser types stay in `fit`; normalization is deterministic.
- Reimport is idempotent; similar artifacts remain separate until reconciled.
- Unknown or unsupported data cannot block preservation of a valid source artifact.
- Interrupted reads leave only discardable staging data; retry is deterministic.
- Corruption produces classified errors without panics, partial database state, or fabricated normalized values.

Real device files stay ignored. They extend support only through a synthetic regression or approved public fixture.

## Development corpus

`garmin-fixtures` seeds three fake profiles and eight FIT files from recorded runs, rides, a swim, a walk, and a hike
through the production importer and storage APIs. Outdoor activities contain real route geometry; a sensor-rich indoor
ride contains speed, cadence, and power. Stable IDs make the seed idempotent.

Alex also starts with three saved routes: City ride, Neighborhood walk, and Mountain hike. The seed exports geometry and
elevation from those same normalized recordings to GPX using the `gpx` writer, then saves them through the production
route importer. These are identified as demo recording exports, not original upstream GPX files. Repeated seeding
preserves their import receipts and any later route edits. Sam and Taylor retain empty route libraries for empty-state
coverage; the recorded activity assignments are unchanged.

The compact normalized recordings come from the GPL-3-licensed real-data examples in `trackeR` 1.6.1, Sam Swift's
MIT-licensed DogWalkGPS recordings, and HikeAlong's CC-BY-4.0 watch recordings. Source timestamps, athlete identity, and
device identity are discarded; the generated FIT files use deterministic 2026 dates and fake device metadata.
[`fixture.toml`](../../infra/fixtures/fit/development-activities/fixture.toml) pins the CRAN archive, every selected
member, generator, transformation, committed-input hash, and expected output case. Generated FIT files and databases
remain build products rather than committed binaries. Additional source URLs, revisions, licenses, and hashes are in the
same manifest; attribution is in `THIRD_PARTY_NOTICES.md`.

Regenerate the additional recordings from the exact hash-verified downloads listed in the manifest:

```sh
python3 infra/fixtures/fit/development-activities/generate.py --walking-hiking walk.gpx hike.gpx
```

Community files help select missing cases. A case becomes durable only after it is recreated synthetically or cleared
for redistribution with its provenance intact.

## External corpus

Large corpora remain optional. Fetch an exact-revision, hash-verified ZIP64 snapshot as one Nix store file and read its
entries directly; do not clone or extract it. Corpus sweeps stay outside normal validation, required CI, and published
binary caches.

## Community evidence

Reviewed 2026-09-01:

- [`ThomasKuehne/FIT-test-files`](https://github.com/ThomasKuehne/FIT-test-files/tree/3d895613ef90fdcf958d9518da942385271128e5)
  aggregates 68,003 FIT files with source URLs and broad device, sport, date, and file-type coverage. It has no
  repository-wide license; use its pinned ZIP snapshot only as optional local input unless an individual fixture has
  verified redistribution terms.
- [`fittie_chained_file.fit`](https://github.com/marcelblijleven/fittie/blob/d0ff26711c1ca7167f04974410371cd9ee6a54fa/tests/data/fittie_chained_file.fit)
  is an MIT-licensed synthetic pair of minimal activity sequences. It proves framing, not activity normalization.
- [`hammerheadnav/fit`](https://github.com/hammerheadnav/fit/tree/5ee5686ddb1ec113afc74b2742ef938020d3335a/testdata/chained)
  has valid activity-plus-settings and corrupted-chain cases. The project is MIT-licensed, but fixture provenance needs
  confirmation before redistribution.
- [`muktihari/fit`](https://github.com/muktihari/fit/blob/98687ab34559923dd6b8f2188aeb52fbabfcb7a0/testdata/chained_Activity_activity_poolswim.fit)
  has a BSD-3-Clause activity-plus-activity chain. Its per-file origin also needs confirmation before redistribution.

Public availability is not redistribution permission. Until provenance is clear, external corpora may drive local
research but not committed fixtures or required CI.
