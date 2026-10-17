use gallery::prelude::*;
use garmin_service_api::snapshots::{
    SnapshotOperation, SnapshotSource, SnapshotState, SnapshotStatus,
};
use garmin_ui::backup::recovery;

scene_meta! { title: "Application / Backup and restore / Recovery" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 640.0, 288.0, 900.0, 1.0);
    let state = ctx.buttons(
        "state",
        &[
            "review",
            "upload",
            "active",
            "restoring",
            "saved",
            "loading",
            "error",
        ],
        0,
    );
    let operations = [SnapshotOperation {
        status: SnapshotStatus {
            operation: "00000000-0000-4000-8000-000000000001"
                .parse()
                .expect("fixture UUID"),
            epoch: "00000000-0000-4000-8000-000000000002"
                .parse()
                .expect("fixture UUID"),
            state: match state {
                1 => SnapshotState::Uploading,
                3 => SnapshotState::Restoring,
                4 => SnapshotState::Completed,
                _ => SnapshotState::AwaitingApproval,
            },
            transferred: 2_000_000,
            total: Some(8_000_000),
            preview: None,
            error: None,
        },
        source: if state == 1 {
            SnapshotSource::Upload
        } else if state == 4 {
            SnapshotSource::ServerSave("/share/backups/garmin-backup-2026-09-29.tar.zst".into())
        } else {
            SnapshotSource::ServerRestore("/share/backups/garmin-backup-2026-09-29.tar.zst".into())
        },
        active: state == 2,
    }];
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.set_width(width);
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(16)
            .show(ui, |ui| {
                let _ = recovery::show(
                    ui,
                    &recovery::Props {
                        intl: &globals.intl(),
                        operations: if state >= 5 { &[] } else { &operations },
                        loading: state == 5,
                        error: (state == 6).then_some(
                            "Connection lost. Reconnect and refresh to recover the operation.",
                        ),
                    },
                );
            });
    });
}
