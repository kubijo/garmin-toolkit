use cint::ColorInterop;
use egui::{Align, Direction, InnerResponse, Layout, Pos2, Rect, Response, Sense, Ui, UiBuilder};
use garmin_color::{Color, theme};

use crate::{icons, theme::color32};

const CONTROL_PADDING: f32 = 8.0;
const COMPACT_WIDTH: f32 = 56.0;
const CARET_SIZE: f32 = 14.0;
const CARET_GAP: f32 = 4.0;
const MENU_BORDER_WIDTH: f32 = 1.0;
const CONTROL_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;
const MENU_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;
const ROW_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;

pub struct MenuGeometry {
    pub position: Pos2,
    pub width: f32,
}

#[derive(Clone, Copy)]
pub enum RowKind {
    Default,
    Danger,
}

#[derive(Clone, Copy)]
struct RowConfig {
    width: f32,
    height: f32,
    divided: bool,
    interactive: bool,
    kind: RowKind,
    level: Option<theme::Level>,
}

pub fn control(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    label: &str,
    expanded: bool,
    content: impl FnOnce(&mut Ui, bool),
) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let response = if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.highlight()
    } else {
        response
    };

    let palette = crate::theme::palette(ui);
    let fill = if response.highlighted() {
        palette.surfaces().layer_hover(theme::Level::One)
    } else {
        palette.surfaces().layer(theme::Level::One)
    };
    ui.painter()
        .rect_filled(rect, CONTROL_RADIUS, fill.into_cint());
    let wide = rect.width() > COMPACT_WIDTH;
    let inner = rect.shrink2(egui::vec2(CONTROL_PADDING, 4.0));
    let (content_rect, layout) = if wide {
        let caret_center = egui::pos2(inner.right() - CARET_SIZE / 2.0, inner.center().y);
        let content_rect = Rect::from_min_max(
            inner.min,
            egui::pos2(
                caret_center.x - CARET_SIZE / 2.0 - CARET_GAP,
                inner.bottom(),
            ),
        );
        icons::Props {
            icon: if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            },
            size: CARET_SIZE,
            color: palette.content().icon_secondary(),
        }
        .paint_at(ui, caret_center);
        (content_rect, Layout::left_to_right(Align::Center))
    } else {
        (
            inner,
            Layout::centered_and_justified(Direction::LeftToRight),
        )
    };
    let mut child = ui.new_child(UiBuilder::new().max_rect(content_rect).layout(layout));
    child.set_clip_rect(content_rect);
    content(&mut child, wide);

    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CONTROL_RADIUS,
            egui::Stroke::new(2.0, palette.interaction().focus().into_cint()),
            egui::StrokeKind::Inside,
        );
    }
    response
}

pub fn preferred_width<'a>(
    ui: &Ui,
    icon_size: f32,
    labels: impl IntoIterator<Item = &'a str>,
) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let foreground = color32(crate::theme::palette(ui).content().text_primary());
    let item_gap = ui.spacing().item_spacing.x;
    let mut width = [
        CONTROL_PADDING,
        CONTROL_PADDING,
        icon_size,
        CARET_GAP,
        CARET_SIZE,
    ]
    .into_iter()
    .sum::<f32>();
    for label in labels {
        width += item_gap
            + ui.painter()
                .layout_no_wrap(label.to_owned(), font.clone(), foreground)
                .size()
                .x;
    }
    width
}

pub fn menu_geometry(trigger: Rect, requested_width: f32, left_limit: f32) -> MenuGeometry {
    let right = trigger.right() + MENU_BORDER_WIDTH;
    let width = (requested_width + MENU_BORDER_WIDTH).min((right - left_limit).max(0.0));
    MenuGeometry {
        position: egui::pos2(right - width, trigger.bottom() - MENU_BORDER_WIDTH),
        width,
    }
}

pub fn menu<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let palette = crate::theme::palette(ui);
    egui::Frame::new()
        .fill(color32(palette.surfaces().background()))
        .corner_radius(MENU_RADIUS)
        .stroke(egui::Stroke::new(
            MENU_BORDER_WIDTH,
            palette.borders().subtle().into_cint(),
        ))
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(96),
        })
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            content(ui)
        })
}

pub fn row(
    ui: &mut Ui,
    height: f32,
    divided: bool,
    interactive: bool,
    kind: RowKind,
    paint: impl FnOnce(&Ui, Rect, Color),
) -> Response {
    row_with_surface(
        ui,
        RowConfig {
            width: ui.available_width(),
            height,
            divided,
            interactive,
            kind,
            level: None,
        },
        paint,
    )
}

/// Draw a fixed-width menu row on a semantic floating layer.
pub fn row_with_width_on_layer(
    ui: &mut Ui,
    width: f32,
    height: f32,
    divided: bool,
    kind: RowKind,
    level: theme::Level,
    paint: impl FnOnce(&Ui, Rect, Color),
) -> Response {
    row_with_surface(
        ui,
        RowConfig {
            width,
            height,
            divided,
            interactive: true,
            kind,
            level: Some(level),
        },
        paint,
    )
}

fn row_with_surface(
    ui: &mut Ui,
    config: RowConfig,
    paint: impl FnOnce(&Ui, Rect, Color),
) -> Response {
    let sense = if config.interactive {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(config.width, config.height), sense);
    if config.interactive && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let palette = crate::theme::palette(ui);
    let base = config.level.map_or_else(
        || palette.surfaces().background(),
        |level| palette.surfaces().layer(level),
    );
    let hover = config.level.map_or_else(
        || palette.surfaces().layer(theme::Level::One),
        |level| palette.surfaces().layer_hover(level),
    );
    let (fill, foreground) = match (config.kind, response.hovered()) {
        (RowKind::Danger, true) => {
            let states = palette.buttons().danger();
            let state = if response.is_pointer_button_down_on() {
                states.active()
            } else {
                states.rest()
            };
            (state.background(), state.foreground())
        }
        (RowKind::Danger, false) => (base, palette.support().error()),
        (RowKind::Default, true) => (hover, palette.content().icon_primary()),
        (RowKind::Default, false) => (base, palette.content().icon_primary()),
    };
    ui.painter().rect_filled(rect, ROW_RADIUS, fill.into_cint());
    if config.divided {
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            egui::Stroke::new(1.0, palette.borders().subtle().into_cint()),
        );
    }
    paint(ui, rect, foreground);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            ROW_RADIUS,
            egui::Stroke::new(2.0, palette.interaction().focus().into_cint()),
            egui::StrokeKind::Inside,
        );
    }
    response
}
