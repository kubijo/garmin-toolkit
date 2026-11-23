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
    activity::{Distance, HeartRate, Power},
    artifact::ArtifactDigest,
    garmin_connect::{
        sync::{Revision, Snapshot},
        trends::{
            ActivityIdentity, BucketValue, History, Metric, Period, Range, Sport, Totals,
            cumulative,
        },
    },
    identity::{MonthlyDistanceTarget, ProfilePreferences, UnitSystem},
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
    import_revision: Option<ArtifactDigest>,
    read_token: Option<uuid::Uuid>,
    read: ReadState,
    period: Period,
    anchor: Option<Date>,
    custom_range: Option<(Range, Range)>,
    range_draft: Option<[String; 2]>,
    cumulative: bool,
    sport: usize,
    comparison_initialized: bool,
    activity_pair: [Option<ActivityIdentity>; 2],
    metric: Metric,
    selected: Option<Date>,
    target_draft: Option<[MonthlyDistanceTarget; 3]>,
    target_presentation: crate::modal::Presentation,
}

impl State {
    fn is_preset(&self, period: Period) -> bool {
        self.custom_range.is_none() && self.period == period
    }

    fn initialize_comparison(&mut self, range: Range) {
        if self.comparison_initialized {
            return;
        }
        let Some(history) = &self.history else {
            return;
        };
        if let Some(index) = Sport::ALL
            .iter()
            .position(|sport| history.totals(range, Some(*sport)).activities > 0)
        {
            self.sport = index;
        }
        self.comparison_initialized = true;
    }

    /// Constrain dialogs to their host surface for embedded previews.
    #[must_use]
    pub fn with_presentation(mut self, presentation: crate::modal::Presentation) -> Self {
        self.target_presentation = presentation;
        self
    }

    /// Open a draft without modifying the saved profile targets.
    pub fn edit_targets(&mut self, targets: [MonthlyDistanceTarget; 3]) {
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
    pub fn with_comparison_metric(mut self, metric: Metric) -> Self {
        if metric == Metric::PeakPower {
            self.sport = Sport::Cycling as usize;
            self.comparison_initialized = true;
        }
        self.metric = metric;
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

    pub fn observe_import_revision(&mut self, revision: Option<ArtifactDigest>) {
        if self.import_revision != revision {
            self.import_revision = revision;
            self.read = ReadState::Idle;
        }
    }

    pub fn complete_read(
        &mut self,
        request: &ReadRequest,
        result: Result<Option<History>, String>,
    ) {
        if self.read_token == Some(request.token)
            && self.import_revision == request.import_revision
            && matches!(self.read, ReadState::Loading)
        {
            self.complete(&request.sync, result);
        }
    }

    fn begin_read(&mut self, sync: Revision) -> ReadRequest {
        let token = uuid::Uuid::new_v4();
        self.read_token = Some(token);
        self.revision = Some(sync.clone());
        self.read = ReadState::Loading;
        ReadRequest {
            sync,
            import_revision: self.import_revision,
            token,
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
            self.activity_pair = [None; 2];
            self.revision = Some(revision.clone());
            self.read = ReadState::Idle;
        }
        matches!(self.read, ReadState::Idle) || self.revision.as_ref() != Some(revision)
    }
}

mod activity_comparison;
#[cfg(test)]
mod tests;

pub struct Props<'a> {
    pub intl: &'a Intl,
    pub today: Date,
    pub preferences: ProfilePreferences,
    pub sync: Option<&'a Snapshot>,
    pub connect_configured: bool,
    pub enabled: bool,
    pub refreshing: bool,
    pub load_progress: Option<&'a garmin_model::garmin_connect::LoadProgress>,
}

pub enum Action {
    Read(ReadRequest),
    Sync,
    SaveTargets([MonthlyDistanceTarget; 3]),
    Cancel(Option<garmin_model::garmin_connect::sync::JobId>),
    OpenActivities,
}

#[derive(Clone)]
pub struct ReadRequest {
    sync: Revision,
    import_revision: Option<ArtifactDigest>,
    token: uuid::Uuid,
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

fn header_metadata(ui: &mut Ui, state: &State, props: &Props<'_>, range: Range) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 0.0;
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 4.0);
        ui.horizontal_wrapped(|ui| {
        let response = metadata_item(ui, range_label(props.intl, range), false);
        crate::semantics::target(ui, &response, "trends.range.active");
        crate::semantics::value(&response, format!("{}/{}", range.from, range.through));
        if let Some(last) = props.sync.and_then(|sync| sync.state.last_success) {
            metadata_item(ui, format_message!(props.intl, default_message: "Last successful sync: {date}", values: { date: props.intl.dates().datetime(last.as_jiff().to_zoned(jiff::tz::TimeZone::system()).datetime()) }), true);
        }
        if let Some(first) = state.history.as_ref().and_then(|history| history.days.first()) {
            metadata_item(ui, format_message!(props.intl, default_message: "Activity history starts {date}", values: { date: props.intl.dates().date(first.date) }), true);
        }
        });
    });
}

fn metadata_item(ui: &mut Ui, text: String, weak: bool) -> egui::Response {
    let text = egui::RichText::new(text);
    let text = if weak { text.weak() } else { text };
    let galley = egui::WidgetText::from(text.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    ui.add_sized(
        egui::vec2(galley.size().x.min(ui.max_rect().width()), galley.size().y),
        egui::Label::new(text).wrap(),
    )
}

fn unit(preferences: ProfilePreferences) -> (&'static str, f64) {
    match preferences.unit_system() {
        UnitSystem::Metric => ("km", 1000.0),
        UnitSystem::Imperial => ("mi", 1609.344),
    }
}

fn elevation_unit(preferences: ProfilePreferences) -> (&'static str, f64) {
    match preferences.unit_system() {
        UnitSystem::Metric => ("m", 1.0),
        UnitSystem::Imperial => ("ft", Distance::METRES_PER_FOOT),
    }
}

fn values(
    history: &History,
    range: Range,
    sport: Option<Sport>,
    metric: Metric,
    monthly: bool,
    accumulated: bool,
    preferences: ProfilePreferences,
) -> (Vec<String>, Vec<trend_chart::Sample>) {
    use trend_chart::Sample;
    let mut buckets = history.series(range, sport, metric, monthly);
    if accumulated {
        cumulative(&mut buckets);
    }
    let labels = buckets
        .iter()
        .map(|bucket| bucket.date.to_string())
        .collect();
    let units = preferences.unit_system();
    (
        labels,
        buckets
            .into_iter()
            .map(|bucket| match bucket.value {
                Some(BucketValue::Distance(distance)) => Sample::Value(match (metric, units) {
                    (Metric::Elevation, UnitSystem::Metric) => distance.as_metres(),
                    (Metric::Elevation, UnitSystem::Imperial) => distance.as_feet(),
                    (_, UnitSystem::Metric) => distance.as_kilometres(),
                    (_, UnitSystem::Imperial) => distance.as_miles(),
                }),
                Some(BucketValue::Duration(duration)) => Sample::Value(duration.as_hours()),
                Some(BucketValue::Count(count)) => Sample::Value(count as f64),
                Some(BucketValue::HeartRate(rate)) => Sample::HeartRate(rate),
                Some(BucketValue::Power(power)) => Sample::Power(power),
                None if metric == Metric::PeakPower => Sample::Empty,
                None if metric == Metric::AverageHeartRate && !bucket.has_activities => {
                    Sample::Empty
                }
                None => Sample::Missing,
            })
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
            let Ok(preset) = state.period.ranges_with_week_start(state.anchor.unwrap_or(props.today), props.today, props.intl.dates().week_start()) else { return; };
            controls::custom_range_editor(ui, state, props);
            let (range, previous) = state.custom_range.unwrap_or(preset);
            header_metadata(ui, state, props, range);
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
                    } else if props.connect_configured {
                        ui.label(format_message!(props.intl, default_message: "Import a FIT activity or sync your Garmin account to see trends."));
                    } else {
                        ui.label(format_message!(props.intl, default_message: "Import a FIT activity to see trends."));
                    }
                });
                return;
            };
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
        output = Some(Action::Read(state.begin_read(revision)));
    }
    output
}

fn dashboard(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range, previous: Range) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 16.0);
        top_widgets(ui, state, props, range);
        metrics(ui, state, props, range, previous);
        lower_widgets(ui, state, props, range, previous);
        activity_comparison::show(ui, state, props, range);
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

#[derive(Default)]
struct RowPlots {
    extra_per_plot: f32,
    count: usize,
}

impl RowPlots {
    fn height(&mut self, minimum: f32) -> f32 {
        self.count += 1;
        minimum + self.extra_per_plot
    }
}

/// Divide the height left by the neighboring card among the visible plots.
/// Measuring the whole chart card accounts for wrapped labels, axes and legends.
fn chart_row(
    ui: &mut Ui,
    id: &str,
    draw: impl FnOnce(&mut Ui, &mut RowPlots) -> (egui::Rect, egui::Rect),
) {
    let key = ui.id().with(id);
    let mut plots = RowPlots {
        extra_per_plot: ui.data(|data| data.get_temp(key)).unwrap_or_default(),
        count: 0,
    };
    let slots = [
        ui.painter().add(egui::Shape::Noop),
        ui.painter().add(egui::Shape::Noop),
    ];
    let (chart, neighbor) = draw(ui, &mut plots);
    fill_row(ui, slots, [chart, neighbor]);
    if plots.count > 0 {
        let remaining = neighbor.height() - chart.height();
        let extra = (plots.extra_per_plot + remaining / plots.count as f32).max(0.0);
        if (extra - plots.extra_per_plot).abs() > 0.5 {
            ui.data_mut(|data| data.insert_temp(key, extra));
            ui.ctx().request_discard("fit Trends plots to their row");
        }
    }
}

fn top_widgets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    if ui.available_width() < 880.0 {
        targets(ui, state, props, range);
        distances(ui, state, props, range, &mut RowPlots::default());
        return;
    }
    chart_row(ui, "distance-row", |ui, plots| {
        ui.columns(2, |columns| {
            targets(&mut columns[0], state, props, range);
            distances(&mut columns[1], state, props, range, plots);
            (columns[1].min_rect(), columns[0].min_rect())
        })
    });
}

fn targets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range) {
    card(ui, "targets", |ui| {
        let goals = props.preferences.monthly_distance_targets();
        let monthly_goals = state.is_preset(Period::Month).then_some(goals);
        let has_goals =
            monthly_goals.is_some() && goals.iter().any(|goal| goal.distance().is_some());
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
                "trends.targets.edit",
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
    goals: Option<[MonthlyDistanceTarget; 3]>,
) {
    for (index, sport) in Sport::ALL.into_iter().enumerate() {
        let totals = history.totals(range, Some(sport));
        let goal =
            goals.and_then(|goals| goals.get(index).and_then(MonthlyDistanceTarget::distance));
        if totals.activities == 0 && goal.is_none() {
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
            let units = props.preferences.unit_system();
            let decimal = props.intl.decimal_formatter();
            let distance = totals.distance.format_major(units, 1, &decimal);
            let distance = if known.is_empty() {
                distance
            } else {
                format!("{distance} {known}")
            };
            if let Some(goal) = goal {
                ui.label(format!(
                    "{} / {}",
                    distance,
                    goal.format_major(units, 1, decimal)
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
                .map(|goal| (totals.distance.as_metres() / goal.as_metres()).min(1.0) as f32);
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

fn rings(ui: &mut Ui, history: &History, range: Range, goals: [MonthlyDistanceTarget; 3]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 176.0), egui::Sense::hover());
    let center = rect.center();
    let track = crate::theme::color32(crate::theme::palette(ui).borders().subtle());
    let mut labels = Vec::new();
    for (ring, (index, goal)) in goals
        .into_iter()
        .enumerate()
        .filter_map(|(index, goal)| goal.distance().map(|distance| (index, distance)))
        .enumerate()
    {
        let radius = 72.0 - ring as f32 * 16.0;
        ui.painter().add(ring_path(center, radius, 1.0, track));
        let sport = Sport::ALL[index];
        let totals = history.totals(range, Some(sport));
        if totals.missing_distance > 0 {
            continue;
        }
        let ratio = totals.distance.as_metres() / goal.as_metres();
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

fn distances(
    ui: &mut Ui,
    state: &mut State,
    props: &Props<'_>,
    range: Range,
    plots: &mut RowPlots,
) {
    card(ui, "distance-chart", |ui| {
        distance_header(ui, state, props.intl);
        let Some(history) = &state.history else {
            return;
        };
        let goals = props.preferences.monthly_distance_targets();
        for (index, sport) in Sport::ALL.into_iter().enumerate() {
            let total = history.totals(range, Some(sport));
            let has_goal = goals
                .get(index)
                .and_then(MonthlyDistanceTarget::distance)
                .is_some();
            if total.activities == 0 && !has_goal {
                continue;
            }
            let label = sport_label(props.intl, sport);
            ui.label(
                RichText::new(format!(
                    "{} · {}",
                    label,
                    total.distance.format_major(
                        props.preferences.unit_system(),
                        1,
                        props.intl.decimal_formatter()
                    )
                ))
                .color(label_color(ui, sport)),
            );
            let (labels, values) = values(
                history,
                range,
                Some(sport),
                Metric::Distance,
                state.is_preset(Period::Year),
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
                    height: plots.height(64.0),
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
            if state.is_preset(Period::Year) {
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
            total.duration.format_hours_minutes(),
            prior.duration.as_seconds(),
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
                Metric::Duration => total.duration.as_seconds(),
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
                state.is_preset(Period::Year),
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
    let total = history.totals(range, None).duration.as_seconds();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 8.0), egui::Sense::hover());
    let mut left = rect.left();
    for sport in Sport::ALL {
        let time = history.totals(range, Some(sport)).duration.as_seconds();
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
                    totals.duration.format_hours_minutes()
                ))
                .size(12.0),
            );
        }
    });
}

fn comparison(
    ui: &mut Ui,
    state: &mut State,
    props: &Props<'_>,
    range: Range,
    previous: Range,
    plots: &mut RowPlots,
) {
    state.initialize_comparison(range);
    card(ui, "comparison", |ui| {
        comparison_header(ui, state, props.intl);
        let Some(history) = &state.history else {
            return;
        };
        let sport = Sport::ALL[state.sport];
        let elevation = state.metric == Metric::Elevation;
        let heart_rate = state.metric == Metric::AverageHeartRate;
        let power = state.metric == Metric::PeakPower;
        let accumulated = !(elevation || heart_rate || power);
        let monthly = state.is_preset(Period::Year);
        let series_for = |period| {
            values(
                history,
                period,
                Some(sport),
                state.metric,
                monthly,
                accumulated,
                props.preferences,
            )
        };
        let (labels, current) = series_for(range);
        let (prior_dates, prior) = series_for(previous);
        let totals = history.totals(range, Some(sport));
        let previous_totals = history.totals(previous, Some(sport));
        let missing = match state.metric {
            Metric::Distance => totals.missing_distance,
            Metric::Duration => totals.missing_duration,
            Metric::Elevation => totals.activities - totals.elevation_activities,
            _ => 0,
        };
        let value = comparison_value(state.metric, props, &totals);
        ui.label(RichText::new(value).size(24.0).strong());
        let chart_unit = comparison_unit(state.metric, props.preferences);
        if elevation {
            elevation_coverage(ui, props, &totals, &previous_totals);
        } else if heart_rate {
            heart_rate_coverage(ui, props.intl, &totals, &previous_totals);
        } else if power {
            power_coverage(ui, props.intl, &totals, &previous_totals);
        } else {
            comparison_delta(
                ui,
                props.intl,
                current.last().and_then(|sample| sample.coordinate()),
                prior.last().and_then(|sample| sample.coordinate()),
                chart_unit,
            );
        }
        if missing > 0 {
            ui.weak(format_message!(props.intl, default_message: "Known values only; missing measurements leave gaps."));
        }
        if totals.activities == 0 {
            ui.label(format_message!(props.intl, default_message: "No activities recorded in this period"));
            if !power || previous_totals.power_activities == 0 {
                return;
            }
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
        let kind = if !accumulated || range.from == range.through {
            trend_chart::Kind::Bars
        } else {
            trend_chart::Kind::Lines
        };
        let frame = trend_chart::show(
            ui,
            "comparison-plot",
            trend_chart::Props {
                labels: &labels,
                series: &series,
                kind,
                height: plots.height(192.0),
                integral: state.metric == Metric::Count || power,
                intl: props.intl,
                unit: chart_unit,
            },
        );
        trend_chart::legend(ui, frame, &series, kind);
    });
}

fn power_coverage(ui: &mut Ui, intl: &Intl, current: &Totals, prior: &Totals) {
    ui.weak(format_message!(intl, default_message: "Rides with recorded power · {covered, number}/{total, number} this period · {previous, number}/{prior_total, number} previous period", values: {
        covered: current.power_activities as u64, total: current.activities as u64,
        previous: prior.power_activities as u64, prior_total: prior.activities as u64
    }));
}

fn comparison_value(metric: Metric, props: &Props<'_>, totals: &Totals) -> String {
    match metric {
        Metric::Distance => totals.distance.format_major(
            props.preferences.unit_system(),
            1,
            props.intl.decimal_formatter(),
        ),
        Metric::Duration => totals.duration.format_hours_minutes(),
        Metric::Elevation if totals.elevation_activities > 0 => {
            totals.elevation_gain.format_elevation(
                props.preferences.unit_system(),
                props.intl.decimal_formatter(),
            )
        }
        Metric::Elevation => "—".to_owned(),
        Metric::AverageHeartRate => totals.heart_rate.value().map_or_else(
            || "—".to_owned(),
            |rate| rate.format(props.intl.decimal_formatter()),
        ),
        Metric::PeakPower => totals.peak_power.map_or_else(
            || "—".to_owned(),
            |power| power.format(props.intl.decimal_formatter()),
        ),
        _ => totals.activities.to_string(),
    }
}

fn elevation_coverage(ui: &mut Ui, props: &Props<'_>, current: &Totals, previous: &Totals) {
    let intl = props.intl;
    if current.activities > 0 {
        ui.weak(format_message!(intl, default_message: "Imported FIT elevation for {covered, number} of {total, number} activities", values: {
            covered: current.elevation_activities as u64, total: current.activities as u64
        }));
    }
    if previous.elevation_activities < previous.activities {
        ui.weak(format_message!(intl, default_message: "Previous period: {covered, number} of {total, number} activities have imported FIT elevation", values: {
            covered: previous.elevation_activities as u64, total: previous.activities as u64
        }));
    }
    if current.activities > 0
        && current.elevation_activities == current.activities
        && previous.elevation_activities == previous.activities
    {
        let (symbol, scale) = elevation_unit(props.preferences);
        comparison_delta(
            ui,
            intl,
            Some(current.elevation_gain.as_metres() / scale),
            Some(previous.elevation_gain.as_metres() / scale),
            symbol,
        );
    }
}

fn heart_rate_coverage(ui: &mut Ui, intl: &Intl, current: &Totals, previous: &Totals) {
    if current.activities > 0 {
        ui.weak(format_message!(intl, default_message: "Average of imported FIT heart rates from {covered, number} of {total, number} activities", values: {
            covered: current.heart_rate.activities() as u64, total: current.activities as u64
        }));
    }
    if previous.activities > 0 {
        ui.weak(format_message!(intl, default_message: "Previous period: {covered, number} of {total, number} activities have imported FIT heart rate", values: {
            covered: previous.heart_rate.activities() as u64, total: previous.activities as u64
        }));
    }
}

fn comparison_unit(metric: Metric, preferences: ProfilePreferences) -> &'static str {
    match metric {
        Metric::Distance => unit(preferences).0,
        Metric::Duration => "h",
        Metric::Elevation => elevation_unit(preferences).0,
        Metric::AverageHeartRate => HeartRate::SYMBOL,
        Metric::PeakPower => Power::SYMBOL,
        _ => "",
    }
}

const COMPARISON_METRIC_WIDTH: f32 = 160.0;

fn comparison_header(ui: &mut Ui, state: &mut State, intl: &Intl) {
    let title = format_message!(intl, default_message: "Period comparison");
    let sports = Sport::ALL
        .iter()
        .enumerate()
        .map(|(index, sport)| (index, sport_label(intl, *sport)))
        .collect::<Vec<_>>();
    let width = segments::natural_width(ui, &sports) + 16.0 + COMPARISON_METRIC_WIDTH;
    chart_header(ui, &title, width, |ui| {
        comparison_controls(ui, state, intl, &sports);
    });
}

fn comparison_controls(ui: &mut Ui, state: &mut State, intl: &Intl, sports: &[(usize, String)]) {
    let width = segments::natural_width(ui, sports) + 16.0 + COMPARISON_METRIC_WIDTH;
    let layout = if ui.available_width() >= width {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::top_down(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        segments::show(ui, &mut state.sport, sports, "trends.sport");
        let mut metrics = Vec::from([
            (
                Metric::Distance,
                "trends.metric.distance",
                format_message!(intl, default_message: "Distance"),
            ),
            (
                Metric::Duration,
                "trends.metric.duration",
                format_message!(intl, default_message: "Activity time"),
            ),
            (
                Metric::Count,
                "trends.metric.count",
                format_message!(intl, default_message: "Activities"),
            ),
            (
                Metric::Elevation,
                "trends.metric.elevation",
                format_message!(intl, default_message: "Elevation gain"),
            ),
            (
                Metric::AverageHeartRate,
                "trends.metric.heart-rate",
                format_message!(intl, default_message: "Average heart rate"),
            ),
        ]);
        if Sport::ALL[state.sport] == Sport::Cycling {
            metrics.push((
                Metric::PeakPower,
                "trends.metric.peak-power",
                format_message!(intl, default_message: "Peak power"),
            ));
        }
        let mut selected = metrics
            .iter()
            .position(|(metric, _, _)| *metric == state.metric)
            .unwrap_or(0);
        let choices = metrics
            .iter()
            .map(|(_, target, label)| crate::select::Choice::new(label).target(target))
            .collect::<Vec<_>>();
        let response = crate::select::show(
            ui,
            ui.id().with("metric"),
            &mut selected,
            &choices,
            crate::select::Props::new("")
                .width(COMPARISON_METRIC_WIDTH.min(ui.available_width()))
                .size(crate::Size::Small),
        );
        crate::semantics::target(ui, &response, "trends.metric");
        state.metric = metrics[selected].0;
    });
}

fn lower_widgets(ui: &mut Ui, state: &mut State, props: &Props<'_>, range: Range, previous: Range) {
    if ui.available_width() >= 880.0 {
        let width = ui.available_width() - 296.0 - 16.0;
        chart_row(ui, "comparison-row", |ui, plots| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let comparison = ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| comparison(ui, state, props, range, previous, plots),
                );
                let calendar = ui.allocate_ui_with_layout(
                    egui::vec2(296.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| calendar(ui, state, props, range),
                );
                (comparison.response.rect, calendar.response.rect)
            })
            .inner
        });
    } else {
        comparison(ui, state, props, range, previous, &mut RowPlots::default());
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
            crate::semantics::target(ui, &response, format!("trends.calendar.{date}"));
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
                    totals.duration.format_hours_minutes()
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
    let intensity = (totals.duration.as_seconds() / 7200.0).min(1.0) as f32;
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
            totals.distance.format_major(
                props.preferences.unit_system(),
                1,
                props.intl.decimal_formatter(),
            )
        },
    );
    let activities = totals
        .as_ref()
        .map_or_else(|| empty.clone(), |totals| totals.activities.to_string());
    let time = totals.as_ref().map_or_else(
        || empty.clone(),
        |totals| totals.duration.format_hours_minutes(),
    );
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
