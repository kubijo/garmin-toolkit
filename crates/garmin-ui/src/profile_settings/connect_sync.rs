use super::{Action, Props};
use crate::{Size, button, input, notification, radio_group, select};
use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::garmin_connect::sync::{Failure, Interval, Outcome, Snapshot, Unit};

#[derive(Clone)]
struct Draft {
    original: Option<Interval>,
    enabled: bool,
    count: u32,
    unit: Unit,
}

pub(super) fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    ui.push_id(props.id.with("connect-sync-panel"), |ui| {
        show_scoped(ui, props)
    })
    .inner
}

fn show_scoped(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let snapshot = props.garmin_connect.sync?;
    let id = props.id.with("connect-sync-settings");
    let mut draft = ui
        .data_mut(|data| data.get_temp::<Draft>(id))
        .filter(|draft| draft.original == snapshot.state.interval)
        .unwrap_or_else(|| {
            let interval = snapshot
                .state
                .interval
                .unwrap_or(Interval::new(1, Unit::Day).expect("positive interval"));
            Draft {
                original: snapshot.state.interval,
                enabled: snapshot.state.interval.is_some(),
                count: interval.count(),
                unit: interval.unit(),
            }
        });
    ui.label(format_message!(props.intl, default_message: "Automatic sync"));
    ui.add_space(8.0);
    crate::typography::body(
        ui,
        &format_message!(props.intl, default_message: "Refreshes cached activities and downloads FIT files while the desktop app or Home Assistant server is running. Imports remain manual."),
    );
    ui.add_space(16.0);
    let off = format_message!(props.intl, default_message: "Off");
    let every = format_message!(props.intl, default_message: "Every");
    if let Some(enabled) = radio_group::show(
        ui,
        draft.enabled,
        &[
            radio_group::Choice::new(&off, false, "connect.sync.off"),
            radio_group::Choice::new(&every, true, "connect.sync.every"),
        ],
        radio_group::Props {
            label: "",
            helper: None,
            enabled: !props.disabled,
        },
    ) {
        draft.enabled = enabled;
    }
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        input::show_number(&mut columns[0], &mut draft.count, input::NumberProps::new("").min(1).disabled(props.disabled || !draft.enabled));
        let labels = [
            format_message!(props.intl, default_message: "{count, plural, one {hour} other {hours}}", values: { count: u64::from(draft.count) }),
            format_message!(props.intl, default_message: "{count, plural, one {day} other {days}}", values: { count: u64::from(draft.count) }),
            format_message!(props.intl, default_message: "{count, plural, one {week} other {weeks}}", values: { count: u64::from(draft.count) }),
        ];
        let units = [Unit::Hour, Unit::Day, Unit::Week];
        let mut selected = units.iter().position(|unit| *unit == draft.unit).unwrap_or(0);
        select::show(&mut columns[1], id.with("unit"), &mut selected, &labels.iter().map(|label| select::Choice::new(label)).collect::<Vec<_>>(), select::Props::new("").disabled(props.disabled || !draft.enabled));
        draft.unit = units[selected];
    });
    let configured = if draft.enabled {
        Interval::new(draft.count, draft.unit)
    } else {
        None
    };
    ui.add_space(8.0);
    crate::typography::body(
        ui,
        &format_message!(props.intl, default_message: "A day is 24 hours; a week is seven days."),
    );
    let mut action = None;
    ui.add_space(16.0);
    if (button::Props {
        label: &format_message!(props.intl, default_message: "Save sync settings"),
        icon: None,
        kind: button::Kind::Primary,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled: !props.disabled
            && configured != snapshot.state.interval
            && (!draft.enabled || configured.is_some()),
    })
    .show(ui)
    .clicked()
    {
        action = Some(Action::UpdateConnectSync(configured));
    }
    if let Some(status_action) = status(ui, props, snapshot, id) {
        action = Some(status_action);
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
    action
}

fn status(ui: &mut Ui, props: &Props<'_>, snapshot: &Snapshot, id: egui::Id) -> Option<Action> {
    let mut action = None;
    ui.add_space(16.0);
    if !props.garmin_connect.activities_busy && feedback(ui, props.intl, snapshot) {
        ui.add_space(16.0);
    }
    let never = format_message!(props.intl, default_message: "Never");
    let paused = format_message!(props.intl, default_message: "Not scheduled");
    let timestamp = |value: garmin_model::value::Timestamp| {
        props.intl.dates().datetime(
            value
                .as_jiff()
                .to_zoned(jiff::tz::TimeZone::system())
                .datetime(),
        )
    };
    let last = snapshot.state.last_success.map_or(never, timestamp);
    let next = snapshot.state.next_attempt.map_or(paused, timestamp);
    crate::facts::show(
        ui,
        id.with("status"),
        &[
            (
                format_message!(props.intl, default_message: "Last successful sync"),
                last.as_str(),
            ),
            (
                format_message!(props.intl, default_message: "Next attempt"),
                next.as_str(),
            ),
        ],
    );
    if snapshot.progress.is_some() {
        ui.add_space(8.0);
        if super::connect_activities::refresh_progress(
            ui,
            props.intl,
            snapshot.progress.as_ref(),
            if matches!(snapshot.state.outcome, Outcome::Cancelled) {
                super::RefreshState::Cancelling
            } else {
                super::RefreshState::Running
            },
        ) && let Some(job) = snapshot.job
        {
            action = Some(Action::CancelConnectSync(job));
        }
    }
    action
}

pub(crate) fn feedback(ui: &mut Ui, intl: &garmin_i18n::Intl, snapshot: &Snapshot) -> bool {
    use crate::notification::Kind;
    if snapshot.progress.is_some() {
        return false;
    }
    let (kind, title, mut detail) = match &snapshot.state.outcome {
        Outcome::Never | Outcome::Running => return false,
        Outcome::Complete { .. } => (
            Kind::Success,
            format_message!(intl, default_message: "Sync completed"),
            format_message!(intl, default_message: "Activity summaries are up to date."),
        ),
        Outcome::Cancelled => (
            Kind::Information,
            format_message!(intl, default_message: "Sync cancelled"),
            format_message!(intl, default_message: "Downloaded files were kept."),
        ),
        Outcome::Interrupted => (
            Kind::Warning,
            format_message!(intl, default_message: "Sync interrupted"),
            format_message!(intl, default_message: "Downloaded files were kept."),
        ),
        Outcome::Failed {
            kind: Failure::RateLimit,
            ..
        } => (
            Kind::Warning,
            format_message!(intl, default_message: "Sync paused: Garmin request limit reached"),
            format_message!(intl, default_message: "Sync did not finish. Downloaded files were kept."),
        ),
        Outcome::Failed { message, .. } => (
            Kind::Error,
            format_message!(intl, default_message: "Sync did not finish"),
            notification::normalize_error_detail(message),
        ),
    };
    let next = snapshot.state.next_attempt.map(|next| {
        intl.dates().datetime(
            next.as_jiff()
                .to_zoned(jiff::tz::TimeZone::system())
                .datetime(),
        )
    });
    if let Some(next) = &next {
        detail.push('\n');
        detail.push_str(&format_message!(intl, default_message: "Next automatic attempt: {next}", values: { next: next.as_str() }));
    }
    let props = notification::Props {
        kind,
        title: &title,
        detail: Some(&detail),
    };
    if let Some(next) = &next {
        notification::show_with_emphasis(ui, &props, next);
    } else {
        notification::show(ui, &props);
    }
    true
}
