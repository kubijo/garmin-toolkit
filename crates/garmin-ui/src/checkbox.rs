//! Independent selections. The box and its label form one focusable target.

use egui::{IntoAtoms, Response, Ui};

use crate::{button, theme};

const BOX_SIZE: f32 = 16.0;
const LABEL_GAP: f32 = 16.0;
/// Indent for supporting content aligned with the label.
pub const LABEL_INSET: f32 = BOX_SIZE + LABEL_GAP;

pub fn show<'a>(ui: &mut Ui, checked: &mut bool, label: impl IntoAtoms<'a>) -> Response {
    control(ui, checked, label, false)
}

/// `None` represents a partial selection. Activating it selects the whole set.
pub fn show_mixed<'a>(
    ui: &mut Ui,
    state: &mut Option<bool>,
    label: impl IntoAtoms<'a>,
) -> Response {
    let mut checked = state.unwrap_or(false);
    let response = control(ui, &mut checked, label, state.is_none());
    if response.changed() {
        *state = Some(checked);
    }
    response
}

fn control<'a>(
    ui: &mut Ui,
    checked: &mut bool,
    label: impl IntoAtoms<'a>,
    mixed: bool,
) -> Response {
    ui.scope(|ui| {
        if (*checked || mixed) && ui.is_enabled() {
            let accent = theme::selection_accent(ui);
            let fill = theme::color32(accent);
            let mark = egui::Stroke::new(2.0, theme::color32(garmin_color::theme::on_fill(accent)));
            let label_color = theme::color32(theme::palette(ui).content().text_primary());
            let visuals = &mut ui.style_mut().visuals;
            visuals.override_text_color = Some(label_color);
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
            ] {
                widget.bg_fill = fill;
                widget.fg_stroke = mark;
                widget.bg_stroke = egui::Stroke::new(1.0, mark.color);
            }
            visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        }
        ui.spacing_mut().interact_size.y = 24.0;
        ui.spacing_mut().icon_width = BOX_SIZE;
        ui.spacing_mut().icon_spacing = LABEL_GAP;
        let response =
            ui.add(egui::Checkbox::new(checked, label.into_atoms()).indeterminate(mixed));
        button::interaction_cursor(response)
    })
    .inner
}
