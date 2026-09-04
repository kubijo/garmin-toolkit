//! Shared application branding.

use std::num::NonZeroU16;

use egui::IconData;

/// Rasterizes the embedded application icon for native surfaces.
/// # Panics
#[must_use]
pub fn icon() -> IconData {
    let size = NonZeroU16::new(256).expect("256 is non-zero");
    let raster = garmin_brand::rasterize_icon(size).expect("the checked-in brand icon rasterizes");
    let (rgba, width, height) = raster.into_parts();

    IconData {
        rgba,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_icon_decodes() {
        assert!(!super::icon().is_empty());
    }
}
