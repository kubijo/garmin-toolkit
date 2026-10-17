# Local changes to fast-mvt 0.6.2

- `MvtReaderRef::with_decode_options` applies Buffa's allocation limits before constructing views.
  `new` now uses the default decode options, including the element-memory limit.
- Polygon signed area uses `i128`: `i32` coordinates can overflow an `i64` accumulator even in
  a short ring. The sign determines exterior/hole grouping.

Application regression tests cover both changes. Remove the patch when an upstream release
provides equivalent bounded decoding and overflow-safe area calculation.
