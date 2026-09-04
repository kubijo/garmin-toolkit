//! Profile preference composition.

use egui::{Id, Ui};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};

use crate::{Size, button, icons, images, profile, select};

/// Current setting selections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Values {
    pub unit_system: UnitSystem,
    pub language: Language,
    pub theme: Theme,
}

/// Measurement presentation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UnitSystem {
    /// Metric units.
    #[default]
    Metric,
    Imperial,
}

impl UnitSystem {
    const fn index(self) -> usize {
        match self {
            Self::Metric => 0,
            Self::Imperial => 1,
        }
    }

    const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Imperial,
            _ => Self::Metric,
        }
    }
}

/// Bundled language.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Language {
    /// English.
    #[default]
    English,
    Czech,
}

impl Language {
    #[must_use]
    pub const fn autonym(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Czech => "Čeština",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::English => 0,
            Self::Czech => 1,
        }
    }

    const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Czech,
            _ => Self::English,
        }
    }
}

/// Application theme.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    /// Follow the system theme.
    #[default]
    Auto,
    Dark,
    Light,
}

/// Profile-settings inputs.
pub struct Props<'a> {
    pub intl: &'a Intl,
    pub values: Values,
    pub profile: profile::ProfileProps<'a>,
    pub disabled: bool,
}

/// One changed preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ChoosePicture,
    UnitSystem(UnitSystem),
    Language(Language),
    Theme(Theme),
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let title = format_message!(props.intl, default_message: "Profile settings");
    ui.heading(title);
    ui.add_space(24.0);

    let mut action = None;
    crate::theme::layer(ui, theme::Level::One, |ui| {
        ui.set_max_width(640.0);

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
            if (button::Props {
                label: &choose_picture,
                icon: Some(icons::CAMERA),
                kind: button::Kind::Tertiary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: !props.disabled,
            })
            .show(ui)
            .clicked()
            {
                action = Some(Action::ChoosePicture);
            }
        });
        ui.add_space(24.0);

        let unit_label = format_message!(props.intl, default_message: "Unit system");
        let unit_helper = format_message!(
            props.intl,
            default_message: "Controls displayed distances and elevations."
        );
        let metric = format_message!(props.intl, default_message: "Metric");
        let imperial = format_message!(props.intl, default_message: "Imperial");
        let units = [select::Choice::new(&metric), select::Choice::new(&imperial)];
        let mut unit_system = props.values.unit_system.index();
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
            action = Some(Action::UnitSystem(UnitSystem::from_index(unit_system)));
        }
        ui.add_space(16.0);

        let language_label = format_message!(props.intl, default_message: "Language");
        let languages = [
            select::Choice::new(Language::English.autonym()).image(images::UNITED_KINGDOM),
            select::Choice::new(Language::Czech.autonym()).image(images::CZECHIA),
        ];
        let mut language = props.values.language.index();
        if select::show(
            ui,
            Id::new("profile-language"),
            &mut language,
            &languages,
            select::Props::new(&language_label).disabled(props.disabled),
        )
        .changed()
        {
            action = Some(Action::Language(Language::from_index(language)));
        }
        ui.add_space(16.0);

        let theme_label = format_message!(props.intl, default_message: "Theme");
        let auto = format_message!(props.intl, default_message: "Auto");
        let dark = format_message!(props.intl, default_message: "Dark");
        let light = format_message!(props.intl, default_message: "Light");
        ui.label(theme_label);
        let themes = [
            button::GroupChoice::new(&auto, icons::DESKTOP, Theme::Auto),
            button::GroupChoice::new(&dark, icons::MOON, Theme::Dark),
            button::GroupChoice::new(&light, icons::SUN, Theme::Light),
        ];
        if let Some(theme) = button::group(
            ui,
            props.values.theme,
            &themes,
            button::GroupProps {
                size: Size::Medium,
                enabled: !props.disabled,
            },
        ) {
            action = Some(Action::Theme(theme));
        }
    });
    action
}
