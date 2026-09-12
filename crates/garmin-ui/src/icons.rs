//! Validated, tintable SVG icons.

use cint::ColorInterop;
use egui::{Image, ImageSource, Pos2, Rect, Response, Ui, Vec2, load::Bytes};
use garmin_color::Color;
use std::fmt;

/// An icon asset source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Source {
    Phosphor,
    Local,
}

/// A validated monochrome SVG asset.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Icon {
    name: &'static str,
    source: Source,
    uri: &'static str,
    svg: &'static [u8],
}

impl Icon {
    const fn new(
        name: &'static str,
        source: Source,
        uri: &'static str,
        svg: &'static [u8],
    ) -> Self {
        Self {
            name,
            source,
            uri,
            svg,
        }
    }

    #[must_use]
    pub const fn as_name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn source(&self) -> Source {
        self.source
    }

    #[must_use]
    pub const fn as_svg(&self) -> &'static [u8] {
        self.svg
    }

    /// Creates an egui image tinted with an application color.
    pub fn image(self, color: Color) -> Image<'static> {
        self.mask().tint(color.into_cint())
    }

    pub(crate) fn mask(self) -> Image<'static> {
        Image::new(ImageSource::Bytes {
            uri: self.uri.into(),
            bytes: Bytes::Static(self.svg),
        })
    }
}

/// Inputs for rendering one icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Props {
    pub icon: Icon,
    pub size: f32,
    pub color: Color,
}

impl Props {
    /// Renders the icon.
    pub fn show(self, ui: &mut Ui) -> Response {
        ui.add(
            self.icon
                .image(self.color)
                .fit_to_exact_size(Vec2::splat(self.size)),
        )
    }

    /// Paints the icon around an existing layout position.
    pub fn paint_at(self, ui: &Ui, center: Pos2) {
        let rect = Rect::from_center_size(center, Vec2::splat(self.size));
        self.icon.image(self.color).paint_at(ui, rect);
    }
}

impl fmt::Debug for Icon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Icon")
            .field("name", &self.name)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

macro_rules! icon_catalog {
    (
        phosphor { $($phosphor_name:ident => $phosphor_svg:expr,)* }
        local { $($local_name:ident => $local_svg:expr,)* }
    ) => {
        $(
            #[doc = concat!("Phosphor `", stringify!($phosphor_name), "` icon.")]
            pub const $phosphor_name: Icon = Icon::new(
                stringify!($phosphor_name),
                Source::Phosphor,
                concat!("bytes://garmin-ui/icons/", stringify!($phosphor_name), ".svg"),
                include_bytes!(concat!(env!("OUT_DIR"), "/icons/", stringify!($phosphor_name), ".svg")),
            );
        )*
        $(
            #[doc = concat!("Local `", stringify!($local_name), "` icon.")]
            pub const $local_name: Icon = Icon::new(
                stringify!($local_name),
                Source::Local,
                concat!("bytes://garmin-ui/icons/", stringify!($local_name), ".svg"),
                include_bytes!(concat!(env!("OUT_DIR"), "/icons/", stringify!($local_name), ".svg")),
            );
        )*

        /// Every shipped icon, ordered by constant name.
        pub const ALL: &[Icon] = &[$($phosphor_name,)* $($local_name,)*];
    };
}

include!("icons/catalog.rs");

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_names_and_svg_payloads_are_unique() {
        let mut names = HashSet::new();
        let mut payloads = HashSet::new();

        for icon in ALL {
            assert!(names.insert(icon.name));
            assert!(payloads.insert(icon.svg));
            assert!(icon.svg.starts_with(b"<svg"));
        }
    }
}
