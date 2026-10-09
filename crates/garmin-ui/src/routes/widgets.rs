use egui::{Response, Ui};
use garmin_color::{Color, theme::Level};
use garmin_service_api::routes::OutlinePoint;

use super::State;
use crate::{
    Size, activity::map::paint_route_endpoints, button, icons, semantics, theme, typography,
};

pub(super) struct Row<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub detail: Option<&'a str>,
    pub outline: Option<(&'a [OutlinePoint], bool)>,
    pub icon: icons::Icon,
    pub selected: bool,
    pub enabled: bool,
    pub target: &'a str,
}

impl Row<'_> {
    pub fn show(&self, ui: &mut Ui) -> Response {
        ui.push_id(self.target, |ui| {
            ui.add_enabled_ui(self.enabled, |ui| self.content(ui)).inner
        })
        .inner
    }

    fn content(&self, ui: &mut Ui) -> Response {
        let height = if self.detail.is_some() { 88.0 } else { 72.0 };
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        let mut response = button::interaction_cursor(response);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                self.detail.map_or_else(
                    || format!("{} · {}", self.title, self.subtitle),
                    |detail| format!("{} · {} · {detail}", self.title, self.subtitle),
                ),
            )
        });
        semantics::target(ui, &response, self.target);
        let palette = theme::palette(ui);
        let fill = if response.hovered() || self.selected {
            palette.surfaces().layer_hover(Level::One)
        } else {
            palette.surfaces().layer(Level::One)
        };
        ui.painter().rect_filled(rect, 0, theme::color32(fill));
        if self.selected {
            let marker = egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height()));
            ui.painter()
                .rect_filled(marker, 0, theme::color32(theme::selection_accent(ui)));
        }
        let primary = if ui.is_enabled() {
            palette.content().text_primary()
        } else {
            palette.content().text_disabled()
        };
        let secondary = if ui.is_enabled() {
            palette.content().text_secondary()
        } else {
            palette.content().text_disabled()
        };
        icons::Props {
            icon: self.icon,
            size: 24.0,
            color: secondary,
        }
        .paint_at(ui, egui::pos2(rect.left() + 28.0, rect.center().y));
        icons::Props {
            icon: if self.selected {
                icons::CHECK
            } else {
                icons::CARET_RIGHT
            },
            size: 16.0,
            color: secondary,
        }
        .paint_at(ui, egui::pos2(rect.right() - 24.0, rect.center().y));
        let outline_rect = self
            .outline
            .filter(|(points, _)| !points.is_empty() && rect.width() >= 520.0)
            .map(|_| {
                egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 136.0, rect.center().y),
                    egui::vec2(152.0, 56.0),
                )
            });
        if let (Some((points, geometry)), Some(outline_rect)) = (self.outline, outline_rect) {
            paint_outline(ui, outline_rect, points, geometry);
        }
        if self.paint_text(ui, rect, outline_rect, primary, secondary) {
            let tooltip = self.detail.map_or_else(
                || format!("{}\n{}", self.title, self.subtitle),
                |detail| format!("{}\n{}\n{detail}", self.title, self.subtitle),
            );
            response = response.on_hover_text(tooltip);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(2.0),
                0,
                egui::Stroke::new(2.0, theme::color32(palette.interaction().focus())),
                egui::StrokeKind::Inside,
            );
        }
        response
    }

    fn paint_text(
        &self,
        ui: &mut Ui,
        rect: egui::Rect,
        outline_rect: Option<egui::Rect>,
        primary: Color,
        secondary: Color,
    ) -> bool {
        let text_width = outline_rect
            .map_or_else(
                || rect.width() - 104.0,
                |outline| outline.left() - rect.left() - 72.0,
            )
            .max(0.0);
        let mut elided = false;
        let mut lines = Vec::with_capacity(3);
        for (label, weight, color, size) in [
            (
                Some(self.title),
                typography::Weight::SemiBold,
                primary,
                14.0,
            ),
            (
                Some(self.subtitle),
                typography::Weight::Regular,
                secondary,
                14.0,
            ),
            (self.detail, typography::Weight::Regular, secondary, 12.0),
        ] {
            let Some(label) = label else { continue };
            let mut job = egui::text::LayoutJob::simple(
                label.to_owned(),
                typography::font(size, weight),
                theme::color32(color),
                text_width,
            );
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            elided |= galley.elided;
            lines.push((galley, color));
        }
        let gaps = if self.detail.is_some() { 16.0 } else { 8.0 };
        let height = lines
            .iter()
            .map(|(galley, _)| galley.mesh_bounds.height())
            .sum::<f32>()
            + gaps;
        let mut ink_top = rect.center().y - height / 2.0;
        for (galley, color) in lines {
            let ink_height = galley.mesh_bounds.height();
            let y = ink_top - galley.mesh_bounds.top();
            ui.painter().galley(
                egui::pos2(rect.left() + 56.0, y),
                galley,
                theme::color32(color),
            );
            ink_top += ink_height + 8.0;
        }
        elided
    }
}

fn paint_outline(ui: &Ui, rect: egui::Rect, outline: &[OutlinePoint], geometry: bool) {
    let palette = theme::palette(ui);
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        0,
        theme::color32(palette.surfaces().layer(Level::Two)),
    );
    let grid = egui::Stroke::new(1.0, theme::color32(palette.borders().subtle()));
    for fraction in [0.25, 0.5, 0.75] {
        let x = rect.left() + rect.width() * fraction;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            grid,
        );
    }
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.center().y),
            egui::pos2(rect.right(), rect.center().y),
        ],
        grid,
    );
    for fraction in [0.125, 0.375, 0.625, 0.875] {
        let x = rect.left() + rect.width() * fraction;
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            grid,
            2.0,
            4.0,
        ));
    }
    for fraction in [0.25, 0.75] {
        let y = rect.top() + rect.height() * fraction;
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            grid,
            2.0,
            4.0,
        ));
    }
    let canvas = rect.shrink2(egui::vec2(16.0, 8.0));
    let points: Vec<_> = outline
        .iter()
        .map(|point| {
            egui::pos2(
                canvas.left() + canvas.width() * f32::from(point.x) / 255.0,
                canvas.top() + canvas.height() * f32::from(point.y) / 255.0,
            )
        })
        .collect();
    let color = theme::color32(theme::selection_accent(ui));
    if geometry && points.len() >= 2 {
        painter.add(egui::Shape::line(
            points.clone(),
            egui::Stroke::new(2.0, color),
        ));
        paint_route_endpoints(
            ui,
            points[0],
            points[points.len() - 1],
            theme::color32(palette.support().success()),
            theme::color32(palette.support().error()),
            theme::color32(palette.surfaces().background()),
            0.75,
        );
    } else {
        if points.len() >= 2 {
            painter.extend(egui::Shape::dashed_line(
                &points,
                egui::Stroke::new(1.0, color),
                4.0,
                4.0,
            ));
        }
        for point in points {
            painter.circle_filled(point, 3.0, color);
        }
    }
}

pub(super) fn button(label: &str, kind: button::Kind, enabled: bool) -> button::Props<'_> {
    button::Props {
        label,
        icon: None,
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
}

pub(super) fn action_button(
    ui: &mut Ui,
    id: &str,
    label: &str,
    kind: button::Kind,
    enabled: bool,
) -> bool {
    let response = button(label, kind, enabled).show(ui);
    semantics::target(ui, &response, id);
    response.clicked()
}

pub(super) fn icon_button(
    ui: &mut Ui,
    id: &str,
    label: &str,
    icon: icons::Icon,
    enabled: bool,
) -> bool {
    let response = button::IconProps {
        label,
        icon,
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        enabled,
    }
    .show(ui);
    semantics::target(ui, &response, id);
    response.clicked()
}

pub(super) fn action_row<A, const N: usize>(
    ui: &mut Ui,
    buttons: [(&str, A, button::Props<'_>); N],
) -> Option<A> {
    let width = buttons
        .iter()
        .map(|(_, _, button)| button.natural_width(ui))
        .sum::<f32>()
        + 8.0 * f32::from(u16::try_from(N.saturating_sub(1)).unwrap_or(u16::MAX));
    let stacked = width > ui.available_width();
    let mut action = None;
    let show = |ui: &mut Ui| {
        for (id, requested, mut button) in buttons {
            if stacked {
                button.width = button::Width::Fill;
            }
            let response = button.show(ui);
            semantics::target(ui, &response, id);
            if response.clicked() {
                action = Some(requested);
            }
        }
    };
    if stacked {
        ui.vertical(show);
    } else {
        ui.horizontal(show);
    }
    action
}

pub(super) fn busy(ui: &mut Ui, state: &State) {
    ui.allocate_ui(egui::vec2(24.0, 24.0), |ui| {
        if state.busy || state.pending.is_some() {
            ui.spinner();
        }
    });
}

pub(super) fn route_placeholders(ui: &mut Ui) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for _ in 0..2 {
            route_placeholder(ui);
        }
    });
}

fn route_placeholder(ui: &mut Ui) {
    let palette = theme::palette(ui);
    let row_fill = theme::color32(palette.surfaces().layer(Level::One));
    let placeholder_fill = theme::color32(palette.surfaces().layer(Level::Two));
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 72.0), egui::Sense::hover());
    let text_width = (rect.width() - 96.0).max(0.0);
    let blocks = [
        (16.0, 20.0, 24.0, 24.0),
        (56.0, 16.0, text_width.min(200.0), 12.0),
        (56.0, 40.0, text_width.min(128.0), 8.0),
    ];
    ui.painter().rect_filled(rect, 0, row_fill);
    for (x, y, width, height) in blocks {
        let block =
            egui::Rect::from_min_size(rect.min + egui::vec2(x, y), egui::vec2(width, height));
        ui.painter().rect_filled(block, 0, placeholder_fill);
    }
}

pub(super) fn loading_overlay(ui: &mut Ui, first_row: egui::Rect, message: &str) {
    let palette = theme::palette(ui);
    let foreground = theme::color32(palette.content().text_primary());
    let text_width = ui
        .painter()
        .layout_no_wrap(
            message.to_owned(),
            typography::font(14.0, typography::Weight::Regular),
            foreground,
        )
        .size()
        .x;
    let width = (text_width + 48.0).min((first_row.width() - 16.0).max(0.0));
    let overlay = egui::Rect::from_center_size(
        egui::pos2(first_row.center().x, first_row.bottom() + 1.0),
        egui::vec2(width, 32.0),
    );
    ui.painter().add(
        egui::Shadow {
            offset: [0, 0],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(96),
        }
        .as_shape(overlay, 0),
    );
    ui.painter().rect_filled(
        overlay,
        0,
        theme::color32(palette.surfaces().layer(Level::Two)),
    );
    ui.painter().rect_stroke(
        overlay,
        0,
        egui::Stroke::new(1.0, theme::color32(palette.borders().subtle())),
        egui::StrokeKind::Inside,
    );
    ui.put(
        egui::Rect::from_min_size(
            egui::pos2(overlay.left() + 8.0, overlay.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        ),
        egui::Spinner::new().size(16.0),
    );
    ui.put(
        egui::Rect::from_min_size(
            egui::pos2(overlay.left() + 32.0, overlay.center().y - 10.0),
            egui::vec2((overlay.width() - 40.0).max(0.0), 20.0),
        ),
        egui::Label::new(message).truncate(),
    );
}
