//! FIT file-ingress controls.

use cint::ColorInterop;
use egui::{Align, Layout, RichText, Stroke, Ui};
use garmin_color::theme;

use crate::{Size, button, icons, theme as widget_theme};

/// File-ingress inputs.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub files_label: &'a str,
    pub folder_label: &'a str,
    pub drop_active: bool,
    pub enabled: bool,
}

/// File-ingress action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Files,
    Folder,
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let palette = crate::theme::palette(ui);
    let fill = if props.drop_active {
        palette.surfaces().layer_hover(theme::Level::Two)
    } else {
        palette.surfaces().layer(theme::Level::Two)
    };
    let mut action = None;
    egui::Frame::new()
        .fill(widget_theme::color32(fill))
        .stroke(if props.drop_active {
            Stroke::new(2.0, palette.interaction().interactive().into_cint())
        } else {
            Stroke::NONE
        })
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if ui.available_width() < 560.0 {
                heading(ui, props, palette);
                ui.add_space(12.0);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if file_button(ui, props, button::Width::Fill).clicked() {
                        action = Some(Action::Files);
                    }
                    if folder_button(ui, props, button::Width::Fill).clicked() {
                        action = Some(Action::Folder);
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    heading(ui, props, palette);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if folder_button(ui, props, button::Width::Fit).clicked() {
                            action = Some(Action::Folder);
                        }
                        if file_button(ui, props, button::Width::Fit).clicked() {
                            action = Some(Action::Files);
                        }
                    });
                });
            }
        });
    action
}

fn heading(ui: &mut Ui, props: &Props<'_>, palette: &theme::Theme) {
    ui.horizontal(|ui| {
        icons::Props {
            icon: icons::UPLOAD_SIMPLE,
            size: 24.0,
            color: if props.drop_active {
                palette.content().icon_primary()
            } else {
                palette.content().icon_secondary()
            },
        }
        .show(ui);
        ui.add_space(4.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.add(egui::Label::new(crate::typography::semibold(props.title)).selectable(false));
            ui.add(
                egui::Label::new(
                    RichText::new(props.description)
                        .size(12.0)
                        .color(palette.content().text_secondary().into_cint()),
                )
                .selectable(false),
            );
        });
    });
}

fn file_button(ui: &mut Ui, props: &Props<'_>, width: button::Width) -> egui::Response {
    button::Props {
        label: props.files_label,
        icon: Some(icons::FILE),
        kind: button::Kind::Secondary,
        size: Size::Medium,
        width,
        enabled: props.enabled,
    }
    .show(ui)
}

fn folder_button(ui: &mut Ui, props: &Props<'_>, width: button::Width) -> egui::Response {
    button::Props {
        label: props.folder_label,
        icon: Some(icons::FOLDER_OPEN),
        kind: button::Kind::Tertiary,
        size: Size::Medium,
        width,
        enabled: props.enabled,
    }
    .show(ui)
}
