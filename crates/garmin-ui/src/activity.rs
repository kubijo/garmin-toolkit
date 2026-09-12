//! Activity browsing components.

use cint::ColorInterop;
use egui::{Align2, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};
use garmin_model::{
    activity::{ActivitySport, ActivitySummary, Distance, HeartRate},
    identity::UnitSystem,
};
use garmin_service_api::ActivitySnapshot;

use crate::{icons, path, theme::color32};

const ROW_HEIGHT: f32 = 72.0;
const PADDING: f32 = 16.0;
const ICON_SIZE: f32 = 24.0;
const SELECTED_MARKER_WIDTH: f32 = 3.0;
const DETAIL_HEIGHT: f32 = 280.0;
const PATH_HEIGHT: f32 = 220.0;
const BROWSER_BREAKPOINT: f32 = 720.0;
const BROWSER_LIST_WIDTH: f32 = 360.0;
const METRIC_GAP: f32 = 1.0;
const METRIC_MAX_HEIGHT: f32 = 96.0;

pub struct Presentation {
    icon: icons::Icon,
    title: String,
    subtitle: String,
    distance: Option<String>,
    duration: String,
    metrics: Vec<Metric>,
}

impl Presentation {
    #[must_use]
    pub fn from_summary(
        summary: ActivitySummary,
        source: &str,
        intl: &Intl,
        units: UnitSystem,
    ) -> Self {
        Self::new(
            summary.sport(),
            summary.time().start().to_string(),
            source,
            summary.totals().timer().into_milliseconds(),
            summary.totals().distance().map(Distance::into_millimeters),
            summary
                .metrics()
                .average_heart_rate()
                .map(HeartRate::into_beats_per_minute),
            summary.totals().ascent().map(Distance::into_millimeters),
            intl,
            units,
        )
    }

    #[must_use]
    pub fn from_snapshot(snapshot: &ActivitySnapshot, intl: &Intl, units: UnitSystem) -> Self {
        Self::from_summary(snapshot.summary, &snapshot.source, intl, units)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments are the activity summary fields"
    )]
    fn new(
        sport: ActivitySport,
        started_at: String,
        source: &str,
        timer_milliseconds: u64,
        distance_millimeters: Option<u64>,
        average_heart_rate: Option<u16>,
        ascent_millimeters: Option<u64>,
        intl: &Intl,
        units: UnitSystem,
    ) -> Self {
        let title = sport_title(sport, intl);
        let subtitle = format_message!(
            intl,
            default_message: "{start} · {source}",
            values: {
                start: started_at,
                source: source,
            },
        );
        let distance = distance_millimeters.map(|value| format_distance(value, units));
        let duration = duration(timer_milliseconds);
        let mut metrics = Vec::with_capacity(4);
        if let Some(distance) = &distance {
            metrics.push(Metric {
                label: format_message!(intl, default_message: "Distance"),
                value: distance.clone(),
            });
        }
        metrics.push(Metric {
            label: format_message!(intl, default_message: "Active time"),
            value: duration.clone(),
        });
        if let Some(heart_rate) = average_heart_rate {
            metrics.push(Metric {
                label: format_message!(intl, default_message: "Average heart rate"),
                value: heart_rate.to_string(),
            });
        }
        if let Some(ascent) = ascent_millimeters {
            metrics.push(Metric {
                label: format_message!(intl, default_message: "Ascent"),
                value: format_distance(ascent, units),
            });
        }
        Self {
            icon: sport_icon(sport),
            title,
            subtitle,
            distance,
            duration,
            metrics,
        }
    }

    #[must_use]
    pub fn item_props(&self) -> ItemProps<'_> {
        ItemProps {
            icon: self.icon,
            title: &self.title,
            subtitle: &self.subtitle,
            distance: self.distance.as_deref(),
            duration: &self.duration,
        }
    }

    #[must_use]
    pub fn metric_props(&self) -> Vec<MetricProps<'_>> {
        self.metrics.iter().map(Metric::props).collect()
    }

    #[must_use]
    pub fn detail_props<'a>(
        &'a self,
        metrics: &'a [MetricProps<'a>],
        path: path::Props<'a>,
    ) -> DetailProps<'a> {
        DetailProps {
            icon: self.icon,
            title: &self.title,
            subtitle: &self.subtitle,
            metrics,
            path: Some(path),
            footer: None,
        }
    }
}

struct Metric {
    label: String,
    value: String,
}

impl Metric {
    fn props(&self) -> MetricProps<'_> {
        MetricProps {
            label: &self.label,
            value: &self.value,
        }
    }
}

fn format_distance(millimeters: u64, units: UnitSystem) -> String {
    match units {
        UnitSystem::Metric => metric_distance(millimeters),
        UnitSystem::Imperial => imperial_distance(millimeters),
    }
}

fn metric_distance(millimeters: u64) -> String {
    let millimeters = u128::from(millimeters);
    if millimeters >= 1_000_000 {
        let hundredths = (millimeters + 5_000) / 10_000;
        format!("{}.{:02} km", hundredths / 100, hundredths % 100)
    } else {
        format!("{} m", (millimeters + 500) / 1_000)
    }
}

fn imperial_distance(millimeters: u64) -> String {
    const MILLIMETERS_PER_MILE: u128 = 1_609_344;
    let millimeters = u128::from(millimeters);
    if millimeters >= MILLIMETERS_PER_MILE {
        let hundredths = (millimeters * 100 + MILLIMETERS_PER_MILE / 2) / MILLIMETERS_PER_MILE;
        format!("{}.{:02} mi", hundredths / 100, hundredths % 100)
    } else {
        let feet = (millimeters * 10 + 1_524) / 3_048;
        format!("{feet} ft")
    }
}

fn duration(milliseconds: u64) -> String {
    let seconds = (milliseconds + 500) / 1_000;
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        format!("{seconds} s")
    }
}

const fn sport_icon(sport: ActivitySport) -> icons::Icon {
    match sport {
        ActivitySport::Running => icons::PERSON_SIMPLE_RUN,
        ActivitySport::Cycling => icons::BICYCLE,
    }
}

fn sport_title(sport: ActivitySport, intl: &Intl) -> String {
    match sport {
        ActivitySport::Running => format_message!(intl, default_message: "Running"),
        ActivitySport::Cycling => format_message!(intl, default_message: "Cycling"),
    }
}

/// One activity-list row.
#[derive(Clone, Copy, Debug)]
pub struct ItemProps<'a> {
    pub icon: icons::Icon,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub distance: Option<&'a str>,
    pub duration: &'a str,
}

/// Activity-list inputs.
pub struct ListProps<'a> {
    pub items: &'a [ItemProps<'a>],
    pub selected: Option<usize>,
    pub empty: &'a str,
}

/// One labeled detail value.
#[derive(Clone, Copy, Debug)]
pub struct MetricProps<'a> {
    pub label: &'a str,
    pub value: &'a str,
}

/// Activity-detail inputs.
pub struct DetailProps<'a> {
    pub icon: icons::Icon,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub metrics: &'a [MetricProps<'a>],
    pub path: Option<path::Props<'a>>,
    pub footer: Option<&'a str>,
}

/// Activity-browser inputs.
pub struct BrowserProps<'a> {
    pub list: ListProps<'a>,
    pub detail: Option<&'a DetailProps<'a>>,
    pub empty_detail: &'a str,
}

/// Activity-browser interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Select(usize),
}

#[must_use]
pub fn list(ui: &mut Ui, props: &ListProps<'_>) -> Option<Action> {
    let height = props
        .items
        .iter()
        .map(|_| ROW_HEIGHT)
        .sum::<f32>()
        .max(ROW_HEIGHT);
    let size = Vec2::new(ui.available_width(), height);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        0.0,
        palette.surfaces().layer(theme::Level::Two).into_cint(),
    );

    if props.items.is_empty() {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            props.empty,
            egui::TextStyle::Body.resolve(ui.style()),
            color32(palette.content().text_secondary()),
        );
        return None;
    }

    let mut action = None;
    let mut row_top = rect.top();
    for (index, item) in props.items.iter().enumerate() {
        let row = Rect::from_min_size(
            egui::pos2(rect.left(), row_top),
            egui::vec2(rect.width(), ROW_HEIGHT),
        );
        row_top += ROW_HEIGHT;
        let response = row_response(ui, row, index, item.title);
        paint_row(ui, row, item, props.selected == Some(index), &response);
        if response.clicked() {
            action = Some(Action::Select(index));
        }
    }
    action
}

/// Renders one activity detail panel.
pub fn detail(ui: &mut Ui, props: &DetailProps<'_>) {
    let height = DETAIL_HEIGHT + props.path.map_or(0.0, |_| PATH_HEIGHT + METRIC_GAP);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        0.0,
        palette.surfaces().layer(theme::Level::One).into_cint(),
    );

    let header_bottom = rect.top() + 88.0;
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, egui::pos2(rect.right(), header_bottom)),
        0.0,
        palette.surfaces().layer(theme::Level::Two).into_cint(),
    );
    let icon_center = egui::pos2(rect.left() + PADDING + ICON_SIZE / 2.0, rect.top() + 40.0);
    icons::Props {
        icon: props.icon,
        size: ICON_SIZE,
        color: palette.content().icon_primary(),
    }
    .paint_at(ui, icon_center);
    let text_left = icon_center.x + ICON_SIZE / 2.0 + 12.0;
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 29.0),
        Align2::LEFT_CENTER,
        props.title,
        crate::typography::font(20.0, crate::typography::Weight::SemiBold),
        color32(palette.content().text_primary()),
    );
    ui.painter().text(
        egui::pos2(text_left, rect.top() + 56.0),
        Align2::LEFT_CENTER,
        props.subtitle,
        egui::TextStyle::Small.resolve(ui.style()),
        color32(palette.content().text_secondary()),
    );

    let metrics_top = props.path.map_or(header_bottom, |mut path| {
        path.height = Some(PATH_HEIGHT);
        let path_rect = Rect::from_min_max(
            egui::pos2(rect.left(), header_bottom + METRIC_GAP),
            egui::pos2(rect.right(), header_bottom + METRIC_GAP + PATH_HEIGHT),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(path_rect), |ui| {
            path::preview(ui, &path);
        });
        path_rect.bottom() + METRIC_GAP
    });
    paint_metrics(ui, rect, metrics_top, props.metrics);
    if let Some(footer) = props.footer {
        ui.painter().text(
            egui::pos2(rect.left() + PADDING, rect.bottom() - PADDING),
            Align2::LEFT_BOTTOM,
            footer,
            egui::TextStyle::Small.resolve(ui.style()),
            color32(palette.content().text_secondary()),
        );
    }
}

#[must_use]
pub fn browser(ui: &mut Ui, props: &BrowserProps<'_>) -> Option<Action> {
    if ui.available_width() >= BROWSER_BREAKPOINT {
        let mut action = None;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(
                    BROWSER_LIST_WIDTH.min(ui.available_width()),
                    ui.available_height(),
                ),
                egui::Layout::top_down(egui::Align::Min),
                |ui| action = list(ui, &props.list),
            );
            ui.add_space(16.0);
            detail_or_empty(ui, props.detail, props.empty_detail);
        });
        action
    } else {
        let action = list(ui, &props.list);
        ui.add_space(16.0);
        detail_or_empty(ui, props.detail, props.empty_detail);
        action
    }
}

fn detail_or_empty(ui: &mut Ui, detail_props: Option<&DetailProps<'_>>, empty: &str) {
    if let Some(detail_props) = detail_props {
        detail(ui, detail_props);
    } else {
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), DETAIL_HEIGHT),
            Sense::hover(),
        );
        let palette = crate::theme::palette(ui);
        ui.painter().rect_filled(
            rect,
            0.0,
            palette.surfaces().layer(theme::Level::Two).into_cint(),
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            empty,
            egui::TextStyle::Body.resolve(ui.style()),
            color32(palette.content().text_secondary()),
        );
    }
}

fn row_response(ui: &Ui, rect: Rect, index: usize, label: &str) -> Response {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("activity-row", index)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.highlight()
    } else {
        response
    }
}

fn paint_row(ui: &Ui, rect: Rect, props: &ItemProps<'_>, selected: bool, response: &Response) {
    let palette = crate::theme::palette(ui);
    if response.highlighted() || selected {
        ui.painter().rect_filled(
            rect,
            0.0,
            palette
                .surfaces()
                .layer_hover(theme::Level::Two)
                .into_cint(),
        );
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(SELECTED_MARKER_WIDTH, rect.height())),
            0.0,
            palette.interaction().interactive().into_cint(),
        );
    }

    let icon_center = egui::pos2(rect.left() + PADDING + ICON_SIZE / 2.0, rect.center().y);
    icons::Props {
        icon: props.icon,
        size: ICON_SIZE,
        color: if selected {
            palette.content().icon_primary()
        } else {
            palette.content().icon_secondary()
        },
    }
    .paint_at(ui, icon_center);

    let text_left = icon_center.x + ICON_SIZE / 2.0 + 12.0;
    let right = rect.right() - PADDING;
    let metric_width = (rect.width() * 0.28).clamp(72.0, 128.0);
    let label_clip = Rect::from_min_max(
        egui::pos2(text_left, rect.top()),
        egui::pos2(right - metric_width, rect.bottom()),
    );
    let painter = ui.painter().with_clip_rect(label_clip);
    painter.text(
        egui::pos2(text_left, rect.center().y - 11.0),
        Align2::LEFT_CENTER,
        props.title,
        egui::TextStyle::Button.resolve(ui.style()),
        color32(palette.content().text_primary()),
    );
    painter.text(
        egui::pos2(text_left, rect.center().y + 13.0),
        Align2::LEFT_CENTER,
        props.subtitle,
        egui::TextStyle::Small.resolve(ui.style()),
        color32(palette.content().text_secondary()),
    );
    if let Some(distance) = props.distance {
        ui.painter().text(
            egui::pos2(right, rect.center().y - 11.0),
            Align2::RIGHT_CENTER,
            distance,
            egui::TextStyle::Button.resolve(ui.style()),
            color32(palette.content().text_primary()),
        );
    }
    ui.painter().text(
        egui::pos2(right, rect.center().y + 13.0),
        Align2::RIGHT_CENTER,
        props.duration,
        egui::TextStyle::Small.resolve(ui.style()),
        color32(palette.content().text_secondary()),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(2.0, palette.interaction().focus().into_cint()),
            egui::StrokeKind::Inside,
        );
    }
}

fn paint_metrics(ui: &Ui, rect: Rect, top: f32, metrics: &[MetricProps<'_>]) {
    if metrics.is_empty() {
        return;
    }
    let palette = crate::theme::palette(ui);
    let columns = if rect.width() >= 480.0 { 4 } else { 2 };
    let rows = metrics.chunks(columns).map(|_| 1.0).sum::<f32>();
    let column_count = if columns == 4 { 4.0 } else { 2.0 };
    let available_height = (rect.bottom() - PADDING - top - 32.0).max(1.0);
    let cell_height = (available_height / rows).min(METRIC_MAX_HEIGHT);
    let cell_width = rect.width() / column_count;

    let mut cell_top = top;
    for row_metrics in metrics.chunks(columns) {
        let mut cell_left = rect.left();
        for metric in row_metrics {
            let cell = Rect::from_min_size(
                egui::pos2(cell_left + METRIC_GAP, cell_top + METRIC_GAP),
                egui::vec2(
                    (cell_width - METRIC_GAP).max(0.0),
                    (cell_height - METRIC_GAP).max(0.0),
                ),
            );
            cell_left += cell_width;
            ui.painter().rect_filled(
                cell,
                0.0,
                palette.surfaces().layer(theme::Level::Two).into_cint(),
            );
            ui.painter().text(
                egui::pos2(cell.left() + PADDING, cell.center().y - 10.0),
                Align2::LEFT_CENTER,
                metric.value,
                FontId::proportional(18.0),
                color32(palette.content().text_primary()),
            );
            ui.painter().text(
                egui::pos2(cell.left() + PADDING, cell.center().y + 14.0),
                Align2::LEFT_CENTER,
                metric.label,
                egui::TextStyle::Small.resolve(ui.style()),
                color32(palette.content().text_secondary()),
            );
        }
        cell_top += cell_height;
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::{duration, format_distance};
    use garmin_model::identity::UnitSystem;

    #[test]
    fn units_change_distance_rendering_without_changing_the_value() {
        let value = 10_000_000;

        assert_eq!(format_distance(value, UnitSystem::Metric), "10.00 km");
        assert_eq!(format_distance(value, UnitSystem::Imperial), "6.21 mi");
    }

    #[test]
    fn activity_duration_is_compact() {
        assert_eq!(duration(42_000), "42 s");
        assert_eq!(duration(3_180_000), "53 min");
        assert_eq!(duration(7_500_000), "2 h 5 min");
    }
}
