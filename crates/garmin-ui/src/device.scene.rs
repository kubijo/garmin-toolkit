use gallery::prelude::*;
use garmin_ui::{device, icons};

scene_meta! { title: "Desktop / Compositions / Attached device" }

#[scene(default)]
fn available(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, (720, 300), |ui| {
        let _action = device::show(
            ui,
            &device::Props {
                name: "Garmin Edge 1050",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Ready to inspect",
                status_label: "Status",
                identifier_label: "Device ID",
                software_label: "Software",
                transfers_label: "Supported transfers",
                transfers: &[],
                storages: &[],
                icon: icons::BICYCLE,
                inspect_label: "Read device details",
                inspect_enabled: true,
            },
        );
    });
}

#[scene]
fn inspected(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
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
        let _action = device::show(
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
                inspect_label: "Device inspected",
                inspect_enabled: false,
            },
        );
    });
}

#[scene]
fn inspecting(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, (720, 300), |ui| {
        let _action = device::show(
            ui,
            &device::Props {
                name: "Garmin fēnix 8",
                connection: "USB/MTP",
                identifier: None,
                software: None,
                status: "Reading device details…",
                status_label: "Status",
                identifier_label: "Device ID",
                software_label: "Software",
                transfers_label: "Supported transfers",
                transfers: &[],
                storages: &[],
                icon: icons::WATCH,
                inspect_label: "Reading device details…",
                inspect_enabled: false,
            },
        );
    });
}
