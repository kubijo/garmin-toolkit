//! Visible, mutually exclusive choices.

use egui::{Atoms, RadioButton, Ui};

use crate::{button, images, semantics};

const ICON_SIZE: f32 = 16.0;
const LABEL_GAP: f32 = 8.0;

/// One labelled option with a stable interaction target.
pub struct Choice<'a, T> {
    label: &'a str,
    pub(super) value: T,
    target: &'a str,
    image: Option<images::Image>,
    enabled: bool,
}

impl<'a, T> Choice<'a, T> {
    pub const fn new(label: &'a str, value: T, target: &'a str) -> Self {
        Self {
            label,
            value,
            target,
            image: None,
            enabled: true,
        }
    }

    #[must_use]
    pub const fn image(mut self, image: images::Image) -> Self {
        self.image = Some(image);
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Render an individual radio target. Its caller owns the selection.
    pub fn show(&self, ui: &mut Ui, selected: bool) -> egui::Response {
        let mut atoms = Atoms::new(self.label);
        if let Some(image) = self.image {
            atoms.push_left(
                image
                    .widget()
                    .fit_to_exact_size(image.size_for_height(ICON_SIZE)),
            );
        }
        let response = ui
            .push_id(self.target, |ui| {
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().icon_width = ICON_SIZE;
                ui.spacing_mut().icon_spacing = LABEL_GAP;
                crate::theme::selected_control(ui, selected && self.enabled, |ui| {
                    button::interaction_cursor(
                        ui.add_enabled(self.enabled, RadioButton::new(selected, atoms)),
                    )
                })
            })
            .inner;
        semantics::target(ui, &response, self.target);
        response
    }
    pub(super) fn width(&self, ui: &Ui) -> f32 {
        egui::WidgetText::from(self.label)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
            .x
            + ICON_SIZE
            + LABEL_GAP
            + self
                .image
                .map_or(0.0, |image| image.size_for_height(ICON_SIZE).x + LABEL_GAP)
    }
}

pub use crate::radio_group::{Props, show};
