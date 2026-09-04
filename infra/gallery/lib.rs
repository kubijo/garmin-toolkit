//! Reloadable Garmin Toolkit gallery scenes.

#![expect(
    clippy::multiple_crate_versions,
    reason = "the isolated gallery combines terminal, desktop, capture, and SVG stacks"
)]
#![expect(
    missing_docs,
    reason = "rs-gallery generates undocumented dynamic-library exports"
)]

mod terminal_input;

gallery::scenes_dylib!();
