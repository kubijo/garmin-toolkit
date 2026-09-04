//! Carbon-derived color swatches; see the third-party notices.

#![expect(
    clippy::unreadable_literal,
    reason = "six-digit literals preserve the upstream RRGGBB notation"
)]

use crate::Color;

macro_rules! rgb {
    ($value:literal) => {
        Color::from_u32(($value << 8) | 0xff)
    };
}

macro_rules! swatch {
    ($name:ident, $label:literal, [$($grade:ident = $value:literal),+ $(,)?]) => {
        #[doc = concat!($label, " color grades.")]
        pub mod $name {
            use crate::Color;

            $(
                #[doc = concat!($label, " ", stringify!($grade), ".")]
                pub const $grade: Color = rgb!($value);
            )+

            /// Colors ordered from grade 10 through 100.
            pub const ALL: [Color; 10] = [$($grade),+];
        }
    };
}

/// Opaque black.
pub const BLACK: Color = rgb!(0x000000);
/// Opaque white.
pub const WHITE: Color = rgb!(0xffffff);
/// Transparent black.
pub const TRANSPARENT: Color = Color::from_u32(0);
/// Application action color.
pub const ACTION: Color = rgb!(0x017cc2);
/// Hovered application action color.
pub const ACTION_HOVER: Color = rgb!(0x00539a);
/// Pressed application action color.
pub const ACTION_ACTIVE: Color = rgb!(0x003a6d);

swatch!(
    yellow,
    "Yellow",
    [
        G10 = 0xfcf4d6,
        G20 = 0xfddc69,
        G30 = 0xf1c21b,
        G40 = 0xd2a106,
        G50 = 0xb28600,
        G60 = 0x8e6a00,
        G70 = 0x684e00,
        G80 = 0x483700,
        G90 = 0x302400,
        G100 = 0x1c1500,
    ]
);

swatch!(
    orange,
    "Orange",
    [
        G10 = 0xfff2e8,
        G20 = 0xffd9be,
        G30 = 0xffb784,
        G40 = 0xff832b,
        G50 = 0xeb6200,
        G60 = 0xba4e00,
        G70 = 0x8a3800,
        G80 = 0x5e2900,
        G90 = 0x3e1a00,
        G100 = 0x231000,
    ]
);

swatch!(
    red,
    "Red",
    [
        G10 = 0xfff1f1,
        G20 = 0xffd7d9,
        G30 = 0xffb3b8,
        G40 = 0xff8389,
        G50 = 0xfa4d56,
        G60 = 0xda1e28,
        G70 = 0xa2191f,
        G80 = 0x750e13,
        G90 = 0x520408,
        G100 = 0x2d0709,
    ]
);

swatch!(
    magenta,
    "Magenta",
    [
        G10 = 0xfff0f7,
        G20 = 0xffd6e8,
        G30 = 0xffafd2,
        G40 = 0xff7eb6,
        G50 = 0xee5396,
        G60 = 0xd02670,
        G70 = 0x9f1853,
        G80 = 0x740937,
        G90 = 0x510224,
        G100 = 0x2a0a18,
    ]
);

swatch!(
    purple,
    "Purple",
    [
        G10 = 0xf6f2ff,
        G20 = 0xe8daff,
        G30 = 0xd4bbff,
        G40 = 0xbe95ff,
        G50 = 0xa56eff,
        G60 = 0x8a3ffc,
        G70 = 0x6929c4,
        G80 = 0x491d8b,
        G90 = 0x31135e,
        G100 = 0x1c0f30,
    ]
);

swatch!(
    blue,
    "Blue",
    [
        G10 = 0xedf5ff,
        G20 = 0xd0e2ff,
        G30 = 0xa6c8ff,
        G40 = 0x78a9ff,
        G50 = 0x4589ff,
        G60 = 0x0f62fe,
        G70 = 0x0043ce,
        G80 = 0x002d9c,
        G90 = 0x001d6c,
        G100 = 0x001141,
    ]
);

swatch!(
    cyan,
    "Cyan",
    [
        G10 = 0xe5f6ff,
        G20 = 0xbae6ff,
        G30 = 0x82cfff,
        G40 = 0x33b1ff,
        G50 = 0x1192e8,
        G60 = 0x0072c3,
        G70 = 0x00539a,
        G80 = 0x003a6d,
        G90 = 0x012749,
        G100 = 0x061727,
    ]
);

swatch!(
    teal,
    "Teal",
    [
        G10 = 0xd9fbfb,
        G20 = 0x9ef0f0,
        G30 = 0x3ddbd9,
        G40 = 0x08bdba,
        G50 = 0x009d9a,
        G60 = 0x007d79,
        G70 = 0x005d5d,
        G80 = 0x004144,
        G90 = 0x022b30,
        G100 = 0x081a1c,
    ]
);

swatch!(
    green,
    "Green",
    [
        G10 = 0xdefbe6,
        G20 = 0xa7f0ba,
        G30 = 0x6fdc8c,
        G40 = 0x42be65,
        G50 = 0x24a148,
        G60 = 0x198038,
        G70 = 0x0e6027,
        G80 = 0x044317,
        G90 = 0x022d0d,
        G100 = 0x071908,
    ]
);

swatch!(
    gray,
    "Gray",
    [
        G10 = 0xf4f4f4,
        G20 = 0xe0e0e0,
        G30 = 0xc6c6c6,
        G40 = 0xa8a8a8,
        G50 = 0x8d8d8d,
        G60 = 0x6f6f6f,
        G70 = 0x525252,
        G80 = 0x393939,
        G90 = 0x262626,
        G100 = 0x161616,
    ]
);

swatch!(
    cool_gray,
    "Cool gray",
    [
        G10 = 0xf2f4f8,
        G20 = 0xdde1e6,
        G30 = 0xc1c7cd,
        G40 = 0xa2a9b0,
        G50 = 0x878d96,
        G60 = 0x697077,
        G70 = 0x4d5358,
        G80 = 0x343a3f,
        G90 = 0x21272a,
        G100 = 0x121619,
    ]
);

swatch!(
    warm_gray,
    "Warm gray",
    [
        G10 = 0xf7f3f2,
        G20 = 0xe5e0df,
        G30 = 0xcac5c4,
        G40 = 0xada8a8,
        G50 = 0x8f8b8b,
        G60 = 0x726e6e,
        G70 = 0x565151,
        G80 = 0x3c3838,
        G90 = 0x272525,
        G100 = 0x171414,
    ]
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_orders_grades_from_light_to_dark() {
        assert_eq!(blue::ALL.len(), 10);
        assert_eq!(blue::ALL[0], blue::G10);
        assert_eq!(blue::ALL[5], blue::G60);
        assert_eq!(blue::ALL[9], blue::G100);
    }
}
