//! Profile preference composition.

use egui::{Id, Ui};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem};

use crate::{Size, button, icons, images, profile, select};

const fn unit_index(unit_system: UnitSystem) -> usize {
    match unit_system {
        UnitSystem::Metric => 0,
        UnitSystem::Imperial => 1,
    }
}

const fn unit_from_index(index: usize) -> UnitSystem {
    match index {
        1 => UnitSystem::Imperial,
        _ => UnitSystem::Metric,
    }
}

const fn language_autonym(language: LanguagePreference) -> &'static str {
    match language {
        LanguagePreference::English => "English",
        LanguagePreference::Czech => "Čeština",
    }
}

const fn language_index(language: LanguagePreference) -> usize {
    match language {
        LanguagePreference::English => 0,
        LanguagePreference::Czech => 1,
    }
}

const fn language_from_index(index: usize) -> LanguagePreference {
    match index {
        1 => LanguagePreference::Czech,
        _ => LanguagePreference::English,
    }
}

/// Profile-settings inputs.
pub struct Props<'a> {
    pub intl: &'a Intl,
    pub preferences: ProfilePreferences,
    pub profile: profile::ProfileProps<'a>,
    pub picture_enabled: bool,
    pub disabled: bool,
}

/// One changed preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ChoosePicture,
    UpdatePreferences(ProfilePreferences),
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let title = format_message!(props.intl, default_message: "Profile settings");
    ui.heading(title);
    ui.add_space(24.0);

    let mut action = None;
    crate::theme::layer(ui, theme::Level::One, |ui| {
        ui.set_max_width(640.0);
        if show_picture(ui, props) {
            action = Some(Action::ChoosePicture);
        }
        ui.add_space(24.0);
        if let Some(preferences) = show_preferences(ui, props) {
            action = Some(Action::UpdatePreferences(preferences));
        }
    });
    action
}

fn show_picture(ui: &mut Ui, props: &Props<'_>) -> bool {
    let picture = format_message!(props.intl, default_message: "Profile picture");
    let choose_picture = format_message!(props.intl, default_message: "Choose a picture");
    ui.label(&picture);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        profile::AvatarProps {
            display_name: props.profile.display_name,
            accent: props.profile.accent,
            image: props.profile.avatar,
            size: 64.0,
        }
        .show(ui);
        ui.add_space(12.0);
        (button::Props {
            label: &choose_picture,
            icon: Some(icons::CAMERA),
            kind: button::Kind::Tertiary,
            size: Size::Medium,
            width: button::Width::Fit,
            enabled: props.picture_enabled && !props.disabled,
        })
        .show(ui)
        .clicked()
    })
    .inner
}

fn show_preferences(ui: &mut Ui, props: &Props<'_>) -> Option<ProfilePreferences> {
    let mut preferences = props.preferences;
    let unit_label = format_message!(props.intl, default_message: "Unit system");
    let unit_helper = format_message!(
        props.intl,
        default_message: "Controls displayed distances and elevations."
    );
    let metric = format_message!(props.intl, default_message: "Metric");
    let imperial = format_message!(props.intl, default_message: "Imperial");
    let units = [select::Choice::new(&metric), select::Choice::new(&imperial)];
    let mut unit_system = unit_index(preferences.unit_system());
    if select::show(
        ui,
        Id::new("profile-unit-system"),
        &mut unit_system,
        &units,
        select::Props::new(&unit_label)
            .helper(&unit_helper)
            .disabled(props.disabled),
    )
    .changed()
    {
        preferences = ProfilePreferences::from_parts(
            unit_from_index(unit_system),
            preferences.language(),
            preferences.theme(),
        );
    }
    ui.add_space(16.0);

    let language_label = format_message!(props.intl, default_message: "Language");
    let languages = [
        select::Choice::new(language_autonym(LanguagePreference::English))
            .image(images::UNITED_KINGDOM),
        select::Choice::new(language_autonym(LanguagePreference::Czech)).image(images::CZECHIA),
    ];
    let mut language = language_index(preferences.language());
    if select::show(
        ui,
        Id::new("profile-language"),
        &mut language,
        &languages,
        select::Props::new(&language_label).disabled(props.disabled),
    )
    .changed()
    {
        preferences = ProfilePreferences::from_parts(
            preferences.unit_system(),
            language_from_index(language),
            preferences.theme(),
        );
    }
    ui.add_space(16.0);

    let theme_label = format_message!(props.intl, default_message: "Theme");
    let auto = format_message!(props.intl, default_message: "Auto");
    let dark = format_message!(props.intl, default_message: "Dark");
    let light = format_message!(props.intl, default_message: "Light");
    ui.label(theme_label);
    let themes = [
        button::GroupChoice::new(&auto, icons::DESKTOP, ThemePreference::Auto),
        button::GroupChoice::new(&dark, icons::MOON, ThemePreference::Dark),
        button::GroupChoice::new(&light, icons::SUN, ThemePreference::Light),
    ];
    if let Some(theme) = button::group(
        ui,
        preferences.theme(),
        &themes,
        button::GroupProps {
            size: Size::Medium,
            enabled: !props.disabled,
        },
    ) {
        preferences = ProfilePreferences::from_parts(
            preferences.unit_system(),
            preferences.language(),
            theme,
        );
    }
    (preferences != props.preferences).then_some(preferences)
}
