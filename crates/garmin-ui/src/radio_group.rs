//! A responsive group of mutually exclusive radio options.

pub use crate::radio::Choice;
use egui::Ui;

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
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::RadioGroup);
            node.set_label(props.label);
            if let Some(helper) = props.helper {
                node.set_description(helper);
            }
        });
        let width = choices
            .iter()
            .fold(-ui.spacing().item_spacing.x, |width, choice| {
                width + choice.width(ui) + ui.spacing().item_spacing.x
            });
        let layout = if width <= ui.available_width() {
            egui::Layout::left_to_right(egui::Align::Center)
        } else {
            egui::Layout::top_down(egui::Align::Min)
        };
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 24.0), layout, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let mut enabled = Vec::with_capacity(choices.len());
            let mut move_focus = None;
            for choice in choices {
                let response = choice.show(ui, selected == choice.value);
                if response.clicked() && selected != choice.value {
                    changed = Some(choice.value);
                }
                if response.enabled() {
                    if response.has_focus() {
                        move_focus = arrow_direction(ui, response.id)
                            .map(|forward| (enabled.len(), forward));
                    }
                    enabled.push((response.id, choice.value));
                }
            }
            if let Some((index, forward)) = move_focus {
                let next = if forward {
                    index + 1
                } else {
                    index + enabled.len() - 1
                } % enabled.len();
                let (id, value) = enabled[next];
                ui.memory_mut(|memory| memory.request_focus(id));
                if value != selected {
                    changed = Some(value);
                }
            }
        });
    });
    if let Some(helper) = props.helper {
        ui.add(egui::Label::new(egui::RichText::new(helper).weak().size(12.0)).wrap());
    }
    changed
}

fn arrow_direction(ui: &Ui, id: egui::Id) -> Option<bool> {
    use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp};
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        );
    });
    ui.input_mut(|input| {
        [
            (ArrowRight, true),
            (ArrowDown, true),
            (ArrowLeft, false),
            (ArrowUp, false),
        ]
        .into_iter()
        .find_map(|(key, forward)| {
            input
                .consume_key(egui::Modifiers::NONE, key)
                .then_some(forward)
        })
    })
}
