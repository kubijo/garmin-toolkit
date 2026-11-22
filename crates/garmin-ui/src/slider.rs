//! Numeric sliders and sliders with individually labeled stops.
use crate::{button, theme::color32};
use egui::{Response, RichText, Sense, Ui, emath::Numeric};
use std::ops::RangeInclusive;

/// A numeric slider with a directly editable value.
pub fn editable<N: Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    range: RangeInclusive<N>,
) -> Response {
    let response = ui
        .horizontal(|ui| {
            numeric_style(ui);
            ui.spacing_mut().button_padding.y = 2.0;
            ui.add(
                egui::Slider::new(value, range)
                    .text(label)
                    .trailing_fill(true),
            )
        })
        .inner;
    // The native response also spans the editor and caption. Preserve the editor's own cursor.
    let mut rail = response.rect;
    if ui.layout().prefer_right_to_left() {
        rail.set_left(rail.right() - ui.spacing().slider_width);
    } else {
        rail.set_right(rail.left() + ui.spacing().slider_width);
    }
    if ui.rect_contains_pointer(rail) {
        button::interaction_cursor(response)
    } else {
        response
    }
}

/// A leading label and a slider track without a number editor.
pub fn track<N: Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    range: RangeInclusive<N>,
) -> Response {
    ui.horizontal(|ui| {
        numeric_style(ui);
        let caption = ui.label(label);
        let response = ui.add(
            egui::Slider::new(value, range)
                .show_value(false)
                .trailing_fill(true),
        );
        button::interaction_cursor(response.labelled_by(caption.id))
    })
    .inner
}

fn numeric_style(ui: &mut Ui) {
    let palette = crate::theme::palette(ui);
    // egui sizes the thumb from the interaction height. Use the compact control height here.
    ui.spacing_mut().interact_size.y = 24.0;
    ui.spacing_mut().item_spacing.x = 8.0;
    ui.spacing_mut().slider_rail_height = 4.0;
    ui.visuals_mut().handle_shape = egui::style::HandleShape::Circle;
    ui.visuals_mut().widgets.inactive.bg_fill = color32(palette.borders().strong());
    ui.visuals_mut().selection.bg_fill = color32(palette.interaction().interactive());
}

/// Snap to one of the named stops, with a tooltip and a stable accessible target for each.
pub fn stops<const N: usize>(
    ui: &mut Ui,
    target_id: &str,
    label: &str,
    selected: &mut usize,
    labels: [&str; N],
) -> Response {
    const {
        assert!(N >= 2, "a slider needs at least two stops");
    }
    ui.allocate_ui_with_layout(
        egui::vec2(0.0, 32.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let palette = crate::theme::palette(ui);
            let rail = color32(palette.content().icon_secondary());
            let foreground = color32(palette.content().icon_primary());
            ui.spacing_mut().interact_size.y = 18.0;
            let last = N - 1;
            let mut step = (*selected).min(last);
            let mut response = stop_track(ui, &mut step, last);
            // egui's circular slider handle reserves this radius at each end of the rail.
            let knob_radius = response.rect.height() / 2.5;
            let positions = response.rect.x_range().shrink(knob_radius);
            let center_y = response.rect.center().y;
            let center = |index| egui::pos2(egui::lerp(positions, fraction(index, last)), center_y);
            ui.painter().rect_filled(
                egui::Rect::from_center_size(
                    response.rect.center(),
                    egui::vec2(response.rect.width(), 1.5),
                ),
                egui::CornerRadius::ZERO,
                rail,
            );
            let mut stop_hovered = false;
            let hit_size = egui::vec2(20.0_f32.min(positions.span() * fraction(1, last)), 32.0);
            for (index, candidate) in labels.iter().enumerate() {
                let center = center(index);
                let target = stop_button(
                    ui,
                    egui::Rect::from_center_size(center, hit_size),
                    response.id.with(("slider-stop", index)),
                    &format!("{target_id}.{index}"),
                    &format!("{label}: {candidate}"),
                );
                stop_hovered |= target.hovered();
                if target.clicked() {
                    step = index;
                    response.request_focus();
                }
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(1.75, 7.0)),
                    egui::CornerRadius::ZERO,
                    rail,
                );
            }
            let knob_color = if response.hovered() || response.dragged() || response.has_focus() {
                foreground
            } else {
                rail
            };
            ui.painter().circle(
                center(step),
                knob_radius,
                knob_color,
                egui::Stroke::new(1.5, knob_color),
            );
            if *selected != step {
                response.mark_changed();
            }
            *selected = step;
            let value = labels[step];
            crate::semantics::target(ui, &response, target_id);
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_label(label);
                node.set_value(value);
            });
            ui.add_sized(
                [36.0, 32.0],
                egui::Label::new(RichText::new(value).size(11.0)).extend(),
            );
            let response = button::interaction_cursor(response);
            if stop_hovered {
                response
            } else {
                response.on_hover_text(format!("{label}: {value}"))
            }
        },
    )
    .inner
}

/// egui owns focus, dragging, arrow keys and accessibility. The caller paints the track and stops.
fn stop_track(ui: &mut Ui, step: &mut usize, last: usize) -> Response {
    let opacity = ui.opacity();
    ui.set_opacity(0.0);
    let response = ui.add(
        egui::Slider::new(step, 0..=last)
            .step_by(1.0)
            .handle_shape(egui::style::HandleShape::Circle)
            .show_value(false),
    );
    ui.set_opacity(opacity);
    response
}

fn stop_button(ui: &Ui, rect: egui::Rect, id: egui::Id, target: &str, label: &str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    crate::semantics::target(ui, &response, target);
    if response.has_focus() {
        let focus = color32(crate::theme::palette(ui).interaction().focus());
        ui.painter().rect_stroke(
            rect,
            0,
            egui::Stroke::new(2.0, focus),
            egui::StrokeKind::Inside,
        );
    }
    button::interaction_cursor(response).on_hover_text(label)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stop indexes are normalized into display coordinates"
)]
fn fraction(index: usize, last: usize) -> f32 {
    index as f32 / last as f32
}
