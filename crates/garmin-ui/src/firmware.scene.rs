use gallery::prelude::*;
use garmin_cli_tui::preview::{PreviewScreen, firmware_components};
use garmin_model::artifact::ByteCount;
use garmin_model::value::Timestamp;
use garmin_service_api::firmware::{
    Component, Inspection, InstalledStatus, Observation, Phase, ReceiptStatus, RetainedTransfer,
    Review, State, Transcript, TransferProgress,
};
use uuid::Uuid;

scene_meta! { title: "Application / Device updates" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let device = ctx.buttons("device", &["Edge", "Watch components", "Watch system"], 0);
    let step = ctx.buttons(
        "state",
        &[
            "catalog",
            "review",
            "staging",
            "ready",
            "reconnected",
            "recovery",
            "empty",
            "consent",
            "review recovery",
            "transfer receipts",
            "previous transfer pending",
            "write failed",
            "reported version differs",
            "checking",
        ],
        0,
    );
    let width = ctx.slider("width", 736.0, 320.0, 1200.0, 8.0);
    let state = preview_state(device, step);
    stage!(ctx, ui, globals.stage((width, 720.0)), |ui| {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let _action = garmin_ui::firmware::show(ui, &globals.intl(), &state);
    });
}

fn preview_state(device: usize, step: usize) -> State {
    let screen = match device {
        0 => PreviewScreen::FirmwareEdge,
        1 => PreviewScreen::FirmwareWatchComponents,
        _ => PreviewScreen::FirmwareWatchSystem,
    };
    let components = firmware_components(screen);
    let selected = components
        .iter()
        .position(|item| item.system_bundle)
        .unwrap_or(0);
    let total = components[selected]
        .download_bytes
        .expect("captured deliverable size");
    let transferred = total.min(ByteCount::from_mebibytes(72));
    let review = (step == 1).then(|| preview_review(&components));
    let phase = match step {
        2 => Phase::Staging(TransferProgress {
            label: components[selected].name.clone(),
            completed: Some(transferred),
            total: Some(total),
        }),
        3 => Phase::ReadyToEject {
            needs_restart: components[selected].needs_restart,
        },
        4 => Phase::Reconnected(
            components
                .iter()
                .take(2)
                .enumerate()
                .map(|(index, item)| Observation {
                    part_number: item.part_number.clone(),
                    name: item.name.clone(),
                    offered: item.available.clone(),
                    status: if index == 0 {
                        InstalledStatus::Confirmed
                    } else {
                        InstalledStatus::Unchanged
                    },
                })
                .collect(),
        ),
        5 | 11 => Phase::RecoveryRequired,
        6 => Phase::Available(Vec::new()),
        7 => Phase::Consent,
        8 => Phase::ReviewRecovery {
            approval: Uuid::nil(),
            files: vec!["Internal · Garmin/GUPDATE.GSP".to_owned()],
        },
        9 => Phase::Transfers(vec![
            RetainedTransfer {
                receipt: Uuid::from_fields(0x3a9b_1460, 0, 0, &[0; 8]),
                registered_at: Timestamp::from_unix_seconds(1_795_262_400).expect("fixture date"),
                status: ReceiptStatus::RecoveryRequired,
            },
            RetainedTransfer {
                receipt: Uuid::from_fields(0x72de_8f54, 0, 0, &[0; 8]),
                registered_at: Timestamp::from_unix_seconds(1_795_179_600).expect("fixture date"),
                status: ReceiptStatus::Confirmed,
            },
        ]),
        10 | 13 => Phase::Checking,
        12 => edge_version_mismatch(),
        _ => Phase::Available(components.clone()),
    };
    State {
        transcript: transfer_transcript(step, total, transferred, &components[selected].name),
        has_retained_transfer: matches!(step, 3 | 4 | 5 | 8..=12),
        simulated: true,
        revision: Uuid::from_u128((device * 14 + step) as u128),
        phase,
        review,
        catalog_id: Uuid::from_u128(device as u128),
        inspection: matches!(step, 4 | 5 | 8..=12).then(|| Inspection {
            background: match step {
                10 => Phase::PreviousTransferPending,
                11 => Phase::WriteFailed(
                    "Device disconnected while writing Garmin/KBD/Updates/Emojis.db".to_owned(),
                ),
                _ => Phase::Available(components),
            },
            from_list: false,
        }),
    }
}

fn edge_version_mismatch() -> Phase {
    Phase::Reconnected(vec![
        Observation {
            part_number: "006-DB585-00".into(),
            name: "DB, Emoji Category Data".into(),
            offered: "1.00".into(),
            status: InstalledStatus::OtherVersion("100.00".into()),
        },
        Observation {
            part_number: "006-DB477-00".into(),
            name: "Audio Prompt Bundle Schema 3".into(),
            offered: "2.08".into(),
            status: InstalledStatus::Confirmed,
        },
    ])
}

fn preview_review(components: &[Component]) -> Review {
    let mut review = Review {
        approval: Uuid::nil(),
        selected: Vec::new(),
        download_bytes: ByteCount::default(),
    };
    for (index, component) in components.iter().enumerate() {
        if let Some(size) = component.download_bytes
            && component.staging_issue.is_none()
        {
            review
                .selected
                .push(u32::try_from(index).expect("small catalog"));
            review.download_bytes = review
                .download_bytes
                .checked_add(size)
                .expect("fixture total");
        }
    }
    review
}

fn transfer_transcript(
    step: usize,
    size: ByteCount,
    transferred: ByteCount,
    name: &str,
) -> Transcript {
    use garmin_progress::{
        OperationStage::{
            Backup, Cleanup, Commit, DeviceFinalize, DeviceVerify, Download, Stage, Verify,
        },
        ProgressAmount, ProgressEvent, ProgressEventKind, ProgressScope, ProgressState,
        model::ProgressModel,
    };
    use std::time::{Duration, UNIX_EPOCH};
    if !matches!(step, 2 | 3 | 11) {
        return Transcript::default();
    }
    let mut model = ProgressModel::new(garmin_service_api::firmware::WRITE_STAGES, "Waiting");
    for (index, (stage, label)) in [
        (Download, "Downloaded update file"),
        (Verify, "Verified file checksum"),
        (Stage, "Device transaction validated"),
        (Backup, "Original files backed up"),
        (Commit, "Wrote update file"),
        (DeviceFinalize, "Device finalized the upload"),
        (DeviceVerify, "Verified file path and size"),
        (Cleanup, "Retained verified backups"),
    ]
    .into_iter()
    .enumerate()
    {
        let writing = stage == Commit && step != 3;
        let event = ProgressEvent {
            recorded_at: UNIX_EPOCH + Duration::from_secs(1_795_262_400 + index as u64),
            scope: ProgressScope::Stage,
            stage,
            state: if writing && step == 11 {
                ProgressState::Failed
            } else if writing {
                ProgressState::Advanced
            } else {
                ProgressState::Completed
            },
            kind: ProgressEventKind::Generic,
            amount: if matches!(stage, Commit | Download | Verify) {
                ProgressAmount::Bytes {
                    completed: if writing { transferred } else { size },
                    total: Some(size),
                }
            } else {
                ProgressAmount::Operations {
                    completed: 1,
                    total: Some(1),
                }
            },
            label: if writing {
                "Writing update file"
            } else {
                label
            }
            .to_owned(),
            path: matches!(stage, Commit | Verify | Download).then(|| name.to_owned()),
        };
        model.apply(&event);
        if writing {
            break;
        }
    }
    model.transcript()
}
