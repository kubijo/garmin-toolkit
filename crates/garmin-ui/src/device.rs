//! Device collection and detail views.

use cint::ColorInterop;
use egui::{RichText, TextStyle, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_service_api::{
    DeviceCapability, DeviceDataType, DeviceSnapshot, InspectionState, TransferDirection,
};

use crate::icons;

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

    egui::Frame::new()
        .fill(crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::One),
        ))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
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
        });

    if !props.storages.is_empty() {
        ui.add_space(16.0);
        for storage in props.storages {
            crate::capacity::show(ui, storage);
        }
    }
}

pub fn show_snapshot(ui: &mut Ui, intl: &Intl, snapshot: &DeviceSnapshot) {
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
            identifier_label: &view.identifier_label,
            software_label: &view.software_label,
            transfers_label: &view.transfers_label,
            transfers: &transfers,
            storages: &storages,
            icon: snapshot_icon(snapshot),
        },
    );
}

#[must_use]
pub fn snapshot_icon(snapshot: &DeviceSnapshot) -> icons::Icon {
    let name = snapshot.name.to_lowercase();
    if name.contains("edge") {
        icons::BICYCLE
    } else if name.contains("fenix") || name.contains("fēnix") || name.contains("venu") {
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
                InspectionState::Running => {
                    format_message!(intl, default_message: "Inspecting…")
                }
                InspectionState::Ready => format_message!(intl, default_message: "Ready"),
                InspectionState::Failed => {
                    format_message!(intl, default_message: "Inspection failed")
                }
            },
            status_label: format_message!(intl, default_message: "Status"),
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

fn format_bytes(bytes: u64) -> String {
    format!(
        "{:.2}",
        byte_unit::Byte::from_u64(bytes).get_appropriate_unit(byte_unit::UnitType::Decimal)
    )
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
