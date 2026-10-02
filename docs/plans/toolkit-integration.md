# Cross-cutting integration gates

1. Verify packaged production/demo IDs, roots, binary names, populated-database startup, and HASS environment
   precedence.
2. Replace the global coverage floor with per-owner behavioral floors; preserve imported 80% requirements.
3. Audit crate boundaries after transaction work. Retain reusable capabilities and dependency-inversion boundaries; keep
   `garmin-progress` limited to observation/cancellation.
4. Measure translation adoption independently of catalog parity. Mechanically reject unregistered production copy;
   review English before translation.
5. Resolve the dependency advisory below without suppressing it.

Before release, keep one state format under [ADR 0037](../decisions/0037-single-execution-path.md). Close with
built-artifact, security-audit, and behavioral evidence.

## Translation gate

Use FormatJS `compile --ast --pseudo-locale en-XA`; preserve interpolation/plurals and reject conflicting IDs. Pseudo
mode is a non-persisted override. Test entry/exit, cached labels, locale-generated dates, glyph coverage,
wrapping/truncation, overlays, and narrow layouts in native/gallery and browser surfaces. Use semantic/rendering
evidence for egui canvas content. Prove detection with deliberately untranslated and clipped content; screenshots and
catalog parity alone do not measure adoption. Pin the renderer/browser through Nix.

## Dependency audit

Recorded 2026-09-26: [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) marks Bincode 1.3.3
unmaintained, with no patched version or specific vulnerability. Dependency path: exact Walkers 0.59 pin →
http-cache-reqwest 0.16 → http-cache 0.21. Lockfile updates alone do not resolve it.

Walkers 0.60.0 selects http-cache-reqwest 1.0.0-alpha.9 / Reqwest 0.13.5 (ours: 0.13.4), with Postcard cache storage and
optional legacy Bincode. Candidate only: the cache stack is prerelease and MVT dependencies also change. Check
[Walkers](https://github.com/podusowski/walkers/blob/main/Cargo.toml) and
[http-cache-reqwest](https://docs.rs/crate/http-cache-reqwest/1.0.0-alpha.9).

Update root/gallery locks, verify Bincode absence, rerun audit, native/WASM builds, and focused map tests before
adoption.
