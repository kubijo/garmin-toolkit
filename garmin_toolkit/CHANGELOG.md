# Changelog

## 0.1.4

- Require signed release baselines for AMD64 and ARM64 upgrade qualification.
- Add recoverable publication-history checkpoints and checks for recovery, withdrawal, and replacement after cutover.
- Clarify release-specific compatibility and recovery guidance.

## 0.1.3

- Add experimental ARM64 images alongside AMD64.
- Qualify restart, reboot, backup/restore, and reinstall on both architectures.
- Strengthen signed release verification, interrupted-publication recovery, and withdrawal checks.
- Refresh application and runtime dependencies.

## 0.1.2

- Refresh application and runtime dependencies after the first public release.
- Qualify upgrades and backup recovery from the signed v0.1.1 image using populated profile, route, and Course data.
- Verify public installation links and strengthen release provenance and lifecycle checks.

## 0.1.1

- Fix registry credentials for signed OCI publication.
- Report build, source export, and HAOS acceptance progress and timings.

## 0.1.0

- Experimental Home Assistant ingress app for profiles, GPX routes, FIT Course generation, and downloads.
- Persistent private app data and cold Supervisor backup support.
- USB access for the packaged Garmin adapter; supported device operations require release-specific hardware evidence.

Version 0.1.1 is the first public experimental AMD64 release. See [the quickstart](DOCS.md) for qualified Home Assistant
versions and device-operation limits.
