//! Source-neutral route-plan transformations.

use garmin_model::{
    route::{
        RevisionProvenance, RevisionSource, RoutePlanRevision, RoutePlanRevisionId, RouteShape,
    },
    value::{ComponentVersion, Timestamp, Transformation},
};
use semver::Version;
use thiserror::Error;

const STRAIGHT_LINES: &str = "straight-lines";

/// Explicitly converts unresolved control points into straight-line geometry.
///
/// Creates an immutable revision linked to `source` while preserving
/// coordinates, elevation, and cues.
/// # Errors
/// Exact or short input, invalid chronology, or a domain violation.
pub fn confirm_straight_lines(
    source: &RoutePlanRevision,
    revision_id: RoutePlanRevisionId,
    created_at: Timestamp,
) -> Result<RoutePlanRevision, Error> {
    if created_at < source.created_at() {
        return Err(Error::TimestampBeforeSource);
    }
    let RouteShape::ControlPoints(points) = source.shape() else {
        return Err(Error::AlreadyExact);
    };
    let shape = RouteShape::from_geometry(points.clone()).map_err(|error| match error {
        garmin_model::route::Error::TooFewGeometryPoints => Error::TooFewControlPoints,
        error => Error::Route(error),
    })?;
    let transformation = Transformation::from_component(ComponentVersion::from_parts(
        STRAIGHT_LINES,
        Version::new(1, 0, 0),
    )?);
    Ok(RoutePlanRevision::from_parts(
        revision_id,
        source.plan_id(),
        Some(source.id()),
        created_at,
        source.name().clone(),
        source.sport(),
        shape,
        source.cues().to_vec(),
        RevisionProvenance::from_parts(RevisionSource::Revision(source.id()), vec![transformation]),
    )?)
}

/// Route transformation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("route already contains exact geometry")]
    AlreadyExact,
    #[error("straight-line conversion requires at least two control points")]
    TooFewControlPoints,
    #[error("route revision cannot predate its source")]
    TimestampBeforeSource,
    #[error(transparent)]
    Route(#[from] garmin_model::route::Error),
    #[error(transparent)]
    Value(#[from] garmin_model::value::Error),
}

#[cfg(test)]
mod tests {
    use garmin_model::route::{
        Coordinate, CueText, Latitude, Longitude, NavigationCue, RevisionProvenance,
        RevisionSource, RouteName, RoutePlanId, RoutePoint, RoutePointIndex, RouteSport,
    };

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn confirmation_preserves_values_and_records_one_transformation() -> TestResult {
        let source = revision(RouteShape::from_control_points(points())?)?;
        let created_at = Timestamp::from_unix_seconds(1_780_000_100)?;
        let revision_id = RoutePlanRevisionId::new_v4();
        let confirmed = confirm_straight_lines(&source, revision_id, created_at)?;

        assert_eq!(confirmed.id(), revision_id);
        assert_eq!(confirmed.plan_id(), source.plan_id());
        assert_eq!(confirmed.previous_id(), Some(source.id()));
        assert_eq!(confirmed.created_at(), created_at);
        assert_eq!(confirmed.name(), source.name());
        assert_eq!(confirmed.sport(), source.sport());
        assert_eq!(confirmed.shape().points(), source.shape().points());
        assert!(confirmed.shape().is_geometry());
        assert_eq!(confirmed.cues(), source.cues());
        assert_eq!(
            confirmed.provenance().source(),
            RevisionSource::Revision(source.id())
        );
        assert_eq!(confirmed.provenance().transformations().len(), 1);
        assert_eq!(
            confirmed.provenance().transformations()[0].to_string(),
            "straight-lines@1.0.0"
        );
        Ok(())
    }

    #[test]
    fn confirmation_rejects_exact_or_single_point_inputs() -> TestResult {
        let exact = revision(RouteShape::from_geometry(points())?)?;
        assert_eq!(
            confirm_straight_lines(
                &exact,
                RoutePlanRevisionId::new_v4(),
                Timestamp::from_unix_seconds(1_780_000_100)?
            ),
            Err(Error::AlreadyExact)
        );

        let single = revision(RouteShape::from_control_points(vec![points()[0]])?)?;
        assert_eq!(
            confirm_straight_lines(
                &single,
                RoutePlanRevisionId::new_v4(),
                Timestamp::from_unix_seconds(1_780_000_100)?
            ),
            Err(Error::TooFewControlPoints)
        );
        assert_eq!(
            confirm_straight_lines(
                &single,
                RoutePlanRevisionId::new_v4(),
                Timestamp::from_unix_seconds(1_779_999_999)?
            ),
            Err(Error::TimestampBeforeSource)
        );
        Ok(())
    }

    fn revision(shape: RouteShape) -> Result<RoutePlanRevision, garmin_model::route::Error> {
        let cues = if shape.points().len() > 1 {
            vec![NavigationCue::from_parts(
                RoutePointIndex::from_usize(1),
                CueText::from_string("Turn right".to_owned())?,
            )]
        } else {
            Vec::new()
        };
        RoutePlanRevision::from_parts(
            RoutePlanRevisionId::new_v4(),
            RoutePlanId::new_v4(),
            None,
            Timestamp::from_unix_seconds(1_780_000_000)
                .expect("the fixture timestamp is in Jiff's supported range"),
            RouteName::from_string("Harbor route".to_owned())?,
            RouteSport::Cycling,
            shape,
            cues,
            RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
        )
    }

    fn points() -> Vec<RoutePoint> {
        vec![point(60.1699, 24.9384), point(60.1708, 24.9410)]
    }

    fn point(latitude: f64, longitude: f64) -> RoutePoint {
        RoutePoint::from_parts(
            Coordinate::from_parts(
                Latitude::from_degrees(latitude)
                    .expect("fixture latitude is inside the geographic range"),
                Longitude::from_degrees(longitude)
                    .expect("fixture longitude is inside the geographic range"),
            ),
            None,
        )
    }
}
