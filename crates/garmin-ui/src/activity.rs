//! Activity browsing components.

use cint::ColorInterop;
use egui::{Align2, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};
use garmin_model::{
    activity::{ActivitySport, ActivitySummary, HeartRate},
    identity::UnitSystem,
};
use garmin_service_api::{ActivitySnapshot, DeviceFitPreviewActivity};

use crate::{
    icons, path,
    theme::{CONTROL_RADIUS, PANEL_RADIUS, color32},
};

mod calendar;
pub(crate) mod map;
pub mod map_composition;
pub mod map_diagnostics;
pub mod map_runtime;
pub use map::remote as map_remote;
mod map_style;
#[cfg(not(target_arch = "wasm32"))]
mod native_latest;
mod navigation;
mod route_index;
mod workspace;

pub use map::{WgpuMapHandle, install_wgpu_map};
pub use workspace::{ActivityCursor, CursorMode, Viewer, ViewerProps, Workspace, WorkspaceProps};

const ROW_HEIGHT: f32 = 52.0;
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
    started_at: jiff::Timestamp,
    local_start: jiff::civil::DateTime,
    sport: ActivitySport,
    icon: icons::Icon,
    title: String,
    subtitle: String,
    distance: Option<String>,
    duration: String,
    metrics: Vec<Metric>,
    original: Option<garmin_service_api::ActivityOriginal>,
}

impl Presentation {
    #[must_use]
    pub fn from_summary(
        summary: ActivitySummary,
        source: &str,
        intl: &Intl,
        units: UnitSystem,
    ) -> Self {
        Self::new(summary, source, intl, units)
    }

    #[must_use]
    pub fn from_snapshot(snapshot: &ActivitySnapshot, intl: &Intl, units: UnitSystem) -> Self {
        let mut presentation = Self::from_summary(snapshot.summary, &snapshot.source, intl, units);
        presentation.original.clone_from(&snapshot.original);
        presentation
    }

    #[must_use]
    pub fn with_original(mut self, original: garmin_service_api::ActivityOriginal) -> Self {
        self.original = Some(original);
        self
    }

    fn recording_identity(&self) -> Option<(&str, &[u8; 32])> {
        let original = self.original.as_ref()?;
        Some((&original.fingerprint_schema, &original.fingerprint_digest))
    }

    fn new(summary: ActivitySummary, source: &str, intl: &Intl, units: UnitSystem) -> Self {
        let sport = summary.sport();
        let started_at = *summary.time().start().as_jiff();
        let local_start = started_at.to_zoned(jiff::tz::TimeZone::system()).datetime();
        let start_label = calendar::date_time_label(intl, local_start);
        let totals = summary.totals();
        let average_heart_rate = summary
            .metrics()
            .average_heart_rate()
            .map(HeartRate::into_beats_per_minute);
        let title = sport_title(sport, intl);
        let subtitle = if source.is_empty() {
            start_label
        } else {
            format_message!(
                intl,
                default_message: "{start} · {source}",
                values: { start: start_label, source: source },
            )
        };
        let decimal = intl.decimal_formatter();
        let distance = totals
            .distance()
            .map(|value| value.format(units, 2, &decimal));
        let duration = totals.timer().format_compact();
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
        if let Some(ascent) = totals.ascent() {
            metrics.push(Metric {
                label: format_message!(intl, default_message: "Ascent"),
                value: ascent.format(units, 2, decimal),
            });
        }
        Self {
            started_at,
            local_start,
            sport,
            icon: sport_icon(sport),
            title,
            subtitle,
            distance,
            duration,
            metrics,
            original: None,
        }
    }

    #[must_use]
    pub const fn sport(&self) -> ActivitySport {
        self.sport
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

pub(crate) const fn sport_icon(sport: ActivitySport) -> icons::Icon {
    match sport {
        ActivitySport::Running => icons::PERSON_SIMPLE_RUN,
        ActivitySport::Cycling => icons::BICYCLE,
        ActivitySport::Swimming => icons::PERSON_SIMPLE_SWIM,
        ActivitySport::Walking => icons::PERSON_SIMPLE_WALK,
        ActivitySport::Hiking => icons::PERSON_SIMPLE_HIKE,
    }
}

fn sport_title(sport: ActivitySport, intl: &Intl) -> String {
    match sport {
        ActivitySport::Running => format_message!(intl, default_message: "Running"),
        ActivitySport::Cycling => format_message!(intl, default_message: "Cycling"),
        ActivitySport::Swimming => format_message!(intl, default_message: "Swimming"),
        ActivitySport::Walking => format_message!(intl, default_message: "Walking"),
        ActivitySport::Hiking => format_message!(intl, default_message: "Hiking"),
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
    Export,
}

#[must_use]
pub fn export_button(ui: &mut Ui, intl: &Intl) -> bool {
    let response = crate::button::Props {
        label: &format_message!(intl, default_message: "Export activity"),
        icon: Some(icons::DOWNLOAD_SIMPLE),
        kind: crate::button::Kind::Tertiary,
        size: crate::Size::Small,
        width: crate::button::Width::Fit,
        enabled: true,
    }
    .show(ui);
    crate::semantics::target(ui, &response, "activity.export");
    response.clicked()
}

/// Review the activity summaries supplied by the browser import service.
#[must_use]
pub fn package_review(
    ui: &mut Ui,
    intl: &Intl,
    destination: &str,
    units: UnitSystem,
    activities: &[(ActivitySummary, bool)],
) -> Option<crate::modal::Action> {
    let title = format_message!(intl, default_message: "Review activity import");
    let description = format_message!(intl, default_message: "Destination profile: {profile}", values: { profile: destination });
    let cancel = format_message!(intl, default_message: "Cancel");
    let count = activities.iter().filter(|(_, existing)| !existing).count();
    let confirm = if count == 0 {
        format_message!(intl, default_message: "View in library")
    } else {
        format_message!(intl, default_message: "{count, plural, one {Import # activity} other {Import # activities}}", values: { count: count as u64 })
    };
    crate::modal::show(
        ui,
        ui.id().with("activity-package-review"),
        &crate::modal::Props {
            title: &title,
            description: Some(&description),
            size: crate::modal::Size::Medium,
            presentation: crate::modal::Presentation::Modal,
            cancel_label: Some(&cancel),
            cancel_disabled: None,
            backdrop_closes: None,
            primary: Some(crate::modal::Primary {
                label: &confirm,
                icon: Some(icons::UPLOAD_SIMPLE),
                kind: crate::modal::PrimaryKind::Confirm,
                enabled: true,
            }),
        },
        |ui| {
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (summary, existing) in activities {
                        let status = if *existing {
                            format_message!(intl, default_message: "Already imported")
                        } else {
                            format_message!(intl, default_message: "Will be imported")
                        };
                        let presentation =
                            Presentation::from_summary(*summary, &status, intl, units);
                        ui.strong(&presentation.title);
                        ui.label(&presentation.subtitle);
                        ui.horizontal(|ui| {
                            if let Some(distance) = &presentation.distance {
                                ui.label(distance);
                            }
                            ui.label(&presentation.duration);
                        });
                        ui.add_space(8.0);
                    }
                });
        },
    )
    .action
}

/// Displays recorded activity details before the host starts importing.
pub struct PackageReview {
    workspace: Workspace,
    selected: usize,
}

impl PackageReview {
    #[must_use]
    pub fn new(runtime: &map_runtime::MapRuntimeHandle) -> Self {
        Self {
            workspace: Workspace::new(runtime),
            selected: 0,
        }
    }

    #[must_use]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        destination: Option<&str>,
        units: UnitSystem,
        activities: &[&DeviceFitPreviewActivity],
        today: jiff::civil::Date,
    ) -> Option<crate::modal::Action> {
        let title = format_message!(intl, default_message: "Review activity import");
        let description = destination.map(|name| {
        format_message!(intl, default_message: "Destination profile: {profile}", values: { profile: name })
    });
        let cancel = format_message!(intl, default_message: "Cancel");
        let confirm = if destination.is_some() {
            format_message!(intl, default_message: "{count, plural, one {Import # activity} other {Import # activities}}", values: { count: activities.len() as u64 })
        } else {
            format_message!(intl, default_message: "Choose a profile")
        };
        let presentations = activities
            .iter()
            .map(|activity| {
                Presentation::from_summary(activity.summary, &activity.source, intl, units)
            })
            .collect::<Vec<_>>();
        let items = presentations
            .iter()
            .map(Presentation::item_props)
            .collect::<Vec<_>>();
        let selected = activities.get(self.selected);
        let recording_key = selected.map(|_| format!("package-review:{}", self.selected));
        let empty = format_message!(intl, default_message: "No activities yet");
        let select = format_message!(intl, default_message: "Select an activity");
        let no_route = format_message!(intl, default_message: "No recorded route");
        let output = crate::modal::show_compact(
            ui,
            ui.id().with("activity-package-review"),
            &crate::modal::Props {
                title: &title,
                description: description.as_deref(),
                size: crate::modal::Size::Large,
                presentation: crate::modal::Presentation::Modal,
                cancel_label: Some(&cancel),
                cancel_disabled: None,
                backdrop_closes: None,
                primary: Some(crate::modal::Primary {
                    label: &confirm,
                    icon: Some(icons::UPLOAD_SIMPLE),
                    kind: crate::modal::PrimaryKind::Confirm,
                    enabled: !activities.is_empty(),
                }),
            },
            |ui| {
                self.workspace.show(
                    ui,
                    intl,
                    &WorkspaceProps {
                        today,
                        items: &items,
                        presentations: &presentations,
                        selected: selected.map(|_| self.selected),
                        recording: selected.map(|activity| &activity.recording),
                        recording_key: recording_key.as_deref(),
                        units,
                        empty_list: &empty,
                        empty_detail: &select,
                        no_route: &no_route,
                    },
                )
            },
        );
        if let Some(Action::Select(index)) = output.inner {
            self.selected = index;
        }
        output.action
    }
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
        PANEL_RADIUS,
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
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_value(if props.selected == Some(index) {
                "selected"
            } else {
                "unselected"
            });
        });
        paint_row(
            ui,
            row,
            item,
            props.selected == Some(index),
            &response,
            None,
            theme::Level::Two,
        );
        if index + 1 < props.items.len() {
            ui.painter().hline(
                row.x_range(),
                row.bottom(),
                Stroke::new(1.0, palette.borders().subtle().into_cint()),
            );
        }
        if response.clicked() {
            action = Some(Action::Select(index));
        }
    }
    action
}

/// Renders a standalone row with the same presentation as the activity list.
pub(crate) fn selectable_row(
    ui: &mut Ui,
    props: &ItemProps<'_>,
    accent: Option<garmin_color::Color>,
) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
    let response = if response.hovered() {
        response.highlight()
    } else {
        response
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), props.title)
    });
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_description(props.subtitle));
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        PANEL_RADIUS,
        color32(palette.surfaces().layer(theme::Level::One)),
    );
    paint_row(ui, rect, props, false, &response, accent, theme::Level::One);
    response
        .on_hover_text(props.title)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Renders one activity detail panel.
pub fn detail(ui: &mut Ui, props: &DetailProps<'_>) {
    let height = DETAIL_HEIGHT + props.path.map_or(0.0, |_| PATH_HEIGHT + METRIC_GAP);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        PANEL_RADIUS,
        palette.surfaces().layer(theme::Level::One).into_cint(),
    );

    let header_bottom = rect.top() + 88.0;
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, egui::pos2(rect.right(), header_bottom)),
        PANEL_RADIUS,
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
            PANEL_RADIUS,
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
    crate::semantics::target(ui, &response, format!("activity.{index}"));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.highlight()
    } else {
        response
    }
}

fn paint_row(
    ui: &Ui,
    rect: Rect,
    props: &ItemProps<'_>,
    selected: bool,
    response: &Response,
    accent: Option<garmin_color::Color>,
    level: theme::Level,
) {
    let palette = crate::theme::palette(ui);
    if response.highlighted() || selected {
        ui.painter().rect_filled(
            rect,
            CONTROL_RADIUS,
            palette.surfaces().layer_hover(level).into_cint(),
        );
    }
    if let Some(accent) = accent {
        crate::profile::paint_accent_wash(ui.painter(), rect, accent);
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(SELECTED_MARKER_WIDTH, rect.height())),
            egui::CornerRadius::ZERO,
            crate::theme::selection_accent_on(ui, palette.surfaces().layer_hover(level))
                .into_cint(),
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
            CONTROL_RADIUS,
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
                CONTROL_RADIUS,
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
    use garmin_model::{
        activity::{ActivityDuration, Distance},
        identity::UnitSystem,
    };

    #[test]
    fn units_change_distance_rendering_without_changing_the_value() {
        let value = Distance::from_millimeters(10_000_000);
        let decimal = |value: f64, digits: usize| format!("{value:.digits$}");

        assert_eq!(
            value.format(UnitSystem::Metric, 2, decimal),
            "10.00\u{a0}km"
        );
        assert_eq!(
            value.format(UnitSystem::Imperial, 2, decimal),
            "6.21\u{a0}mi"
        );
    }

    #[test]
    fn activity_duration_is_compact() {
        assert_eq!(
            ActivityDuration::from_milliseconds(42_000).format_compact(),
            "42 s"
        );
        assert_eq!(
            ActivityDuration::from_milliseconds(3_180_000).format_compact(),
            "53 min"
        );
        assert_eq!(
            ActivityDuration::from_milliseconds(7_500_000).format_compact(),
            "2 h 5 min"
        );
    }
}
