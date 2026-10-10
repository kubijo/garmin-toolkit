use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, ArtifactId, ByteCount},
    route::{
        Coordinate, CourseGenerationId, CourseGenerationOperationId, Latitude, Longitude,
        RouteCandidateSource, RoutePlanId, RoutePlanRevisionId, RoutePoint, RouteSport,
    },
    value::ComponentVersion,
};
use garmin_service_api::course_transfer::{
    CourseTarget, CourseTransferPhase, CourseTransferProgress, CourseTransferReview,
    CourseTransferStatus,
};
use garmin_service_api::routes::{
    CourseVersion, GpxCandidate, GpxUpload, GpxUploadPhase, OutlinePoint, RouteSummary,
};
use garmin_service_api::{DeviceSnapshot, InspectionState};
use garmin_ui::{activity::map_runtime, routes};

scene_meta! { title: "Application / Routes" }

thread_local! {
    static ROUTES: crate::SceneState<(routes::Workspace, routes::State), 17> = const { crate::SceneState::empty() };
}

struct NoTiles;
impl map_runtime::Backend for NoTiles {
    fn fetch(&self, _request: map_runtime::TileCoordinates, reply: map_runtime::TileReply) {
        reply(Err("No network in this scene".into()));
    }
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let width = ctx.slider("width", 800.0, 320.0, 1120.0, 1.0);
    let mode = ctx.buttons(
        "state",
        &[
            "review",
            "controls",
            "empty",
            "library",
            "detail",
            "failure",
            "loading",
            "chooser",
            "invalid",
            "picker",
            "uploading",
            "course",
            "transfer",
            "transfer review",
            "transfer running",
            "transfer verified",
            "transfer accepted",
        ],
        3,
    );
    let intl = globals.intl();
    stage!(ctx, ui, globals.stage((width, 800.0)), |ui| {
        ROUTES.with_scene(
            mode,
            || {
                let runtime =
                    map_runtime::MapRuntimeHandle::new(NoTiles, map_runtime::Renderer::software());
                (routes::Workspace::new(&runtime), review(mode))
            },
            |(workspace, state)| {
                let _ = workspace.show(ui, &intl, state);
            },
        );
    });
}

fn review(mode: usize) -> routes::State {
    if mode == 2 {
        return routes::State::default();
    }
    if mode == 6 {
        return routes::State {
            pending: Some(garmin_service_api::routes::RouteRequest::List { offset: 0 }),
            ..routes::State::default()
        };
    }
    let source = if matches!(mode, 0 | 7 | 9 | 10) {
        RouteCandidateSource::TrackSegment {
            track: 0,
            segment: 0,
        }
    } else {
        RouteCandidateSource::Route { route: 0 }
    };
    // Coordinates from the existing GPX candidates fixture; no recorded activity is synthesized.
    let points = [(50.0755, 14.4378), (50.0810, 14.4510)]
        .into_iter()
        .map(|(lat, lon)| {
            RoutePoint::from_parts(
                Coordinate::from_parts(
                    Latitude::from_degrees(lat).expect("fixture latitude is within bounds"),
                    Longitude::from_degrees(lon).expect("fixture longitude is within bounds"),
                ),
                None,
            )
        })
        .collect();
    let mut state = routes::State {
        upload: Some(GpxUpload {
            operation: AcquisitionOperationId::from_u128(1),
            file_name: "candidates.gpx".into(),
            received: ByteCount::from_u64(1024),
            total: ByteCount::from_u64(1024),
            phase: GpxUploadPhase::Review {
                digest: ArtifactDigest::from_bytes(b"gallery"),
                candidates: 1,
                rejected: 0,
            },
        }),
        candidates: vec![GpxCandidate {
            source,
            suggested_name: Some("Morning loop".parse().expect("fixture route name is valid")),
            geometry: mode != 1,
            point_count: 2,
            outline: vec![
                OutlinePoint { x: 0, y: 255 },
                OutlinePoint { x: 52, y: 176 },
                OutlinePoint { x: 108, y: 208 },
                OutlinePoint { x: 156, y: 76 },
                OutlinePoint { x: 255, y: 0 },
            ],
        }],
        candidate: Some(source),
        name: "Morning loop".into(),
        sport: Some(RouteSport::Walking),
        points,
        points_ready: true,
        ..routes::State::default()
    };
    match mode {
        3 | 4 => show_saved_route(&mut state, mode == 4),
        5 => show_worker_failure(&mut state),
        7 | 9 | 10 => show_path_chooser(&mut state),
        8 => show_invalid_file(&mut state),
        11 => show_course_version(&mut state),
        12 => show_transfer_dialog(&mut state),
        13 => show_transfer_review(&mut state),
        14 => show_transfer_running(&mut state),
        15 => show_transfer_verified(&mut state),
        16 => show_transfer_accepted(&mut state),
        _ => {}
    }
    if mode == 9 {
        state.busy = true;
    }
    if mode == 10 {
        state.busy = true;
        state.begin_upload(GpxUpload {
            operation: AcquisitionOperationId::from_u128(4),
            file_name: "replacement.gpx".into(),
            received: ByteCount::from_u64(0),
            total: ByteCount::from_u64(4_096),
            phase: GpxUploadPhase::Uploading,
        });
    }
    state
}

fn show_worker_failure(state: &mut routes::State) {
    let message = "GPX import stopped after upload because the required garmin-gpx-worker executable was not found beside the application. The file was not inspected or saved. Rebuild or reinstall Garmin Toolkit, then retry.";
    if let Some(upload) = state.upload.as_mut() {
        upload.phase = GpxUploadPhase::Failed(message.into());
    }
    state.candidates.clear();
    state.candidate = None;
    state.points.clear();
    state.error = Some(message.into());
    state.retry = Some(garmin_service_api::routes::RouteRequest::Inspect {
        operation: AcquisitionOperationId::from_u128(1),
    });
}

fn show_invalid_file(state: &mut routes::State) {
    let message = "GPX could not be parsed: invalid GPX: missing version attribute";
    if let Some(upload) = state.upload.as_mut() {
        upload.file_name = "hass-malformed.gpx".into();
        upload.phase = GpxUploadPhase::InvalidFile(message.into());
    }
    state.candidates.clear();
    state.candidate = None;
    state.points.clear();
    state.error = Some(message.into());
    state.invalid_file_notice = true;
}

fn show_path_chooser(state: &mut routes::State) {
    state.candidate = None;
    state.name.clear();
    state.sport = None;
    if let Some(upload) = &mut state.upload {
        upload.file_name = "weekend-adventure.gpx".into();
        upload.received = ByteCount::from_u64(58_184);
        upload.total = ByteCount::from_u64(58_184);
        upload.phase = GpxUploadPhase::Review {
            digest: ArtifactDigest::from_bytes(b"gallery"),
            candidates: 2,
            rejected: 0,
        };
    }
    state.candidates[0].point_count = 1_842;
    state.candidates.push(GpxCandidate {
        source: RouteCandidateSource::Route { route: 0 },
        suggested_name: None,
        geometry: false,
        point_count: 12,
        outline: vec![
            OutlinePoint { x: 8, y: 216 },
            OutlinePoint { x: 120, y: 96 },
            OutlinePoint { x: 248, y: 24 },
        ],
    });
}

fn show_saved_route(state: &mut routes::State, detail: bool) {
    let route = RouteSummary {
        id: RoutePlanId::from_u128(2),
        revision: RoutePlanRevisionId::from_u128(3),
        name: "Morning loop".parse().expect("fixture route name is valid"),
        sport: RouteSport::Walking,
        created_at: "2026-10-01T12:00:00Z"
            .parse()
            .expect("fixture timestamp is valid"),
        geometry: true,
        point_count: 2,
    };
    state.upload = None;
    state.routes = vec![route.clone()];
    if detail {
        state.detail = Some(route);
    }
}

fn show_course_version(state: &mut routes::State) {
    show_saved_route(state, true);
    let route = state.detail.as_ref().expect("gallery route detail exists");
    state.versions = vec![CourseVersion {
        id: CourseGenerationId::new_v4(),
        operation: CourseGenerationOperationId::new_v4(),
        revision: route.revision,
        artifact: ArtifactId::new_v4(),
        version: 1.try_into().expect("fixture version is positive"),
        serial: 1.try_into().expect("fixture serial is positive"),
        encoder: ComponentVersion::from_parts(
            "gallery",
            "1.0.0".parse().expect("fixture semver is valid"),
        )
        .expect("fixture encoder is valid"),
        current_encoder: true,
        generated_at: route.created_at,
        byte_count: ByteCount::from_u64(9_380),
        digest: ArtifactDigest::from_bytes(b"gallery course"),
    }];
}

fn show_transfer_dialog(state: &mut routes::State) {
    show_course_version(state);
    state.transfer = Some(routes::TransferState::new(state.versions[0].operation));
    state.devices = vec![DeviceSnapshot {
        key: "edge-1050".into(),
        name: "Edge 1050".into(),
        identifier: None,
        software_version: None,
        inspection: InspectionState::Ready,
        inspection_error: None,
        report: None,
        capabilities: Vec::new(),
        storages: Vec::new(),
    }];
}

fn show_transfer_review(state: &mut routes::State) {
    show_transfer_dialog(state);
    let version = &state.versions[0];
    let transfer = state.transfer.as_mut().expect("gallery transfer exists");
    transfer.device_key = Some("edge-1050".into());
    transfer.review = Some(CourseTransferReview {
        approval: uuid::Uuid::default(),
        transfer: uuid::Uuid::default(),
        generation: version.id,
        generation_operation: version.operation,
        artifact: version.artifact,
        version: 1,
        digest: version.digest,
        byte_count: version.byte_count,
        target: CourseTarget {
            device_key: "edge-1050".into(),
            device_name: "Edge 1050".into(),
            device_digest: "gallery-device".into(),
            storage_id: "internal".into(),
            storage_label: "Internal Storage".into(),
            directory: "/Garmin/NewFiles".into(),
            free_bytes: Some(1_000_000),
        },
        file_name: "gf-fe04fceb-0928-4b15-8e93-61075289222b.fit".into(),
    });
}

fn show_transfer_running(state: &mut routes::State) {
    show_transfer_review(state);
    let transfer = state.transfer.as_mut().expect("gallery transfer exists");
    let review = transfer.review.take().expect("gallery review exists");
    transfer.status = Some(CourseTransferStatus {
        transfer: review.transfer,
        generation: review.generation,
        target: review.target,
        file_name: review.file_name,
        phase: CourseTransferPhase::Running,
        progress: Some(CourseTransferProgress {
            bytes_sent: 4_690,
            total_bytes: 9_380,
            finishing: false,
        }),
    });
}

fn show_transfer_verified(state: &mut routes::State) {
    show_transfer_running(state);
    let status = state
        .transfer
        .as_mut()
        .and_then(|transfer| transfer.status.as_mut())
        .expect("gallery status exists");
    status.phase = CourseTransferPhase::Verified;
    status.progress = None;
}

fn show_transfer_accepted(state: &mut routes::State) {
    show_transfer_verified(state);
    let status = state
        .transfer
        .as_mut()
        .and_then(|transfer| transfer.status.as_mut())
        .expect("gallery status exists");
    status.phase = CourseTransferPhase::Accepted;
}
