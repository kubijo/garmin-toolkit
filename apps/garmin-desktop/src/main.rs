//! Native desktop target entry point.

#![expect(
    clippy::multiple_crate_versions,
    reason = "eframe and device/rendering adapters require incompatible transitive releases"
)]

fn main() -> Result<(), garmin_desktop::Error> {
    use clap::Parser as _;
    garmin_desktop::run_with_options(garmin_desktop::Options::parse())
}
