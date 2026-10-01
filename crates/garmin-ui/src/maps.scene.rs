use gallery::prelude::*;
use garmin_service_api::maps::{
    Action, Choice, Component, Phase, Plan, Progress, ProgressStatus, Recovery, State,
};

scene_meta! { title: "Application / Manage maps" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let variant = ctx.buttons(
        "state",
        &[
            "consent",
            "loading",
            "catalog",
            "review",
            "running",
            "recovery",
            "complete",
            "failed",
            "cancelled",
            "empty",
            "rollback",
        ],
        2,
    );
    let width = ctx.slider("width", 880.0, 320.0, 1120.0, 1.0);
    let height = ctx.slider("height", 920.0, 320.0, 1200.0, 1.0);
    let mut state = fixture();
    state.dry_run = ctx.toggle("simulation", false);
    state.phase = match variant {
        0 => Phase::Consent,
        1 => Phase::Loading,
        3 => Phase::Review,
        4 | 10 => Phase::Running,
        5 => Phase::Recovery,
        6 => Phase::Completed,
        7 => Phase::Failed,
        8 => Phase::Cancelled,
        _ => Phase::Catalog,
    };
    state.actions = match state.phase {
        Phase::Consent => vec![Action::ContactService],
        Phase::Loading => Vec::new(),
        Phase::Catalog => vec![Action::Choose, Action::Review, Action::ContactService],
        Phase::Review => vec![Action::Approve, Action::Back],
        Phase::Running => vec![Action::Cancel],
        Phase::Recovery => vec![Action::Recover, Action::ClearRecovery, Action::Refresh],
        _ => vec![Action::Refresh],
    };
    if variant == 7 {
        state.error = Some("The device disconnected while writing Garmin/Mock/europe-routing.img. Reconnect the same device to inspect its retained recovery evidence.".to_owned());
    }
    if variant == 9 {
        state.components.clear();
    }
    if matches!(variant, 4 | 10) {
        progress_steps(&mut state, variant == 10);
    }
    if ctx.toggle("history", false) {
        let mut event = state.progress[0].clone();
        "Update files written and verified".clone_into(&mut event.label);
        event.completed = event.total.unwrap_or(event.completed);
        event.status = ProgressStatus::Completed;
        event.bytes_per_second = None;
        event.remaining_seconds = None;
        state.events.push(event);
        state.history = [
            (Phase::Completed, "Removal completed"),
            (Phase::Completed, "Completed"),
            (Phase::Cancelled, "mounted MTP update cancelled"),
            (
                Phase::Failed,
                "The device disconnected while writing Garmin/Mock/europe-routing.img.",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(
            |(index, (phase, message))| garmin_service_api::maps::Outcome {
                id: format!("00000000-0000-4000-8000-{index:012}")
                    .parse()
                    .expect("synthetic outcome UUID"),
                phase,
                message: message.to_owned(),
            },
        )
        .collect();
    }
    stage!(ctx, ui, globals.stage((width, height)), |ui| {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let _command = garmin_ui::maps::show(ui, &globals.intl(), &state);
    });
}

fn progress_steps(state: &mut State, rollback: bool) {
    let mut backup = state.progress[0].clone();
    "Backup".clone_into(&mut backup.stage);
    "All affected device files are backed up".clone_into(&mut backup.label);
    backup.completed = 8_000_000;
    backup.total = Some(8_000_000);
    backup.elapsed_ms = 18_000;
    backup.bytes_per_second = None;
    backup.remaining_seconds = None;
    backup.status = ProgressStatus::Completed;
    let mut verified = backup.clone();
    "Verify".clone_into(&mut verified.stage);
    "Device transaction validated".clone_into(&mut verified.label);
    verified.elapsed_ms = 2_000;
    let mut write = backup.clone();
    "Commit".clone_into(&mut write.stage);
    "Writing update file".clone_into(&mut write.label);
    write.completed = 4_194_304;
    write.elapsed_ms = 66_000;
    write.path = Some("Garmin/Mock/europe-routing.img".to_owned());
    write.status = ProgressStatus::Running;
    write.stalled = rollback;
    state.progress = vec![backup, verified];
    if rollback {
        let mut failed = write.clone();
        "Inspect".clone_into(&mut failed.stage);
        "Storage refresh failed — device operation cancelled".clone_into(&mut failed.label);
        failed.status = ProgressStatus::Failed;
        failed.bytes = false;
        failed.completed = 0;
        failed.total = None;
        failed.path = None;
        state.progress.insert(0, failed);
        let mut cleanup = write.clone();
        "Cleanup".clone_into(&mut cleanup.stage);
        "Rolling back the map update".clone_into(&mut cleanup.label);
        cleanup.bytes = false;
        cleanup.completed = 0;
        cleanup.total = Some(1);
        cleanup.path = None;
        cleanup.stalled = false;
        state.progress.push(cleanup);
    }
    state.active = vec![write];
}

fn fixture() -> State {
    let id = "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("synthetic UUID");
    State {
        id,
        revision: id,
        device: "mock-watch".to_owned(),
        phase: Phase::Catalog,
        service: garmin_service_api::maps::CatalogService::Simulation,
        components: vec![
            Component {
                index: 0,
                name: "TopoActive Europe — Central and Western Europe".to_owned(),
                installed: Some("2025.20".to_owned()),
                available: Some("2026.10".to_owned()),
                operation: garmin_model::map::MapOperation::Update,
                can_install: true,
                can_remove: true,
                choice: Choice::Install,
                download_bytes: 2_400_000_000,
                cached_files: 1,
                total_files: 3,
            },
            Component {
                index: 1,
                name: "Worldwide routable basemap".to_owned(),
                installed: Some("2026.10".to_owned()),
                available: Some("2026.10".to_owned()),
                operation: garmin_model::map::MapOperation::Reinstall,
                can_install: true,
                can_remove: false,
                choice: Choice::Keep,
                download_bytes: 48_000_000,
                cached_files: 0,
                total_files: 1,
            },
        ],
        verified_backup: true,
        dry_run: false,
        actions: Vec::new(),
        storages: vec![garmin_model::device::DeviceStorageState {
            id: "internal".to_owned(),
            label: "Internal storage".to_owned(),
            capacity: garmin_model::device::StorageCapacity::Available {
                total_bytes: 32_000_000_000,
                free_bytes: 12_000_000_000,
            },
            writable: Some(true),
        }],
        plan: Some(Plan {
            approval: id,
            digest: "8e51d6f093b27a4c6d9e02f1a748c35be690d124f7a3c85d21906e4b5f8a732c".to_owned(),
            removal: false,
            components: vec!["TopoActive Europe — Central and Western Europe".to_owned()],
            download_bytes: 2_400_000_000,
            write_count: 3,
            remove_count: 2,
            remove_bytes: 1_800_000_000,
            paths: vec![
                "Garmin/Mock/europe.img".to_owned(),
                "Garmin/Mock/europe-routing.img".to_owned(),
            ],
            storage_requirements: vec![garmin_service_api::maps::StorageRequirement {
                storage: "internal".to_owned(),
                required_free_bytes: 1_000_000_000,
            }],
        }),
        recovery: Some(Recovery {
            plan: "8e51d6f093b27a4c6d9e02f1a748c35be690d124f7a3c85d21906e4b5f8a732c".to_owned(),
            removal: false,
            simulated: false,
            actions: vec![Action::Recover, Action::ClearRecovery],
        }),
        progress: vec![Progress {
            stage: "Download".to_owned(),
            label: "Downloading TopoActive Europe".to_owned(),
            completed: 1_200_000_000,
            total: Some(2_400_000_000),
            bytes: true,
            elapsed_ms: 12_000,
            bytes_per_second: Some(100_000_000),
            remaining_seconds: Some(12),
            stalled: false,
            path: None,
            status: garmin_service_api::maps::ProgressStatus::Running,
        }],
        active: Vec::new(),
        events: Vec::new(),
        history: Vec::new(),
        error: None,
        storage_error: None,
    }
}
