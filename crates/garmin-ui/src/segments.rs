//! Named adapter for the shared compact choice group.

use crate::{Size, button};

/// Width of the choices before wrapping, including the compact gaps.
#[must_use]
pub fn natural_width<T>(ui: &egui::Ui, choices: &[(T, String)]) -> f32 {
    choices
        .iter()
        .map(|(_, label)| {
            button::Props {
                label,
                icon: None,
                kind: button::Kind::Ghost,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: true,
            }
            .natural_width(ui)
        })
        .sum::<f32>()
        + choices.windows(2).map(|_| 2.0).sum::<f32>()
}

pub fn show<T: Copy + Eq>(
    ui: &mut egui::Ui,
    selected: &mut T,
    choices: &[(T, String)],
    target: &str,
) {
    ui.push_id(target, |ui| {
        let targets = (0..choices.len())
            .map(|index| format!("{target}.{index}"))
            .collect::<Vec<_>>();
        let choices = choices
            .iter()
            .zip(&targets)
            .map(|((value, label), target)| button::GroupChoice::text(label, *value).target(target))
            .collect::<Vec<_>>();
        if let Some(value) = button::group(
            ui,
            *selected,
            &choices,
            button::GroupProps {
                size: Size::Small,
                width: button::Width::Fit,
                enabled: true,
                style: button::GroupStyle::Underlined,
            },
        ) {
            *selected = value;
        }
    });
}
