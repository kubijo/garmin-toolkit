//! Route-plan persistence.

use garmin_model::{
    artifact::{Acquisition, Artifact, ArtifactDigest, ByteCount},
    identity::UserId,
    route::{
        CueText, Elevation, Latitude, Longitude, NavigationCue, RevisionProvenance, RevisionSource,
        RouteName, RoutePlan, RoutePlanId, RoutePlanRevision, RoutePlanRevisionId, RoutePoint,
        RoutePointIndex, RouteShape, RouteSport,
    },
    value::{ComponentVersion, Timestamp, Transformation},
};
use semver::Version;
use sqlx::{Sqlite, SqliteConnection, Transaction};
use thiserror::Error;

use crate::{Storage, ingestion};

pub(crate) mod imports;
pub mod source;

/// One initial route-plan revision imported from immutable source bytes.
pub struct RouteImport<'a> {
    artifact: &'a Artifact,
    bytes: &'a [u8],
    acquisition: &'a Acquisition,
    plan: &'a RoutePlan,
    revision: &'a RoutePlanRevision,
}

impl<'a> RouteImport<'a> {
    /// Validates one atomic imported-route write.
    /// # Errors
    /// [`RoutePersistenceError`] when bytes, ownership, or references disagree.
    pub fn from_parts(
        artifact: &'a Artifact,
        bytes: &'a [u8],
        acquisition: &'a Acquisition,
        plan: &'a RoutePlan,
        revision: &'a RoutePlanRevision,
    ) -> Result<Self, RoutePersistenceError> {
        if artifact.digest() != ArtifactDigest::from_bytes(bytes)
            || artifact.byte_count() != ByteCount::from_u64(bytes.len() as u64)
        {
            return Err(RoutePersistenceError::ArtifactBytesMismatch);
        }
        if acquisition.artifact_id() != artifact.id() {
            return Err(RoutePersistenceError::AcquisitionArtifactMismatch);
        }
        if acquisition.owner_id() != plan.owner_id() {
            return Err(RoutePersistenceError::OwnerMismatch);
        }
        if revision.plan_id() != plan.id()
            || plan.current_revision_id() != revision.id()
            || revision.previous_id().is_some()
        {
            return Err(RoutePersistenceError::InitialRevisionMismatch);
        }
        if revision.provenance().source() != RevisionSource::Artifact(artifact.id()) {
            return Err(RoutePersistenceError::SourceArtifactMismatch);
        }
        Ok(Self {
            artifact,
            bytes,
            acquisition,
            plan,
            revision,
        })
    }
}

/// One route-plan list row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRoutePlanSummary {
    plan: RoutePlan,
    created_at: Timestamp,
    name: RouteName,
    sport: RouteSport,
    geometry: bool,
    point_count: usize,
}

impl StoredRoutePlanSummary {
    #[must_use]
    pub const fn plan(&self) -> RoutePlan {
        self.plan
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
    pub const fn is_geometry(&self) -> bool {
        self.geometry
    }

    #[must_use]
    pub const fn point_count(&self) -> usize {
        self.point_count
    }
}

/// One route plan with its complete current revision.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRoutePlan {
    plan: RoutePlan,
    revision: RoutePlanRevision,
}

impl StoredRoutePlan {
    #[must_use]
    pub const fn plan(&self) -> RoutePlan {
        self.plan
    }

    #[must_use]
    pub const fn revision(&self) -> &RoutePlanRevision {
        &self.revision
    }

    #[must_use]
    pub fn into_parts(self) -> (RoutePlan, RoutePlanRevision) {
        (self.plan, self.revision)
    }
}

impl Storage {
    /// Reads one owned current-head summary without allocating its geometry.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid metadata.
    pub async fn route_plan_summary(
        &self,
        owner: UserId,
        plan: RoutePlanId,
    ) -> Result<Option<StoredRoutePlanSummary>, crate::Error> {
        let owner = owner.to_string();
        let plan = plan.to_string();
        sqlx::query_file_as!(
            RouteSummaryRow,
            "queries/route-plan-summary.sql",
            owner,
            plan
        )
        .fetch_optional(&self.pool)
        .await?
        .map(decode_summary)
        .transpose()
    }

    /// Reads a bounded page of owned current-head summaries.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid metadata.
    pub async fn route_plans_page(
        &self,
        owner: UserId,
        offset: u32,
        count: u32,
    ) -> Result<Vec<StoredRoutePlanSummary>, crate::Error> {
        let owner = owner.to_string();
        let offset = i64::from(offset);
        let count = i64::from(count);
        sqlx::query_file_as!(
            RouteSummaryRow,
            "queries/route-plans-page.sql",
            owner,
            count,
            offset
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(decode_summary)
        .collect()
    }

    /// Reads a bounded slice of an owned immutable revision without loading its full geometry.
    /// Absent and foreign revisions both return no points.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted coordinates.
    pub async fn route_points_page(
        &self,
        owner: UserId,
        revision: RoutePlanRevisionId,
        offset: u32,
        count: u32,
    ) -> Result<Vec<RoutePoint>, crate::Error> {
        let owner = owner.to_string();
        let revision = revision.to_string();
        let offset = i64::from(offset);
        let count = i64::from(count);
        sqlx::query_file_as!(
            PointRow,
            "queries/route-points-page.sql",
            owner,
            revision,
            offset,
            count
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(decode_point)
        .collect()
    }

    /// Atomically stores source bytes, acquisition provenance, and one initial route revision.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or immutable conflicts.
    pub async fn save_route_import(&self, import: RouteImport<'_>) -> Result<(), crate::Error> {
        let mut transaction = self.pool.begin().await?;
        persist_import(&mut transaction, &import).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Atomically appends a revision and advances its user-owned plan head.
    /// # Errors
    /// [`enum@crate::Error`] for invalid ownership, linkage, data, or expected head.
    pub async fn save_route_revision(
        &self,
        owner_id: UserId,
        plan: RoutePlan,
        revision: &RoutePlanRevision,
    ) -> Result<StoredRoutePlan, crate::Error> {
        if plan.owner_id() != owner_id {
            return Err(RoutePersistenceError::OwnerMismatch.into());
        }
        let next = plan
            .with_revision(revision)
            .map_err(|_| RoutePersistenceError::RevisionUpdateMismatch)?;
        if revision.provenance().source() != RevisionSource::Revision(plan.current_revision_id()) {
            return Err(RoutePersistenceError::SourceRevisionMismatch.into());
        }

        let mut transaction = self.pool.begin().await?;
        persist_revision(&mut transaction, revision).await?;
        let next_revision_id = revision.id().to_string();
        let plan_id = plan.id().to_string();
        let previous_revision_id = plan.current_revision_id().to_string();
        let owner_id = owner_id.to_string();
        if sqlx::query_file!(
            "queries/advance-route-head.sql",
            next_revision_id,
            plan_id,
            previous_revision_id,
            owner_id,
        )
        .fetch_optional(&mut *transaction)
        .await?
        .is_none()
        {
            return Err(RoutePersistenceError::HeadConflict.into());
        }
        transaction.commit().await?;
        Ok(StoredRoutePlan {
            plan: next,
            revision: revision.clone(),
        })
    }

    /// Lists one user's route plans by current revision creation time.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted values.
    pub async fn route_plans(
        &self,
        owner_id: UserId,
    ) -> Result<Vec<StoredRoutePlanSummary>, crate::Error> {
        let owner_id = owner_id.to_string();
        sqlx::query_file_as!(
            RouteSummaryRow,
            "queries/route-plan-summaries.sql",
            owner_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(decode_summary)
        .collect()
    }

    /// Loads one route plan within its owner's scope.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted values.
    pub async fn route_plan(
        &self,
        owner_id: UserId,
        plan_id: RoutePlanId,
    ) -> Result<Option<StoredRoutePlan>, crate::Error> {
        let owner_id = owner_id.to_string();
        let plan_id = plan_id.to_string();
        let mut transaction = self.pool.begin().await?;
        let Some(row) = sqlx::query_file_as!(RouteRow, "queries/route-plan.sql", owner_id, plan_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(None);
        };
        let route = read_route(&mut transaction, row).await?;
        transaction.commit().await?;
        Ok(Some(route))
    }

    /// Loads one immutable revision, including revisions behind the current head.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted values.
    pub async fn route_revision(
        &self,
        owner_id: UserId,
        revision_id: RoutePlanRevisionId,
    ) -> Result<Option<RoutePlanRevision>, crate::Error> {
        let mut transaction = self.pool.begin().await?;
        let revision = read_revision(&mut transaction, owner_id, revision_id).await?;
        transaction.commit().await?;
        Ok(revision)
    }
}

pub(crate) async fn read_revision(
    connection: &mut SqliteConnection,
    owner_id: UserId,
    revision_id: RoutePlanRevisionId,
) -> Result<Option<RoutePlanRevision>, crate::Error> {
    let owner_id = owner_id.to_string();
    let revision_id = revision_id.to_string();
    let Some(row) = sqlx::query_file_as!(
        RouteRow,
        "queries/route-revision.sql",
        owner_id,
        revision_id,
    )
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(read_route(connection, row).await?.revision))
}

async fn read_route(
    connection: &mut SqliteConnection,
    row: RouteRow,
) -> Result<StoredRoutePlan, crate::Error> {
    let revision_id = row.revision_id.clone();
    let points = sqlx::query_file_as!(PointRow, "queries/route-plan-points.sql", revision_id)
        .fetch_all(&mut *connection)
        .await?;
    let cues = sqlx::query_file_as!(CueRow, "queries/route-plan-cues.sql", revision_id)
        .fetch_all(&mut *connection)
        .await?;
    let transformations = sqlx::query_file_as!(
        TransformationRow,
        "queries/route-plan-transformations.sql",
        revision_id
    )
    .fetch_all(&mut *connection)
    .await?;
    decode_route(row, points, cues, transformations)
}

async fn persist_import(
    transaction: &mut Transaction<'_, Sqlite>,
    import: &RouteImport<'_>,
) -> Result<(), crate::Error> {
    ingestion::persist_blob(transaction, import.artifact, import.bytes).await?;
    ingestion::persist_artifact(transaction, import.artifact).await?;
    ingestion::persist_acquisition(transaction, import.acquisition).await?;
    persist_plan(transaction, import.plan).await?;
    persist_revision(transaction, import.revision).await?;
    persist_head(transaction, import.plan).await?;
    Ok(())
}

async fn persist_plan(
    transaction: &mut Transaction<'_, Sqlite>,
    plan: &RoutePlan,
) -> Result<(), crate::Error> {
    let id = plan.id().to_string();
    let owner_id = plan.owner_id().to_string();
    if sqlx::query_file!("queries/persist-route-plan.sql", id, owner_id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
    {
        return Err(RoutePersistenceError::PlanConflict.into());
    }
    Ok(())
}

async fn persist_revision(
    transaction: &mut Transaction<'_, Sqlite>,
    revision: &RoutePlanRevision,
) -> Result<(), crate::Error> {
    let id = revision.id().to_string();
    let plan_id = revision.plan_id().to_string();
    let previous_revision_id = revision.previous_id().map(|id| id.to_string());
    let created_at = revision.created_at().to_string();
    let name = revision.name().as_str();
    let sport = encode_sport(revision.sport());
    let shape = if revision.shape().is_geometry() {
        "geometry"
    } else {
        "control_points"
    };
    let (source_kind, source_artifact_id, source_revision_id) =
        encode_source(revision.provenance().source());
    if sqlx::query_file!(
        "queries/persist-route-revision.sql",
        id,
        plan_id,
        previous_revision_id,
        created_at,
        name,
        sport,
        shape,
        source_kind,
        source_artifact_id,
        source_revision_id,
    )
    .fetch_optional(&mut **transaction)
    .await?
    .is_none()
    {
        return Err(RoutePersistenceError::RevisionConflict.into());
    }

    persist_revision_contents(transaction, revision).await
}

async fn persist_revision_contents(
    transaction: &mut Transaction<'_, Sqlite>,
    revision: &RoutePlanRevision,
) -> Result<(), crate::Error> {
    let id = revision.id().to_string();

    for (position, point) in revision.shape().points().iter().enumerate() {
        let position = index(position)?;
        let coordinate = point.coordinate();
        let latitude = coordinate.latitude().as_degrees();
        let longitude = coordinate.longitude().as_degrees();
        let elevation = point.elevation().map(Elevation::into_meters);
        if sqlx::query_file!(
            "queries/persist-route-point.sql",
            id,
            position,
            latitude,
            longitude,
            elevation,
        )
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
        {
            return Err(RoutePersistenceError::PointConflict.into());
        }
    }
    for (position, cue) in revision.cues().iter().enumerate() {
        let position = index(position)?;
        let point_position = index(cue.point_index().as_usize())?;
        let text = cue.text().as_str();
        if sqlx::query_file!(
            "queries/persist-route-cue.sql",
            id,
            position,
            point_position,
            text,
        )
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
        {
            return Err(RoutePersistenceError::CueConflict.into());
        }
    }
    for (position, transformation) in revision.provenance().transformations().iter().enumerate() {
        let position = index(position)?;
        let component = transformation.as_component();
        let component_name = component.name();
        let component_version = component.version().to_string();
        if sqlx::query_file!(
            "queries/persist-route-transformation.sql",
            id,
            position,
            component_name,
            component_version,
        )
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
        {
            return Err(RoutePersistenceError::TransformationConflict.into());
        }
    }

    let counts = sqlx::query_file!("queries/route-revision-counts.sql", id, id, id)
        .fetch_one(&mut **transaction)
        .await?;
    if usize::try_from(counts.point_count) != Ok(revision.shape().points().len())
        || usize::try_from(counts.cue_count) != Ok(revision.cues().len())
        || usize::try_from(counts.transformation_count)
            != Ok(revision.provenance().transformations().len())
    {
        return Err(RoutePersistenceError::RevisionContentsConflict.into());
    }
    Ok(())
}

async fn persist_head(
    transaction: &mut Transaction<'_, Sqlite>,
    plan: &RoutePlan,
) -> Result<(), crate::Error> {
    let plan_id = plan.id().to_string();
    let revision_id = plan.current_revision_id().to_string();
    if sqlx::query_file!("queries/persist-route-head.sql", plan_id, revision_id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
    {
        return Err(RoutePersistenceError::HeadConflict.into());
    }
    Ok(())
}

fn decode_summary(row: RouteSummaryRow) -> Result<StoredRoutePlanSummary, crate::Error> {
    let plan_id = parse_id("route plan ID", &row.plan_id)?;
    let owner_id = parse_id("route owner ID", &row.owner_id)?;
    let revision_id = parse_id("route revision ID", &row.revision_id)?;
    Ok(StoredRoutePlanSummary {
        plan: RoutePlan::from_parts(plan_id, owner_id, revision_id),
        created_at: parse_timestamp(&row.created_at)?,
        name: parse_name(row.name)?,
        sport: decode_sport(&row.sport)?,
        geometry: decode_shape_kind(&row.shape)?,
        point_count: usize::try_from(row.point_count)
            .map_err(|error| invalid("point count", error))?,
    })
}

fn decode_route(
    row: RouteRow,
    points: Vec<PointRow>,
    cues: Vec<CueRow>,
    transformations: Vec<TransformationRow>,
) -> Result<StoredRoutePlan, crate::Error> {
    let plan_id = parse_id("route plan ID", &row.plan_id)?;
    let owner_id = parse_id("route owner ID", &row.owner_id)?;
    let revision_id = parse_id("route revision ID", &row.revision_id)?;
    let shape = decode_shape(&row.shape, points)?;
    let cues = cues
        .into_iter()
        .map(|cue| {
            let point_position = usize::try_from(cue.point_position)
                .map_err(|error| invalid("cue point position", error))?;
            let text =
                CueText::from_string(cue.cue_text).map_err(|error| invalid("cue text", error))?;
            Ok(NavigationCue::from_parts(
                RoutePointIndex::from_usize(point_position),
                text,
            ))
        })
        .collect::<Result<Vec<_>, crate::Error>>()?;
    let transformations = transformations
        .into_iter()
        .map(|row| {
            let version = row
                .component_version
                .parse::<Version>()
                .map_err(|error| invalid("route transformation version", error))?;
            ComponentVersion::from_parts(row.component_name, version)
                .map(Transformation::from_component)
                .map_err(|error| invalid("route transformation", error))
        })
        .collect::<Result<Vec<_>, crate::Error>>()?;
    let source = decode_source(&row)?;
    let revision = RoutePlanRevision::from_parts(
        revision_id,
        plan_id,
        row.previous_revision_id
            .as_deref()
            .map(|id| parse_id("previous route revision ID", id))
            .transpose()?,
        parse_timestamp(&row.created_at)?,
        parse_name(row.name)?,
        decode_sport(&row.sport)?,
        shape,
        cues,
        RevisionProvenance::from_parts(source, transformations),
    )
    .map_err(|error| invalid("route revision", error))?;
    Ok(StoredRoutePlan {
        plan: RoutePlan::from_parts(plan_id, owner_id, revision_id),
        revision,
    })
}

fn decode_shape(kind: &str, points: Vec<PointRow>) -> Result<RouteShape, crate::Error> {
    let points = points
        .into_iter()
        .map(decode_point)
        .collect::<Result<Vec<_>, crate::Error>>()?;
    match kind {
        "geometry" => RouteShape::from_geometry(points),
        "control_points" => RouteShape::from_control_points(points),
        _ => return Err(invalid("route shape", kind)),
    }
    .map_err(|error| invalid("route shape", error))
}

fn decode_point(point: PointRow) -> Result<RoutePoint, crate::Error> {
    let coordinate = garmin_model::route::Coordinate::from_parts(
        Latitude::from_degrees(point.latitude_degrees)
            .map_err(|error| invalid("route latitude", error))?,
        Longitude::from_degrees(point.longitude_degrees)
            .map_err(|error| invalid("route longitude", error))?,
    );
    let elevation = point
        .elevation_m
        .map(Elevation::from_meters)
        .transpose()
        .map_err(|error| invalid("route elevation", error))?;
    Ok(RoutePoint::from_parts(coordinate, elevation))
}

fn decode_source(row: &RouteRow) -> Result<RevisionSource, crate::Error> {
    match row.source_kind.as_str() {
        "freehand" => Ok(RevisionSource::Freehand),
        "artifact" => row
            .source_artifact_id
            .as_deref()
            .ok_or_else(|| invalid("route source artifact", "missing"))
            .and_then(|id| parse_id("route source artifact ID", id))
            .map(RevisionSource::Artifact),
        "revision" => row
            .source_revision_id
            .as_deref()
            .ok_or_else(|| invalid("route source revision", "missing"))
            .and_then(|id| parse_id("route source revision ID", id))
            .map(RevisionSource::Revision),
        value => Err(invalid("route source kind", value)),
    }
}

fn encode_source(source: RevisionSource) -> (&'static str, Option<String>, Option<String>) {
    match source {
        RevisionSource::Freehand => ("freehand", None, None),
        RevisionSource::Artifact(id) => ("artifact", Some(id.to_string()), None),
        RevisionSource::Revision(id) => ("revision", None, Some(id.to_string())),
    }
}

const fn encode_sport(sport: RouteSport) -> &'static str {
    match sport {
        RouteSport::Cycling => "cycling",
        RouteSport::Running => "running",
        RouteSport::Walking => "walking",
        RouteSport::Hiking => "hiking",
    }
}

fn decode_sport(value: &str) -> Result<RouteSport, crate::Error> {
    match value {
        "cycling" => Ok(RouteSport::Cycling),
        "running" => Ok(RouteSport::Running),
        "walking" => Ok(RouteSport::Walking),
        "hiking" => Ok(RouteSport::Hiking),
        _ => Err(invalid("route sport", value)),
    }
}

fn decode_shape_kind(value: &str) -> Result<bool, crate::Error> {
    match value {
        "geometry" => Ok(true),
        "control_points" => Ok(false),
        _ => Err(invalid("route shape", value)),
    }
}

fn parse_name(value: String) -> Result<RouteName, crate::Error> {
    RouteName::from_string(value).map_err(|error| invalid("route name", error))
}

fn parse_timestamp(value: &str) -> Result<Timestamp, crate::Error> {
    value
        .parse()
        .map_err(|error| invalid("route timestamp", error))
}

fn parse_id<T>(field: &'static str, value: &str) -> Result<T, crate::Error>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|error| invalid(field, error))
}

fn index(value: usize) -> Result<i64, RoutePersistenceError> {
    i64::try_from(value).map_err(|_| RoutePersistenceError::TooManyRecords)
}

fn invalid(field: &'static str, reason: impl std::fmt::Display) -> crate::Error {
    crate::Error::InvalidData {
        field,
        reason: reason.to_string(),
    }
}

struct RouteSummaryRow {
    plan_id: String,
    owner_id: String,
    revision_id: String,
    created_at: String,
    name: String,
    sport: String,
    shape: String,
    point_count: i64,
}

struct RouteRow {
    plan_id: String,
    owner_id: String,
    revision_id: String,
    previous_revision_id: Option<String>,
    created_at: String,
    name: String,
    sport: String,
    shape: String,
    source_kind: String,
    source_artifact_id: Option<String>,
    source_revision_id: Option<String>,
}

#[derive(Clone, Copy)]
struct PointRow {
    latitude_degrees: f64,
    longitude_degrees: f64,
    elevation_m: Option<f64>,
}

struct CueRow {
    point_position: i64,
    cue_text: String,
}

struct TransformationRow {
    component_name: String,
    component_version: String,
}

/// Invalid route persistence data or an immutable conflict.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RoutePersistenceError {
    #[error("route import operation was already committed with different arguments")]
    ImportOperationConflict,
    #[error("route import identity disagrees with its source or revision")]
    ImportIdentityMismatch,
    #[error("route artifact metadata does not match its bytes")]
    ArtifactBytesMismatch,
    #[error("route acquisition and artifact IDs differ")]
    AcquisitionArtifactMismatch,
    #[error("route acquisition and plan owners differ")]
    OwnerMismatch,
    #[error("initial route revision does not define the plan head")]
    InitialRevisionMismatch,
    #[error("initial route revision does not cite its imported artifact")]
    SourceArtifactMismatch,
    #[error("route revision does not follow the supplied plan head")]
    RevisionUpdateMismatch,
    #[error("route revision does not cite the previous revision")]
    SourceRevisionMismatch,
    #[error("route plan conflicts with stored data")]
    PlanConflict,
    #[error("route revision conflicts with stored data")]
    RevisionConflict,
    #[error("route point conflicts with stored data")]
    PointConflict,
    #[error("route cue conflicts with stored data")]
    CueConflict,
    #[error("route transformation conflicts with stored data")]
    TransformationConflict,
    #[error("route revision contents conflict with stored data")]
    RevisionContentsConflict,
    #[error("route plan head conflicts with stored data")]
    HeadConflict,
    #[error("route revision contains too many records")]
    TooManyRecords,
}

#[cfg(test)]
mod tests {
    use futures_lite::future::block_on;
    use garmin_model::{
        identity::{Profile, Role, User},
        route::{Coordinate, Latitude, Longitude},
    };
    use tempfile::tempdir;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn sport_migration_preserves_linked_revisions_and_the_current_head() -> TestResult {
        block_on(async {
            let root = tempdir()?;
            let path = root.path().join("legacy-routes.sqlite3");
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    sqlx::sqlite::SqliteConnectOptions::new()
                        .filename(&path)
                        .create_if_missing(true)
                        .foreign_keys(true),
                )
                .await?;
            crate::MIGRATOR.run_to(3, &pool).await?;
            let storage = Storage { pool };
            let owner = User::from_parts(
                UserId::new_v4(),
                Role::Owner,
                Profile::from_display_name("Walker".parse()?),
            );
            storage.save_user(&owner).await?;
            let initial = revision(RoutePlanId::new_v4(), None, RevisionSource::Freehand)?;
            let plan = RoutePlan::from_parts(initial.plan_id(), owner.id(), initial.id());
            let mut transaction = storage.pool.begin().await?;
            persist_plan(&mut transaction, &plan).await?;
            persist_revision(&mut transaction, &initial).await?;
            persist_head(&mut transaction, &plan).await?;
            transaction.commit().await?;
            let next = revision(
                plan.id(),
                Some(initial.id()),
                RevisionSource::Revision(initial.id()),
            )?;
            let expected = storage.save_route_revision(owner.id(), plan, &next).await?;
            storage.close().await;

            let storage = Storage::open(&path).await?;
            assert_eq!(
                storage.route_plan(owner.id(), plan.id()).await?,
                Some(expected)
            );
            assert_eq!(
                storage.route_revision(owner.id(), initial.id()).await?,
                Some(initial)
            );
            assert_eq!(
                storage.route_revision(UserId::new_v4(), next.id()).await?,
                None
            );
            assert_eq!(
                storage
                    .route_revision(owner.id(), RoutePlanRevisionId::new_v4())
                    .await?,
                None
            );
            assert_eq!(
                storage.route_revision(owner.id(), next.id()).await?,
                Some(next.clone())
            );
            let mut current = RoutePlan::from_parts(plan.id(), owner.id(), next.id());
            for sport in [RouteSport::Walking, RouteSport::Hiking] {
                let updated = revision_with_sport(
                    plan.id(),
                    Some(current.current_revision_id()),
                    RevisionSource::Revision(current.current_revision_id()),
                    sport,
                )?;
                let saved = storage
                    .save_route_revision(owner.id(), current, &updated)
                    .await?;
                assert_eq!(saved.revision().sport(), sport);
                current = saved.plan();
            }
            storage.close().await;
            Ok(())
        })
    }

    #[test]
    fn revision_update_is_owner_scoped_and_compares_the_expected_head() -> TestResult {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("route-update.sqlite3")).await?;
            let owner = User::from_parts(
                UserId::new_v4(),
                Role::Owner,
                Profile::from_display_name("Rider".parse()?),
            );
            storage.save_user(&owner).await?;
            let initial = revision(RoutePlanId::new_v4(), None, RevisionSource::Freehand)?;
            let plan = RoutePlan::from_parts(initial.plan_id(), owner.id(), initial.id());
            let mut transaction = storage.pool.begin().await?;
            persist_plan(&mut transaction, &plan).await?;
            persist_revision(&mut transaction, &initial).await?;
            persist_head(&mut transaction, &plan).await?;
            transaction.commit().await?;

            let first = revision(
                plan.id(),
                Some(initial.id()),
                RevisionSource::Revision(initial.id()),
            )?;
            let stale = revision(
                plan.id(),
                Some(initial.id()),
                RevisionSource::Revision(initial.id()),
            )?;
            assert!(matches!(
                storage
                    .save_route_revision(UserId::new_v4(), plan, &first)
                    .await,
                Err(crate::Error::Route(RoutePersistenceError::OwnerMismatch))
            ));
            let saved = storage
                .save_route_revision(owner.id(), plan, &first)
                .await?;
            assert_eq!(saved.plan().current_revision_id(), first.id());
            assert!(matches!(
                storage.save_route_revision(owner.id(), plan, &stale).await,
                Err(crate::Error::Route(RoutePersistenceError::HeadConflict))
            ));
            let stale_id = stale.id().to_string();
            assert_eq!(
                sqlx::query_file_scalar!("queries/test-route-revision-count.sql", stale_id)
                    .fetch_one(&storage.pool)
                    .await?,
                0
            );
            assert_eq!(
                storage
                    .route_plan(owner.id(), plan.id())
                    .await?
                    .ok_or("route plan disappeared")?
                    .revision()
                    .id(),
                first.id()
            );
            storage.close().await;
            Ok(())
        })
    }

    fn revision(
        plan_id: RoutePlanId,
        previous_id: Option<RoutePlanRevisionId>,
        source: RevisionSource,
    ) -> Result<RoutePlanRevision, Box<dyn std::error::Error>> {
        revision_with_sport(plan_id, previous_id, source, RouteSport::Cycling)
    }

    fn revision_with_sport(
        plan_id: RoutePlanId,
        previous_id: Option<RoutePlanRevisionId>,
        source: RevisionSource,
        sport: RouteSport,
    ) -> Result<RoutePlanRevision, Box<dyn std::error::Error>> {
        let points = vec![point(50.0755, 14.4378), point(50.0810, 14.4510)];
        let transformations = previous_id
            .map(|_| {
                ComponentVersion::from_parts("test-transform", Version::new(1, 0, 0))
                    .map(Transformation::from_component)
                    .map(|transformation| vec![transformation])
            })
            .transpose()?
            .unwrap_or_default();
        Ok(RoutePlanRevision::from_parts(
            RoutePlanRevisionId::new_v4(),
            plan_id,
            previous_id,
            Timestamp::from_unix_seconds(1_780_000_000)?,
            RouteName::from_string("Test route".to_owned())?,
            sport,
            RouteShape::from_geometry(points)?,
            vec![NavigationCue::from_parts(
                RoutePointIndex::from_usize(1),
                CueText::from_string("Turn left".to_owned())?,
            )],
            RevisionProvenance::from_parts(source, transformations),
        )?)
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
