use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem};
use garmin_ui::{profile, profile_settings, shell, workspace};

scene_meta! { title: "Application / Profiles / Settings" }

thread_local! {
    static SETTINGS: crate::SceneState<(ProfilePreferences, Option<garmin_color::Color>), 6> = const { crate::SceneState::empty() };
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 640.0, 320.0, 1120.0, 1.0);
    let height = ctx.slider("height", 900.0, 480.0, 1000.0, 1.0);
    let application = ctx.toggle("application", false);
    let disabled = ctx.toggle("disabled", false);
    let accent_index = ctx.buttons("accent", &["default", "pink", "green"], 0);
    let initial_accent = match accent_index {
        1 => Some(garmin_color::swatch::magenta::G50),
        2 => Some(garmin_color::swatch::green::G50),
        _ => None,
    };
    let intl = globals.intl();
    stage!(ctx, ui, globals.stage((width, height)), |ui| {
        SETTINGS.with_scene(
            usize::from(matches!(globals.language, crate::GalleryLanguage::Czech)) * 3
                + accent_index,
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
                    initial_accent,
                )
            },
            |(preferences, accent)| {
                let action = show_settings(
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
                        disabled,
                    },
                    application,
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

fn show_settings(
    ui: &mut Ui,
    props: &profile_settings::Props<'_>,
    application: bool,
) -> Option<profile_settings::Action> {
    if !application {
        return profile_settings::show(ui, props);
    }
    let profiles = [profile::ProfileProps {
        display_name: props.profile.display_name,
        accent: props.profile.accent,
        avatar: props.profile.avatar,
    }];
    workspace::show(
        ui,
        &workspace::Props {
            product_name: "Garmin Toolkit Demo",
            intl: props.intl,
            profiles: &profiles,
            selected_profile: 0,
            profile_menu_expanded: false,
            page: &workspace::Page::ProfileSettings,
            navigation: shell::Navigation::Expanded,
            devices: &[],
            backup_enabled: true,
            window_controls: None,
        },
        |ui| profile_settings::show(ui, props),
    )
    .inner
}
