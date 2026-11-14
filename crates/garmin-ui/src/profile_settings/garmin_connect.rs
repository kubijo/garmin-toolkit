use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::garmin_connect::PersonalApiKey;

use super::{Action, Props};
use crate::{Size, button, input};

#[derive(Clone, Default)]
struct Draft {
    key: String,
    invalid: bool,
    submitted: bool,
    replacing: bool,
}

pub(super) fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let id = props.id.with("garmin-connect-key");
    let mut draft = ui.data_mut(|data| data.remove_temp::<Draft>(id).unwrap_or_default());
    if draft.submitted && !props.garmin_connect.checking && !props.disabled {
        if props.garmin_connect.facts.is_some() {
            draft = Draft::default();
        } else {
            draft.submitted = false;
        }
    }
    let status = if props.garmin_connect.configured {
        format_message!(props.intl, default_message: "Personal API key saved")
    } else {
        format_message!(props.intl, default_message: "No Personal API key saved")
    };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.heading("Garmin Connect");
        if props.garmin_connect.checking {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(format_message!(props.intl, default_message: "Checking Garmin Connect…"));
            });
        } else {
            ui.label(status);
        }
    });
    if !props.garmin_connect.configured || draft.replacing {
        ui.add_space(8.0);
        editor(ui, props, &mut draft);
    }
    ui.add_space(8.0);
    let action = buttons(ui, props, &mut draft);
    if let Some(facts) = props
        .garmin_connect
        .facts
        .filter(|_| !props.garmin_connect.checking)
    {
        ui.add_space(16.0);
        show_facts(ui, props, facts);
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
    action
}

fn editor(ui: &mut Ui, props: &Props<'_>, draft: &mut Draft) {
    let create = format_message!(props.intl, default_message: "Create a Personal API key");
    ui.hyperlink_to(
        create,
        "https://connect.garmin.com/app/settings/personalApiToken",
    );
    ui.add_space(8.0);
    let label = if props.garmin_connect.configured {
        format_message!(props.intl, default_message: "New Personal API key")
    } else {
        format_message!(props.intl, default_message: "Personal API key")
    };
    let helper = format_message!(props.intl, default_message: "Saved in this profile's database. Excluded from backups.");
    let invalid =
        format_message!(props.intl, default_message: "Enter a key without spaces or line breaks.");
    let message = if draft.invalid {
        input::Message::Error(&invalid)
    } else {
        input::Message::Helper(&helper)
    };
    let response = input::show(
        ui,
        &mut draft.key,
        input::Props::new(&label)
            .password(true)
            .disabled(props.disabled || props.garmin_connect.checking)
            .message(message),
    );
    crate::semantics::target(ui, &response, "profile.garmin-connect.key");
    if response.changed() {
        draft.invalid = false;
    }
}

fn buttons(ui: &mut Ui, props: &Props<'_>, draft: &mut Draft) -> Option<Action> {
    let mut action = None;
    let enabled = !props.disabled && !props.garmin_connect.checking;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if !props.garmin_connect.configured || draft.replacing {
            let save = format_message!(props.intl, default_message: "Verify and save");
            if clicked(
                ui,
                &save,
                button::Kind::Primary,
                enabled && !draft.key.trim().is_empty(),
            ) {
                match PersonalApiKey::from_string(draft.key.clone()) {
                    Ok(key) => {
                        draft.submitted = true;
                        action = Some(Action::UpdateGarminConnectKey(Some(key)));
                    }
                    Err(_) => draft.invalid = true,
                }
            }
        }
        if props.garmin_connect.configured {
            if draft.replacing {
                let cancel = format_message!(props.intl, default_message: "Cancel");
                if clicked(ui, &cancel, button::Kind::Tertiary, enabled) {
                    *draft = Draft::default();
                }
            } else {
                let test = format_message!(props.intl, default_message: "Test connection");
                if clicked(ui, &test, button::Kind::Primary, enabled) {
                    action = Some(Action::TestGarminConnectKey);
                }
                let replace = format_message!(props.intl, default_message: "Replace key");
                if clicked(ui, &replace, button::Kind::Tertiary, enabled) {
                    draft.replacing = true;
                }
                let remove = format_message!(props.intl, default_message: "Remove key");
                if clicked(ui, &remove, button::Kind::Tertiary, enabled) {
                    *draft = Draft::default();
                    action = Some(Action::UpdateGarminConnectKey(None));
                }
            }
        }
    });
    action
}

fn clicked(ui: &mut Ui, label: &str, kind: button::Kind, enabled: bool) -> bool {
    button::Props {
        label,
        icon: None,
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
    .show(ui)
    .clicked()
}

fn show_facts(ui: &mut Ui, props: &Props<'_>, facts: &garmin_model::garmin_connect::AccountFacts) {
    ui.label(format_message!(props.intl, default_message: "Verified with Garmin Connect"));
    let unknown = format_message!(props.intl, default_message: "Not provided");
    let name_label = if facts
        .display_name
        .as_ref()
        .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
    {
        format_message!(props.intl, default_message: "Garmin profile ID")
    } else {
        format_message!(props.intl, default_message: "Display name")
    };
    let time_zone = facts
        .time_zone
        .as_deref()
        .map(|name| time_zone_label(name, props.garmin_connect.now));
    let rows = [
        (
            name_label,
            facts.display_name.as_deref().unwrap_or(&unknown),
        ),
        (
            format_message!(props.intl, default_message: "Locale"),
            facts.locale.as_deref().unwrap_or(&unknown),
        ),
        (
            format_message!(props.intl, default_message: "Units"),
            facts.measurement_system.as_deref().unwrap_or(&unknown),
        ),
        (
            format_message!(props.intl, default_message: "Time zone"),
            time_zone.as_deref().unwrap_or(&unknown),
        ),
    ];
    crate::facts::show(ui, props.id.with("garmin-connect-facts"), &rows);
}

fn time_zone_label(name: &str, now: jiff::Timestamp) -> String {
    jiff::tz::TimeZone::get(name).map_or_else(
        |_| name.to_owned(),
        |zone| format!("{name} (UTC{})", now.to_zoned(zone).strftime("%:z")),
    )
}

#[cfg(test)]
mod tests {
    use super::time_zone_label;

    #[test]
    fn zone_offset_follows_daylight_saving_and_preserves_unknown_zones() {
        let winter = "2026-01-01T00:00:00Z".parse().unwrap();
        let summer = "2026-07-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            time_zone_label("Europe/Athens", winter),
            "Europe/Athens (UTC+02:00)"
        );
        assert_eq!(
            time_zone_label("Europe/Athens", summer),
            "Europe/Athens (UTC+03:00)"
        );
        assert_eq!(
            time_zone_label("Asia/Kolkata", winter),
            "Asia/Kolkata (UTC+05:30)"
        );
        assert_eq!(time_zone_label("Unknown/Zone", winter), "Unknown/Zone");
    }
}
