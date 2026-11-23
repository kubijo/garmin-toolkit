//! Compare individual activities from the same published cache and sport.

use egui::{Align, Ui};
use garmin_i18n::format_message;
use garmin_model::{
    activity::{ActivityDuration, Distance, Speed},
    garmin_connect::trends::{ActivityIdentity, ActivitySummary},
    identity::UnitSystem,
};

use super::{Props, Range, Sport, State, card, sport_label};
use crate::summary_table;
use crate::summary_table::{Cell, Column};

pub(super) const MAX_WIDTH: f32 = 480.0;

pub(super) fn show(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    let Some(history) = &state.history else {
        return;
    };
    card(ui, "activity-comparison", |ui| {
        let title = format_message!(props.intl, default_message: "Activity comparison");
        ui.strong(&title);
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
        let width = ui.available_width().min(MAX_WIDTH);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                let ((first, first_label), (second, second_label)) = ui.columns(3, |columns| {
                    let (first, first_label) = picker(
                        &mut columns[1],
                        props,
                        &first_choices,
                        &mut state.activity_pair[0],
                        true,
                    );
                    let second_choices = activities
                        .into_iter()
                        .filter(|activity| {
                            activity.sport == first.sport && activity.identity() != first.identity()
                        })
                        .collect::<Vec<_>>();
                    let second = picker(
                        &mut columns[2],
                        props,
                        &second_choices,
                        &mut state.activity_pair[1],
                        false,
                    );
                    columns[0].weak(sport_label(props.intl, first.sport));
                    ((first, first_label), second)
                });
                ui.add_space(8.0);
                let rows = measurements(props, first)
                    .into_iter()
                    .zip(measurements(props, second))
                    .map(|((label, first), (_, second))| {
                        [
                            Cell::new(label),
                            Cell::new(first.unwrap_or_else(|| "—".to_owned())),
                            Cell::new(second.unwrap_or_else(|| "—".to_owned())),
                        ]
                    });
                summary_table::show_body(
                    ui,
                    &title,
                    [
                        Column::new("", 1, Align::Min),
                        Column::new(&first_label, 1, Align::Max),
                        Column::new(&second_label, 1, Align::Max),
                    ],
                    rows,
                );
            },
        );
    });
}

fn picker<'a>(
    ui: &mut Ui,
    props: &Props<'_>,
    activities: &[&'a ActivitySummary],
    selected: &mut Option<ActivityIdentity>,
    first: bool,
) -> (&'a ActivitySummary, String) {
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
    let mut labels = activities
        .iter()
        .map(|activity| {
            let date = props.intl.dates().date(activity.date);
            if activity.name.trim().is_empty() {
                match activity.identity() {
                    ActivityIdentity::Connect(id) => format!("{date} · #{id}"),
                    ActivityIdentity::Local(id) => {
                        let id = id.to_string();
                        format!(
                            "{} · {date} · {}",
                            sport_label(props.intl, activity.sport),
                            &id[..8]
                        )
                    }
                }
            } else {
                format!("{date} · {}", activity.name)
            }
        })
        .collect::<Vec<_>>();
    let choices = labels
        .iter()
        .map(|label| crate::select::Choice::new(label))
        .collect::<Vec<_>>();
    let mut index = activities
        .iter()
        .position(|activity| Some(activity.identity()) == *selected)
        .unwrap_or(0);
    let response = crate::select::show(
        ui,
        ui.id().with(target),
        &mut index,
        &choices,
        crate::select::Props::new(&title)
            .width(ui.available_width().min(480.0))
            .menu_width(384.0),
    )
    .on_hover_text(&labels[index]);
    crate::semantics::target(ui, &response, target);
    let activity = activities[index];
    *selected = Some(activity.identity());
    (activity, labels.swap_remove(index))
}

fn measurements(props: &Props<'_>, activity: &ActivitySummary) -> Vec<(String, Option<String>)> {
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
    let mut rows = Vec::from([
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
    ]);
    if activity.sport == Sport::Cycling {
        let cadence = activity
            .average_cadence
            .map(|cadence| cadence.format(props.intl.decimal_formatter()));
        rows.push((
            format_message!(props.intl, default_message: "Average cadence"),
            cadence,
        ));
        for (label, power) in [
            (
                format_message!(props.intl, default_message: "Average power"),
                activity.average_power,
            ),
            (
                format_message!(props.intl, default_message: "Peak power"),
                activity.peak_power,
            ),
        ] {
            let value = power.map(|power| power.format(props.intl.decimal_formatter()));
            rows.push((label, value));
        }
    }
    rows
}
