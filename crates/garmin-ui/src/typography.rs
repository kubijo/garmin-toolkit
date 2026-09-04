//! Application fonts and weights; see the third-party notices.

use std::sync::{Arc, LazyLock};

use egui::{FontDefinitions, FontFamily, FontId, RichText, Style, TextStyle};

const REGULAR_DATA: &str = "NotoSans-Regular";
const SEMIBOLD_DATA: &str = "NotoSans-SemiBold";
const UBUNTU_LIGHT_DATA: &str = "Ubuntu-Light";
static REGULAR_FAMILY: LazyLock<FontFamily> =
    LazyLock::new(|| FontFamily::Name(REGULAR_DATA.into()));
static SEMIBOLD_FAMILY: LazyLock<FontFamily> =
    LazyLock::new(|| FontFamily::Name(SEMIBOLD_DATA.into()));

/// Supported application font weights.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Weight {
    Regular,
    SemiBold,
}

impl Weight {
    fn family(self) -> FontFamily {
        match self {
            Self::Regular => REGULAR_FAMILY.clone(),
            Self::SemiBold => SEMIBOLD_FAMILY.clone(),
        }
    }
}

#[must_use]
pub fn font(size: f32, weight: Weight) -> FontId {
    FontId::new(size, weight.family())
}

#[must_use]
pub fn semibold(text: impl Into<String>) -> RichText {
    RichText::new(text).family(Weight::SemiBold.family())
}

pub(crate) fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

pub(crate) fn apply(style: &mut Style) {
    for (text_style, font) in &mut style.text_styles {
        if *text_style != TextStyle::Monospace {
            font.family = if *text_style == TextStyle::Heading {
                Weight::SemiBold.family()
            } else {
                Weight::Regular.family()
            };
        }
    }
}

fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.remove(UBUNTU_LIGHT_DATA);
    fonts.font_data.insert(
        REGULAR_DATA.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSans-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        SEMIBOLD_DATA.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSans-SemiBold.ttf"
        ))),
    );

    for chain in fonts.families.values_mut() {
        chain.retain(|name| name != UBUNTU_LIGHT_DATA);
    }
    if let Some(chain) = fonts.families.get_mut(&FontFamily::Monospace) {
        chain.insert(1.min(chain.len()), REGULAR_DATA.to_owned());
    }

    let proportional_fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name != REGULAR_DATA)
        .collect::<Vec<_>>();
    let mut regular = vec![REGULAR_DATA.to_owned()];
    regular.extend(proportional_fallbacks.iter().cloned());
    let mut semibold = vec![SEMIBOLD_DATA.to_owned(), REGULAR_DATA.to_owned()];
    semibold.extend(proportional_fallbacks);

    fonts
        .families
        .insert(FontFamily::Proportional, regular.clone());
    fonts.families.insert(Weight::Regular.family(), regular);
    fonts.families.insert(Weight::SemiBold.family(), semibold);
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_select_distinct_static_faces() {
        let fonts = definitions();

        assert_eq!(fonts.families[&Weight::Regular.family()][0], REGULAR_DATA);
        assert_eq!(fonts.families[&Weight::SemiBold.family()][0], SEMIBOLD_DATA);
        assert_ne!(
            fonts.font_data[REGULAR_DATA].font,
            fonts.font_data[SEMIBOLD_DATA].font
        );
    }

    #[test]
    fn ubuntu_light_is_not_a_fallback() {
        let fonts = definitions();

        assert!(!fonts.font_data.contains_key(UBUNTU_LIGHT_DATA));
        assert!(
            fonts
                .families
                .values()
                .flatten()
                .all(|name| name != UBUNTU_LIGHT_DATA)
        );
    }
}
