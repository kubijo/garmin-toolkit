# FIT corpus

## Implemented coverage

[`garmin-fit` fixtures](../../crates/garmin-fit/src/fixture.rs) generate activities, chained activity/settings
sequences, and a Course. [`garmin-fit` tests](../../crates/garmin-fit/src/lib.rs) cover activity normalization,
independently absent sensor values, creator/envelope/timestamp validation, separate chained activities, preserved
settings, and rejected unsupported or corrupt sequences. [Course tests](../../crates/garmin-fit/src/course.rs) check
semantic round trips, message anatomy, corruption, and unresolved geometry.

Independent fixtures and [boundary tests](../../crates/garmin-fit/src/integrity_tests.rs) characterize:

- [Integrity and multisession](../../crates/garmin-fit/src/fixture/integrity.rs): 12/14-byte headers, every proper
  prefix and single-bit mutation of the minimal recording, isolated CRC errors, length mismatches, and corrupt chained
  tails. Activity and Course decoding share length/CRC checks; two sessions in one sequence are rejected.
- [Health](../../crates/garmin-fit/src/fixture/health.rs) and
  [workouts](../../crates/garmin-fit/src/fixture/workout.rs): monitoring heart rate, sleep transitions, HRV
  intervals/missing values, and a timed two-step workout. All decode; their file types are rejected by the activity
  normalizer.
- [Developer fields](../../crates/garmin-fit/src/fixture/developer.rs): complete metadata, missing descriptions, and
  missing developer IDs. All are accepted without overriding native normalized values. Undescribed custom values are
  omitted from decoded output; described values decode even without a developer ID.

[Importer tests](../../crates/garmin-importer/tests/fit_import.rs) verify exact original-byte preservation, stable
retries, and no partial activities from rejected chains. These synthetic examples neither qualify device transfers nor
add normalization support. Single-session activities are normalized; settings are preservation-only. The
[data contract](../architecture/data.md) owns those invariants.

`rustyfit` 0.10.2 rejects valid 14-byte files with an omitted (zero) header CRC when their file CRC covers the complete
sequence, as required by the [protocol](https://developer.garmin.com/fit/articles/fit-protocol/fit_protocol.html). Those
inputs remain preserved parser rejections; this coverage does not add support for normalizing them.

Expectations must come from the synthetic definition, not parser output or Garmin SDK fixtures. Real device files stay
ignored; durable regressions need synthetic replacements or approved public fixtures.

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

### Suites to shadow

- [Garmin Python](https://github.com/garmin/fit-python-sdk/blob/e809dbc6acfb4568259625afc7b04724fda9c4e0/tests/test_decoder.py)
  (FIT Protocol License): integrity, chaining, invalid values, field sizes, and developer data.
- [muktihari](https://github.com/muktihari/fit/blob/7addcbcbb7a3d99e88db5568bf691308ff4d66cb/decoder/decoder_test.go)
  (BSD-3-Clause): malformed definitions, compressed timestamps, big-endian short fields, and missing developer metadata.

Recreate named behaviors with independent bytes and expected values. These implementation suites are not a complete
certification oracle: Python's compressed-timestamp rejection is an implementation limit, not a protocol rule.
`muktihari/testdata` mixes SDK samples, forum recordings, and fuzz inputs; verify per-file provenance before reuse.

### Optional recordings

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
