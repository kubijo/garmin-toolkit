//! Device collection and detail views.

use cint::ColorInterop;
use egui::{Align, Layout, RichText, TextStyle, Ui};

use crate::{Size, button, icons};

const DEVICE_ICON_SIZE: f32 = 32.0;

/// Display data for one attached device.
pub struct Props<'a> {
    pub name: &'a str,
    pub connection: &'a str,
    pub identifier: Option<&'a str>,
    pub software: Option<&'a str>,
    pub status: &'a str,
    pub status_label: &'a str,
    pub identifier_label: &'a str,
    pub software_label: &'a str,
    pub transfers_label: &'a str,
    pub transfers: &'a [Transfer<'a>],
    pub storages: &'a [crate::capacity::Props<'a>],
    pub icon: icons::Icon,
    pub inspect_label: &'a str,
    pub inspect_enabled: bool,
}

/// One display row of supported device transfers.
pub struct Transfer<'a> {
    pub data: &'a str,
    pub directions: &'a str,
}

/// Device-page interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Inspect,
}

/// Availability of a device collection supplied by another process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionState<'a> {
    Connecting(&'a str),
    Empty(&'a str),
    Error(&'a str),
}

/// Renders collection-level feedback.
pub fn show_collection_state(ui: &mut Ui, state: CollectionState<'_>) {
    match state {
        CollectionState::Connecting(message) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(message);
            });
        }
        CollectionState::Empty(message) => {
            ui.label(message);
        }
        CollectionState::Error(message) => {
            let color = crate::theme::palette(ui).support().error().into_cint();
            let message = crate::text::balanced(ui, message, &TextStyle::Body, false);
            ui.label(RichText::new(message).color(color));
        }
    }
}

#[must_use]
pub fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let palette = crate::theme::palette(ui);
    ui.horizontal(|ui| {
        icons::Props {
            icon: props.icon,
            size: DEVICE_ICON_SIZE,
            color: palette.content().icon_primary(),
        }
        .show(ui);
        ui.vertical(|ui| {
            ui.heading(props.name);
            ui.label(
                RichText::new(props.connection)
                    .color(palette.content().text_secondary().into_cint()),
            );
        });
    });
    ui.add_space(20.0);

    let mut inspect = false;
    egui::Frame::new()
        .fill(crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::One),
        ))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    metadata(ui, props.status_label, props.status);
                    if let Some(identifier) = props.identifier {
                        metadata(ui, props.identifier_label, identifier);
                    }
                    if let Some(software) = props.software {
                        metadata(ui, props.software_label, software);
                    }
                    if !props.transfers.is_empty() {
                        ui.label(
                            RichText::new(props.transfers_label)
                                .small()
                                .color(palette.content().text_secondary().into_cint()),
                        );
                        for transfer in props.transfers {
                            ui.horizontal(|ui| {
                                ui.label(transfer.data);
                                ui.label(
                                    RichText::new(transfer.directions)
                                        .color(palette.content().text_secondary().into_cint()),
                                );
                            });
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    inspect = button::Props {
                        label: props.inspect_label,
                        icon: Some(icons::MAGNIFYING_GLASS),
                        kind: button::Kind::Primary,
                        size: Size::Medium,
                        width: button::Width::Fit,
                        enabled: props.inspect_enabled,
                    }
                    .show(ui)
                    .clicked();
                });
            });
        });

    if !props.storages.is_empty() {
        ui.add_space(16.0);
        for storage in props.storages {
            crate::capacity::show(ui, storage);
        }
    }
    inspect.then_some(Action::Inspect)
}

fn metadata(ui: &mut Ui, label: &str, value: &str) {
    let palette = crate::theme::palette(ui);
    ui.label(
        RichText::new(label)
            .small()
            .color(palette.content().text_secondary().into_cint()),
    );
    ui.label(value);
    ui.add_space(8.0);
}
