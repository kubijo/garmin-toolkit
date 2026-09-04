//! Native and headless host for Garmin Toolkit scenes.

#![expect(
    clippy::multiple_crate_versions,
    reason = "the isolated gallery combines terminal, desktop, capture, and SVG stacks"
)]

fn main() -> gallery::eframe::Result {
    gallery::launch!(
        garmin_ui::install_assets,
        gallery::Settings::new(gallery::Renderer::Wgpu)
            .window_icon(garmin_ui::brand::icon())
            .controls_default_width(260.0)
            .collapsed(false)
    )
}
