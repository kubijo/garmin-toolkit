use gallery::prelude::*;
use garmin_service_api::snapshots::{SnapshotPreview, SnapshotState, SnapshotStatus};
use garmin_ui::backup;

scene_meta! { title: "Application / Backup and restore" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let locations = ctx.buttons("locations", &["local", "local-and-server"], 0);
    let state = ctx.buttons(
        "state",
        &[
            "idle",
            "preparing",
            "transfer",
            "verify",
            "confirm",
            "restore",
            "saved",
            "cancelled",
            "failed",
            "restored",
            "recovery-failed",
            "download",
        ],
        0,
    );
    let width = ctx.slider("width", 640.0, 320.0, 900.0, 1.0);
    let intl = globals.intl();
    let status = SnapshotStatus {
        operation: "00000000-0000-4000-8000-000000000001"
            .parse()
            .expect("synthetic UUID"),
        epoch: "00000000-0000-4000-8000-000000000002"
            .parse()
            .expect("synthetic UUID"),
        state: match state {
            3 => SnapshotState::Verifying,
            4 => SnapshotState::AwaitingApproval,
            5 => SnapshotState::Restoring,
            _ => SnapshotState::Uploading,
        },
        transferred: 2_000_000,
        total: Some(8_000_000),
        error: None,
        preview: (state == 4).then(|| SnapshotPreview {
            format_version: 1,
            app_version: "0.1.0".into(),
            created_at: 1_790_640_000,
            database_bytes: 8_000_000,
            database_sha256: "a".repeat(64),
            approval: "00000000-0000-4000-8000-000000000003"
                .parse()
                .expect("synthetic UUID"),
        }),
    };
    let available = state != 10;
    let name = if locations == 1 {
        "/share/backups/garmin-backup-2026-09-29.tar.zst"
    } else {
        "garmin-backup-2026-09-29.tar.zst"
    }
    .to_owned();
    let file = match state {
        0 | 10 => None,
        1 | 6 | 11 => Some(backup::SelectedFile::SaveDestination(name)),
        _ => Some(backup::SelectedFile::RestoreSource(name)),
    };
    let state = match state {
        0 => backup::State::Idle,
        1 => backup::State::Starting,
        6 => backup::State::Saved,
        7 => backup::State::Cancelled,
        8 => backup::State::Failed("The selected backup failed its database integrity check. Your current data is unchanged.".into()),
        9 => backup::State::Restored(backup::RestoreSummary {
            elapsed: Some(std::time::Duration::from_secs(18)),
            archive_bytes: Some(1_200_000),
            database_bytes: Some(8_000_000),
            created_at: Some(1_790_640_000),
        }),
        11 => backup::State::DownloadStarted,
        10 => backup::State::Failed("The selected database could not be reopened: permission denied. The application is unavailable until recovery succeeds.".into()),
        _ => backup::State::Running(status),
    };
    stage!(ctx, ui, |ui| {
        ui.set_width(width);
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .show(ui, |ui| {
                let _ = backup::show(
                    ui,
                    &backup::Props {
                        server_files: locations == 1,
                        intl: &intl,
                        state: &state,
                        file: file.as_ref(),
                        enabled: available,
                    },
                );
            });
    });
}
