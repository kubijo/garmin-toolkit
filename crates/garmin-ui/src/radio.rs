//! Visible, mutually exclusive choices.

use egui::{Atoms, RadioButton, Ui};

use crate::{button, images, semantics};

/// One labelled option with a stable interaction target.
pub struct Choice<'a, T> {
    label: &'a str,
    value: T,
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
}

/// Group label, helper text, and interaction state.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub label: &'a str,
    pub helper: Option<&'a str>,
    pub enabled: bool,
}

/// Renders one row when all options fit, otherwise one compact vertical group.
pub fn show<T: Copy + PartialEq>(
    ui: &mut Ui,
    selected: T,
    choices: &[Choice<'_, T>],
    props: Props<'_>,
) -> Option<T> {
    ui.label(props.label);
    let mut changed = None;
    ui.add_enabled_ui(props.enabled, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 4.0);
        ui.spacing_mut().interact_size.y = 24.0;
        ui.spacing_mut().icon_spacing = 8.0;
        let width = choices
            .iter()
            .fold(-ui.spacing().item_spacing.x, |width, choice| {
                width + choice_width(ui, choice) + ui.spacing().item_spacing.x
            });
        let layout = if width <= ui.available_width() {
            egui::Layout::left_to_right(egui::Align::Center)
        } else {
            egui::Layout::top_down(egui::Align::Min)
        };
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 24.0), layout, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            for choice in choices {
                let mut atoms = Atoms::new(choice.label);
                if let Some(image) = choice.image {
                    atoms.push_left(
                        image
                            .widget()
                            .fit_to_exact_size(image.size_for_height(16.0)),
                    );
                }
                let response = ui
                    .push_id(choice.target, |ui| {
                        crate::theme::selected_control(ui, selected == choice.value, |ui| {
                            button::interaction_cursor(ui.add_enabled(
                                choice.enabled,
                                RadioButton::new(selected == choice.value, atoms),
                            ))
                        })
                    })
                    .inner;
                semantics::target(ui, &response, choice.target);
                if response.clicked() && selected != choice.value {
                    changed = Some(choice.value);
                }
            }
        });
    });
    if let Some(helper) = props.helper {
        ui.add(egui::Label::new(egui::RichText::new(helper).weak().size(12.0)).wrap());
    }
    changed
}

fn choice_width<T>(ui: &Ui, choice: &Choice<'_, T>) -> f32 {
    egui::WidgetText::from(choice.label)
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
        + ui.spacing().icon_width
        + ui.spacing().icon_spacing
        + choice.image.map_or(0.0, |image| {
            image.size_for_height(16.0).x + ui.spacing().icon_spacing
        })
}
