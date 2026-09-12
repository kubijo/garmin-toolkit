use gallery::prelude::*;
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem};
use garmin_ui::profile_settings;

scene_meta! { title: "Application / Profiles / Settings" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let theme = ctx.buttons("theme", &["auto", "dark", "light"], 0);
    let intl = globals.intl();
    stage!(ctx, ui, |ui| {
        ui.set_width(760.0);
        let _ = profile_settings::show(
            ui,
            &profile_settings::Props {
                intl: &intl,
                preferences: ProfilePreferences::from_parts(
                    UnitSystem::Metric,
                    match globals.language {
                        crate::GalleryLanguage::English => LanguagePreference::English,
                        crate::GalleryLanguage::Czech => LanguagePreference::Czech,
                    },
                    match theme {
                        1 => ThemePreference::Dark,
                        2 => ThemePreference::Light,
                        _ => ThemePreference::Auto,
                    },
                ),
                profile: garmin_ui::profile::ProfileProps {
                    display_name: "Alex Rider",
                    accent: garmin_color::swatch::ACTION,
                    avatar: None,
                },
                picture_enabled: true,
                disabled: false,
            },
        );
    });
}
