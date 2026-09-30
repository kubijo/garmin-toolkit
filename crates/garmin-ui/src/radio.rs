//! Visible, mutually exclusive choices.

use egui::{Atoms, RadioButton, Ui};

use crate::{button, images, semantics};

/// One labelled option with a stable interaction target.
pub struct Choice<'a, T> {
    label: &'a str,
    value: T,
    target: &'a str,
    image: Option<images::Image>,
}

impl<'a, T> Choice<'a, T> {
    pub const fn new(label: &'a str, value: T, target: &'a str) -> Self {
        Self {
            label,
            value,
            target,
            image: None,
        }
    }

    #[must_use]
    pub const fn image(mut self, image: images::Image) -> Self {
        self.image = Some(image);
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

/// Renders every option, wrapping whole controls onto another row as needed.
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
        ui.spacing_mut().interact_size.y = 32.0;
        ui.spacing_mut().icon_spacing = 8.0;
        ui.horizontal_wrapped(|ui| {
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
                            button::interaction_cursor(
                                ui.add(RadioButton::new(selected == choice.value, atoms)),
                            )
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
