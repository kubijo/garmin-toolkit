# Third-party notices

## Carbon Design System colors and Gray 100 theme

- Source: `@carbon/colors` 11.57.0 and `@carbon/themes` 11.80.0 at Carbon revision
  [`7518c84`](https://github.com/carbon-design-system/carbon/tree/7518c84ffd00f22434fe19d83119692c12fccb2f).
- Integrity: colors `sha512-ji8xOBiCzCmeR/Pg/rDbnViVYDnjSVWRclylQKqeAQNUIsfIWDB69xq/o33lba3vVccVDNrGcGkrVg5My0Ct7g==`;
  themes `sha512-SqtfqwReq1SRrBoC121Bllrh0DvW2j/LWFo/zVn+lblh+AJlLZsmeBC9oyxphpp8GpHfLhjmeg1J2hfs0AVX4A==`.
- License: Apache-2.0; Copyright IBM Corp. 2015, 2018, 2025, 2026.
- Reused: the 12 base color ramps and selected Gray 100 semantic values.
- Modified: values are encoded as typed Rust constants, semantic roles use application names, and fractional alpha is
  rounded to the nearest eight-bit channel.
- Validation: registry integrity was verified; tests lock representative ramp and semantic mappings.

## Phosphor icons

- Source:
  [`phosphor-svgs` 0.3.0](https://github.com/Meadowsys/phosphor-svgs/tree/203cae3115c5596dd0dc791e84b7171d39ff346a),
  embedding [Phosphor Core 2.1.1](https://github.com/phosphor-icons/core/tree/7790ae563ef83ac36094b15b5e109d89fef09337).
- License: MIT; Copyright 2024 Meadowsys and Copyright 2023 Phosphor Icons.
- Reused: a curated set of regular-weight SVG glyphs listed in `crates/garmin-ui/src/icons/catalog.rs`.
- Modified: build validation resolves each monochrome glyph to white filled paths for runtime tinting.

## Flag images

- Source: [flag-icons 7.5.0](https://github.com/lipis/flag-icons/tree/7aa5b2bdddd570ece62c812c0cb588ccdc099e2e).
- License: MIT; Copyright 2013 Panayiotis Lipiridis.
- Reused: the 4:3 Czechia and United Kingdom SVGs; repository formatting changes markup only.

## Noto Sans

- Source: [Noto Sans 2.015](https://github.com/notofonts/latin-greek-cyrillic/releases/tag/NotoSans-v2.015).
- License: SIL Open Font License 1.1; Copyright 2022 The Noto Project Authors.
- Reused: the hinted static Regular and SemiBold TrueType faces without modification.
- Integrity: Regular `sha256:478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823`; SemiBold
  `sha256:a4e91fd530ac2b4ef5367240144ff37d7d65d66cf76f2e9a2187b93c676f92d0`.

Cargo dependency and asset license texts are harvested into the generated [desktop](assets/licenses/bundle-desktop.json)
and [HASS](assets/licenses/bundle-hass.json) bundles.
