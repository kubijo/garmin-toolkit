//! Compact page controls and the transactional target editor.

use super::{Action, Props, ReadState, State};
use crate::{Size, button, icons, input, modal, segments};
use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::garmin_connect::{
    LoadProgress,
    sync::Outcome,
    trends::{Period, Range},
};
use garmin_model::identity::MonthlyDistanceTarget;
use jiff::civil::Date;

pub(super) fn button(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    kind: button::Kind,
    enabled: bool,
    target: &str,
) -> egui::Response {
    let response = ui
        .scope(|ui| {
            ui.visuals_mut().override_text_color = None;
            button::Props {
                label,
                icon: Some(icon),
                kind,
                size: Size::Small,
                width: button::Width::Fit,
                enabled,
            }
            .show(ui)
        })
        .inner;
    crate::semantics::target(ui, &response, target);
    response
}

fn icon(ui: &mut Ui, label: &str, icon: icons::Icon, enabled: bool, target: &str) -> bool {
    let response = button::IconProps {
        label,
        icon,
        kind: button::Kind::Ghost,
        size: Size::Small,
        enabled,
    }
    .show(ui);
    crate::semantics::target(ui, &response, target);
    response.clicked()
}

pub(super) fn toolbar(ui: &mut Ui, state: &mut State, props: &Props<'_>) -> Option<Action> {
    let narrow = ui.available_width() < 600.0;
    if narrow {
        period_selector(ui, state, props);
    }
    let mut output = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        if !narrow {
            period_selector(ui, state, props);
        }
        period_navigation(ui, state, props);
        ui.horizontal(|ui| {
            output = cloud_actions(ui, state, props);
        });
    });
    output
}

fn period_selector(ui: &mut Ui, state: &mut State, props: &Props<'_>) {
    let old = if state.custom_range.is_some() {
        None
    } else {
        Some(state.period)
    };
    let mut selected = old;
    segments::show(
        ui,
        &mut selected,
        &[
            (
                Some(Period::Day),
                format_message!(props.intl, default_message: "Day"),
            ),
            (
                Some(Period::Week),
                format_message!(props.intl, default_message: "Week"),
            ),
            (
                Some(Period::Month),
                format_message!(props.intl, default_message: "Month"),
            ),
            (
                Some(Period::Year),
                format_message!(props.intl, default_message: "Year"),
            ),
            (None, format_message!(props.intl, default_message: "Custom")),
        ],
        "trends.period",
    );
    if old != selected {
        if let Some(period) = selected {
            state.period = period;
            state.anchor = None;
            state.custom_range = None;
            state.range_draft = None;
        } else {
            open_range_editor(state, props);
        }
    }
}

fn open_range_editor(state: &mut State, props: &Props<'_>) {
    let range = state.custom_range.map(|(range, _)| range).or_else(|| {
        state
            .period
            .ranges_with_week_start(
                state.anchor.unwrap_or(props.today),
                props.today,
                props.intl.dates().week_start(),
            )
            .ok()
            .map(|(range, _)| range)
    });
    if let Some(range) = range {
        state.range_draft = Some([range.from.to_string(), range.through.to_string()]);
    }
}

fn custom_ranges(draft: &[String; 2], today: Date) -> Option<(Range, Range)> {
    let from: Date = draft[0].trim().parse().ok()?;
    let through: Date = draft[1].trim().parse().ok()?;
    if from > through || through > today {
        return None;
    }
    let days = from.until(through).ok()?.get_days() + 1;
    if days > 366 {
        return None;
    }
    Some((
        Range { from, through },
        Range {
            from: from.checked_sub(jiff::Span::new().days(days)).ok()?,
            through: from.checked_sub(jiff::Span::new().days(1)).ok()?,
        },
    ))
}

#[cfg(test)]
#[test]
fn custom_comparison_uses_the_immediately_preceding_equal_window() {
    let today = "2024-03-01".parse().expect("today");
    let dates = |from: &str, through: &str| [from.to_owned(), through.to_owned()];
    let (range, previous) =
        custom_ranges(&dates("2024-02-28", "2024-03-01"), today).expect("valid leap-day range");
    assert_eq!(range.dates().len(), 3);
    assert_eq!(previous.dates().len(), 3);
    assert_eq!(previous.from.to_string(), "2024-02-25");
    assert_eq!(previous.through.to_string(), "2024-02-27");
    assert!(custom_ranges(&dates("2023-03-01", "2024-03-01"), today).is_none());
    assert!(custom_ranges(&dates("2024-03-01", "2024-02-28"), today).is_none());
    assert!(custom_ranges(&dates("2024-02-28", "2024-03-02"), today).is_none());
    assert!(custom_ranges(&dates("", "2024-03-01"), today).is_none());
}

pub(super) fn custom_range_editor(ui: &mut Ui, state: &mut State, props: &Props<'_>) {
    let Some(draft) = &mut state.range_draft else {
        return;
    };
    let width = ui.available_width().min(224.0);
    ui.horizontal_wrapped(|ui| {
        let label = format_message!(props.intl, default_message: "From date");
        custom_date_field(ui, props, width, 0, &label, &mut draft[0]);
        let label = format_message!(props.intl, default_message: "Through date");
        custom_date_field(ui, props, width, 1, &label, &mut draft[1]);
    });
    let ranges = custom_ranges(draft, props.today);
    if ranges.is_none() {
        ui.weak(format_message!(props.intl, default_message: "Choose up to 366 days ending today or earlier, with enough room for the preceding period."));
    }
    ui.horizontal_wrapped(|ui| {
        if button(
            ui,
            &format_message!(props.intl, default_message: "Apply date range"),
            icons::CALENDAR,
            button::Kind::Primary,
            ranges.is_some(),
            "trends.range.apply",
        )
        .clicked()
        {
            state.custom_range = ranges;
            state.selected = None;
            state.range_draft = None;
        }
        if button(
            ui,
            &format_message!(props.intl, default_message: "Cancel"),
            icons::X,
            button::Kind::Ghost,
            true,
            "trends.range.cancel",
        )
        .clicked()
        {
            state.range_draft = None;
        }
    });
}

fn custom_date_field(
    ui: &mut Ui,
    props: &Props<'_>,
    width: f32,
    index: usize,
    label: &str,
    value: &mut String,
) {
    let target = ["trends.range.from", "trends.range.through"][index];
    let id = ui.id().with(target);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 72.0), egui::Sense::hover());
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            let response =
                crate::date_input::show(ui, id, value, props.intl, label, true, props.today);
            crate::semantics::target(ui, &response, target);
        },
    );
}

fn cloud_actions(ui: &mut Ui, state: &mut State, props: &Props<'_>) -> Option<Action> {
    ui.spacing_mut().item_spacing.x = 8.0;
    if icon(
        ui,
        &format_message!(props.intl, default_message: "Reload cached history"),
        icons::DATABASE,
        !matches!(state.read, ReadState::Loading),
        "trends.reload",
    ) {
        state.read = ReadState::Idle;
    }
    if !props.connect_configured {
        return None;
    }
    let busy = props.refreshing || props.sync.is_some_and(|sync| sync.job.is_some());
    if busy {
        button(
            ui,
            &format_message!(props.intl, default_message: "Cancel refresh"),
            icons::STOP,
            button::Kind::Tertiary,
            true,
            "trends.cancel-refresh",
        )
        .clicked()
        .then_some(Action::Cancel(if props.refreshing {
            None
        } else {
            props.sync.and_then(|sync| sync.job)
        }))
    } else {
        button(
            ui,
            &format_message!(props.intl, default_message: "Sync activities"),
            icons::ARROWS_CLOCKWISE,
            button::Kind::Primary,
            props.enabled,
            "trends.sync",
        )
        .clicked()
        .then_some(Action::Sync)
    }
}

fn period_navigation(ui: &mut Ui, state: &mut State, props: &Props<'_>) {
    if state.custom_range.is_some() {
        if button(
            ui,
            &format_message!(props.intl, default_message: "Edit date range"),
            icons::CALENDAR,
            button::Kind::Ghost,
            true,
            "trends.range.edit",
        )
        .clicked()
        {
            open_range_editor(state, props);
        }
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let anchor = state.anchor.unwrap_or(props.today);
        if icon(
            ui,
            &format_message!(props.intl, default_message: "Previous"),
            icons::CARET_LEFT,
            true,
            "trends.previous",
        ) {
            state.anchor = state.period.shift(anchor, -1).ok();
        }
        if icon(
            ui,
            &format_message!(props.intl, default_message: "Current period"),
            icons::CALENDAR,
            state.anchor.is_some(),
            "trends.current",
        ) {
            state.anchor = None;
        }
        let next = state.period.shift(anchor, 1).ok();
        if icon(
            ui,
            &format_message!(props.intl, default_message: "Next"),
            icons::CARET_RIGHT,
            next.is_some_and(|next| next <= props.today),
            "trends.next",
        ) {
            state.anchor = next;
        }
    });
}

pub(super) fn targets_dialog(ui: &mut Ui, state: &mut State, props: &Props<'_>) -> Option<Action> {
    let draft = state.target_draft.as_mut()?;
    let title = format_message!(props.intl, default_message: "Edit monthly distance targets");
    let description = format_message!(props.intl, default_message: "Monthly targets in kilometres. Zero leaves a sport without a target.");
    let save = format_message!(props.intl, default_message: "Save targets");
    let cancel = format_message!(props.intl, default_message: "Cancel");
    let output = ui
        .scope(|ui| {
            ui.visuals_mut().override_text_color = None;
            modal::show_with_footer(
                ui,
                ui.id().with("trends-target-editor"),
                &modal::Props {
                    title: &title,
                    description: Some(&description),
                    size: modal::Size::Small,
                    presentation: state.target_presentation,
                    cancel_label: Some(&cancel),
                    cancel_disabled: None,
                    backdrop_closes: Some(false),
                    primary: Some(modal::Primary {
                        label: &save,
                        icon: Some(icons::FLOPPY_DISK),
                        kind: modal::PrimaryKind::Confirm,
                        enabled: props.enabled
                            && *draft != props.preferences.monthly_distance_targets(),
                    }),
                },
                modal::FooterOptions {
                    cancel_icon: Some(icons::X),
                    ..modal::FooterOptions::default()
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = 16.0;
                    for (index, value) in draft.iter_mut().enumerate() {
                        let mut kilometres = value.as_kilometres();
                        let response = input::show_number(
                            ui,
                            &mut kilometres,
                            input::NumberProps::new(&super::sport_label(
                                props.intl,
                                super::Sport::ALL[index],
                            ))
                            .min(0)
                            .max(1_000_000)
                            .step(1)
                            .disabled(!props.enabled),
                        );
                        *value = MonthlyDistanceTarget::from_kilometres(kilometres);
                        crate::semantics::target(
                            ui,
                            &response,
                            [
                                "trends.target.running",
                                "trends.target.cycling",
                                "trends.target.swimming",
                            ][index],
                        );
                    }
                },
            )
        })
        .inner;
    let action =
        (output.action == Some(modal::Action::Primary)).then_some(Action::SaveTargets(*draft));
    if output.action.is_some() {
        state.target_draft = None;
    }
    action
}

pub(super) fn sync_feedback(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    if !props.connect_configured {
        return None;
    }
    let progress = props
        .load_progress
        .or_else(|| props.sync.and_then(|sync| sync.progress.as_ref()));
    if progress.is_none()
        && props.sync.is_none_or(|sync| {
            matches!(
                sync.state.outcome,
                Outcome::Complete { unavailable: 0 } | Outcome::Never | Outcome::Running
            )
        })
    {
        return None;
    }
    let mut output = None;
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width().min(640.0), 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        if let Some(progress) = progress {
            let value = match progress {
                LoadProgress::Downloading { total, downloaded, reused, unavailable, .. } => crate::progress::Value::Determinate { completed: downloaded + reused + unavailable, total: *total },
                _ => crate::progress::Value::Indeterminate,
            };
            crate::progress::show(ui, &crate::progress::Props {
                label: &format_message!(props.intl, default_message: "Sync is running. Trends use the last published summary cache."), detail: None, value, height: None,
            });
        } else if let Some(sync) = props.sync {
            match sync.state.outcome {
                Outcome::Complete { unavailable: 0 } | Outcome::Never | Outcome::Running => {},
                Outcome::Complete { unavailable } => {
                    ui.horizontal_wrapped(|ui| {
                        let label = format_message!(props.intl, default_message: "{count, plural, one {# original FIT unavailable} other {# original FITs unavailable}}; Trends use activity summaries.", values: { count: unavailable as u64 });
                        ui.weak(label).on_hover_text(format_message!(props.intl, default_message: "Missing originals do not affect Trends. Sync retries these downloads; some Garmin activities have no downloadable FIT file."));
                        if button(ui, &format_message!(props.intl, default_message: "View cloud activities"), icons::CLOUD, button::Kind::Ghost, true, "trends.cloud").clicked() {
                            output = Some(Action::OpenActivities);
                        }
                    });
                },
                _ => { crate::profile_settings::connect_sync::feedback(ui, props.intl, sync); },
            }
        }
    });
    output
}
