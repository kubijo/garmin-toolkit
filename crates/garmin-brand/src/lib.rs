//! Canonical brand assets and backend-neutral rasterization.

use std::num::NonZeroU16;

use resvg::{tiny_skia, usvg};
use thiserror::Error;

/// Canonical application icon SVG.
pub const ICON_SVG: &[u8] = include_bytes!("../assets/icon.svg");

/// Unpremultiplied encoded-sRGB raster.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Raster {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl Raster {
    #[must_use]
    pub fn as_rgba(&self) -> &[u8] {
        &self.rgba
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, u32, u32) {
        (self.rgba, self.width, self.height)
    }
}

/// Rasterizes the canonical icon into a square RGBA image.
/// # Errors
/// [`enum@Error`] when the embedded SVG is invalid or the raster cannot be allocated.
pub fn rasterize_icon(size: NonZeroU16) -> Result<Raster, Error> {
    let tree = usvg::Tree::from_data(ICON_SVG, &usvg::Options::default())?;
    let dimension = u32::from(size.get());
    let mut pixmap = tiny_skia::Pixmap::new(dimension, dimension).ok_or(Error::Allocation {
        width: dimension,
        height: dimension,
    })?;
    let source = tree.size();
    let dimension_float = f32::from(size.get());
    let scale = (dimension_float / source.width()).min(dimension_float / source.height());
    let offset_x = source.width().mul_add(-scale, dimension_float) / 2.0;
    let offset_y = source.height().mul_add(-scale, dimension_float) / 2.0;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, offset_x, offset_y);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();

    Ok(Raster {
        rgba,
        width: dimension,
        height: dimension,
    })
}

/// Brand asset failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("the embedded brand icon is invalid: {0}")]
    Svg(#[from] usvg::Error),
    #[error("could not allocate a {width}x{height} brand raster")]
    Allocation {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    #[test]
    fn icon_raster_has_requested_geometry_and_visible_pixels() {
        let icon = super::rasterize_icon(NonZeroU16::new(32).expect("32 is non-zero"))
            .expect("the checked-in brand icon rasterizes");

        assert_eq!(icon.width(), 32);
        assert_eq!(icon.height(), 32);
        assert_eq!(icon.as_rgba().len(), 32 * 32 * 4);
        assert!(
            icon.as_rgba()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
    }
}
