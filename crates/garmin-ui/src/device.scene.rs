use gallery::prelude::*;
use garmin_ui::{device, icons};

scene_meta! { title: "Application / Devices" }

#[scene]
fn details(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    use garmin_model::device::{DeviceInspection, InspectionSection, StorageInspection};
    use garmin_service_api::{
        DeviceCapability, DeviceDataType, DeviceSnapshot, InspectionState, TransferDirection,
    };
    let width = ctx.slider("width", 720.0, 320.0, 880.0, 1.0);
    let state = ctx.buttons("state", &["ready", "inspecting", "failed"], 0);
    let snapshot = DeviceSnapshot {
        key: "mock-watch".to_owned(),
        name: "Mock Watch-o-Matic 9000".to_owned(),
        identifier: (state == 0).then_some(42_530_200),
        software_version: (state == 0).then_some(1870),
        inspection: match state {
            1 => InspectionState::Running,
            2 => InspectionState::Failed,
            _ => InspectionState::Ready,
        },
        inspection_error: None,
        capabilities: if state == 0 {
            vec![
                DeviceCapability {
                    data_type: DeviceDataType::Activity,
                    direction: TransferDirection::OutputFromUnit,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Workout,
                    direction: TransferDirection::InputOutput,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Course,
                    direction: TransferDirection::InputOutput,
                },
            ]
        } else {
            Vec::new()
        },
        storages: Vec::new(),
        report: (state == 0).then(|| DeviceInspection {
            manifest: InspectionSection::Missing,
            storage: InspectionSection::Missing,
            toolkit: vec![StorageInspection {
                storage_id: "internal".to_owned(),
                namespace: InspectionSection::Available(
                    "9e132b21-4910-47bd-93ac-1ea7a9afb2da"
                        .parse()
                        .expect("valid demo toolkit UUID"),
                ),
                identity: InspectionSection::Missing,
                transaction: InspectionSection::Missing,
            }],
        }),
    };
    stage!(ctx, ui, globals.stage((width, 400.0)), |ui| {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let _action = device::show_snapshot(ui, &globals.intl(), &snapshot, false);
            });
    });
}

#[scene]
fn partial_inspection(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    use garmin_model::device::{
        DeviceInspection, DeviceStateSnapshot, DeviceStorageState, IdentityInspection,
        InspectionFailure, InspectionFailureKind, InspectionSection, StorageCapacity,
        StorageInspection,
    };
    use garmin_service_api::{DeviceSnapshot, InspectionState};
    let storage = DeviceStorageState {
        id: "internal".to_owned(),
        label: "Internal storage".to_owned(),
        capacity: StorageCapacity::new(32_000_000_000, 8_600_000_000),
        writable: Some(true),
    };
    let snapshot = DeviceSnapshot {
        key: "mock-watch".to_owned(),
        name: "Mock Watch-o-Matic 9000".to_owned(),
        identifier: Some(42),
        software_version: Some(1870),
        inspection: InspectionState::Failed,
        inspection_error: None,
        capabilities: Vec::new(),
        storages: vec![storage.clone()],
        report: Some(DeviceInspection {
            manifest: InspectionSection::Missing,
            storage: InspectionSection::Available(DeviceStateSnapshot {
                storages: vec![storage],
            }),
            toolkit: vec![StorageInspection {
                storage_id: "internal".to_owned(),
                namespace: InspectionSection::Missing,
                identity: InspectionSection::Available(IdentityInspection {
                    device_id: "00000000-0000-4000-8000-000000000001"
                        .parse()
                        .expect("the synthetic device UUID is valid"),
                    device_digest: "a".repeat(32),
                    paired_user_id: None,
                    verified: true,
                }),
                transaction: InspectionSection::Unavailable(InspectionFailure {
                    kind: InspectionFailureKind::UnsupportedVersion,
                    message: "unsupported active-transaction device-state version 99".to_owned(),
                }),
            }],
        }),
    };
    stage!(ctx, ui, (360, 720), |ui| {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let _action = device::show_snapshot(ui, &globals.intl(), &snapshot, false);
            });
    });
}

#[scene(default)]
fn inspecting(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, (720, 300), |ui| {
        device::show(
            ui,
            &device::Props {
                name: "Mock Cycle-o-Matic 9000",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Inspecting…",
                status_label: "Status",
                inspection_error: None,
                inspection_error_label: "Inspection error",
                identifier_label: "Device ID",
                software_label: "Software",
                transfers_label: "Supported transfers",
                transfers: &[],
                storages: &[],
                icon: icons::BICYCLE,
            },
        );
    });
}

#[scene]
fn inspected(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, (720, 360), |ui| {
        let transfers = [
            device::Transfer {
                data: "Activities",
                directions: "Read",
            },
            device::Transfer {
                data: "Workouts",
                directions: "Read and write",
            },
            device::Transfer {
                data: "Courses",
                directions: "Read and write",
            },
        ];
        device::show(
            ui,
            &device::Props {
                name: "Mock Cycle-o-Matic 9000",
                connection: "USB/MTP",
                identifier: Some("1234567890"),
                software: Some("9.12"),
                status: "Ready",
                status_label: "Status",
                inspection_error: None,
                inspection_error_label: "Inspection error",
                identifier_label: "Device ID",
                software_label: "Software",
                transfers_label: "Supported transfers",
                transfers: &transfers,
                storages: &[],
                icon: icons::BICYCLE,
            },
        );
    });
}

#[scene]
fn failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, (720, 300), |ui| {
        device::show(
            ui,
            &device::Props {
                name: "Mock Watch-o-Matic 9000",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Inspection failed",
                status_label: "Status",
                inspection_error: Some("The device metadata could not be read."),
                inspection_error_label: "Inspection error",
                identifier_label: "Device ID",
                software_label: "Software",
                transfers_label: "Supported transfers",
                transfers: &[],
                storages: &[],
                icon: icons::WATCH,
            },
        );
    });
}
