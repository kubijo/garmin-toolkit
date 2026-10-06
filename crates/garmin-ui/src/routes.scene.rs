use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, ByteCount},
    route::{
        Coordinate, Latitude, Longitude, RouteCandidateSource, RoutePlanId, RoutePlanRevisionId,
        RoutePoint, RouteSport,
    },
};
use garmin_service_api::routes::{GpxCandidate, GpxUpload, GpxUploadPhase, RouteSummary};
use garmin_ui::{activity::map_runtime, routes};

scene_meta! { title: "Application / Routes" }

thread_local! {
    static ROUTES: crate::SceneState<(routes::Workspace, routes::State), 5> = const { crate::SceneState::empty() };
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
        &["review", "controls", "empty", "library", "detail"],
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
    let source = if mode == 0 {
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
        }],
        candidate: Some(source),
        name: "Morning loop".into(),
        sport: Some(RouteSport::Walking),
        points,
        points_ready: true,
        ..routes::State::default()
    };
    if mode >= 3 {
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
        if mode == 4 {
            state.detail = Some(route);
        }
    }
    state
}
