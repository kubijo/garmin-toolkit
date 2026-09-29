use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem};
use garmin_ui::profile_settings;

scene_meta! { title: "Application / Profiles / Settings" }

thread_local! {
    static SETTINGS: crate::SceneState<(ProfilePreferences, Option<garmin_color::Color>), 2> = const { crate::SceneState::empty() };
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 640.0, 320.0, 760.0, 1.0);
    let intl = globals.intl();
    stage!(ctx, ui, globals.stage((width, 900.0)), |ui| {
        SETTINGS.with_scene(
            usize::from(matches!(globals.language, crate::GalleryLanguage::Czech)),
            || {
                (
                    ProfilePreferences::from_parts(
                        UnitSystem::Metric,
                        match globals.language {
                            crate::GalleryLanguage::English => LanguagePreference::English,
                            crate::GalleryLanguage::Czech => LanguagePreference::Czech,
                        },
                        ThemePreference::Auto,
                    ),
                    None,
                )
            },
            |(preferences, accent)| {
                let action = profile_settings::show(
                    ui,
                    &profile_settings::Props {
                        id: gallery::egui::Id::new("settings-profile"),
                        intl: &intl,
                        accent: *accent,
                        preferences: *preferences,
                        profile: garmin_ui::profile::ProfileProps {
                            display_name: "Alex Rider",
                            accent: accent.unwrap_or(garmin_color::swatch::ACTION),
                            avatar: None,
                        },
                        picture_enabled: true,
                        disabled: false,
                    },
                );
                match action {
                    Some(profile_settings::Action::UpdatePreferences(value)) => {
                        *preferences = value;
                    }
                    Some(profile_settings::Action::UpdateAccent(value)) => *accent = value,
                    _ => {}
                }
            },
        );
    });
}
