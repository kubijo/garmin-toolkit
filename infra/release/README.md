# Local HASS release candidates

These development commands assemble and check local candidates. They do not build images, contact a registry, publish,
or qualify a Home Assistant installation. Output is explicitly `status: candidate` with no acceptance results. The
[publication plan](../../docs/plans/device-expansion-and-publication.md#hass-experimental-release) owns the later gates.

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

The command uses the pinned development shell, Tyro CLI, and locked Python dependencies. `dirspec` is required. Its
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
