use gallery::prelude::*;
use garmin_i18n::{Language, Translations};
use garmin_ui::profile_settings;
use std::sync::OnceLock;

scene_meta! { title: "Desktop / Profiles / Settings" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let language = ctx.buttons("language", &["English", "Čeština"], 0);
    let appearance = ctx.buttons("appearance", &["dark", "light"], 0);
    let theme = ctx.buttons("theme", &["auto", "dark", "light"], 0);
    garmin_ui::theme::apply_palette(
        ui.style_mut(),
        if appearance == 1 {
            &garmin_color::theme::GRAY_10
        } else {
            &garmin_color::theme::GRAY_100
        },
    );
    let intl = formatter(language);
    stage!(ctx, ui, |ui| {
        ui.set_width(760.0);
        let _ = profile_settings::show(
            ui,
            &profile_settings::Props {
                intl: &intl,
                values: profile_settings::Values {
                    unit_system: profile_settings::UnitSystem::Metric,
                    language: if language == 1 {
                        profile_settings::Language::Czech
                    } else {
                        profile_settings::Language::English
                    },
                    theme: match theme {
                        1 => profile_settings::Theme::Dark,
                        2 => profile_settings::Theme::Light,
                        _ => profile_settings::Theme::Auto,
                    },
                },
                profile: garmin_ui::profile::ProfileProps {
                    display_name: "Alex Rider",
                    accent: garmin_color::swatch::ACTION,
                    avatar: None,
                },
                disabled: false,
            },
        );
    });
}

fn formatter(language: usize) -> garmin_i18n::Intl {
    static TRANSLATIONS: OnceLock<Translations> = OnceLock::new();
    TRANSLATIONS
        .get_or_init(|| {
            Translations::bundled().expect("embedded catalogs are validated during the build")
        })
        .formatter(if language == 1 {
            Language::Czech
        } else {
            Language::English
        })
        .expect("the gallery requests a bundled language")
}
