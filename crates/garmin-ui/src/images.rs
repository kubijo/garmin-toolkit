//! Untinted image assets.

use egui::{ImageSource, Rect, Ui, Vec2, load::Bytes};

/// A bundled image rendered with its original colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Image {
    uri: &'static str,
    bytes: &'static [u8],
    aspect_ratio: f32,
}

impl Image {
    const fn new(uri: &'static str, bytes: &'static [u8], aspect_ratio: f32) -> Self {
        Self {
            uri,
            bytes,
            aspect_ratio,
        }
    }

    #[must_use]
    pub fn size_for_height(self, height: f32) -> Vec2 {
        egui::vec2(height * self.aspect_ratio, height)
    }

    /// Paints the image into an allocated rectangle.
    pub fn paint_at(self, ui: &Ui, rect: Rect) {
        egui::Image::new(ImageSource::Bytes {
            uri: self.uri.into(),
            bytes: Bytes::Static(self.bytes),
        })
        .paint_at(ui, rect);
    }
}

/// Flag of the United Kingdom.
pub const UNITED_KINGDOM: Image = Image::new(
    "bytes://garmin-ui/images/flag-united-kingdom.svg",
    include_bytes!("../assets/images/flag-united-kingdom.svg"),
    4.0 / 3.0,
);

/// Flag of Czechia.
pub const CZECHIA: Image = Image::new(
    "bytes://garmin-ui/images/flag-czechia.svg",
    include_bytes!("../assets/images/flag-czechia.svg"),
    4.0 / 3.0,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_images_are_valid_svg() {
        for image in [UNITED_KINGDOM, CZECHIA] {
            usvg::Tree::from_data(image.bytes, &usvg::Options::default())
                .expect("bundled image SVG must parse");
        }
    }
}
