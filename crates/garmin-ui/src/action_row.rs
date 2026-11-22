//! Responsive groups of independent action buttons.

use crate::{button, semantics};
use egui::{Align, Layout, Ui};

const GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    Leading,
    Trailing,
}

pub(crate) fn width<A>(ui: &Ui, actions: &[(&str, A, button::Props<'_>)]) -> f32 {
    actions
        .iter()
        .enumerate()
        .map(|(index, (_, _, button))| {
            button.natural_width(ui) + if index == 0 { 0.0 } else { GAP }
        })
        .sum()
}

/// Put the first action at the requested edge. When stacked, it becomes the top button.
pub fn show<A, const N: usize>(
    ui: &mut Ui,
    placement: Placement,
    actions: [(&str, A, button::Props<'_>); N],
) -> Option<A> {
    if actions.is_empty() {
        return None;
    }
    let stacked = width(ui, &actions) > ui.available_width();
    let layout = if stacked {
        Layout::top_down(Align::Min)
    } else {
        match placement {
            Placement::Leading => Layout::left_to_right(Align::Center),
            Placement::Trailing => Layout::right_to_left(Align::Center),
        }
    };
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 0.0), layout, |ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::splat(GAP);
        let mut clicked = None;
        for (id, action, mut button) in actions {
            button.width = if stacked {
                button::Width::Fill
            } else {
                button::Width::Fit
            };
            let response = ui.push_id(id, |ui| button.show(ui)).inner;
            semantics::target(ui, &response, id);
            if response.clicked() {
                clicked = Some(action);
            }
        }
        clicked
    })
    .inner
}
