# Garmin Personal API contract

`just connect-api::test` verifies the pinned descriptor, generates the complete Rust client with
`openapi-to-rust 0.23.0`, compiles it and runs loopback HTTP tests. No account credentials or Garmin service calls are
needed. Cargo writes generated Rust under its target directory; authored tooling and tests live here.

`openapi.json` is the byte-exact [community snapshot](https://github.com/Likenttt/connect-personal-api-ts) of Garmin's
[official Swagger descriptor](https://developer.garmin.com/gc-developer-program/personal/swagger-docs/). `input.json`
pins its upstream commit and SHA-256. Every build verifies the checksum; the Nix `garmin-connect` check also fetches
that immutable upstream artifact and compares it with the checked-in copy. The upstream repository supplies the snapshot
under Apache-2.0; see [its notice](https://github.com/Likenttt/connect-personal-api-ts/blob/main/NOTICE).

Update with `just connect-api::update FULL_UPSTREAM_COMMIT EXPECTED_SHA256`. Review the descriptor diff and test results
before accepting it. Generator updates are explicit Cargo dependency and lockfile changes.

`build.rs` keeps two generated contracts: the raw snapshot and a corrected binary contract. The correction declares the
ZIP payload as binary and promotes its old `default` response to `200`, matching current official documentation. Tests
cover typed JSON, pagination, malformed responses, byte-exact downloads, HTTP errors and buffered size limits. Two tests
record generator limitations: the raw schema discards downloads, and HRV's documented `204` becomes an error. Streaming
consumers must enforce their own total byte limit.

The application transport in [`crates/garmin-connect`](../../crates/garmin-connect) uses the same pinned input and
checksum loader and binary correction, generating the profile, activity-list, activity-detail and download operations it
uses. This tooling workspace verifies the full descriptor. Remaining live qualification and synchronization work is in
the [implementation plan](../../docs/plans/garmin-connect-import.md).
