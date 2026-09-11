use gallery::prelude::*;
use garmin_ui::{capacity, device, icons};

scene_meta! { title: "Components / Device state / Storage capacity" }

#[scene(default)]
fn multiple_storages(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(ctx, ui, false, false);
}

#[scene]
fn unavailable(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(ctx, ui, true, false);
}

#[scene]
fn long_metadata(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show(ctx, ui, true, true);
}

fn show(ctx: &mut SceneCtx<'_>, ui: &mut Ui, unavailable: bool, long: bool) {
    garmin_ui::theme::apply(ui.style_mut());
    let width = ctx.buttons("width", &["360", "720"], 1);
    ctx.stage(
        ui,
        Stage::Fixed(egui::vec2(if width == 0 { 360.0 } else { 720.0 }, 540.0)),
        |ui| {
            let storages = [
                capacity::Props {
                    label: if long {
                        "Internal storage exposed through a long desktop mount name"
                    } else {
                        "Internal storage"
                    },
                    detail: if long {
                        "Could not read storage usage: device disconnected. Reconnect, then refresh."
                    } else if unavailable {
                        "Storage usage could not be read"
                    } else {
                        "23.40 GB used · 8.60 GB free · 32.00 GB total"
                    },
                    bytes: (!unavailable).then_some((23_400_000_000, 32_000_000_000)),
                },
                capacity::Props {
                    label: "Memory card · read-only",
                    detail: "12.00 GB used · 52.00 GB free · 64.00 GB total",
                    bytes: Some((12_000_000_000, 64_000_000_000)),
                },
            ];
            device::show(
                ui,
                &device::Props {
                    name: "Garmin fēnix 8",
                    connection: "USB/MTP",
                    identifier: Some("1234567890"),
                    software: Some("9.12"),
                    status: "Ready",
                    status_label: "Status",
                    identifier_label: "Device ID",
                    software_label: "Software",
                    transfers_label: "Supported transfers",
                    transfers: &[],
                    storages: &storages,
                    icon: icons::WATCH,
                },
            );
        },
    );
}
