//! User-owned route plans and immutable revisions.

use std::fmt;

use thiserror::Error;

use crate::{
    artifact::ArtifactId,
    identity::UserId,
    value::{Timestamp, Transformation},
};

define_id!(
    RoutePlanIdKind,
    RoutePlanId,
    "route-plan",
    "Type marker for route-plan IDs.",
    "A stable user-owned route-plan ID."
);
define_id!(
    RoutePlanRevisionIdKind,
    RoutePlanRevisionId,
    "route-plan-revision",
    "Type marker for route-plan revision IDs.",
    "An immutable route-plan revision ID."
);

text_value!(
    RouteName,
    Error,
    Error::EmptyRouteName,
    "A route-plan revision name."
);
text_value!(
    CueText,
    Error,
    Error::EmptyCueText,
    "User-authored navigation-cue text."
);

/// Latitude in canonical decimal degrees.
#[garmin_macros::portable(copy, custom_deserialize)]
pub struct Latitude(f64);

impl<'de> serde::Deserialize<'de> for Latitude {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let degrees = <f64 as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_degrees(degrees).map_err(serde::de::Error::custom)
    }
}

impl Latitude {
    /// Validates decimal degrees in the inclusive `-90..=90` range.
    /// # Errors
    /// [`Error::InvalidLatitude`] for non-finite or out-of-range values.
    pub fn from_degrees(degrees: f64) -> Result<Self, Error> {
        if !degrees.is_finite() || !(-90.0..=90.0).contains(&degrees) {
            return Err(Error::InvalidLatitude);
        }
        Ok(Self(degrees))
    }

    #[must_use]
    pub const fn as_degrees(&self) -> f64 {
        self.0
    }

    #[must_use]
    pub const fn into_degrees(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Latitude {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:.7}°", self.0)
    }
}

/// Longitude in canonical decimal degrees.
#[garmin_macros::portable(copy, custom_deserialize)]
pub struct Longitude(f64);

impl<'de> serde::Deserialize<'de> for Longitude {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let degrees = <f64 as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_degrees(degrees).map_err(serde::de::Error::custom)
    }
}

impl Longitude {
    /// Validates decimal degrees in the inclusive `-180..=180` range.
    /// # Errors
    /// [`Error::InvalidLongitude`] for non-finite or out-of-range values.
    pub fn from_degrees(degrees: f64) -> Result<Self, Error> {
        if !degrees.is_finite() || !(-180.0..=180.0).contains(&degrees) {
            return Err(Error::InvalidLongitude);
        }
        Ok(Self(degrees))
    }

    #[must_use]
    pub const fn as_degrees(&self) -> f64 {
        self.0
    }

    #[must_use]
    pub const fn into_degrees(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Longitude {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:.7}°", self.0)
    }
}

/// Elevation in canonical meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevation(f64);

impl Elevation {
    /// Validates a finite meter value.
    /// # Errors
    /// [`Error::InvalidElevation`] for a non-finite value.
    pub const fn from_meters(meters: f64) -> Result<Self, Error> {
        if !meters.is_finite() {
            return Err(Error::InvalidElevation);
        }
        Ok(Self(meters))
    }

    #[must_use]
    pub const fn as_meters(&self) -> f64 {
        self.0
    }

    #[must_use]
    pub const fn into_meters(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Elevation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} m", self.0)
    }
}

/// A validated geographic coordinate.
#[garmin_macros::portable(copy)]
pub struct Coordinate {
    latitude: Latitude,
    longitude: Longitude,
}

impl Coordinate {
    #[must_use]
    pub const fn from_parts(latitude: Latitude, longitude: Longitude) -> Self {
        Self {
            latitude,
            longitude,
        }
    }

    /// Validates a `geo-types` coordinate, whose `x` is longitude and `y` is latitude.
    /// # Errors
    /// An error when either component is invalid.
    pub fn from_geo(coordinate: geo_types::Coord<f64>) -> Result<Self, Error> {
        Ok(Self {
            latitude: Latitude::from_degrees(coordinate.y)?,
            longitude: Longitude::from_degrees(coordinate.x)?,
        })
    }

    #[must_use]
    pub const fn latitude(&self) -> Latitude {
        self.latitude
    }

    #[must_use]
    pub const fn longitude(&self) -> Longitude {
        self.longitude
    }

    #[must_use]
    pub const fn as_geo(&self) -> geo_types::Coord<f64> {
        geo_types::Coord {
            x: self.longitude.0,
            y: self.latitude.0,
        }
    }

    #[must_use]
    pub const fn into_geo(self) -> geo_types::Coord<f64> {
        self.as_geo()
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}, {}", self.latitude, self.longitude)
    }
}

/// One point in route geometry or an unresolved control-point sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoutePoint {
    coordinate: Coordinate,
    elevation: Option<Elevation>,
}

impl RoutePoint {
    #[must_use]
    pub const fn from_parts(coordinate: Coordinate, elevation: Option<Elevation>) -> Self {
        Self {
            coordinate,
            elevation,
        }
    }

    #[must_use]
    pub const fn coordinate(&self) -> Coordinate {
        self.coordinate
    }

    #[must_use]
    pub const fn elevation(&self) -> Option<Elevation> {
        self.elevation
    }

    #[must_use]
    pub const fn into_parts(self) -> (Coordinate, Option<Elevation>) {
        (self.coordinate, self.elevation)
    }
}

/// Whether route points are deployable geometry or unresolved planning inputs.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteShape {
    Geometry(Vec<RoutePoint>),
    ControlPoints(Vec<RoutePoint>),
}

impl RouteShape {
    /// Validates exact geometry.
    /// # Errors
    /// [`Error::TooFewGeometryPoints`] for fewer than two points.
    pub fn from_geometry(points: Vec<RoutePoint>) -> Result<Self, Error> {
        if points.len() < 2 {
            return Err(Error::TooFewGeometryPoints);
        }
        Ok(Self::Geometry(points))
    }

    /// Validates unresolved control points.
    /// # Errors
    /// [`Error::NoControlPoints`] for an empty sequence.
    pub fn from_control_points(points: Vec<RoutePoint>) -> Result<Self, Error> {
        if points.is_empty() {
            return Err(Error::NoControlPoints);
        }
        Ok(Self::ControlPoints(points))
    }

    #[must_use]
    pub fn points(&self) -> &[RoutePoint] {
        match self {
            Self::Geometry(points) | Self::ControlPoints(points) => points,
        }
    }

    #[must_use]
    pub const fn is_geometry(&self) -> bool {
        matches!(self, Self::Geometry(_))
    }
}

/// Supported route-planning sport.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteSport {
    Cycling,
    Running,
}

/// A zero-based route-point index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RoutePointIndex(usize);

impl RoutePointIndex {
    #[must_use]
    pub const fn from_usize(index: usize) -> Self {
        Self(index)
    }

    #[must_use]
    pub const fn as_usize(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn into_usize(self) -> usize {
        self.0
    }
}

impl fmt::Display for RoutePointIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One explicit navigation cue attached to a route point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationCue {
    point_index: RoutePointIndex,
    text: CueText,
}

impl NavigationCue {
    #[must_use]
    pub const fn from_parts(point_index: RoutePointIndex, text: CueText) -> Self {
        Self { point_index, text }
    }

    #[must_use]
    pub const fn point_index(&self) -> RoutePointIndex {
        self.point_index
    }

    #[must_use]
    pub const fn text(&self) -> &CueText {
        &self.text
    }
}

/// The direct input from which a revision was made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionSource {
    Freehand,
    Artifact(ArtifactId),
    Revision(RoutePlanRevisionId),
}

/// Reproducible creation provenance for one revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionProvenance {
    source: RevisionSource,
    transformations: Vec<Transformation>,
}

impl RevisionProvenance {
    #[must_use]
    pub const fn from_parts(source: RevisionSource, transformations: Vec<Transformation>) -> Self {
        Self {
            source,
            transformations,
        }
    }

    #[must_use]
    pub const fn source(&self) -> RevisionSource {
        self.source
    }

    #[must_use]
    pub fn transformations(&self) -> &[Transformation] {
        &self.transformations
    }

    #[must_use]
    pub fn into_parts(self) -> (RevisionSource, Vec<Transformation>) {
        (self.source, self.transformations)
    }
}

/// One immutable route-plan revision.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutePlanRevision {
    id: RoutePlanRevisionId,
    plan_id: RoutePlanId,
    previous_id: Option<RoutePlanRevisionId>,
    created_at: Timestamp,
    name: RouteName,
    sport: RouteSport,
    shape: RouteShape,
    cues: Vec<NavigationCue>,
    provenance: RevisionProvenance,
}

impl RoutePlanRevision {
    /// Validates and creates an immutable revision.
    /// # Errors
    /// Rejects self-reference or a cue outside the route shape.
    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable revision is fully defined at creation"
    )]
    pub fn from_parts(
        id: RoutePlanRevisionId,
        plan_id: RoutePlanId,
        previous_id: Option<RoutePlanRevisionId>,
        created_at: Timestamp,
        name: RouteName,
        sport: RouteSport,
        shape: RouteShape,
        cues: Vec<NavigationCue>,
        provenance: RevisionProvenance,
    ) -> Result<Self, Error> {
        if previous_id == Some(id) || provenance.source() == RevisionSource::Revision(id) {
            return Err(Error::SelfReferentialRevision);
        }
        if cues
            .iter()
            .any(|cue| cue.point_index.as_usize() >= shape.points().len())
        {
            return Err(Error::CueOutsideShape);
        }
        Ok(Self {
            id,
            plan_id,
            previous_id,
            created_at,
            name,
            sport,
            shape,
            cues,
            provenance,
        })
    }

    #[must_use]
    pub const fn id(&self) -> RoutePlanRevisionId {
        self.id
    }

    #[must_use]
    pub const fn plan_id(&self) -> RoutePlanId {
        self.plan_id
    }

    #[must_use]
    pub const fn previous_id(&self) -> Option<RoutePlanRevisionId> {
        self.previous_id
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    #[must_use]
    pub const fn name(&self) -> &RouteName {
        &self.name
    }

    #[must_use]
    pub const fn sport(&self) -> RouteSport {
        self.sport
    }

    #[must_use]
    pub const fn shape(&self) -> &RouteShape {
        &self.shape
    }

    #[must_use]
    pub fn cues(&self) -> &[NavigationCue] {
        &self.cues
    }

    #[must_use]
    pub const fn provenance(&self) -> &RevisionProvenance {
        &self.provenance
    }
}

/// Stable route-plan ownership and current revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutePlan {
    id: RoutePlanId,
    owner_id: UserId,
    current_revision_id: RoutePlanRevisionId,
}

impl RoutePlan {
    #[must_use]
    pub const fn from_parts(
        id: RoutePlanId,
        owner_id: UserId,
        current_revision_id: RoutePlanRevisionId,
    ) -> Self {
        Self {
            id,
            owner_id,
            current_revision_id,
        }
    }

    #[must_use]
    pub const fn id(&self) -> RoutePlanId {
        self.id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn current_revision_id(&self) -> RoutePlanRevisionId {
        self.current_revision_id
    }

    /// a new aggregate head after the revision has been persisted.
    /// # Errors
    /// Rejects a different plan or a revision that does not follow the current head.
    pub fn with_revision(self, revision: &RoutePlanRevision) -> Result<Self, Error> {
        if revision.plan_id() != self.id {
            return Err(Error::RevisionForAnotherPlan);
        }
        if revision.previous_id() != Some(self.current_revision_id) {
            return Err(Error::RevisionDoesNotFollowHead);
        }
        Ok(Self {
            current_revision_id: revision.id(),
            ..self
        })
    }
}

/// Invalid route-plan data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("route name cannot be blank")]
    EmptyRouteName,
    #[error("navigation cue cannot be blank")]
    EmptyCueText,
    #[error("latitude must be finite and between -90 and 90 degrees")]
    InvalidLatitude,
    #[error("longitude must be finite and between -180 and 180 degrees")]
    InvalidLongitude,
    #[error("elevation must be finite")]
    InvalidElevation,
    #[error("route geometry requires at least two points")]
    TooFewGeometryPoints,
    #[error("control-point route cannot be empty")]
    NoControlPoints,
    #[error("navigation cue index is outside the route shape")]
    CueOutsideShape,
    #[error("revision belongs to another route plan")]
    RevisionForAnotherPlan,
    #[error("revision does not follow the current route-plan revision")]
    RevisionDoesNotFollowHead,
    #[error("route-plan revision cannot refer to itself")]
    SelfReferentialRevision,
}
