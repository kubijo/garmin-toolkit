use super::PreviewState;
use crate::{
    LINK_BENCHMARK_COMPLETE, LINK_BENCHMARK_STAGES, OperationView, PIPELINE_PROBE_COMPLETE,
    PIPELINE_PROBE_STAGES, ProgressModel, ProgressPhase, ProgressPresentation,
    REMOVAL_RECOVERY_COMPLETE, REMOVAL_RECOVERY_STAGES, REMOVAL_STAGES, UPDATE_COMPLETE,
    UPDATE_PROGRESS_TITLE, UPDATE_RECOVERY_COMPLETE, UPDATE_RECOVERY_STAGES, UPDATE_STAGES,
};
use garmin_progress::{
    OperationStage, ProgressEvent, ProgressEventKind, ProgressReporter, ProgressState, ProgressUnit,
};
use ratatui::{Frame, layout::Rect};
use std::time::{Duration, SystemTime};

pub(super) fn render_stage(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    active: OperationStage,
    preview: &mut PreviewState,
) {
    let mut model = progress_fixture(UPDATE_STAGES, active, preview_stage_details, None);
    if active == OperationStage::Backup {
        for state in [ProgressState::Started, ProgressState::Advanced] {
            model.apply(&ProgressEvent {
                recorded_at: SystemTime::now(),
                scope: garmin_progress::ProgressScope::Item {
                    id: "backup:internal:Garmin/D6184140A.img".to_owned(),
                },
                stage: OperationStage::Backup,
                state,
                kind: ProgressEventKind::default(),
                unit: ProgressUnit::Bytes,
                label: "Backing up device file".to_owned(),
                path: Some("Garmin/D6184140A.img".to_owned()),
                completed: if state == ProgressState::Started {
                    0
                } else {
                    1_560_000_000
                },
                total: Some(3_402_039_296),
            });
        }
        let now = std::time::Instant::now();
        if let Some((_, stage)) = model
            .stages
            .iter_mut()
            .find(|(stage, _)| *stage == OperationStage::Backup)
        {
            stage.elapsed = Some(Duration::from_secs(400));
        }
        for item in model.active_mut() {
            item.view.started_at = now.checked_sub(Duration::from_secs(135));
            item.view.updated_at = now.checked_sub(Duration::from_secs(3));
        }
    }
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: UPDATE_PROGRESS_TITLE,
            phase: ProgressPhase::Running,
        },
        &model,
    );
}

fn progress_fixture(
    stages: &[OperationStage],
    active: OperationStage,
    details: fn(OperationStage) -> (&'static str, Option<&'static str>, u64),
    completion: Option<&str>,
) -> ProgressModel {
    let active_index = stages
        .iter()
        .position(|stage| *stage == active)
        .unwrap_or_default();
    let mut model = ProgressModel::new(stages, "Preparing preview…");

    for (index, stage) in stages[..=active_index].iter().copied().enumerate() {
        let (label, path, total) = details(stage);
        model.apply(&ProgressEvent {
            recorded_at: SystemTime::now(),
            scope: garmin_progress::ProgressScope::Stage,
            stage,
            state: ProgressState::Started,
            kind: ProgressEventKind::default(),
            unit: preview_progress_unit(stage),
            label: label.to_owned(),
            path: path.map(str::to_owned),
            completed: 0,
            total: Some(total),
        });
        let state = if index == active_index && completion.is_none() {
            ProgressState::Advanced
        } else {
            ProgressState::Completed
        };
        model.apply(&ProgressEvent {
            recorded_at: SystemTime::now(),
            scope: garmin_progress::ProgressScope::Stage,
            stage,
            state,
            kind: ProgressEventKind::default(),
            unit: preview_progress_unit(stage),
            label: label.to_owned(),
            path: path.map(str::to_owned),
            completed: if state == ProgressState::Completed {
                total
            } else {
                total.saturating_mul(2) / 5
            },
            total: Some(total),
        });
        model.stages[index].1.elapsed =
            Some(Duration::from_secs(if total >= 10_000_000 { 4 } else { 1 }));
    }
    if let Some(message) = completion {
        model.current = OperationView::completed(message);
    }
    stabilize_history(&mut model);
    model
}

fn stabilize_history(model: &mut ProgressModel) {
    for (index, operation) in model.history.iter_mut().enumerate() {
        operation.recorded_at = Some(
            std::time::UNIX_EPOCH
                + Duration::from_secs(45_296 + u64::try_from(index).unwrap_or(u64::MAX)),
        );
        operation.duration = operation.stage.and_then(|stage| {
            model
                .stages
                .iter()
                .find(|(candidate, _)| *candidate == stage)
                .and_then(|(_, view)| view.elapsed)
        });
    }
}

pub(super) fn render_removal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    active: OperationStage,
    completion: Option<&str>,
    preview: &mut PreviewState,
) {
    let model = progress_fixture(
        REMOVAL_STAGES,
        active,
        removal_preview_stage_details,
        completion,
    );
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Removing map components",
            phase: if completion.is_some() {
                ProgressPhase::Complete
            } else {
                ProgressPhase::Running
            },
        },
        &model,
    );
}

pub(super) fn render_removal_recovery_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let model = progress_fixture(
        REMOVAL_RECOVERY_STAGES,
        OperationStage::Cleanup,
        removal_recovery_preview_stage_details,
        Some(REMOVAL_RECOVERY_COMPLETE),
    );
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Recovering component removal",
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

pub(super) fn render_update_recovery_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let model = progress_fixture(
        UPDATE_RECOVERY_STAGES,
        OperationStage::Cleanup,
        update_recovery_preview_stage_details,
        Some(UPDATE_RECOVERY_COMPLETE),
    );
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Recovering map update",
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

pub(super) fn render_pipeline_complete_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let mut model = ProgressModel::new(PIPELINE_PROBE_STAGES, "Preparing pipeline benchmark…");

    for stage in PIPELINE_PROBE_STAGES {
        let (label, path, total) = pipeline_preview_stage_details(*stage);
        for state in [ProgressState::Started, ProgressState::Completed] {
            model.apply(&ProgressEvent {
                recorded_at: SystemTime::now(),
                scope: garmin_progress::ProgressScope::Stage,
                stage: *stage,
                state,
                kind: ProgressEventKind::default(),
                unit: stage.progress_unit(),
                label: label.to_owned(),
                path: path.map(str::to_owned),
                completed: if state == ProgressState::Completed {
                    total
                } else {
                    0
                },
                total: Some(total),
            });
        }
    }
    for (stage, view) in &mut model.stages {
        view.elapsed = match stage {
            OperationStage::Download => Some(Duration::from_secs(8)),
            OperationStage::Verify => Some(Duration::from_secs(2)),
            OperationStage::Upload => Some(Duration::from_secs(42)),
            _ => view.elapsed,
        };
    }
    stabilize_history(&mut model);
    model.current = OperationView::completed(PIPELINE_PROBE_COMPLETE);
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Real update pipeline probe",
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

pub(super) fn render_link_benchmark_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let mut model = progress_fixture(
        LINK_BENCHMARK_STAGES,
        OperationStage::DeviceVerify,
        link_benchmark_preview_stage_details,
        None,
    );
    set_link_benchmark_timings(&mut model, true);
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Device link benchmark",
            phase: ProgressPhase::Running,
        },
        &model,
    );
}

pub(super) fn render_link_benchmark_complete_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let mut model = progress_fixture(
        LINK_BENCHMARK_STAGES,
        OperationStage::Delete,
        link_benchmark_preview_stage_details,
        Some(LINK_BENCHMARK_COMPLETE),
    );
    set_link_benchmark_timings(&mut model, false);
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Device link benchmark",
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

fn set_link_benchmark_timings(model: &mut ProgressModel, reading: bool) {
    for (stage, view) in &mut model.stages {
        if *stage == OperationStage::DeviceVerify {
            view.unit = ProgressUnit::Bytes;
        }
        view.elapsed = match stage {
            OperationStage::Upload => Some(Duration::from_millis(16_170)),
            OperationStage::DeviceFinalize => Some(Duration::from_millis(30)),
            OperationStage::DeviceVerify if reading => Some(Duration::from_secs(38)),
            OperationStage::DeviceVerify => Some(Duration::from_millis(95_960)),
            OperationStage::Delete => Some(Duration::from_millis(790)),
            _ => view.elapsed,
        };
        view.label = match (*stage, view.state) {
            (OperationStage::Upload, Some(ProgressState::Completed)) => "MTP payload sent",
            (OperationStage::DeviceFinalize, Some(ProgressState::Completed)) => {
                "Device finalized the MTP upload"
            }
            (OperationStage::DeviceVerify, Some(ProgressState::Advanced)) => {
                "Reading MTP object back"
            }
            (OperationStage::DeviceVerify, Some(ProgressState::Completed)) => {
                "Read back and SHA-256 verified"
            }
            (OperationStage::Delete, Some(ProgressState::Completed)) => {
                "Disposable MTP object removed"
            }
            _ => view.label.as_str(),
        }
        .to_owned();
    }
    for operation in &mut model.history {
        operation.label = match operation.stage {
            Some(OperationStage::Upload) => "MTP payload sent",
            Some(OperationStage::DeviceFinalize) => "Device finalized the MTP upload",
            Some(OperationStage::DeviceVerify) => "Read back and SHA-256 verified",
            Some(OperationStage::Delete) => "Disposable MTP object removed",
            _ => operation.label.as_str(),
        }
        .to_owned();
    }
    if reading {
        "Reading MTP object back".clone_into(&mut model.current.label);
    }
    stabilize_history(model);
}

pub(super) fn render_link_benchmark_finalize_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    recovering: bool,
    preview: &mut PreviewState,
) {
    let mut model = progress_fixture(
        LINK_BENCHMARK_STAGES,
        OperationStage::DeviceFinalize,
        link_benchmark_preview_stage_details,
        None,
    );
    if let Some((_, upload)) = model
        .stages
        .iter_mut()
        .find(|(stage, _)| *stage == OperationStage::Upload)
    {
        upload.elapsed = Some(Duration::from_millis(16_320));
    }
    if recovering {
        model.apply(&ProgressEvent {
            recorded_at: SystemTime::now(),
            scope: garmin_progress::ProgressScope::Stage,
            stage: OperationStage::DeviceFinalize,
            state: ProgressState::Advanced,
            kind: ProgressEventKind::default(),
            unit: ProgressUnit::Operations,
            label: "Response timed out; reopening the device in 5 seconds".to_owned(),
            path: None,
            completed: 0,
            total: Some(1),
        });
    }
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: "garmin-cli — Device link benchmark",
            phase: ProgressPhase::Running,
        },
        &model,
    );
}

const fn preview_progress_unit(stage: OperationStage) -> ProgressUnit {
    if matches!(stage, OperationStage::Commit | OperationStage::DeviceVerify) {
        ProgressUnit::Operations
    } else {
        stage.progress_unit()
    }
}

pub(super) fn render_verification_complete_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let model = progress_fixture(
        UPDATE_STAGES,
        OperationStage::Cleanup,
        simulation_preview_stage_details,
        Some(UPDATE_COMPLETE),
    );
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: UPDATE_PROGRESS_TITLE,
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

fn simulation_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    if stage == OperationStage::Cleanup {
        (
            "Simulation files retained; physical source unchanged",
            Some("/safe/new/session/simulation"),
            1,
        )
    } else {
        preview_stage_details(stage)
    }
}
fn preview_stage_details(stage: OperationStage) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::Inspect => ("Device manifest parsed", Some("GarminDevice.xml"), 1),
        OperationStage::Query => ("Update metadata received from Garmin", None, 1),
        OperationStage::Plan => ("Selected update file", Some("Garmin/gmapbmap.img"), 1),
        OperationStage::Backup => (
            "Backing up files affected by the update",
            None,
            11_480_000_000,
        ),
        OperationStage::Download => (
            "Downloading Mock Cycle Map Europe",
            Some("Garmin/Mock/europe.img"),
            30_000_000,
        ),
        OperationStage::Verify => (
            "Verifying Mock Cycle Map Europe",
            Some("Garmin/Mock/europe.img"),
            30_000_000,
        ),
        OperationStage::Authorize => ("Requesting device-bound map authorization data", None, 1),
        OperationStage::Stage => (
            "Staging Mock Cycle Map Europe",
            Some("Garmin/Mock/europe.img"),
            30_000_000,
        ),
        OperationStage::Commit => ("Committing verified files", None, 3),
        OperationStage::Cleanup => ("Verified recovery backups retained in the capture", None, 1),
        OperationStage::Upload => ("Desktop MTP upload complete", None, 12_000_000),
        OperationStage::DeviceFinalize => {
            ("Waiting for the device to finalize the upload", None, 1)
        }
        OperationStage::DeviceVerify => ("Uploaded file path and size checked", None, 1),
        OperationStage::Delete => ("Disposable object removed", None, 1),
    }
}

fn removal_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::Backup => (
            "Backed up and verified device file",
            Some("Garmin/Mock/removal.img"),
            12_012_000,
        ),
        OperationStage::Commit => (
            "Removed size-checked device file",
            Some("Garmin/Mock/example.sid"),
            2,
        ),
        OperationStage::Cleanup => ("Recovery backups retained in the capture", None, 1),
        _ => preview_stage_details(stage),
    }
}

fn removal_recovery_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::Cleanup => (
            "Recovered device path and size checked",
            Some("Garmin/Mock/removal.img"),
            2,
        ),
        _ => preview_stage_details(stage),
    }
}

fn update_recovery_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::DeviceVerify => ("Current device file paths and sizes checked", None, 3),
        OperationStage::Cleanup => ("Mounted MTP update rolled back", None, 1),
        _ => preview_stage_details(stage),
    }
}

fn pipeline_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::Download => (
            "Downloaded selected map file",
            Some("Garmin/Mock/example.img"),
            12_000_000,
        ),
        OperationStage::Verify => (
            "Verified selected map file",
            Some("Garmin/Mock/example.img"),
            12_000_000,
        ),
        _ => preview_stage_details(stage),
    }
}

fn link_benchmark_preview_stage_details(
    stage: OperationStage,
) -> (&'static str, Option<&'static str>, u64) {
    match stage {
        OperationStage::Upload => ("MTP payload sent", None, 256_000_000),
        OperationStage::DeviceFinalize => {
            ("Waiting for the device to finalize the MTP upload", None, 1)
        }
        OperationStage::DeviceVerify => ("Read back and SHA-256 verified", None, 256_000_000),
        OperationStage::Delete => ("Disposable device file removed", None, 1),
        _ => preview_stage_details(stage),
    }
}
pub(super) fn render_completion_preview(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    preview: &mut PreviewState,
) {
    let model = progress_fixture(
        UPDATE_STAGES,
        OperationStage::Cleanup,
        preview_stage_details,
        Some(UPDATE_COMPLETE),
    );
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: UPDATE_PROGRESS_TITLE,
            phase: ProgressPhase::Complete,
        },
        &model,
    );
}

pub(super) fn render_concurrent(
    frame: &mut Frame<'_>,
    area: Rect,
    tick: usize,
    overflow: bool,
    preview: &mut PreviewState,
) {
    let (progress, receiver) = ProgressReporter::channel();
    let count = if overflow { 8 } else { 3 };
    let total = count * 80_000_000;
    progress.completed(
        OperationStage::Backup,
        "Skipped by user; automatic rollback is unavailable",
        0,
        None,
    );
    progress.started(
        OperationStage::Download,
        "Downloading map files",
        Some(total),
    );
    progress.started(OperationStage::Verify, "Verifying map files", Some(total));
    let step = u64::try_from(tick % 80).unwrap_or_default() * 500_000;
    for index in 0..count {
        let item = progress.for_item(format!("download:{index}"));
        let path = match index {
            0 => "Garmin/TopoActive_Europe_Central.img".to_owned(),
            1 => "Garmin/TopoActive_Europe_West.img".to_owned(),
            2 => "Garmin/Maps/A deliberately long map directory/TopoActive cycle routes and elevation data for Northern Europe.img".to_owned(),
            _ => format!("Garmin/TopoActive_region_{index}.img"),
        };
        item.started_with_path(
            OperationStage::Download,
            "Downloading map",
            &path,
            Some(80_000_000),
        );
        item.advanced_with_path(
            OperationStage::Download,
            "Downloading map",
            &path,
            8_000_000 + step,
            Some(80_000_000),
        );
        if index % 2 == 1 {
            item.completed_with_path(
                OperationStage::Download,
                "Download complete",
                &path,
                80_000_000,
                Some(80_000_000),
            );
            item.started_with_path(
                OperationStage::Verify,
                "Checking checksum",
                &path,
                Some(80_000_000),
            );
            item.completed_with_path(
                OperationStage::Verify,
                "Garmin MD5 verified",
                &path,
                80_000_000,
                Some(80_000_000),
            );
        }
    }
    progress.advanced(
        OperationStage::Download,
        format!("Downloading {count} map files"),
        count / 2 * (88_000_000 + step),
        Some(total),
    );
    progress.advanced(
        OperationStage::Verify,
        format!("Verifying {count} map files"),
        count / 2 * 80_000_000,
        Some(total),
    );
    let mut model = ProgressModel::new(UPDATE_STAGES, "Preparing update…");
    for event in receiver.try_iter() {
        model.apply(&event);
    }
    for (operation, stage) in &mut model.stages {
        if *operation != OperationStage::Backup {
            stage.elapsed = Some(std::time::Duration::from_secs(15));
        }
    }
    for item in model.active_mut() {
        item.view.elapsed = Some(std::time::Duration::from_secs(15));
    }
    preview.render_progress(
        frame,
        area,
        ProgressPresentation {
            title: UPDATE_PROGRESS_TITLE,
            phase: ProgressPhase::Running,
        },
        &model,
    );
}
