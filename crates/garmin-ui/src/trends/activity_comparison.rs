//! Compare individual activities from the same published cache and sport.

use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::{
    activity::{ActivityDuration, Distance, Speed},
    garmin_connect::trends::ActivitySummary,
    identity::UnitSystem,
};

use super::{Props, Range, Sport, State, card, sport_label};

pub(super) fn show(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    let Some(history) = &state.history else {
        return;
    };
    card(ui, "activity-comparison", |ui| {
        ui.strong(format_message!(props.intl, default_message: "Activity comparison"));
        let activities = history
            .activities
            .iter()
            .filter(|activity| range.contains(activity.date))
            .collect::<Vec<_>>();
        let mut counts = std::collections::BTreeMap::new();
        for activity in &activities {
            *counts.entry(activity.sport).or_insert(0_usize) += 1;
        }
        let first_choices = activities
            .iter()
            .copied()
            .filter(|activity| counts[&activity.sport] >= 2)
            .collect::<Vec<_>>();
        if first_choices.is_empty() {
            ui.weak(format_message!(props.intl, default_message: "Choose a period with at least two activities of the same sport."));
            return;
        }
        ui.columns(2, |columns| {
            let (left, right) = columns.split_at_mut(1);
            let first = picker(
                &mut left[0],
                props,
                &first_choices,
                &mut state.activity_pair[0],
                true,
            );
            let second_choices = activities
                .into_iter()
                .filter(|activity| activity.sport == first.sport && activity.id != first.id)
                .collect::<Vec<_>>();
            let second = picker(
                &mut right[0],
                props,
                &second_choices,
                &mut state.activity_pair[1],
                false,
            );
            measurements(&mut left[0], props, first);
            measurements(&mut right[0], props, second);
        });
    });
}

fn picker<'a>(
    ui: &mut Ui,
    props: &Props<'_>,
    activities: &[&'a ActivitySummary],
    selected: &mut Option<i64>,
    first: bool,
) -> &'a ActivitySummary {
    let (target, title) = if first {
        (
            "trends.activity.first",
            format_message!(props.intl, default_message: "First activity"),
        )
    } else {
        (
            "trends.activity.second",
            format_message!(props.intl, default_message: "Second activity"),
        )
    };
    let labels = activities
        .iter()
        .map(|activity| {
            let date = props.intl.dates().date(activity.date);
            if activity.name.trim().is_empty() {
                format!("{date} · #{}", activity.id)
            } else {
                format!("{} · {date}", activity.name)
            }
        })
        .collect::<Vec<_>>();
    let choices = labels
        .iter()
        .map(|label| crate::select::Choice::new(label))
        .collect::<Vec<_>>();
    let mut index = activities
        .iter()
        .position(|activity| Some(activity.id) == *selected)
        .unwrap_or(0);
    let response = crate::select::show(
        ui,
        ui.id().with(target),
        &mut index,
        &choices,
        crate::select::Props::new(&title).width(ui.available_width().min(480.0)),
    )
    .on_hover_text(&labels[index]);
    crate::semantics::target(ui, &response, target);
    let activity = activities[index];
    *selected = Some(activity.id);
    activity
}

fn measurements(ui: &mut Ui, props: &Props<'_>, activity: &ActivitySummary) {
    let units = props.preferences.unit_system();
    let distance = activity
        .distance
        .map(|value| value.format(units, 2, props.intl.decimal_formatter()));
    let duration = activity.duration.map(ActivityDuration::format_clock);
    let elevation = activity
        .elevation_gain
        .map(|gain| gain.format_elevation(units, props.intl.decimal_formatter()));
    let speed = activity
        .distance
        .zip(activity.duration)
        .and_then(|(distance, duration)| Speed::from_distance_and_duration(distance, duration));
    let (rate_label, rate) = if activity.sport == Sport::Cycling {
        (
            format_message!(props.intl, default_message: "Average speed"),
            speed.map(|value| value.format(units, 1, props.intl.decimal_formatter())),
        )
    } else {
        let (pace_distance, pace_unit) = match (activity.sport, units) {
            (Sport::Swimming, _) => (Distance::from_millimeters(100_000), "100 m"),
            (_, UnitSystem::Metric) => (Distance::from_millimeters(1_000_000), "km"),
            (_, UnitSystem::Imperial) => (Distance::from_millimeters(1_609_344), "mi"),
        };
        (
            format_message!(props.intl, default_message: "Average pace"),
            speed
                .and_then(|value| value.time_for(pace_distance))
                .map(|pace| format!("{} / {pace_unit}", pace.format_clock())),
        )
    };
    ui.weak(format!(
        "{} · {}",
        sport_label(props.intl, activity.sport),
        props.intl.dates().date(activity.date)
    ));
    for (label, value) in [
        (
            format_message!(props.intl, default_message: "Distance"),
            distance,
        ),
        (
            format_message!(props.intl, default_message: "Activity time"),
            duration,
        ),
        (rate_label, rate),
        (
            format_message!(props.intl, default_message: "Elevation gain"),
            elevation,
        ),
        (
            format_message!(props.intl, default_message: "Average heart rate"),
            activity
                .average_heart_rate
                .map(|rate| rate.format(props.intl.decimal_formatter())),
        ),
    ] {
        ui.weak(label);
        ui.label(value.as_deref().unwrap_or("—"));
    }
}
