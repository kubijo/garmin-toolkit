use gallery::prelude::*;
use garmin_ui::{device, icons};

scene_meta! { title: "Application / Devices" }

#[scene(default)]
fn inspecting(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    stage!(ctx, ui, (720, 300), |ui| {
        device::show(
            ui,
            &device::Props {
                name: "Garmin Edge 1050",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Inspecting…",
                status_label: "Status",
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
                name: "Garmin Edge 1050",
                connection: "USB/MTP",
                identifier: Some("1234567890"),
                software: Some("9.12"),
                status: "Ready",
                status_label: "Status",
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
                name: "Garmin fēnix 8",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Inspection failed",
                status_label: "Status",
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
