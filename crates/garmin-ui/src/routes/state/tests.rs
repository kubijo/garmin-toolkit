use super::*;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, ByteCount},
    route::{Coordinate, Latitude, Longitude, RoutePlanId, RoutePlanRevisionId},
    value::Timestamp,
};

fn review() -> State {
    let mut state = State {
        upload: Some(GpxUpload {
            operation: AcquisitionOperationId::new_v4(),
            file_name: "walk.gpx".into(),
            received: ByteCount::from_u64(1),
            total: ByteCount::from_u64(1),
            phase: GpxUploadPhase::Review {
                digest: ArtifactDigest::from_bytes(b"x"),
                candidates: 1,
                rejected: 0,
            },
        }),
        ..State::default()
    };
    state.candidates.push(GpxCandidate {
        source: RouteCandidateSource::TrackSegment {
            track: 0,
            segment: 0,
        },
        suggested_name: Some("Walk".parse().unwrap()),
        geometry: true,
        point_count: 2,
    });
    state
}

#[test]
fn saving_requires_explicit_sport_and_complete_selected_geometry() {
    let mut state = review();
    state.select_candidate(state.candidates[0].source);
    assert!(state.confirm().is_none());
    state.sport = Some(RouteSport::Walking);
    assert!(state.confirm().is_none());
    let point = RoutePoint::from_parts(
        Coordinate::from_parts(
            Latitude::from_degrees(50.0).unwrap(),
            Longitude::from_degrees(14.0).unwrap(),
        ),
        None,
    );
    state.accept(RouteReply::Points {
        offset: 0,
        points: vec![point, point],
        end: true,
    });
    let RouteRequest::Confirm { selection, .. } = state.confirm().unwrap() else {
        panic!("expected confirmation");
    };
    assert_eq!(selection.sport, RouteSport::Walking);
    state.select_candidate(state.candidates[0].source);
    assert!(
        state.confirm().is_none(),
        "a new selection must finish its own preview"
    );
}

#[test]
fn more_routes_preserves_earlier_pages_and_refresh_replaces_them() {
    let route = RouteSummary {
        id: RoutePlanId::new_v4(),
        revision: RoutePlanRevisionId::new_v4(),
        name: "Walk".parse().unwrap(),
        sport: RouteSport::Walking,
        created_at: Timestamp::from_unix_seconds(0).unwrap(),
        geometry: true,
        point_count: 2,
    };
    let mut second = route.clone();
    second.id = RoutePlanId::new_v4();
    let mut state = State::default();
    state.accept(RouteReply::Routes {
        items: vec![route.clone()],
        next: Some(1),
    });
    state.queue(RouteRequest::List { offset: 1 });
    assert!(state.take_request().is_some());
    state.accept(RouteReply::Routes {
        items: vec![route.clone(), second.clone()],
        next: None,
    });
    assert_eq!(state.routes.len(), 2);
    assert_eq!(state.routes[0].id, route.id);
    assert_eq!(state.routes[1].id, second.id);
    state.queue(RouteRequest::List { offset: 0 });
    assert!(state.take_request().is_some());
    state.accept(RouteReply::Routes {
        items: vec![second.clone()],
        next: None,
    });
    assert_eq!(state.routes.len(), 1);
    assert_eq!(state.routes[0].id, second.id);
}

#[test]
fn a_lost_confirmation_response_retries_the_original_selection() {
    let mut state = review();
    state.candidate = Some(state.candidates[0].source);
    state.name = "Walk".into();
    state.sport = Some(RouteSport::Walking);
    state.points_ready = true;
    let confirm = state.confirm().unwrap();
    state.queue(confirm);
    let RouteRequest::Confirm { operation, .. } = state.take_request().unwrap() else {
        panic!("expected confirmation");
    };
    state.fail("disconnected".into());
    state.name = "Changed draft".into();
    let RouteRequest::Confirm {
        operation: retry,
        selection,
    } = state.retry.as_ref().unwrap()
    else {
        panic!("missing retry");
    };
    assert_eq!(*retry, operation);
    assert_eq!(selection.name.as_str(), "Walk");
}

#[test]
fn saved_geometry_rejects_out_of_order_or_nonadvancing_chunks() {
    let mut state = State::default();
    state.accept(RouteReply::Detail {
        route: RouteSummary {
            id: RoutePlanId::new_v4(),
            revision: RoutePlanRevisionId::new_v4(),
            name: "Walk".parse().unwrap(),
            sport: RouteSport::Walking,
            created_at: Timestamp::from_unix_seconds(0).unwrap(),
            geometry: true,
            point_count: 2,
        },
        source: None,
    });
    state.accept(RouteReply::Points {
        offset: 0,
        points: Vec::new(),
        end: false,
    });
    assert!(state.error.is_some());
    assert!(state.pending.is_none());
    assert!(!state.points_ready);
}

#[test]
fn cancellation_queued_during_status_poll_is_not_overwritten_by_next_poll() {
    let mut state = review();
    let mut upload = state.upload.clone().unwrap();
    upload.phase = GpxUploadPhase::Parsing;
    state.queue(RouteRequest::Cancel {
        operation: upload.operation,
    });
    state.accept(RouteReply::Upload(upload));
    assert!(matches!(state.pending, Some(RouteRequest::Cancel { .. })));
}

#[test]
fn finishing_an_import_requires_a_new_sport_choice_for_the_next_file() {
    let mut state = review();
    state.sport = Some(RouteSport::Hiking);
    state.accept(RouteReply::Cancelled);
    assert!(state.sport.is_none());
    assert!(state.upload.is_none());
}
