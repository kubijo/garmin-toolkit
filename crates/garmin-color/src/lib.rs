//! Backend-neutral encoded-sRGB colors, swatches, and themes.

use std::{fmt, num::ParseIntError, str::FromStr};

use ::palette::{Clamp, FromColor, IntoColor, Mix, Oklab, Oklch, Srgb, Srgba};
use cint::{Alpha, ColorInterop, EncodedSrgb};
use thiserror::Error;

pub mod swatch;
pub mod theme;

/// Unpremultiplied encoded-sRGB RGBA.
#[garmin_macros::portable(copy, custom_debug, hash)]
#[repr(transparent)]
pub struct Color(u32);

impl Color {
    /// WCAG contrast ratio for opaque encoded-sRGB colors.
    #[must_use]
    pub fn contrast_ratio(self, other: Self) -> f32 {
        fn luminance(color: Color) -> f32 {
            let [r, g, b, _] = color.as_rgba();
            let linear = Srgb::new(r, g, b).into_format::<f32>().into_linear();
            0.2126 * linear.red + 0.7152 * linear.green + 0.0722 * linear.blue
        }
        let left = luminance(self);
        let right = luminance(other);
        (left.max(right) + 0.05) / (left.min(right) + 0.05)
    }

    /// Resolve a visible profile marker against its surface without changing stored color.
    #[must_use]
    pub fn contrasting_marker(self, surface: Self) -> Self {
        let color = self.with_alpha(255);
        if color.contrast_ratio(surface) >= 3.0 {
            return color;
        }
        let target =
            if swatch::WHITE.contrast_ratio(surface) > swatch::BLACK.contrast_ratio(surface) {
                swatch::WHITE
            } else {
                swatch::BLACK
            };
        for step in 0..=32u8 {
            let resolved = color.mix(target, f32::from(step) / 32.0).unwrap_or(target);
            if resolved.contrast_ratio(surface) >= 3.0 {
                return resolved;
            }
        }
        target
    }

    #[must_use]
    pub const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::from_rgba(red, green, blue, u8::MAX)
    }

    #[must_use]
    pub const fn from_rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self(u32::from_be_bytes([red, green, blue, alpha]))
    }

    #[must_use]
    pub const fn from_u32(rgba: u32) -> Self {
        Self(rgba)
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn as_rgba(&self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    #[must_use]
    pub const fn into_rgba(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    #[must_use]
    pub const fn with_alpha(self, alpha: u8) -> Self {
        let [red, green, blue, _] = self.into_rgba();
        Self::from_rgba(red, green, blue, alpha)
    }

    /// Mixes colors in `OKLab` and alpha linearly.
    /// # Errors
    /// [`ColorError::InvalidUnitInterval`] unless `amount` is finite and within `0..=1`.
    pub fn mix(self, other: Self, amount: f32) -> Result<Self, ColorError> {
        validate_unit_interval(amount)?;
        let [red, green, blue, alpha] = self.into_rgba();
        let [other_red, other_green, other_blue, other_alpha] = other.into_rgba();
        let left: Oklab = Srgb::new(red, green, blue)
            .into_format::<f32>()
            .into_color();
        let right: Oklab = Srgb::new(other_red, other_green, other_blue)
            .into_format::<f32>()
            .into_color();
        let mixed: Srgb<f32> = Srgb::from_color(left.mix(right, amount)).clamp();
        let mixed = mixed.into_format::<u8>();
        let mixed_alpha = Srgba::new(0.0, 0.0, 0.0, f32::from(alpha) / 255.0)
            .mix(
                Srgba::new(0.0, 0.0, 0.0, f32::from(other_alpha) / 255.0),
                amount,
            )
            .into_format::<u8, u8>()
            .alpha;
        Ok(Self::from_rgba(
            mixed.red,
            mixed.green,
            mixed.blue,
            mixed_alpha,
        ))
    }

    /// Replaces OKLCH lightness while preserving hue, chroma, and alpha.
    /// # Errors
    /// [`ColorError::InvalidUnitInterval`] unless `lightness` is finite and within `0..=1`.
    pub fn with_lightness(self, lightness: f32) -> Result<Self, ColorError> {
        validate_unit_interval(lightness)?;
        let [red, green, blue, alpha] = self.into_rgba();
        let mut oklch: Oklch = Srgb::new(red, green, blue)
            .into_format::<f32>()
            .into_color();
        oklch.l = lightness;
        let converted: Srgb<f32> = Srgb::from_color(oklch).clamp();
        let converted = converted.into_format::<u8>();
        Ok(Self::from_rgba(
            converted.red,
            converted.green,
            converted.blue,
            alpha,
        ))
    }
}

impl From<Color> for Alpha<EncodedSrgb<u8>> {
    fn from(color: Color) -> Self {
        let [red, green, blue, alpha] = color.into_rgba();
        Self {
            color: EncodedSrgb {
                r: red,
                g: green,
                b: blue,
            },
            alpha,
        }
    }
}

impl From<Alpha<EncodedSrgb<u8>>> for Color {
    fn from(value: Alpha<EncodedSrgb<u8>>) -> Self {
        Self::from_rgba(value.color.r, value.color.g, value.color.b, value.alpha)
    }
}

impl ColorInterop for Color {
    type CintTy = Alpha<EncodedSrgb<u8>>;
}

impl FromStr for Color {
    type Err = ColorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hex) = value.strip_prefix('#') else {
            return Err(ColorError::MissingHash);
        };
        let rgba = match hex.len() {
            6 => (u32::from_str_radix(hex, 16)? << 8) | u32::from(u8::MAX),
            8 => u32::from_str_radix(hex, 16)?,
            length => return Err(ColorError::InvalidLength(length + 1)),
        };
        Ok(Self::from_u32(rgba))
    }
}

impl fmt::Debug for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Color")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{:08x}", self.0)
    }
}

fn validate_unit_interval(value: f32) -> Result<(), ColorError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ColorError::InvalidUnitInterval)
    }
}

/// Invalid color text or operation input.
#[derive(Debug, Error)]
pub enum ColorError {
    #[error("color must start with '#'")]
    MissingHash,
    #[error("color has invalid length {0}; expected 7 or 9 bytes")]
    InvalidLength(usize),
    #[error("color contains invalid hexadecimal: {0}")]
    InvalidHex(#[from] ParseIntError),
    #[error("color operation value must be finite and within 0..=1")]
    InvalidUnitInterval,
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_markers_remain_visible_in_both_themes() {
        for palette in [super::theme::GRAY_100, super::theme::GRAY_10] {
            let surface = palette.surfaces().layer(super::theme::Level::One);
            for color in [
                super::swatch::BLACK,
                super::swatch::WHITE,
                super::swatch::ACTION,
                super::swatch::magenta::G50,
            ] {
                assert!(color.contrasting_marker(surface).contrast_ratio(surface) >= 3.0);
            }
        }
    }

    use super::*;

    #[test]
    fn packed_text_and_cint_round_trip() -> Result<(), ColorError> {
        let color = Color::from_rgba(0x12, 0x34, 0x56, 0x78);

        assert_eq!(color.as_u32(), 0x1234_5678);
        assert_eq!(color.as_rgba(), [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(color.to_string(), "#12345678");
        assert_eq!(color.to_string().parse::<Color>()?, color);
        assert_eq!("#123456".parse::<Color>()?, color.with_alpha(u8::MAX));
        assert_eq!(Color::from(Alpha::<EncodedSrgb<u8>>::from(color)), color);
        Ok(())
    }

    #[test]
    fn text_parsing_is_strict() {
        assert!(matches!(
            "123456".parse::<Color>(),
            Err(ColorError::MissingHash)
        ));
        assert!(matches!(
            "#12345".parse::<Color>(),
            Err(ColorError::InvalidLength(6))
        ));
        assert!(matches!(
            "#gggggg".parse::<Color>(),
            Err(ColorError::InvalidHex(_))
        ));
    }

    #[test]
    fn perceptual_operations_validate_inputs_and_preserve_alpha() -> Result<(), ColorError> {
        let dark = Color::from_rgba(0x00, 0x43, 0xCE, 0x80);
        let light = Color::from_rgba(0xA6, 0xC8, 0xFF, 0x40);

        assert_eq!(dark.mix(light, 0.0)?, dark);
        assert_eq!(dark.mix(light, 0.5)?.as_rgba()[3], 0x60);
        assert_eq!(dark.mix(light, 1.0)?, light);
        assert_eq!(dark.with_lightness(0.8)?.as_rgba()[3], 0x80);
        assert!(dark.mix(light, f32::NAN).is_err());
        assert!(dark.with_lightness(1.1).is_err());
        Ok(())
    }
}
