//! Shared account trends.
//! Only explicit actions request cache reads or synchronization.

#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Plot coordinates, heat intensities, and bounded calendar cell indices use floating pixel coordinates."
)]

use egui::{Color32, RichText, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_model::{
    garmin_connect::{
        sync::{Revision, Snapshot},
        trends::{History, Metric, Period, Range, Sport, cumulative},
    },
    identity::{ProfilePreferences, UnitSystem},
};
use jiff::civil::Date;

use crate::{segments, trend_chart};

mod controls;

#[derive(Default)]
enum ReadState {
    #[default]
    Idle,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Default)]
pub struct State {
    history: Option<History>,
    revision: Option<Revision>,
    read: ReadState,
    period: Period,
    anchor: Option<Date>,
    cumulative: bool,
    sport: usize,
    metric: Metric,
    selected: Option<Date>,
    target_draft: Option<[u32; 3]>,
    target_presentation: crate::modal::Presentation,
}

impl State {
    /// Constrain dialogs to their host surface for embedded previews.
    #[must_use]
    pub fn with_presentation(mut self, presentation: crate::modal::Presentation) -> Self {
        self.target_presentation = presentation;
        self
    }

    /// Open a draft without modifying the saved profile targets.
    pub fn edit_targets(&mut self, targets: [u32; 3]) {
        self.target_draft = Some(targets);
    }

    #[must_use]
    pub fn with_cumulative(mut self, cumulative: bool) -> Self {
        self.cumulative = cumulative;
        self
    }

    #[must_use]
    pub fn with_period(mut self, period: Period) -> Self {
        self.period = period;
        self
    }

    #[must_use]
    pub fn from_cache(history: Option<History>, revision: Revision) -> Self {
        Self {
            history,
            revision: Some(revision),
            read: ReadState::Ready,
            ..Self::default()
        }
    }

    pub fn complete(&mut self, revision: &Revision, result: Result<Option<History>, String>) {
        if self.revision.as_ref() != Some(revision) {
            return;
        }
        match result {
            Ok(history) => {
                self.history = history;
                self.read = ReadState::Ready;
            }
            Err(error) => self.read = ReadState::Failed(error),
        }
    }

    fn needs_read(&mut self, revision: &Revision) -> bool {
        if self
            .revision
            .as_ref()
            .is_some_and(|old| old.generation != revision.generation)
        {
            // A publication and an externally replaced account share this revision signal.
            // Discard the previous report before rendering or accepting another response.
            self.history = None;
            self.selected = None;
            self.revision = Some(revision.clone());
            self.read = ReadState::Idle;
        }
        matches!(self.read, ReadState::Idle) || self.revision.as_ref() != Some(revision)
    }
}

#[cfg(test)]
mod tests;

pub struct Props<'a> {
    pub intl: &'a Intl,
    pub today: Date,
    pub preferences: ProfilePreferences,
    pub sync: Option<&'a Snapshot>,
    pub enabled: bool,
    pub refreshing: bool,
    pub load_progress: Option<&'a garmin_model::garmin_connect::LoadProgress>,
}

pub enum Action {
    Read(Revision),
    Sync,
    SaveTargets([u32; 3]),
    Cancel(Option<garmin_model::garmin_connect::sync::JobId>),
    OpenActivities,
}

fn sport_label(intl: &Intl, sport: Sport) -> String {
    match sport {
        Sport::Running => format_message!(intl, default_message: "Running"),
        Sport::Cycling => format_message!(intl, default_message: "Cycling"),
        Sport::Swimming => format_message!(intl, default_message: "Swimming"),
        Sport::Walking => format_message!(intl, default_message: "Walking"),
        Sport::Hiking => format_message!(intl, default_message: "Hiking"),
        Sport::Other => format_message!(intl, default_message: "Other"),
    }
}

fn color(ui: &Ui, sport: Sport) -> Color32 {
    crate::theme::color32(garmin_color::charts::series(
        ui.visuals().dark_mode,
        Sport::ALL
            .iter()
            .position(|candidate| *candidate == sport)
            .unwrap_or(5),
    ))
}

fn label_color(ui: &Ui, sport: Sport) -> Color32 {
    crate::theme::color32(garmin_color::charts::text(
        ui.visuals().dark_mode,
        sport as usize,
    ))
}

fn card<R>(ui: &mut Ui, id: &str, content: impl FnOnce(&mut Ui) -> R) -> R {
    let fill = crate::theme::color32(
        crate::theme::palette(ui)
            .surfaces()
            .layer(garmin_color::theme::Level::One),
    );
    ui.push_id(id, |ui| {
        egui::Frame::NONE
            .fill(fill)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                crate::theme::layer(ui, garmin_color::theme::Level::Two, content)
            })
            .inner
    })
    .inner
}

fn range_label(intl: &Intl, range: Range) -> String {
    if range.from == range.through {
        intl.dates().date(range.from)
    } else {
        format!(
            "{} – {}",
            intl.dates().date(range.from),
            intl.dates().date(range.through)
        )
    }
}

fn unit(preferences: ProfilePreferences) -> (&'static str, f64) {
    match preferences.unit_system() {
        UnitSystem::Metric => ("km", 1000.0),
        UnitSystem::Imperial => ("mi", 1609.344),
    }
}

fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

fn values(
    history: &History,
    range: Range,
    sport: Option<Sport>,
    metric: Metric,
    period: Period,
    accumulated: bool,
    preferences: ProfilePreferences,
) -> (Vec<String>, Vec<Option<f64>>) {
    let mut buckets = history.series(range, sport, metric, period == Period::Year);
    if accumulated {
        cumulative(&mut buckets);
    }
    let labels = buckets
        .iter()
        .map(|bucket| bucket.date.to_string())
        .collect();
    let divisor = match metric {
        Metric::Distance => unit(preferences).1,
        Metric::Duration => 3600.0,
        _ => 1.0,
    };
    (
        labels,
        buckets
            .into_iter()
            .map(|bucket| bucket.value.map(|value| value / divisor))
            .collect(),
    )
}

pub fn show(ui: &mut Ui, state: &mut State, props: &Props<'_>) -> Option<Action> {
    let bounds = ui.max_rect();
    ui.visuals_mut().override_text_color = Some(crate::theme::color32(
        garmin_color::charts::foreground(ui.visuals().dark_mode),
    ));
    ui.painter().rect_filled(
        ui.max_rect(),
        0,
        crate::theme::color32(crate::theme::palette(ui).surfaces().background()),
    );
    let revision = props.sync.map(Snapshot::revision).unwrap_or_default();
    let needs_read = state.needs_read(&revision);
    let mut output = None;
    egui::ScrollArea::vertical().id_salt("trends-scroll").show(ui, |ui| {
        ui.scope_builder(egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::Min)),|ui|{
        egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.heading(format_message!(props.intl, default_message: "Trends"));
            ui.label(format_message!(props.intl, default_message: "Distance, time and consistency across your sports."));
            ui.add_space(8.0);
            output = controls::toolbar(ui, state, props);
            let Ok((range, previous)) = state.period.ranges_with_week_start(state.anchor.unwrap_or(props.today), props.today, props.intl.dates().week_start()) else { return; };
            ui.label(range_label(props.intl, range));
            if let Some(action) = controls::sync_feedback(ui, props) { output = Some(action); }
            if let ReadState::Failed(error) = &state.read {
                crate::notification::show(ui, &crate::notification::Props { kind: crate::notification::Kind::Error, title: &format_message!(props.intl, default_message: "Cached history could not be read"), detail: Some(error) });
            }
            let Some(history) = state.history.as_ref() else {
                ui.add_space(8.0);
                card(ui, "empty", |ui| {
                    ui.label(format_message!(props.intl, default_message: "Your activity history will appear here"));
                    if matches!(state.read, ReadState::Loading) || needs_read {
                        ui.spinner();
                    } else if props.refreshing || props.sync.is_some_and(|sync| sync.job.is_some()) {
                        ui.label(format_message!(props.intl, default_message: "Sync is running. Trends use the last published summary cache."));
                    } else {
                        ui.label(format_message!(props.intl, default_message: "Sync your Garmin account to see trends. Activities remain unimported."));
                    }
                });
                return;
            };
            if let Some(first) = history.days.first() {
                ui.weak(format_message!(props.intl, default_message: "Cached activity history starts {date}", values: { date: props.intl.dates().date(first.date) }));
            }
            if history.undated > 0 {
                ui.weak(format_message!(props.intl, default_message: "{count} summaries have no usable local date and are excluded from period totals.", values: { count: history.undated as u64 }));
            }
            let totals = history.totals(range, None);
            if totals.activities == 0 {
                ui.label(format_message!(props.intl, default_message: "No activities recorded in this period"));
            }
            if totals.missing_distance > 0 || totals.missing_duration > 0 {
                ui.weak(format_message!(props.intl, default_message: "Some measurements are missing. Totals include known values only; charts leave gaps."));
            }
            ui.add_space(8.0);
            dashboard(ui, state, props, range, previous);
        });
        });
    });
    let mut dialog_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("trends-dialog")
            .max_rect(bounds)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    if let Some(action) = controls::targets_dialog(&mut dialog_ui, state, props) {
        output = Some(action);
    }
    if output.is_none() && (needs_read || matches!(state.read, ReadState::Idle)) {
        state.revision = Some(revision.clone());
        state.read = ReadState::Loading;
        output = Some(Action::Read(revision));
    }
    output
}

fn dashboard(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range, previous: Range) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 16.0);
        top_widgets(ui, state, props, range);
        metrics(ui, state, props, range, previous);
        lower_widgets(ui, state, props, range, previous);
    });
}

fn fill_row<const N: usize>(ui: &Ui, slots: [egui::layers::ShapeIdx; N], rects: [egui::Rect; N]) {
    let bottom = rects
        .iter()
        .map(egui::Rect::bottom)
        .fold(f32::NEG_INFINITY, f32::max);
    let fill = crate::theme::color32(
        crate::theme::palette(ui)
            .surfaces()
            .layer(garmin_color::theme::Level::One),
    );
    for (slot, mut rect) in slots.into_iter().zip(rects) {
        rect.max.y = bottom;
        ui.painter()
            .set(slot, egui::Shape::rect_filled(rect, 0, fill));
    }
}

fn top_widgets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    if ui.available_width() < 880.0 {
        targets(ui, state, props, range);
        distances(ui, state, props, range);
        return;
    }
    let slots = [
        ui.painter().add(egui::Shape::Noop),
        ui.painter().add(egui::Shape::Noop),
    ];
    let rects = ui.columns(2, |columns| {
        targets(&mut columns[0], state, props, range);
        distances(&mut columns[1], state, props, range);
        [columns[0].min_rect(), columns[1].min_rect()]
    });
    fill_row(ui, slots, rects);
}

fn targets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    card(ui, "targets", |ui| {
        let goals = props.preferences.monthly_distance_targets();
        let monthly_goals = (state.period == Period::Month).then_some(goals);
        let has_goals = state.period == Period::Month && goals.iter().any(|goal| *goal > 0);
        ui.strong(if has_goals {
            format_message!(props.intl, default_message: "Distance targets")
        } else {
            format_message!(props.intl, default_message: "Distance by sport")
        });
        ui.add_space(16.0);
        if let Some(history) = &state.history {
            if has_goals {
                if ui.available_width() >= 400.0 {
                    ui.horizontal_top(|ui| {
                        rings(ui, history, range, goals);
                        ui.vertical(|ui| target_rows(ui, history, props, range, Some(goals)));
                    });
                } else {
                    rings(ui, history, range, goals);
                    target_rows(ui, history, props, range, Some(goals));
                }
            } else {
                target_rows(ui, history, props, range, monthly_goals);
            }
        }
        ui.horizontal(|ui| {
            if controls::button(
                ui,
                &format_message!(props.intl, default_message: "Edit monthly distance targets"),
                crate::icons::PENCIL,
                crate::button::Kind::Ghost,
                props.enabled,
            )
            .clicked()
            {
                state.edit_targets(goals);
            }
        });
    });
}

fn target_rows(
    ui: &mut Ui,
    history: &History,
    props: &Props<'_>,
    range: Range,
    goals: Option<[u32; 3]>,
) {
    for (index, sport) in Sport::ALL.into_iter().enumerate() {
        let totals = history.totals(range, Some(sport));
        if index >= 3 && totals.activities == 0 {
            continue;
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                crate::typography::semibold(sport_label(props.intl, sport))
                    .color(label_color(ui, sport)),
            );
            let known = if totals.missing_distance > 0 {
                format_message!(props.intl, default_message: "known distance")
            } else {
                String::new()
            };
            let goal = goals
                .and_then(|goals| goals.get(index).copied())
                .filter(|goal| *goal > 0);
            let distance = format!(
                "{:.1} {} {}",
                totals.distance_metres / unit(props.preferences).1,
                unit(props.preferences).0,
                known
            );
            if let Some(goal) = goal {
                ui.label(format!(
                    "{} / {:.1} {}",
                    distance,
                    f64::from(goal) * 1000.0 / unit(props.preferences).1,
                    unit(props.preferences).0
                ));
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(distance);
                    ui.weak(if goals.is_some() {
                        format_message!(props.intl, default_message: "No target")
                    } else {
                        format_message!(props.intl, default_message: "Monthly targets only")
                    });
                });
            }
            let progress = goal
                .filter(|_| totals.missing_distance == 0)
                .map(|goal| (totals.distance_metres / (f64::from(goal) * 1000.0)).min(1.0) as f32);
            ui.add_space(4.0);
            target_track(ui, sport, progress);
        });
        ui.add_space(24.0);
    }
}

fn target_track(ui: &mut Ui, sport: Sport, progress: Option<f32>) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
    let track = crate::theme::color32(crate::theme::palette(ui).borders().subtle());
    ui.painter().rect_filled(rect, 0, track);
    if let Some(progress) = progress.filter(|progress| *progress > 0.0) {
        let fill =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * progress, rect.height()));
        ui.painter().rect_filled(fill, 0, color(ui, sport));
    }
}

fn rings(ui: &mut Ui, history: &History, range: Range, goals: [u32; 3]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 176.0), egui::Sense::hover());
    let center = rect.center();
    let track = crate::theme::color32(crate::theme::palette(ui).borders().subtle());
    let mut labels = Vec::new();
    for (index, goal) in goals.into_iter().enumerate() {
        let radius = 72.0 - index as f32 * 16.0;
        ui.painter().add(ring_path(center, radius, 1.0, track));
        if goal == 0 {
            continue;
        }
        let sport = Sport::ALL[index];
        let totals = history.totals(range, Some(sport));
        if totals.missing_distance > 0 {
            continue;
        }
        let ratio = totals.distance_metres / (f64::from(goal) * 1000.0);
        let end = -std::f32::consts::FRAC_PI_2 + ratio.min(1.0) as f32 * std::f32::consts::TAU;
        if ratio > 0.0 {
            ui.painter().add(ring_path(
                center,
                radius,
                ratio.min(1.0) as f32,
                color(ui, sport),
            ));
        }
        let endpoint = center + egui::vec2(end.cos(), end.sin()) * radius;
        labels.push((endpoint, sport, ratio));
    }
    labels.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));
    let mut previous_y = rect.top();
    for (endpoint, sport, ratio) in labels {
        let outward = (endpoint - center).normalized() * 8.0;
        let y = (endpoint.y + outward.y)
            .max(previous_y + 16.0)
            .min(rect.bottom() - 8.0);
        previous_y = y;
        let left = endpoint.x < center.x;
        let x = if left {
            rect.left() + 32.0
        } else {
            rect.right() - 32.0
        };
        let line_x = if left { x + 4.0 } else { x - 4.0 };
        ui.painter().add(egui::Shape::line(
            vec![
                endpoint,
                egui::pos2(endpoint.x + outward.x, y),
                egui::pos2(line_x, y),
            ],
            egui::Stroke::new(1.0, label_color(ui, sport)),
        ));
        ui.painter().text(
            egui::pos2(x, y),
            if left {
                egui::Align2::RIGHT_CENTER
            } else {
                egui::Align2::LEFT_CENTER
            },
            format!("{:.0}%", ratio * 100.0),
            egui::FontId::proportional(12.0),
            label_color(ui, sport),
        );
    }
}

fn ring_path(center: egui::Pos2, radius: f32, progress: f32, color: Color32) -> egui::Shape {
    // Circle strokes in egui are outside-aligned; paths keep track and arc centered alike.
    if progress <= 0.0 {
        return egui::Shape::Noop;
    }
    let steps = (progress * 128.0).ceil() as usize;
    let mut points = (0..=steps)
        .map(|step| {
            let fraction = (step as f32 / 128.0).min(progress);
            let angle = -std::f32::consts::FRAC_PI_2 + fraction * std::f32::consts::TAU;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect::<Vec<_>>();
    let stroke = egui::Stroke::new(10.0, color);
    if progress >= 1.0 {
        points.pop();
        egui::Shape::closed_line(points, stroke)
    } else {
        let start = points[0];
        let end = points[points.len() - 1];
        egui::Shape::Vec(vec![
            egui::Shape::line(points, stroke),
            egui::Shape::circle_filled(start, stroke.width / 2.0, color),
            egui::Shape::circle_filled(end, stroke.width / 2.0, color),
        ])
    }
}

fn distances(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    card(ui, "distance-chart", |ui| {
        distance_header(ui, state, props.intl);
        let Some(history) = &state.history else {
            return;
        };
        for sport in Sport::ALL {
            let total = history.totals(range, Some(sport));
            if !matches!(sport, Sport::Running | Sport::Cycling | Sport::Swimming)
                && total.activities == 0
            {
                continue;
            }
            let label = sport_label(props.intl, sport);
            ui.label(
                RichText::new(format!(
                    "{} · {:.1} {}",
                    label,
                    total.distance_metres / unit(props.preferences).1,
                    unit(props.preferences).0
                ))
                .color(label_color(ui, sport)),
            );
            let (labels, values) = values(
                history,
                range,
                Some(sport),
                Metric::Distance,
                state.period,
                state.cumulative,
                props.preferences,
            );
            trend_chart::show(
                ui,
                format!("sport-{sport:?}"),
                trend_chart::Props {
                    labels: &labels,
                    series: &[trend_chart::Series {
                        name: &label,
                        values: &values,
                        color: color(ui, sport),
                        dashed: false,
                        dates: None,
                    }],
                    kind: if state.cumulative {
                        trend_chart::Kind::Lines
                    } else {
                        trend_chart::Kind::Bars
                    },
                    height: 64.0,
                    integral: false,
                    intl: props.intl,
                    unit: unit(props.preferences).0,
                },
            );
        }
    });
}

fn distance_header(ui: &mut Ui, state: &mut State, intl: &Intl) {
    let title = format_message!(intl, default_message: "Distance over time");
    let choices = [
        (
            false,
            if state.period == Period::Year {
                format_message!(intl, default_message: "Monthly")
            } else {
                format_message!(intl, default_message: "Daily")
            },
        ),
        (true, format_message!(intl, default_message: "Cumulative")),
    ];
    let width = segments::natural_width(ui, &choices);
    chart_header(ui, &title, width, |ui| {
        segments::show(ui, &mut state.cumulative, &choices, "trends.shape");
    });
}

fn chart_header(ui: &mut Ui, title: &str, controls_width: f32, controls: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        let title_width = ui
            .painter()
            .layout_no_wrap(
                title.to_owned(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x;
        let width = controls_width.min(ui.available_width());
        let stacked = ui.available_width() < title_width + controls_width + 24.0;
        if stacked {
            ui.strong(title);
        }
        ui.horizontal(|ui| {
            if !stacked {
                ui.strong(title);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 32.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    controls,
                );
            });
        });
    });
}

fn metrics(ui: &mut Ui, state: &State, props: &Props<'_>, range: Range, previous: Range) {
    let Some(history) = &state.history else {
        return;
    };
    let total = history.totals(range, None);
    let prior = history.totals(previous, None);
    let metric_data = [
        (
            Metric::Count,
            format_message!(props.intl, default_message: "Activities"),
            total.activities.to_string(),
            prior.activities as f64,
        ),
        (
            Metric::Duration,
            format_message!(props.intl, default_message: "Activity time"),
            duration(total.duration_seconds),
            prior.duration_seconds,
        ),
        (
            Metric::ActiveDays,
            format_message!(props.intl, default_message: "Active days"),
            format!("{} / {}", history.active_days(range), range.dates().len()),
            history.active_days(previous) as f64,
        ),
    ];
    let draw = |ui: &mut Ui, index: usize| {
        let (metric, label, value, previous_value) = &metric_data[index];
        card(ui, &format!("metric-{index}"), |ui| {
            ui.label(label);
            ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(112.0,64.0),egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.add(egui::Label::new(crate::typography::semibold(value).size(24.0)).extend());
            let current = match metric {
                Metric::Count => total.activities as f64,
                Metric::Duration => total.duration_seconds,
                _ => history.active_days(range) as f64,
            };
            if *metric == Metric::Duration && (total.missing_duration > 0 || prior.missing_duration > 0) {
                crate::typography::helper(ui,&format_message!(props.intl, default_message: "Known values only"));
            } else if *previous_value > 0.0 {
                crate::typography::helper(ui,&format_message!(props.intl, default_message: "{change}% vs previous period", values:{change:format!("{:+.0}",(current/previous_value-1.0)*100.0)}));
            } else {
                crate::typography::helper(ui,&format_message!(props.intl, default_message: "No previous activity"));
            }
            });
            if *metric == Metric::Duration { ui.vertical(|ui| time_mix(ui, history, props, range)); return; }
            let (labels, values) = values(
                history,
                range,
                None,
                *metric,
                state.period,
                false,
                props.preferences,
            );
            trend_chart::show(
                ui,
                format!("metric-chart-{index}"),
                trend_chart::Props { labels: &labels,
                series: &[trend_chart::Series {
                    name: label,
                    values: &values,
                    color: color(ui, Sport::Running),
                    dashed: false,
                    dates: None,
                }],
                kind: trend_chart::Kind::Bars,
                height: 64.0,
                integral: *metric != Metric::Duration,
                intl: props.intl,
                unit: "",
                },
            );
            });
        });
    };
    if ui.available_width() >= 880.0 {
        let slots: [egui::layers::ShapeIdx; 3] =
            std::array::from_fn(|_| ui.painter().add(egui::Shape::Noop));
        let rects = ui.columns(3, |columns| {
            for (index, ui) in columns.iter_mut().enumerate() {
                draw(ui, index);
            }
            [
                columns[0].min_rect(),
                columns[1].min_rect(),
                columns[2].min_rect(),
            ]
        });
        fill_row(ui, slots, rects);
    } else {
        for index in 0..3 {
            draw(ui, index);
        }
    }
}

fn time_mix(ui: &mut Ui, history: &History, props: &Props<'_>, range: Range) {
    let total = history.totals(range, None).duration_seconds;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 8.0), egui::Sense::hover());
    let mut left = rect.left();
    for sport in Sport::ALL {
        let time = history.totals(range, Some(sport)).duration_seconds;
        if total > 0.0 {
            let right = left + width * (time / total) as f32;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    egui::pos2(right, rect.bottom()),
                ),
                0,
                color(ui, sport),
            );
            left = right;
        }
    }
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for sport in Sport::ALL {
            let totals = history.totals(range, Some(sport));
            if totals.activities == 0 {
                continue;
            }
            ui.label(
                RichText::new(format!(
                    "{} {}",
                    sport_label(props.intl, sport),
                    duration(totals.duration_seconds)
                ))
                .size(12.0),
            );
        }
    });
}

fn comparison(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range, previous: Range) {
    card(ui, "comparison", |ui| {
        comparison_header(ui, state, props.intl);
        let Some(history) = &state.history else {
            return;
        };
        let sport = Sport::ALL[state.sport];
        let (labels, current) = values(
            history,
            range,
            Some(sport),
            state.metric,
            state.period,
            true,
            props.preferences,
        );
        let (prior_dates, prior) = values(
            history,
            previous,
            Some(sport),
            state.metric,
            state.period,
            true,
            props.preferences,
        );
        let totals = history.totals(range, Some(sport));
        let missing = match state.metric {
            Metric::Distance => totals.missing_distance,
            Metric::Duration => totals.missing_duration,
            _ => 0,
        };
        let value = match state.metric {
            Metric::Distance => format!(
                "{:.1} {}",
                totals.distance_metres / unit(props.preferences).1,
                unit(props.preferences).0
            ),
            Metric::Duration => duration(totals.duration_seconds),
            _ => totals.activities.to_string(),
        };
        ui.label(RichText::new(value).size(24.0).strong());
        comparison_delta(
            ui,
            props.intl,
            current.last().copied().flatten(),
            prior.last().copied().flatten(),
            match state.metric {
                Metric::Distance => unit(props.preferences).0,
                Metric::Duration => "h",
                _ => "",
            },
        );
        if missing > 0 {
            ui.weak(format_message!(props.intl, default_message: "Known values only; missing measurements leave gaps."));
        }
        if totals.activities == 0 {
            ui.label(format_message!(props.intl, default_message: "No activities recorded in this period"));
            return;
        }
        let current_label = range_label(props.intl, range);
        let previous_label = range_label(props.intl, previous);
        let series = [
            trend_chart::Series {
                name: &current_label,
                values: &current,
                color: color(ui, sport),
                dashed: false,
                dates: None,
            },
            trend_chart::Series {
                name: &previous_label,
                values: &prior,
                color: ui.visuals().weak_text_color(),
                dashed: true,
                dates: Some(&prior_dates),
            },
        ];
        let frame = trend_chart::show(
            ui,
            "comparison-plot",
            trend_chart::Props {
                labels: &labels,
                series: &series,
                kind: if state.period == Period::Day {
                    trend_chart::Kind::Bars
                } else {
                    trend_chart::Kind::Lines
                },
                height: 192.0,
                integral: state.metric == Metric::Count,
                intl: props.intl,
                unit: match state.metric {
                    Metric::Distance => unit(props.preferences).0,
                    Metric::Duration => "h",
                    _ => "",
                },
            },
        );
        trend_chart::legend(ui, frame, &series);
    });
}

fn comparison_header(ui: &mut Ui, state: &mut State, intl: &Intl) {
    let title = format_message!(intl, default_message: "Period comparison");
    let sports = Sport::ALL
        .iter()
        .enumerate()
        .map(|(index, sport)| (index, sport_label(intl, *sport)))
        .collect::<Vec<_>>();
    let width = segments::natural_width(ui, &sports) + 16.0 + 128.0;
    chart_header(ui, &title, width, |ui| {
        comparison_controls(ui, state, intl, &sports);
    });
}

fn comparison_controls(ui: &mut Ui, state: &mut State, intl: &Intl, sports: &[(usize, String)]) {
    let width = segments::natural_width(ui, sports) + 16.0 + 128.0;
    let layout = if ui.available_width() >= width {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::top_down(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        segments::show(ui, &mut state.sport, sports, "trends.sport");
        let metric_labels = [
            format_message!(intl, default_message: "Distance"),
            format_message!(intl, default_message: "Activity time"),
            format_message!(intl, default_message: "Activities"),
        ];
        let mut selected = match state.metric {
            Metric::Duration => 1,
            Metric::Count => 2,
            _ => 0,
        };
        let choices = metric_labels
            .iter()
            .map(|label| crate::select::Choice::new(label))
            .collect::<Vec<_>>();
        crate::select::show(
            ui,
            ui.id().with("metric"),
            &mut selected,
            &choices,
            crate::select::Props::new("")
                .width(128.0)
                .size(crate::Size::Small),
        );
        state.metric = [Metric::Distance, Metric::Duration, Metric::Count][selected];
    });
}

fn lower_widgets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range, previous: Range) {
    if ui.available_width() >= 880.0 {
        let width = ui.available_width() - 296.0 - 16.0;
        let slots = [
            ui.painter().add(egui::Shape::Noop),
            ui.painter().add(egui::Shape::Noop),
        ];
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let comparison = ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| comparison(ui, state, props, range, previous),
            );
            let calendar = ui.allocate_ui_with_layout(
                egui::vec2(296.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| calendar(ui, state, props, range),
            );
            fill_row(
                ui,
                slots,
                [comparison.response.rect, calendar.response.rect],
            );
        });
    } else {
        comparison(ui, state, props, range, previous);
        calendar(ui, state, props, range);
    }
}

fn comparison_delta(
    ui: &mut Ui,
    intl: &Intl,
    current: Option<f64>,
    previous: Option<f64>,
    unit: &str,
) {
    let Some((current, previous)) = current.zip(previous) else {
        return;
    };
    let difference = if unit.is_empty() {
        format!("{:+.0}", current - previous)
    } else {
        format!("{:+.1} {}", current - previous, unit)
    };
    if previous > 0.0 {
        ui.weak(format_message!(intl,default_message:"{difference} ({percent}%) vs previous period",values:{difference:difference.as_str(),percent:format!("{:+.0}",(current/previous-1.0)*100.0)}));
    } else {
        let no_previous = format_message!(intl, default_message: "No previous activity");
        ui.weak(format!("{difference} · {no_previous}"));
    }
}

fn calendar(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    card(ui, "calendar", |ui| {
        ui.strong(format_message!(props.intl, default_message: "Activity calendar"));
        let month = props.intl.dates().month(range.through);
        ui.label(&month.title);
        let Some(history) = &state.history else {
            return;
        };
        let gap = 2.0;
        let cell = egui::vec2(((ui.available_width() - 6.0 * gap) / 7.0).max(0.0), 32.0);
        let cells = month
            .days
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |last| (last + 1).div_ceil(7) * 7);
        let weeks = (cells / 7) as f32;
        let size = egui::vec2(ui.available_width(), (weeks + 1.0) * cell.y + weeks * gap);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        calendar_weekdays(ui, &month, rect, cell, gap);
        let Some((first_index, first)) = calendar_first(&month) else {
            return;
        };
        for index in 0..cells {
            let offset =
                i32::try_from(index).unwrap_or(0) - i32::try_from(first_index).unwrap_or(0);
            let Ok(date) = first.checked_add(jiff::Span::new().days(offset)) else {
                continue;
            };
            let day = Range {
                from: date,
                through: date,
            };
            let totals = history.totals(day, None);
            let current_month = month.days[index].is_some();
            let enabled = date <= props.today;
            let origin = rect.min
                + egui::vec2(
                    (index % 7) as f32 * (cell.x + gap),
                    ((index / 7) + 1) as f32 * (cell.y + gap),
                );
            let tile = egui::Rect::from_min_size(origin, cell);
            let fill = calendar_fill(ui, tile, &totals, current_month);
            let label = month.days[index]
                .as_ref()
                .map_or_else(|| date.day().to_string(), |day| day.label.clone());
            let text_color = calendar_text_color(ui, fill, current_month);
            ui.painter().text(
                tile.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                text_color,
            );
            let response = ui.interact(
                tile,
                ui.id().with(date),
                if enabled {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    enabled,
                    state.selected == Some(date),
                    props.intl.dates().date(date),
                )
            });
            if response.clicked() && enabled {
                state.selected = Some(date);
            }
            if state.selected == Some(date) {
                ui.painter().rect_stroke(
                    tile,
                    0,
                    egui::Stroke::new(1.0, ui.visuals().text_color()),
                    egui::StrokeKind::Inside,
                );
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(if enabled {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Default
                });
                response.on_hover_text(format!(
                    "{} · {} · {}",
                    props.intl.dates().date(date),
                    totals.activities,
                    duration(totals.duration_seconds)
                ));
            }
        }
        calendar_readout(ui, props, history, state.selected);
        calendar_legend(ui, props.intl);
    });
}

fn calendar_text_color(ui: &Ui, fill: Color32, current_month: bool) -> Color32 {
    if current_month {
        let fill = garmin_color::Color::from_u32(u32::from_be_bytes(fill.to_srgba_unmultiplied()));
        crate::theme::color32(garmin_color::charts::on_fill(fill))
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.45)
    }
}

fn calendar_fill(
    ui: &Ui,
    tile: egui::Rect,
    totals: &garmin_model::garmin_connect::trends::Totals,
    current_month: bool,
) -> Color32 {
    let intensity = (totals.duration_seconds / 7200.0).min(1.0) as f32;
    let intensity = if totals.activities > 0 && totals.missing_duration == 0 {
        intensity.max(0.15)
    } else {
        0.0
    };
    let fill = calendar_color(ui, intensity);
    let fill = if current_month {
        fill
    } else {
        let background = crate::theme::color32(
            crate::theme::palette(ui)
                .surfaces()
                .layer(garmin_color::theme::Level::One),
        );
        fill.lerp_to_gamma(background, 0.75)
    };
    ui.painter().rect_filled(tile, 0, fill);
    if totals.missing_duration > 0 {
        let painter = ui.painter().with_clip_rect(tile);
        for offset in (-32..64).step_by(6) {
            painter.line_segment(
                [
                    tile.left_top() + egui::vec2(offset as f32, 0.0),
                    tile.left_bottom() + egui::vec2(offset as f32 + 32.0, 0.0),
                ],
                egui::Stroke::new(
                    1.0,
                    ui.visuals()
                        .weak_text_color()
                        .gamma_multiply(if current_month { 0.3 } else { 0.1 }),
                ),
            );
        }
    }
    fill
}

fn calendar_color(ui: &Ui, intensity: f32) -> Color32 {
    let neutral = crate::theme::color32(
        crate::theme::palette(ui)
            .surfaces()
            .layer(garmin_color::theme::Level::Two),
    );
    neutral.lerp_to_gamma(color(ui, Sport::Running), intensity)
}

fn calendar_legend(ui: &mut Ui, intl: &Intl) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.weak(format_message!(intl, default_message: "0 min"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for intensity in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let (rect, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 0, calendar_color(ui, intensity));
            }
        });
        ui.weak(format_message!(intl, default_message: "2 h+"));
    });
}

fn calendar_weekdays(
    ui: &Ui,
    month: &garmin_i18n::MonthGrid,
    rect: egui::Rect,
    cell: egui::Vec2,
    gap: f32,
) {
    for (index, label) in month.weekdays.iter().enumerate() {
        ui.painter().text(
            rect.min + egui::vec2(index as f32 * (cell.x + gap) + cell.x / 2.0, cell.y / 2.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
    }
}

fn calendar_first(month: &garmin_i18n::MonthGrid) -> Option<(usize, Date)> {
    month
        .days
        .iter()
        .enumerate()
        .find_map(|(index, day)| day.as_ref().map(|day| (index, day.date)))
}

fn calendar_readout(ui: &mut Ui, props: &Props<'_>, history: &History, date: Option<Date>) {
    let totals = date.map(|date| {
        history.totals(
            Range {
                from: date,
                through: date,
            },
            None,
        )
    });
    if let Some(date) = date {
        ui.strong(props.intl.dates().date(date));
    } else {
        ui.weak(
            format_message!(props.intl, default_message: "Select a day to pin its totals here."),
        );
    }
    let empty = "—".to_owned();
    let distance = totals.as_ref().map_or_else(
        || empty.clone(),
        |totals| {
            format!(
                "{:.1} {}",
                totals.distance_metres / unit(props.preferences).1,
                unit(props.preferences).0
            )
        },
    );
    let activities = totals
        .as_ref()
        .map_or_else(|| empty.clone(), |totals| totals.activities.to_string());
    let time = totals
        .as_ref()
        .map_or_else(|| empty.clone(), |totals| duration(totals.duration_seconds));
    crate::facts::show(
        ui,
        "calendar-totals",
        &[
            (
                format_message!(props.intl, default_message: "Activities"),
                activities,
            ),
            (
                format_message!(props.intl, default_message: "Distance"),
                distance,
            ),
            (
                format_message!(props.intl, default_message: "Activity time"),
                time,
            ),
        ],
    );
    if totals.is_some_and(|totals| totals.missing_distance > 0 || totals.missing_duration > 0) {
        ui.weak(format_message!(props.intl, default_message: "Known values only"));
    }
}
