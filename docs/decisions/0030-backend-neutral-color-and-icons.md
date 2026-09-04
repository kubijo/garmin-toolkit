# 0030: Backend-neutral color and icons

## Decision

Use one application-owned `Color` across data and presentation. `garmin-color` owns stable encoded-sRGB RGBA, parsing,
serialization, alpha, and perceptual operations through [`palette`](https://docs.rs/palette); implement no color-space
math locally.

Use [`cint`](https://docs.rs/cint) as the renderer boundary. `Color` converts losslessly to and from its encoded-sRGB
alpha type. Backend colors never enter storage or public component/theme/icon APIs. Persistence and calculations remain
unpremultiplied even when renderers premultiply.

Store profile accents as `Color`; resolve semantic themes through explicit OKLab/OKLCH operations. Components perform no
local RGB arithmetic. Builders may accept conversions, but stored fields remain concrete.

Use one `Icon` for curated and local SVGs. Generate named constants for completion and compile-time references. Icons
contain monochrome filled paths; build validation rejects strokes, invalid or empty geometry, unsupported features,
duplicate names, and non-monochrome paint.

Pin [`phosphor-svgs`](https://docs.rs/phosphor-svgs) as a build dependency. Curated upstream and local SVGs share one
validator and generator; production imports only generated constants.

The accepted 0.3.0 package contains Phosphor Core 2.1.1, six weights, filled `currentColor` paths, and no Rust
dependencies. Re-audit upgrades and preserve both MIT notices.

Rasterize icons by physical size and tint at the renderer boundary. Gallery scenes exercise every glyph across sizes,
themes, accents, and states. Replace rasterization only after visual tests prove equivalent small-size antialiasing.

## Why

Backend types would couple stored data and components to one renderer. `cint` supplies interchange, `palette` reviewed
math, and generated SVGs one representation and failure policy for upstream and local icons.

## Consequences

Headless code needs no egui. Opaque colors cross bit-for-bit; renderer premultiplication may round translucent output
without changing canonical `Color`.
