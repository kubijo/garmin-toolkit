//! Device collection and detail views.

use cint::ColorInterop;
use egui::{Id, RichText, TextStyle, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_service_api::{
    DeviceCapability, DeviceDataType, DeviceSnapshot, InspectionState, TransferDirection,
};

use crate::{Size, button, icons, modal, text::format_bytes};

#[cfg(test)]
mod acceptance;
mod inspection;

const DEVICE_ICON_SIZE: f32 = 32.0;
const CONTENT_MAX_WIDTH: f32 = 880.0;

/// Display data for one attached device.
pub struct Props<'a> {
    pub name: &'a str,
    pub connection: &'a str,
    pub identifier: Option<&'a str>,
    pub software: Option<&'a str>,
    pub status: &'a str,
    pub status_label: &'a str,
    pub inspection_error: Option<&'a str>,
    pub inspection_error_label: &'a str,
    pub identifier_label: &'a str,
    pub software_label: &'a str,
    pub transfers_label: &'a str,
    pub transfers: &'a [Transfer<'a>],
    pub storages: &'a [crate::capacity::Props<'a>],
    pub icon: icons::Icon,
}

/// One display row of supported device transfers.
pub struct Transfer<'a> {
    pub data: &'a str,
    pub directions: &'a str,
}

/// Availability of a device collection supplied by another process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionState<'a> {
    Connecting(&'a str),
    Empty(&'a str),
    Error(&'a str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    BrowseFiles,
    ManageMaps,
    Pair,
    Refresh,
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

pub fn show(ui: &mut Ui, props: &Props<'_>) {
    ui.set_max_width(CONTENT_MAX_WIDTH.min(ui.available_width()));
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
    ui.add_space(8.0);

    egui::Frame::new()
        .fill(crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::One),
        ))
        .corner_radius(crate::theme::PANEL_RADIUS)
        .stroke(egui::Stroke::new(
            1.0,
            crate::theme::color32(palette.borders().subtle()),
        ))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let summary = [
                    Some((props.status_label, props.status)),
                    props
                        .identifier
                        .map(|value| (props.identifier_label, value)),
                    props.software.map(|value| (props.software_label, value)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                let transfers = props
                    .transfers
                    .iter()
                    .map(|transfer| (transfer.data, transfer.directions))
                    .collect::<Vec<_>>();
                let table = crate::facts::Table::new(
                    ui,
                    summary.iter().chain(&transfers).map(|(label, _)| *label),
                );
                table.show(ui, "device-summary", &summary);
                if let Some(error) = props.inspection_error {
                    ui.add_space(8.0);
                    inspection_error(ui, props.inspection_error_label, error);
                }
                if !props.transfers.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new(props.transfers_label)
                            .color(palette.content().text_secondary().into_cint()),
                    );
                    ui.add_space(8.0);
                    table.show(ui, "device-transfers", &transfers);
                }
            });
        });

    if !props.storages.is_empty() {
        ui.add_space(8.0);
        for storage in props.storages {
            crate::capacity::show(ui, storage);
        }
    }
}

pub fn show_snapshot(
    ui: &mut Ui,
    intl: &Intl,
    snapshot: &DeviceSnapshot,
    browser_loading: bool,
    pairing_available: bool,
) -> Option<Action> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().interact_size.y = ui.text_style_height(&TextStyle::Body);
        snapshot_content(ui, intl, snapshot, browser_loading, pairing_available)
    })
    .inner
}

/// Confirm an explicit write of the selected profile association to the device.
pub fn pairing_confirmation(
    ui: &mut Ui,
    intl: &Intl,
    device_name: &str,
    profile_name: &str,
    previous_profile: Option<&str>,
    enabled: bool,
    presentation: modal::Presentation,
) -> Option<modal::Action> {
    let title = if previous_profile.is_some() {
        format_message!(intl, default_message: "Reassign device to this profile?")
    } else {
        format_message!(intl, default_message: "Pair device with profile?")
    };
    let description = if previous_profile.is_some() {
        format_message!(intl, default_message: "This changes the device's profile association. Previous marker revisions remain on the device.")
    } else {
        format_message!(intl, default_message: "This writes a profile marker to the selected device. The marker is not a credential.")
    };
    let target = format_message!(
        intl,
        default_message: "Pair {device} with {profile}",
        values: { device: device_name, profile: profile_name },
    );
    let cancel = format_message!(intl, default_message: "Cancel");
    let confirm = if previous_profile.is_some() {
        format_message!(intl, default_message: "Reassign device")
    } else {
        format_message!(intl, default_message: "Pair device")
    };
    let output = modal::show(
        ui,
        Id::new("confirm-device-pairing"),
        &modal::Props {
            title: &title,
            description: Some(&description),
            size: modal::Size::Small,
            presentation,
            cancel_label: Some(&cancel),
            backdrop_closes: Some(false),
            primary: modal::Primary {
                label: &confirm,
                icon: Some(if previous_profile.is_some() {
                    icons::ARROWS_CLOCKWISE
                } else {
                    icons::PLUS
                }),
                kind: if previous_profile.is_some() {
                    modal::PrimaryKind::Danger
                } else {
                    modal::PrimaryKind::Confirm
                },
                enabled,
            },
        },
        |ui| {
            if let Some(previous_profile) = previous_profile {
                ui.label(format_message!(
                    intl,
                    default_message: "Currently paired with {profile}",
                    values: { profile: previous_profile },
                ));
                ui.add_space(8.0);
            }
            ui.label(&target);
        },
    );
    crate::semantics::target(ui, &output.primary, "device.pair.confirm");
    if let Some(cancel) = &output.cancel {
        crate::semantics::target(ui, cancel, "device.pair.cancel");
    }
    output.action
}

fn snapshot_content(
    ui: &mut Ui,
    intl: &Intl,
    snapshot: &DeviceSnapshot,
    browser_loading: bool,
    pairing_available: bool,
) -> Option<Action> {
    let view = SnapshotView::new(snapshot, intl);
    let transfers = view
        .transfers
        .iter()
        .map(|transfer| Transfer {
            data: &transfer.data,
            directions: &transfer.directions,
        })
        .collect::<Vec<_>>();
    let storages = view
        .storages
        .iter()
        .map(|storage| crate::capacity::Props {
            label: &storage.label,
            detail: &storage.detail,
            bytes: storage.bytes,
        })
        .collect::<Vec<_>>();
    show(
        ui,
        &Props {
            name: &snapshot.name,
            connection: "USB/MTP",
            identifier: view.identifier.as_deref(),
            software: view.software.as_deref(),
            status: &view.status,
            status_label: &view.status_label,
            inspection_error: snapshot.inspection_error.as_deref(),
            inspection_error_label: &view.inspection_error_label,
            identifier_label: &view.identifier_label,
            software_label: &view.software_label,
            transfers_label: &view.transfers_label,
            transfers: &transfers,
            storages: &storages,
            icon: snapshot_icon(snapshot),
        },
    );
    if let Some(report) = &snapshot.report {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(16, 0))
            .show(ui, |ui| inspection::show(ui, intl, report));
    }
    controls(ui, intl, snapshot, browser_loading, pairing_available)
}

fn controls(
    ui: &mut Ui,
    intl: &Intl,
    snapshot: &DeviceSnapshot,
    browser_loading: bool,
    pairing_available: bool,
) -> Option<Action> {
    ui.horizontal_wrapped(|ui| {
        let refresh = button::Props {
            label: &format_message!(intl, default_message: "Refresh"),
            icon: Some(icons::ARROWS_CLOCKWISE),
            kind: button::Kind::Secondary,
            size: Size::Medium,
            width: button::Width::Fit,
            enabled: snapshot.inspection != InspectionState::Running,
        }
        .show(ui);
        crate::semantics::target(ui, &refresh, "device.refresh");
        let mut action = refresh.clicked().then_some(Action::Refresh);
        if pairing_available {
            let reassigning = snapshot.report.as_ref().is_some_and(|report| {
                report.toolkit.iter().any(|storage| {
                    matches!(
                        &storage.marker,
                        garmin_model::device::InspectionSection::Available(_)
                    )
                })
            });
            let label = if reassigning {
                format_message!(intl, default_message: "Reassign")
            } else {
                format_message!(intl, default_message: "Pair")
            };
            let response = button::Props {
                label: &label,
                icon: Some(if reassigning {
                    icons::ARROWS_CLOCKWISE
                } else {
                    icons::PLUS
                }),
                kind: button::Kind::Secondary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: snapshot.inspection == InspectionState::Ready,
            }
            .show(ui);
            crate::semantics::target(ui, &response, "device.pair");
            if response.clicked() {
                action = Some(Action::Pair);
            }
        }
        if !snapshot.storages.is_empty() {
            let browse = if browser_loading {
                format_message!(intl, default_message: "Reading files…")
            } else {
                format_message!(intl, default_message: "Browse files")
            };
            let response = button::Props {
                label: &browse,
                icon: Some(icons::FOLDER_OPEN),
                kind: button::Kind::Secondary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: !browser_loading,
            }
            .show(ui);
            crate::semantics::target(ui, &response, "device.files");
            if response.clicked() {
                action = Some(Action::BrowseFiles);
            }
            let response = button::Props {
                label: &format_message!(intl, default_message: "Manage maps"),
                icon: Some(icons::MAP_TRIFOLD),
                kind: button::Kind::Secondary,
                size: Size::Medium,
                width: button::Width::Fit,
                enabled: snapshot.inspection == InspectionState::Ready,
            }
            .show(ui);
            crate::semantics::target(ui, &response, "device.maps");
            if response.clicked() {
                action = Some(Action::ManageMaps);
            }
        }
        action
    })
    .inner
}

#[must_use]
pub fn snapshot_icon(snapshot: &DeviceSnapshot) -> icons::Icon {
    let name = snapshot.name.to_lowercase();
    if name.contains("edge") || name.contains("bike") || name.contains("cycle") {
        icons::BICYCLE
    } else if name.contains("fenix")
        || name.contains("fēnix")
        || name.contains("venu")
        || name.contains("watch")
    {
        icons::WATCH
    } else {
        icons::HARD_DRIVE
    }
}

struct SnapshotView {
    identifier: Option<String>,
    software: Option<String>,
    status: String,
    status_label: String,
    inspection_error_label: String,
    identifier_label: String,
    software_label: String,
    transfers_label: String,
    transfers: Vec<TransferView>,
    storages: Vec<StorageView>,
}

impl SnapshotView {
    fn new(snapshot: &DeviceSnapshot, intl: &Intl) -> Self {
        Self {
            identifier: snapshot.identifier.map(|value| value.to_string()),
            software: snapshot
                .software_version
                .map(|value| format!("{}.{:02}", value / 100, value % 100)),
            status: match snapshot.inspection {
                InspectionState::Running if snapshot.report.is_some() => {
                    format_message!(intl, default_message: "Refreshing…")
                }
                InspectionState::Running => {
                    format_message!(intl, default_message: "Inspecting…")
                }
                InspectionState::Ready => format_message!(intl, default_message: "Ready"),
                InspectionState::Failed => {
                    format_message!(intl, default_message: "Some device information is unavailable")
                }
            },
            status_label: format_message!(intl, default_message: "Status"),
            inspection_error_label: format_message!(intl, default_message: "Inspection error"),
            identifier_label: format_message!(intl, default_message: "Device ID"),
            software_label: format_message!(intl, default_message: "Software"),
            transfers_label: format_message!(intl, default_message: "Supported transfers"),
            transfers: transfer_views(&snapshot.capabilities, intl),
            storages: snapshot
                .storages
                .iter()
                .map(|storage| StorageView::new(storage, intl))
                .collect(),
        }
    }
}

struct TransferView {
    data: String,
    directions: String,
}

fn transfer_views(capabilities: &[DeviceCapability], intl: &Intl) -> Vec<TransferView> {
    [
        (
            DeviceDataType::Activity,
            format_message!(intl, default_message: "Activities"),
        ),
        (
            DeviceDataType::Workout,
            format_message!(intl, default_message: "Workouts"),
        ),
        (
            DeviceDataType::Course,
            format_message!(intl, default_message: "Courses"),
        ),
    ]
    .into_iter()
    .filter_map(|(data_type, data)| {
        let readable = capabilities.iter().any(|capability| {
            capability.data_type == data_type
                && matches!(
                    capability.direction,
                    TransferDirection::OutputFromUnit | TransferDirection::InputOutput
                )
        });
        let writable = capabilities.iter().any(|capability| {
            capability.data_type == data_type
                && matches!(
                    capability.direction,
                    TransferDirection::InputToUnit | TransferDirection::InputOutput
                )
        });
        let directions = match (readable, writable) {
            (true, true) => format_message!(intl, default_message: "Read and write"),
            (true, false) => format_message!(intl, default_message: "Read"),
            (false, true) => format_message!(intl, default_message: "Write"),
            (false, false) => return None,
        };
        Some(TransferView { data, directions })
    })
    .collect()
}

struct StorageView {
    label: String,
    detail: String,
    bytes: Option<(u64, u64)>,
}

impl StorageView {
    fn new(storage: &garmin_model::device::DeviceStorageState, intl: &Intl) -> Self {
        let label = if storage.writable == Some(false) {
            format_message!(
                intl,
                default_message: "{storage} · read-only",
                values: { storage: storage.label.as_str() },
            )
        } else {
            storage.label.clone()
        };
        let measured = storage.capacity.bytes();
        let bytes = measured.map(|(total, free)| (total - free, total));
        let detail = measured.map_or_else(
            || {
                let reason = match &storage.capacity {
                    garmin_model::device::StorageCapacity::Unavailable { reason } => {
                        reason.as_str()
                    }
                    garmin_model::device::StorageCapacity::Available { .. } => {
                        "The device reported invalid storage totals"
                    }
                };
                format_message!(
                    intl,
                    default_message: "Could not read storage usage: {reason}",
                    values: { reason: reason },
                )
            },
            |(total, free)| {
                format_message!(
                    intl,
                    default_message: "{used} used · {free} free · {total} total",
                    values: {
                        used: format_bytes(total - free),
                        free: format_bytes(free),
                        total: format_bytes(total),
                    },
                )
            },
        );
        Self {
            label,
            detail,
            bytes,
        }
    }
}

fn inspection_error(ui: &mut Ui, label: &str, value: &str) {
    let palette = crate::theme::palette(ui);
    let color = palette.support().error().into_cint();
    ui.label(
        RichText::new(label)
            .small()
            .color(palette.content().text_secondary().into_cint()),
    );
    let value = crate::text::balanced(ui, value, &TextStyle::Body, false);
    ui.label(RichText::new(value).color(color));
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use garmin_i18n::{Language, Translations};

    use super::*;

    #[test]
    fn transfer_capabilities_are_grouped_by_data_type() {
        let intl = Translations::bundled()
            .unwrap()
            .formatter(Language::English)
            .unwrap();
        let views = transfer_views(
            &[
                DeviceCapability {
                    data_type: DeviceDataType::Activity,
                    direction: TransferDirection::OutputFromUnit,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Workout,
                    direction: TransferDirection::OutputFromUnit,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Workout,
                    direction: TransferDirection::InputToUnit,
                },
            ],
            &intl,
        );

        assert_eq!(views.len(), 2);
        assert_eq!(views[0].directions, "Read");
        assert_eq!(views[1].directions, "Read and write");
    }
}
