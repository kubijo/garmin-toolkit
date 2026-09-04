//! Activity browsing components.

use cint::ColorInterop;
use egui::{Align2, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use garmin_color::theme;

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
