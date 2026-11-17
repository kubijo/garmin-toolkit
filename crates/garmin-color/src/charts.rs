//! Stable categorical identities for account trends, separate from profile accents.

use crate::Color;

#[must_use]
pub const fn foreground(dark: bool) -> Color {
    if dark {
        crate::swatch::gray::G10
    } else {
        crate::swatch::gray::G80
    }
}

#[must_use]
pub fn on_fill(fill: Color) -> Color {
    let light = crate::swatch::gray::G10;
    let dark = crate::swatch::gray::G100;
    if light.contrast_ratio(fill) > dark.contrast_ratio(fill) {
        light
    } else {
        dark
    }
}

#[must_use]
pub fn text(dark: bool, index: usize) -> Color {
    let background = if dark {
        crate::swatch::gray::G90
    } else {
        crate::swatch::WHITE
    };
    let base = series(dark, index);
    let target = foreground(dark);
    for step in 0..=32u8 {
        let candidate = base.mix(target, f32::from(step) / 32.0).unwrap_or(target);
        if candidate.contrast_ratio(background) >= 4.5 {
            return candidate;
        }
    }
    target
}

#[must_use]
pub const fn series(dark: bool, index: usize) -> Color {
    let colors = if dark {
        [
            Color::from_rgb(166, 200, 255),
            Color::from_rgb(61, 219, 217),
            Color::from_rgb(212, 187, 255),
            Color::from_rgb(255, 179, 184),
            Color::from_rgb(249, 225, 113),
            Color::from_rgb(168, 168, 168),
        ]
    } else {
        [
            Color::from_rgb(75, 137, 212),
            Color::from_rgb(39, 159, 152),
            Color::from_rgb(161, 108, 218),
            Color::from_rgb(207, 104, 125),
            Color::from_rgb(163, 130, 42),
            Color::from_rgb(111, 111, 111),
        ]
    };
    colors[index % colors.len()]
}

#[cfg(test)]
mod tests {
    #[test]
    fn heatmap_text_retains_readable_contrast_in_both_themes() {
        for dark in [false, true] {
            for index in 0..6 {
                let fill = super::series(dark, index);
                assert!(super::on_fill(fill).contrast_ratio(fill) >= 4.5);
                let background = if dark {
                    crate::swatch::gray::G90
                } else {
                    crate::swatch::WHITE
                };
                assert!(super::text(dark, index).contrast_ratio(background) >= 4.5);
            }
        }
    }
}
