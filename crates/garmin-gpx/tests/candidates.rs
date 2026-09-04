//! GPX candidate and semantic round-trip coverage.

use std::error::Error;

use garmin_gpx::{CandidateSource, parse, write};
use garmin_model::{
    route::{
        RevisionProvenance, RevisionSource, RouteName, RoutePlanId, RoutePlanRevision,
        RoutePlanRevisionId, RouteShape, RouteSport,
    },
    value::Timestamp,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/candidates.gpx");

#[test]
fn tracks_remain_separate_and_routes_remain_unresolved() -> Result<(), Box<dyn Error>> {
    let document = parse(FIXTURE)?;

    assert_eq!(document.candidates().len(), 3);
    assert_eq!(document.rejected().len(), 1);
    assert_eq!(
        document.rejected()[0].source(),
        CandidateSource::TrackSegment {
            track: 1,
            segment: 0,
        }
    );
    assert_eq!(
        document.rejected()[0].reason(),
        garmin_model::route::Error::TooFewGeometryPoints
    );
    assert_eq!(
        document.candidates()[0].source(),
        CandidateSource::TrackSegment {
            track: 0,
            segment: 0,
        }
    );
    assert_eq!(
        document.candidates()[1].source(),
        CandidateSource::TrackSegment {
            track: 0,
            segment: 1,
        }
    );
    assert!(document.candidates()[0].shape().is_geometry());
    assert!(matches!(
        document.candidates()[2].shape(),
        RouteShape::ControlPoints(_)
    ));
    assert_eq!(
        document.candidates()[2]
            .suggested_name()
            .map(RouteName::as_str),
        Some("Control points")
    );
    Ok(())
}

#[test]
fn malformed_xml_is_rejected() {
    assert!(parse(b"<gpx><trk></gpx>").is_err());
}

#[test]
fn writing_retains_normalized_geometry_and_elevation() -> Result<(), Box<dyn Error>> {
    let source = parse(FIXTURE)?.candidates()[0].clone();
    let revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        RoutePlanId::new_v4(),
        None,
        Timestamp::from_unix_seconds(1_788_198_400)?,
        "Morning loop".parse()?,
        RouteSport::Cycling,
        source.clone().into_shape(),
        Vec::new(),
        RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
    )?;

    let encoded = write(&revision)?;
    let reparsed = parse(&encoded)?;

    assert_eq!(reparsed.candidates().len(), 1);
    assert_eq!(reparsed.candidates()[0].shape(), source.shape());
    Ok(())
}
