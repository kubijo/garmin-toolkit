//! Reloadable Garmin Toolkit gallery scenes.

#![expect(
    clippy::multiple_crate_versions,
    reason = "the isolated gallery combines terminal, desktop, capture, and SVG stacks"
)]
#![expect(
    missing_docs,
    reason = "rs-gallery generates undocumented dynamic-library exports"
)]

mod terminal_input;

use std::sync::{LazyLock, OnceLock};

use gallery::{CatalogGlobals, GlobalControls, Icon};
use garmin_i18n::{Intl, Language as IntlLanguage, Translations};
use serde::{Deserialize, Serialize};

struct GlobalIcons {
    light: Icon,
    dark: Icon,
    english: Icon,
    czech: Icon,
}

static GLOBAL_ICONS: LazyLock<GlobalIcons> = LazyLock::new(|| GlobalIcons {
    light: Icon::from_svg(garmin_ui::icons::SUN.as_svg()),
    dark: Icon::from_svg(garmin_ui::icons::MOON.as_svg()),
    english: Icon::from_svg(garmin_ui::icons::GLOBE.as_svg()),
    czech: Icon::from_svg(garmin_ui::icons::TRANSLATE.as_svg()),
});

#[derive(Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
pub(crate) enum GalleryTheme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
pub(crate) enum GalleryLanguage {
    #[default]
    English,
    Czech,
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Globals {
    pub(crate) theme: GalleryTheme,
    pub(crate) language: GalleryLanguage,
}

impl Globals {
    pub(crate) fn intl(&self) -> Intl {
        static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();
        TRANSLATIONS
            .get_or_init(|| {
                Translations::bundled().expect("embedded catalogs are validated during the build")
            })
            .formatter(match self.language {
                GalleryLanguage::English => IntlLanguage::English,
                GalleryLanguage::Czech => IntlLanguage::Czech,
            })
            .expect("gallery globals select only bundled languages")
    }
}

impl CatalogGlobals for Globals {
    fn controls(&mut self, controls: &mut GlobalControls<'_>) {
        self.theme = controls.icon_buttons(
            "Theme",
            &[
                ("Light", &GLOBAL_ICONS.light, GalleryTheme::Light),
                ("Dark", &GLOBAL_ICONS.dark, GalleryTheme::Dark),
            ],
            GalleryTheme::Dark,
        );
        self.language = controls.icon_buttons(
            "Language",
            &[
                ("English", &GLOBAL_ICONS.english, GalleryLanguage::English),
                ("Čeština", &GLOBAL_ICONS.czech, GalleryLanguage::Czech),
            ],
            GalleryLanguage::English,
        );
    }

    fn prepare(&self, ui: &mut gallery::egui::Ui) {
        garmin_ui::theme::apply_palette(
            ui.style_mut(),
            match self.theme {
                GalleryTheme::Light => &garmin_color::theme::GRAY_10,
                GalleryTheme::Dark => &garmin_color::theme::GRAY_100,
            },
        );
    }
}

gallery::scenes_dylib!(Globals);
