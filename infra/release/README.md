# Local HASS release candidates

The release commands inspect existing build outputs and assemble local candidates. They do not contact a registry,
publish, or qualify a Home Assistant installation. Candidate manifests use `status: candidate` with no acceptance
results. The [publication plan](../../docs/plans/device-expansion-and-publication.md#hass-experimental-release) owns the
later gates.

## Native image inventories and build receipts

`packages.SYSTEM.garmin-hass-app-sbom` produces SPDX 2.3 JSON for the corresponding production image. Build it on the
matching Linux architecture with the worker limits in
[development safeguards](../../docs/development.md#execution-safeguards):

```sh
nix build .#garmin-hass-app-image .#garmin-hass-app-sbom \
    --no-link --print-out-paths --max-jobs 1 --cores 2 --no-update-lock-file
```

Production native and WebAssembly builds embed cargo-auditable dependency metadata. The pinned Syft scanner reads the
finished image; the generator independently hashes every regular layer file, compares the actual Nix store paths with
the producing runtime closure, and checks the native Rust dependency inventory. It reads the frontend's embedded Rust
graph directly and adds the reviewed asset inventory with its original license text. Generation and verification share
the expected package records and relationships: changed names, versions, package URLs, license text, duplicate
identities, and missing file/dependency associations fail. The generator accepts the Nix-produced layer format;
whiteouts, hardlinks, and conflicting entries are rejected. Archives are limited to 4 GiB compressed and uncompressed,
with 100,000 members per archive and across image layers. JSON metadata has separate byte limits.

The SPDX document describes the original archive and records its SHA-256. Registry manifest/index digests remain
separate. Generation does not run image contents or contact a registry. Syft needs an uncompressed, seekable Docker
archive: compressed input is expanded inside private scratch under the Nix build directory or repository `.tmp`, then
removed on success or failure. Published archive bytes are unchanged. Creation time uses the builder's source epoch;
repeated generation is covered by a real scanner test. These inventories do not establish license/source redistribution
compliance or signed build provenance.

Record the resulting files from a clean source checkout on the same native Linux platform:

```sh
just release::record-build \
    --source . \
    --archive /nix/store/IMAGE-garmin-toolkit-hass.tar.gz \
    --sbom /nix/store/SBOM-garmin-hass-image.spdx.json \
    --derivation /nix/store/IMAGE-garmin-toolkit-hass.tar.gz.drv \
    --output .tmp/hass-native-build
```

Use the evaluated image derivation path from `nix eval --raw .#garmin-hass-app-image.drvPath`; the placeholders above
are not literal commands. A producing GitHub job supplies `--workflow-run` with its actual run URL; local builds leave
it absent. Recording checks the derivation's native system and exact output path, source/image versions and labels, and
SBOM contents before exporting the unchanged archive, SPDX file, and `native-build.json`. The filesystem-backed
prescription rejects missing, empty, or unexpected output. Existing destinations are never overwritten and partial
output is discarded.

Native receipts deliberately omit registry manifest/index digests. A clean branch build records `source.tag: null`; the
exact version tag is recorded when it points at that commit. The later registry workflow must combine native receipts
with independently verified registry descriptors before constructing the assembly receipt below. Assembly still requires
its clean detached release tag reachable from main. Unsigned receipts cannot prove the source of a build; signed
workflow provenance remains a separate publication gate.

CI's `hass-native-image` matrix runs AMD64 and ARM64 builders independently. Each caps the actual Nix daemon at two CPUs
and 6 GiB and checks both limits before starting builds. Its `garmin-hass-native-ARCH` artifact contains the original
image archive, SPDX inventory, and unsigned native receipt. These build artifacts are not published releases or HAOS
runtime qualification.

## Inputs

Use a clean detached `vVERSION` source checkout reachable from fetched `origin/main` or local `main`. Versions are
numeric `MAJOR.MINOR.PATCH`; prerelease/build suffixes need a later version contract. Workspace/app/image versions and
the receipt's source commit and lockfile hash must agree. Fetch the release tag and main history before assembly.

The build receipt uses the `assembly` definition in [hass-release.schema.json](hass-release.schema.json):

- `schema_version: 1` and the release `version`;
- `source`: repository URL, `vVERSION` tag, full commit, and SHA-256 of `flake.lock`;
- `image`: canonical GHCR repository and multi-platform index digest;
- `platforms`: one entry per selected `amd64`/`aarch64` architecture, with platform manifest and image-config digests,
  archive/SBOM SHA-256 values recorded by the producing job, native builder system, Nix derivation path, and producing
  GitHub workflow-run URL (`null` for a local build). Assembly checks those producer hashes before accepting the inputs;
  a local build must not invent a GitHub run identity.

Each platform must supply the unchanged Docker image archive and a nonempty SPDX 2.3 package-inventory JSON file. The
filenames are defined by [hass-files.toml](hass-files.toml). The artifact input directory contains only those platform
files. No symlinks, empty files, subdirectories, extra platforms, or unexpected filenames are accepted.

```sh
just release::assemble \
    --source /path/to/clean-tag-checkout \
    --receipt .tmp/native-builds.json \
    --artifacts .tmp/native-build-artifacts \
    --output .tmp/hass-release-candidate
```

The command uses the pinned Nix release tool, Tyro CLI, and locked Python dependencies. `dirspec` is included. Its
filesystem prescription is [hass-release.dirspec.toml](hass-release.dirspec.toml); exact version/platform filenames and
roles are additionally checked against the manifest. QA checks prescription syntax and exercises the real checker.

## Output and verification

Assembly copies the platform bytes, reads license/copyright/notices and the HASS license bundle from committed Git
blobs, and makes the source archive with `git archive` at the exact admitted commit. It creates canonical `release.json`
and GNU-compatible `SHA256SUMS`, verifies everything in a temporary sibling, then renames the completed directory into
place. A failure cleans the temporary candidate; existing output is never replaced.

The printed `release.json` SHA-256 is the local verification anchor. Obtain that expected value independently of the
directory being verified. For publication, the attested checksum list will provide it after provenance verification. Do
not compute a new expected value from a suspected download and treat that as origin verification.

```sh
just release::verify \
    --directory .tmp/hass-release-candidate \
    --manifest-sha256 EXPECTED_RELEASE_JSON_SHA256 \
    --version 0.1.0 \
    --commit EXPECTED_FULL_SOURCE_COMMIT
```

The verifier checks the manifest hash first, then its schema/source identity, canonical encoding, exact inventory,
nonempty regular files, sizes, every payload hash, and the complete checksum list. It checks the image archive's
platform, Home Assistant labels, version tag, and actual configuration digest through the shared HAOS image reader.
`SHA256SUMS` includes `release.json`; neither file attempts to hash itself. It can also be checked with
`sha256sum --check SHA256SUMS` from the candidate directory after its expected origin has been established.

The local contract records index/platform manifest digests from build receipts. It does not query a registry, verify
signed provenance, prove the image was built from the declared source, certify SBOM completeness, or test image
startup/HAOS/USB. Those checks remain mandatory publication work. Candidate verification cannot set published or passed
acceptance status. Archive checksums, registry digests, config digests, and Nix store paths remain separate fields.

The JSON schema, inventory TOML, and Python are formatted/linted through repository QA. The plain-text Jinja checksum
template is compiled/rendered in regression tests and checked with GNU `sha256sum`; it has no separate formatter.
