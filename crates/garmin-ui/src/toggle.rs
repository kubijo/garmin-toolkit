//! An immediately applied on/off setting with one focusable track and label.

use egui::{Atom, AtomLayout, Response, Sense, Ui};

use crate::{button, theme};

pub fn show(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let track_id = egui::Id::new("switch track");
    let layout = AtomLayout::new((Atom::custom(track_id, egui::vec2(32.0, 16.0)), label))
        .gap(8.0)
        .min_size(egui::vec2(0.0, 24.0))
        .sense(Sense::click())
        .show(ui);
    let track = layout.rect(track_id);
    let mut response = layout.response;
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, response.enabled(), *on, label)
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(egui::accesskit::Role::Switch);
    });
    if let Some(track) = track {
        let palette = theme::palette(ui);
        let fill = if *on {
            theme::selection_accent(ui)
        } else {
            palette.borders().strong()
        };
        let thumb = palette.content().text_primary().contrasting_marker(fill);
        ui.painter().rect_filled(track, 8, theme::color32(fill));
        let x = if *on {
            track.right() - 8.0
        } else {
            track.left() + 8.0
        };
        ui.painter()
            .circle_filled(egui::pos2(x, track.center().y), 6.0, theme::color32(thumb));
        if response.has_focus() || response.hovered() {
            ui.painter().rect_stroke(
                track.expand(2.0),
                10,
                egui::Stroke::new(1.0, theme::color32(palette.borders().interactive())),
                egui::StrokeKind::Outside,
            );
        }
    }
    button::interaction_cursor(response)
}
