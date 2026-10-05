//! Opt-in GPU rendering probe; never used by the desktop package.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    garmin_desktop::render_probe::run()
}
