//! [GPX 1.0 and 1.1](https://www.topografix.com/gpx.asp)
//! parsing and writing behind source-neutral route types.

#![expect(
    clippy::multiple_crate_versions,
    reason = "gpx 0.10 still uses thiserror 1 while the workspace uses thiserror 2"
)]

use std::io::Cursor;

use garmin_model::route::{Elevation, RouteName, RoutePlanRevision, RoutePoint, RouteShape};
use gpx_format::{Gpx, GpxVersion, Route, Track, TrackSegment, Waypoint};
use thiserror::Error;

/// Adapter identity recorded in route-revision provenance.
pub const ADAPTER_NAME: &str = "garmin-gpx";
/// Adapter version recorded in route-revision provenance.
pub const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Maximum accepted GPX source size.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Maximum accepted points across all tracks and routes.
pub const MAX_POINTS: usize = 1_000_000;

/// Location of one candidate in the GPX document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CandidateSource {
    TrackSegment {
        /// Zero-based track index.
        track: usize,
        /// Zero-based segment index within the track.
        segment: usize,
    },
    Route {
        /// Zero-based route index.
        route: usize,
    },
}

/// One valid route-plan candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    source: CandidateSource,
    suggested_name: Option<RouteName>,
    shape: RouteShape,
}

impl Candidate {
    #[must_use]
    pub const fn source(&self) -> CandidateSource {
        self.source
    }

    #[must_use]
    pub const fn suggested_name(&self) -> Option<&RouteName> {
        self.suggested_name.as_ref()
    }

    #[must_use]
    pub const fn shape(&self) -> &RouteShape {
        &self.shape
    }

    #[must_use]
    pub fn into_shape(self) -> RouteShape {
        self.shape
    }
}

/// A candidate rejected without hiding valid siblings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectedCandidate {
    source: CandidateSource,
    reason: garmin_model::route::Error,
}

impl RejectedCandidate {
    #[must_use]
    pub const fn source(&self) -> CandidateSource {
        self.source
    }

    #[must_use]
    pub const fn reason(&self) -> garmin_model::route::Error {
        self.reason
    }
}

/// Parsed GPX candidates and local validation failures.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    candidates: Vec<Candidate>,
    rejected: Vec<RejectedCandidate>,
}

impl Document {
    #[must_use]
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    #[must_use]
    pub fn rejected(&self) -> &[RejectedCandidate] {
        &self.rejected
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<Candidate>, Vec<RejectedCandidate>) {
        (self.candidates, self.rejected)
    }
}

/// Parses GPX 1.0 or 1.1 into independently selectable route candidates.
///
/// Standalone waypoints are ignored. Empty tracks, segments, and routes produce no candidate.
/// # Errors
/// [`enum@Error`] for oversized or malformed input.
pub fn parse(bytes: &[u8]) -> Result<Document, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let document = gpx_format::read(Cursor::new(bytes))?;
    let point_count = document
        .tracks
        .iter()
        .flat_map(|track| &track.segments)
        .map(|segment| segment.points.len())
        .chain(document.routes.iter().map(|route| route.points.len()))
        .try_fold(0_usize, usize::checked_add)
        .ok_or(Error::TooManyPoints)?;
    if point_count > MAX_POINTS {
        return Err(Error::TooManyPoints);
    }

    let metadata_name = document
        .metadata
        .as_ref()
        .and_then(|metadata| route_name(metadata.name.as_deref()));
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for (track_index, track) in document.tracks.iter().enumerate() {
        let suggested_name = route_name(track.name.as_deref()).or_else(|| metadata_name.clone());
        for (segment_index, segment) in track.segments.iter().enumerate() {
            if segment.points.is_empty() {
                continue;
            }
            let source = CandidateSource::TrackSegment {
                track: track_index,
                segment: segment_index,
            };
            match route_points(&segment.points).and_then(RouteShape::from_geometry) {
                Ok(shape) => candidates.push(Candidate {
                    source,
                    suggested_name: suggested_name.clone(),
                    shape,
                }),
                Err(reason) => rejected.push(RejectedCandidate { source, reason }),
            }
        }
    }
    for (route_index, route) in document.routes.iter().enumerate() {
        if route.points.is_empty() {
            continue;
        }
        let source = CandidateSource::Route { route: route_index };
        let suggested_name = route_name(route.name.as_deref()).or_else(|| metadata_name.clone());
        match route_points(&route.points).and_then(RouteShape::from_control_points) {
            Ok(shape) => candidates.push(Candidate {
                source,
                suggested_name,
                shape,
            }),
            Err(reason) => rejected.push(RejectedCandidate { source, reason }),
        }
    }
    Ok(Document {
        candidates,
        rejected,
    })
}

/// Writes one route-plan revision as GPX 1.1.
///
/// Exact geometry becomes one track segment. Unresolved control points become one route.
/// # Errors
/// [`enum@Error`] when the GPX writer rejects the document.
pub fn write(revision: &RoutePlanRevision) -> Result<Vec<u8>, Error> {
    let mut document = Gpx {
        version: GpxVersion::Gpx11,
        creator: Some("Garmin Toolkit".to_owned()),
        ..Gpx::default()
    };
    match revision.shape() {
        RouteShape::Geometry(points) => {
            let mut track = Track {
                name: Some(revision.name().to_string()),
                ..Track::default()
            };
            track.segments.push(TrackSegment {
                points: points.iter().map(waypoint).collect(),
            });
            document.tracks.push(track);
        }
        RouteShape::ControlPoints(points) => {
            document.routes.push(Route {
                name: Some(revision.name().to_string()),
                points: points.iter().map(waypoint).collect(),
                ..Route::default()
            });
        }
    }
    let mut bytes = Vec::new();
    gpx_format::write(&document, &mut bytes)?;
    Ok(bytes)
}

fn route_name(value: Option<&str>) -> Option<RouteName> {
    value.and_then(|value| RouteName::from_string(value.to_owned()).ok())
}

fn route_points(points: &[Waypoint]) -> Result<Vec<RoutePoint>, garmin_model::route::Error> {
    points
        .iter()
        .map(|point| {
            let coordinate = garmin_model::route::Coordinate::from_geo(point.point().0)?;
            let elevation = point.elevation.map(Elevation::from_meters).transpose()?;
            Ok(RoutePoint::from_parts(coordinate, elevation))
        })
        .collect()
}

fn waypoint(point: &RoutePoint) -> Waypoint {
    let coordinate = point.coordinate();
    let mut waypoint = Waypoint::new(geo_types::Point::new(
        coordinate.longitude().as_degrees(),
        coordinate.latitude().as_degrees(),
    ));
    waypoint.elevation = point.elevation().map(Elevation::into_meters);
    waypoint
}

/// GPX adapter failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("GPX input exceeds {MAX_BYTES} bytes")]
    TooLarge,
    #[error("GPX input exceeds {MAX_POINTS} route points")]
    TooManyPoints,
    #[error("invalid GPX: {0}")]
    Format(#[from] gpx_format::errors::GpxError),
}
