//! Profile preference composition.

use egui::{Id, Ui};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem};

use crate::{Size, button, icons, images, profile, radio};
mod accent;

const fn language_autonym(language: LanguagePreference) -> &'static str {
    match language {
        LanguagePreference::English => "English",
        LanguagePreference::Czech => "Čeština",
    }
}

/// Profile-settings inputs.
pub struct Props<'a> {
    pub id: Id,
    pub intl: &'a Intl,
    pub preferences: ProfilePreferences,
    pub accent: Option<garmin_color::Color>,
    pub profile: profile::ProfileProps<'a>,
    pub picture_enabled: bool,
    pub disabled: bool,
}

/// One changed preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ChoosePicture,
    UpdateAccent(Option<garmin_color::Color>),
    UpdatePreferences(ProfilePreferences),
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let title = format_message!(props.intl, default_message: "Profile settings");
    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt(props.id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::NONE.inner_margin(24).show(ui, |ui| {
                ui.heading(title);
                ui.add_space(24.0);
                crate::theme::layer(ui, theme::Level::One, |ui| {
                    ui.set_max_width(ui.available_width().min(480.0));
                    if show_picture(ui, props) {
                        action = Some(Action::ChoosePicture);
                    }
                    ui.add_space(24.0);
                    if let Some(update) = accent::show(ui, props) {
                        action = Some(update);
                    }
                    ui.add_space(24.0);
                    if let Some(preferences) = show_preferences(ui, props) {
                        action = Some(Action::UpdatePreferences(preferences));
                    }
                });
            });
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
            accent: accent::preview(ui, props),
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
    let units = [
        radio::Choice::new(&metric, UnitSystem::Metric, "profile.units.metric"),
        radio::Choice::new(&imperial, UnitSystem::Imperial, "profile.units.imperial"),
    ];
    if let Some(unit_system) = radio::show(
        ui,
        preferences.unit_system(),
        &units,
        radio::Props {
            label: &unit_label,
            helper: Some(&unit_helper),
            enabled: !props.disabled,
        },
    ) {
        preferences = ProfilePreferences::from_parts(
            unit_system,
            preferences.language(),
            preferences.theme(),
        );
    }
    ui.add_space(16.0);

    let language_label = format_message!(props.intl, default_message: "Language");
    let languages = [
        radio::Choice::new(
            language_autonym(LanguagePreference::English),
            LanguagePreference::English,
            "profile.language.english",
        )
        .image(images::UNITED_KINGDOM),
        radio::Choice::new(
            language_autonym(LanguagePreference::Czech),
            LanguagePreference::Czech,
            "profile.language.czech",
        )
        .image(images::CZECHIA),
    ];
    if let Some(language) = radio::show(
        ui,
        preferences.language(),
        &languages,
        radio::Props {
            label: &language_label,
            helper: None,
            enabled: !props.disabled,
        },
    ) {
        preferences = ProfilePreferences::from_parts(
            preferences.unit_system(),
            language,
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
            width: button::Width::Fit,
            enabled: !props.disabled,
        },
    ) {
        preferences = ProfilePreferences::from_parts(
            preferences.unit_system(),
            preferences.language(),
            theme,
        );
    }
    preferences = preferences.with_show_hidden_files(props.preferences.show_hidden_files());
    preferences = preferences.with_inline_file_windows(props.preferences.inline_file_windows());
    ui.add_space(16.0);
    if let Some(inline) = show_file_windows(ui, props, preferences.inline_file_windows()) {
        preferences = preferences.with_inline_file_windows(inline);
    }
    (preferences != props.preferences).then_some(preferences)
}

fn show_file_windows(ui: &mut Ui, props: &Props<'_>, selected: bool) -> Option<bool> {
    let label = format_message!(props.intl, default_message: "File browser windows");
    let separate = format_message!(props.intl, default_message: "Separate window");
    let inline = format_message!(props.intl, default_message: "Inside the app");
    let helper = format_message!(props.intl, default_message: "Choose where file browsers and choosers open.");
    let choices = [
        radio::Choice::new(&separate, false, "profile.file-windows.separate"),
        radio::Choice::new(&inline, true, "profile.file-windows.inline"),
    ];
    radio::show(
        ui,
        selected,
        &choices,
        radio::Props {
            label: &label,
            helper: Some(&helper),
            enabled: !props.disabled,
        },
    )
}
