use super::*;
use garmin_model::route::{Coordinate, Elevation, Latitude, Longitude};

#[test]
fn route_geometry_and_candidate_identity_survive_postcard() {
    let point = RoutePoint::from_parts(
        Coordinate::from_parts(
            Latitude::from_degrees(50.0).unwrap(),
            Longitude::from_degrees(14.0).unwrap(),
        ),
        Some(Elevation::from_meters(123.5).unwrap()),
    );
    let reply = RouteReply::Points {
        offset: 7,
        points: vec![point],
        end: true,
    };
    let decoded =
        postcard::from_bytes::<RouteReply>(&postcard::to_stdvec(&reply).unwrap()).unwrap();
    let RouteReply::Points {
        offset,
        points,
        end,
    } = decoded
    else {
        panic!("wrong reply");
    };
    assert_eq!(offset, 7);
    assert_eq!(points, vec![point]);
    assert!(end);
    let request = RouteRequest::Confirm {
        operation: AcquisitionOperationId::new_v4(),
        selection: RouteSelection {
            digest: ArtifactDigest::from_bytes(b"original"),
            candidate: RouteCandidateSource::TrackSegment {
                track: 3,
                segment: 8,
            },
            name: "Hike".parse().unwrap(),
            sport: RouteSport::Hiking,
        },
    };
    let decoded =
        postcard::from_bytes::<RouteRequest>(&postcard::to_stdvec(&request).unwrap()).unwrap();
    let RouteRequest::Confirm { selection, .. } = decoded else {
        panic!("wrong request");
    };
    assert_eq!(selection.digest, ArtifactDigest::from_bytes(b"original"));
    assert_eq!(selection.sport, RouteSport::Hiking);
    assert_eq!(
        selection.candidate,
        RouteCandidateSource::TrackSegment {
            track: 3,
            segment: 8
        }
    );
}

#[test]
fn invalid_gpx_upload_phase_survives_postcard() {
    assert_eq!(
        postcard::to_stdvec(&GpxUploadPhase::Failed("worker".into()))
            .expect("encode legacy failure")
            .first()
            .copied(),
        Some(3),
    );
    let reply = RouteReply::Upload(GpxUpload {
        operation: AcquisitionOperationId::new_v4(),
        file_name: "malformed.gpx".into(),
        received: ByteCount::from_u64(17),
        total: ByteCount::from_u64(17),
        phase: GpxUploadPhase::InvalidFile("missing GPX version".into()),
    });
    let bytes = postcard::to_stdvec(&reply).expect("encode upload status");
    let decoded: RouteReply = postcard::from_bytes(&bytes).expect("decode upload status");
    let RouteReply::Upload(upload) = decoded else {
        panic!("wrong reply");
    };
    assert!(matches!(
        upload.phase,
        GpxUploadPhase::InvalidFile(reason) if reason == "missing GPX version"
    ));
}

#[test]
fn route_wire_values_reject_nonfinite_elevation_and_empty_encoder_name() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(postcard::from_bytes::<Elevation>(&postcard::to_stdvec(&value).unwrap()).is_err());
    }
    assert!(
        postcard::from_bytes::<ComponentVersion>(&postcard::to_stdvec(&("", "1.0.0")).unwrap())
            .is_err()
    );
    assert!(
        postcard::from_bytes::<ComponentVersion>(
            &postcard::to_stdvec(&("encoder", "invalid")).unwrap()
        )
        .is_err()
    );
}
