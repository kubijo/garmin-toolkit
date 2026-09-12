//! Shared egui presentation components.

#![expect(
    clippy::multiple_crate_versions,
    reason = "egui and resvg require compatible bitflags, kurbo, and miniz_oxide versions"
)]

pub mod activity;
pub mod brand;
pub mod button;
pub mod capacity;
pub mod device;
pub mod file_import;
mod header_selector;
pub mod icons;
pub mod image_crop;
pub mod images;
pub mod input;
pub mod modal;
pub mod notification;
pub mod offline;
pub mod path;
pub mod profile;
pub mod profile_settings;
pub mod progress;
pub mod select;
pub mod shell;
mod size;
mod text;
pub mod theme;
pub mod typography;
pub mod workspace;

pub use size::Size;

/// Installs shared loaders and styling.
pub fn install(ctx: &egui::Context) {
    install_assets(ctx);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        theme::apply_palette(style, &garmin_color::theme::GRAY_100);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        theme::apply_palette(style, &garmin_color::theme::GRAY_10);
    });
    ctx.set_theme(egui::ThemePreference::System);
}

/// Installs bundled fonts and asset loaders without changing the host style.
pub fn install_assets(ctx: &egui::Context) {
    typography::install(ctx);
    egui_extras::install_image_loaders(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_installation_includes_raster_decoding() {
        let context = egui::Context::default();

        install_assets(&context);

        assert!(
            context.is_loader_installed(egui_extras::loaders::image_loader::ImageCrateLoader::ID)
        );
    }
}
