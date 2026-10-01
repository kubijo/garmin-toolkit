//! Full-width disclosure rows with shared icon, surface, and focus styling.

use egui::{Ui, collapsing_header::CollapsingState};

use crate::{icons, theme};

pub struct Props<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub default_open: bool,
    pub inline_padding: i8,
}

/// Show a keyboard-accessible disclosure with padded, unindented content.
pub fn show(ui: &mut Ui, props: &Props<'_>, content: impl FnOnce(&mut Ui)) {
    let mut state = CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(props.id),
        props.default_open,
    );
    let palette = theme::palette(ui);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(f32::from(props.inline_padding), 8.0);
            let icon = if state.is_open() {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            ui.add(
                egui::Button::new(())
                    .left_text((
                        props.label,
                        icon.image(palette.content().icon_primary())
                            .fit_to_exact_size(egui::Vec2::splat(16.0)),
                    ))
                    .gap(16.0)
                    .image_tint_follows_text_color(false)
                    .frame_when_inactive(false)
                    .corner_radius(0)
                    .wrap()
                    .min_size(egui::vec2(ui.available_width(), 40.0)),
            )
        })
        .inner;
    let response = crate::button::interaction_cursor(response);
    if response.clicked() {
        state.toggle(ui);
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::CollapsingHeader,
            ui.is_enabled(),
            props.label,
        )
    });
    crate::semantics::target(ui, &response, props.id);
    crate::semantics::value(&response, if state.is_open() { "open" } else { "closed" });
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_expanded(state.is_open()));
    let stroke = egui::Stroke::new(1.0, theme::color32(palette.borders().subtle()));
    ui.painter().line_segment(
        [response.rect.left_top(), response.rect.right_top()],
        stroke,
    );
    state.show_body_unindented(ui, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(props.inline_padding, 16))
            .show(ui, content);
    });
    let bottom = ui.cursor().top() - ui.spacing().item_spacing.y;
    ui.painter().line_segment(
        [
            egui::pos2(response.rect.left(), bottom),
            egui::pos2(response.rect.right(), bottom),
        ],
        stroke,
    );
}
