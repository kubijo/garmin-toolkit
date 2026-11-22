//! Full-width disclosure rows with shared icon, surface, and focus styling.

use egui::{Ui, collapsing_header::CollapsingState};

use crate::{icons, theme};

pub struct Props<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub default_open: bool,
    pub inline_padding: i8,
}

/// A compact disclosure within another item, identified independently of its translated label.
pub fn inline(ui: &mut Ui, id: &str, label: &str, content: impl FnOnce(&mut Ui)) -> egui::Response {
    let header = egui::CollapsingHeader::new(label)
        .id_salt(id)
        .show(ui, content)
        .header_response;
    let header = crate::button::interaction_cursor(header);
    let expanded = CollapsingState::load_with_default_open(ui.ctx(), header.id, false).is_open();
    ui.ctx()
        .accesskit_node_builder(header.id, |node| node.set_expanded(expanded));
    crate::semantics::target(ui, &header, id);
    header
}

/// Show a keyboard-accessible disclosure with padded, unindented content.
pub fn show(ui: &mut Ui, props: &Props<'_>, content: impl FnOnce(&mut Ui)) {
    show_with_body_margin(
        ui,
        props,
        egui::Margin::symmetric(props.inline_padding, 16),
        content,
    );
}

pub(crate) fn show_with_body_margin(
    ui: &mut Ui,
    props: &Props<'_>,
    margin: egui::Margin,
    content: impl FnOnce(&mut Ui),
) {
    let mut state = CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(props.id),
        props.default_open,
    );
    let palette = theme::palette(ui);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(f32::from(props.inline_padding), 8.0);
            let widgets = &mut ui.style_mut().visuals.widgets;
            widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            widgets.inactive.bg_stroke = egui::Stroke::NONE;
            widgets.hovered.bg_stroke = egui::Stroke::NONE;
            widgets.hovered.expansion = 0.0;
            widgets.active.bg_stroke = egui::Stroke::NONE;
            widgets.active.expansion = 0.0;
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
        egui::Frame::NONE.inner_margin(margin).show(ui, content);
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
