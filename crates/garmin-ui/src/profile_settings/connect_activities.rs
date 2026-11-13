use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::garmin_connect::{ActivityRequest, ActivityResponse, RemoteActivity};

use super::Props;
use crate::{Size, activity, button, icons, select};

mod batch;
pub use batch::{Batch, Counts, FailureReason};

/// Feedback for one explicit import, separate from the cached browser page.
#[must_use]
pub fn receipt(intl: &garmin_i18n::Intl, response: &ActivityResponse) -> Option<String> {
    let ActivityResponse::Imported {
        activities,
        duplicate,
        ..
    } = response
    else {
        return None;
    };
    Some(if *duplicate {
        format_message!(intl, default_message: "This FIT file was already imported")
    } else {
        format_message!(intl, default_message: "{count, plural, one {# activity imported} other {# activities imported}}", values: { count: *activities })
    })
}

/// A profile-scoped cloud browser. Entering the page requests its first page once.
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<super::Action> {
    let id = props.id.with("connect-activity-browser");
    let requested = ui.data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    if props.garmin_connect.activities.is_some() {
        ui.data_mut(|data| data.insert_temp(id, false));
    }
    let enabled = props.garmin_connect.configured
        && !props.disabled
        && !props.garmin_connect.checking
        && !props.garmin_connect.activities_busy
        && props
            .garmin_connect
            .batch
            .is_none_or(|batch| !batch.running());
    egui::Frame::NONE
        .inner_margin(24)
        .show(ui, |ui| {
            ui.heading(format_message!(props.intl, default_message: "Garmin activities"));
            ui.label(format_message!(props.intl, default_message: "Select an activity to preview"));
            ui.add_space(16.0);
            let request = browser(ui, props, enabled);
            if !requested && props.garmin_connect.activities.is_none() && enabled {
                ui.data_mut(|data| data.insert_temp(id, true));
                Some(super::Action::GarminConnectActivity(
                    ActivityRequest::List { start: 0 },
                ))
            } else {
                request
            }
        })
        .inner
}

fn browser(ui: &mut Ui, props: &Props<'_>, enabled: bool) -> Option<super::Action> {
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    let mut request = None;
    let Some(ActivityResponse::Page {
        start,
        activities,
        has_more,
    }) = props.garmin_connect.activities
    else {
        if props.garmin_connect.activities_busy {
            progress(ui, props);
        } else {
            let refresh = format_message!(props.intl, default_message: "Refresh");
            if action(ui, &refresh, Some(icons::ARROWS_CLOCKWISE), enabled) {
                return Some(super::Action::GarminConnectActivity(
                    ActivityRequest::List { start: 0 },
                ));
            }
        }
        return None;
    };
    let mut batch_action = toolbar(ui, props, enabled, *start, activities);
    if let Some(batch) = props.garmin_connect.batch
        && batch_status(ui, props, batch)
    {
        batch_action = Some(super::Action::CancelGarminConnectPageImport);
    }
    let height = if activities.is_empty() {
        96.0
    } else {
        let footer = if ui.available_width() < 360.0 {
            96.0
        } else {
            64.0
        };
        (ui.available_height() - footer).max(96.0)
    };
    egui::ScrollArea::vertical()
        .id_salt((props.id, "cloud-rows", start))
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if activities.is_empty() {
                ui.label(
                    format_message!(props.intl, default_message: "No activities on this page"),
                );
            }
            let columns = if ui.available_width() >= 800.0 { 2 } else { 1 };
            let width = (ui.available_width() - if columns == 2 { 8.0 } else { 0.0 })
                / if columns == 2 { 2.0 } else { 1.0 };
            egui::Grid::new("cloud-activity-grid")
                .num_columns(columns)
                .spacing(egui::vec2(8.0, 8.0))
                .show(ui, |ui| {
                    for (index, remote) in activities.iter().enumerate() {
                        ui.push_id(remote.id, |ui| {
                            ui.allocate_ui(egui::vec2(width, 56.0), |ui| {
                                let row = Row::new(remote, props);
                                let accent = remote
                                    .imported
                                    .then_some(crate::theme::palette(ui).support().success());
                                let response = ui.add_enabled_ui(enabled, |ui| {
                                    activity::selectable_row(ui, &row.props(&remote.name), accent)
                                });
                                if remote.imported {
                                    imported_badge(ui, response.inner.rect);
                                }
                                if response.inner.clicked() {
                                    request = Some(ActivityRequest::Preview { id: remote.id });
                                }
                            });
                        });
                        if (index + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                    if activities.len() % columns != 0 {
                        ui.end_row();
                    }
                });
        });
    ui.separator();
    pager(ui, props, *start, activities.len(), *has_more, enabled)
        .or(request)
        .map(super::Action::GarminConnectActivity)
        .or(batch_action)
}

fn toolbar(
    ui: &mut Ui,
    props: &Props<'_>,
    enabled: bool,
    start: u32,
    activities: &[RemoteActivity],
) -> Option<super::Action> {
    let mut result = None;
    ui.horizontal_wrapped(|ui| {
        let refresh = format_message!(props.intl, default_message: "Refresh");
        if action(ui, &refresh, Some(icons::ARROWS_CLOCKWISE), enabled) {
            result = Some(super::Action::GarminConnectActivity(
                ActivityRequest::List { start },
            ));
        }
        let import = format_message!(props.intl, default_message: "Import all on this page");
        if action(
            ui,
            &import,
            Some(icons::UPLOAD_SIMPLE),
            enabled && activities.iter().any(|activity| !activity.imported),
        ) {
            result = Some(super::Action::StartGarminConnectPageImport);
        }
        if props.garmin_connect.activities_busy
            && props
                .garmin_connect
                .batch
                .is_none_or(|batch| !batch.running())
        {
            progress(ui, props);
        }
    });
    result
}

fn batch_status(ui: &mut Ui, props: &Props<'_>, batch: &Batch) -> bool {
    let counts = batch.counts;
    let mut cancel = false;
    ui.horizontal_wrapped(|ui| {
        if batch.running() {
            ui.add(egui::Spinner::new().size(16.0));
            if let Some(name) = batch.current_name() { ui.label(format_message!(props.intl, default_message: "Importing {name}", values: { name: name })); }
            let label = if batch.cancelled() { format_message!(props.intl, default_message: "Stopping…") } else { format_message!(props.intl, default_message: "Cancel import") };
            cancel = action(ui, &label, None, !batch.cancelled());
        } else {
            ui.label(if batch.cancelled() { format_message!(props.intl, default_message: "Page import cancelled") } else if batch.stopped() { format_message!(props.intl, default_message: "Page import stopped") } else { format_message!(props.intl, default_message: "Page import complete") });
        }
    });
    let summary = format_message!(props.intl, default_message: "Processed {completed}/{total} · Imported {imported} · Skipped {skipped} · Failed {failed}", values: { completed: counts.processed() as u64, total: counts.total as u64, imported: counts.imported as u64, skipped: counts.skipped as u64, failed: counts.failed as u64 });
    if batch.running() && counts.total > 0 {
        crate::progress::show(
            ui,
            &crate::progress::Props {
                label: &summary,
                detail: None,
                value: crate::progress::Value::Determinate {
                    completed: counts.processed(),
                    total: counts.total,
                },
                height: Some(8.0),
            },
        );
    } else {
        ui.label(summary);
    }
    if !batch.failures.is_empty() {
        let label = format_message!(props.intl, default_message: "Failed imports");
        crate::accordion::show(
            ui,
            &crate::accordion::Props {
                id: "cloud-failed-imports",
                label: &label,
                default_open: false,
                inline_padding: 0,
            },
            |ui| {
                egui::ScrollArea::vertical().max_height(144.0).show(ui, |ui| {
                for failure in &batch.failures {
                    let reason = match &failure.reason { FailureReason::Request(reason) => reason.message.clone(), FailureReason::UnexpectedResponse => format_message!(props.intl, default_message: "Unexpected activity response") };
                    ui.label(format!("{}: {reason}", failure.name));
                }
            });
            },
        );
    }
    cancel
}

fn pager(
    ui: &mut Ui,
    props: &Props<'_>,
    start: u32,
    count: usize,
    has_more: bool,
    enabled: bool,
) -> Option<ActivityRequest> {
    let mut request = None;
    ui.horizontal_wrapped(|ui| {
        // Overlap the one-pixel outlines so joined controls share their dividers.
        ui.spacing_mut().item_spacing.x = -1.0;
        let previous = format_message!(props.intl, default_message: "Previous page");
        if arrow(ui, &previous, icons::CARET_LEFT, enabled && start > 0) {
            request = Some(ActivityRequest::List {
                start: start.saturating_sub(20),
            });
        }
        let current = start / 20;
        let known = current + u32::from(has_more);
        let pages = std::iter::once(0)
            .chain((current.saturating_sub(7)..=known).filter(|page| *page != 0))
            .collect::<Vec<_>>();
        let labels = pages.iter().map(|index| {
            let page = u64::from(index + 1);
            format_message!(props.intl, default_message: "Page {page}", values: { page: page })
        }).collect::<Vec<_>>();
        let choices = labels.iter().map(|label| select::Choice::new(label)).collect::<Vec<_>>();
        let mut selected = pages.iter().position(|page| *page == current).unwrap_or(0);
        let previous_selection = selected;
        select::show(ui, props.id.with("cloud-page"), &mut selected, &choices,
            select::Props::new("").width(112.0).disabled(!enabled).outlined(true));
        if selected != previous_selection {
            request = Some(ActivityRequest::List { start: pages[selected].saturating_mul(20) });
        }
        let next = format_message!(props.intl, default_message: "Next page");
        if arrow(ui, &next, icons::CARET_RIGHT, enabled && has_more) {
            request = Some(ActivityRequest::List {
                start: start.saturating_add(20),
            });
        }
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(16.0);
        let first = if count == 0 { 0 } else { u64::from(start) + 1 };
        let last = if count == 0 { 0 } else { u64::from(start) + count as u64 };
        ui.label(format_message!(props.intl, default_message: "Activities {first}–{last}", values: { first: first, last: last }));
    });
    request
}

fn arrow(ui: &mut Ui, label: &str, icon: icons::Icon, enabled: bool) -> bool {
    button::IconProps {
        label,
        icon,
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        enabled,
    }
    .show(ui)
    .clicked()
}

fn imported_badge(ui: &Ui, rect: egui::Rect) {
    let palette = crate::theme::palette(ui);
    let center = egui::pos2(rect.left() + 40.0, rect.center().y + 12.0);
    icons::Props {
        icon: icons::CHECK,
        size: 16.0,
        color: palette.support().success(),
    }
    .paint_at(ui, center);
}

struct Row {
    icon: icons::Icon,
    subtitle: String,
    distance: Option<String>,
    duration: String,
}

impl Row {
    fn new(remote: &RemoteActivity, props: &Props<'_>) -> Self {
        let icon = remote.sport.map_or(icons::ACTIVITY, activity::sport_icon);
        let mut subtitle = remote
            .started_at
            .as_deref()
            .map_or_else(String::new, |value| {
                value
                    .replace(' ', "T")
                    .parse::<jiff::civil::DateTime>()
                    .map_or_else(
                        |_| format!("{value} UTC"),
                        |date| format!("{} UTC", props.intl.dates().datetime(date)),
                    )
            });
        if remote.imported {
            let imported = format_message!(props.intl, default_message: "Imported");
            subtitle = format!("{imported} · {subtitle}");
        }
        let distance = remote.distance_metres.map(|metres| {
            activity::format_distance(millimeters(metres), props.preferences.unit_system())
        });
        let duration = remote
            .duration_seconds
            .and_then(|seconds| std::time::Duration::try_from_secs_f64(seconds).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .map_or_else(String::new, activity::duration);
        Self {
            icon,
            subtitle,
            distance,
            duration,
        }
    }

    fn props<'a>(&'a self, title: &'a str) -> activity::ItemProps<'a> {
        activity::ItemProps {
            icon: self.icon,
            title,
            subtitle: &self.subtitle,
            distance: self.distance.as_deref(),
            duration: &self.duration,
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "remote nonnegative metres are rounded to canonical millimetres; oversized values saturate"
)]
fn millimeters(metres: f64) -> u64 {
    (metres * 1000.0).round() as u64
}

fn progress(ui: &mut Ui, props: &Props<'_>) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(16.0));
        ui.label(format_message!(props.intl, default_message: "Contacting Garmin Connect…"));
    });
}

fn action(ui: &mut Ui, label: &str, icon: Option<icons::Icon>, enabled: bool) -> bool {
    button::Props {
        label,
        icon,
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
    .show(ui)
    .clicked()
}
