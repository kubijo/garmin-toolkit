//! GPX route-plan import orchestration.

use std::fmt;

use garmin_gpx::CandidateSource;
use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionIdKind, AcquisitionOperationId, Artifact,
        ArtifactDigest, ArtifactId, ArtifactIdKind, MediaType, SourceIdentity,
    },
    identity::{Source, UserId},
    route::{
        RevisionProvenance, RevisionSource, RouteImportIdentity, RouteName, RoutePlan, RoutePlanId,
        RoutePlanIdKind, RoutePlanRevision, RoutePlanRevisionId, RoutePlanRevisionIdKind,
        RouteSport,
    },
    value::{ComponentVersion, Timestamp, Transformation},
};
use garmin_storage::{RouteImport, Storage};
use thiserror::Error;

use crate::ids::derived_id;

pub use garmin_model::route::RouteImportReceipt;

const GPX_MEDIA_TYPE: &str = "application/gpx+xml";

/// One explicitly selected GPX candidate.
pub struct RouteImportRequest<'a> {
    actor_id: UserId,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
    candidate: CandidateSource,
    name: RouteName,
    sport: RouteSport,
}

impl<'a> RouteImportRequest<'a> {
    /// Creates a complete route import request.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "an import request fully defines source, selection, and user choices"
    )]
    pub const fn from_parts(
        actor_id: UserId,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
        candidate: CandidateSource,
        name: RouteName,
        sport: RouteSport,
    ) -> Self {
        Self {
            actor_id,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
            candidate,
            name,
            sport,
        }
    }
}

/// GPX route-plan import service.
pub struct RouteImporter<'a> {
    storage: &'a Storage,
}

impl<'a> RouteImporter<'a> {
    #[must_use]
    pub const fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    /// Parses, selects, and atomically stores one GPX candidate.
    /// # Errors
    /// [`RouteImportError`] for invalid ownership, selection, definitions, or storage.
    pub async fn import(
        &self,
        request: RouteImportRequest<'_>,
    ) -> Result<RouteImportReceipt, RouteImportError> {
        let identity = request_identity(&request)?;
        if let Some(receipt) = self.retry(&request, &identity).await? {
            return Ok(receipt);
        }
        let document = garmin_gpx::parse(request.bytes)?;
        let adapter = ComponentVersion::from_parts(
            garmin_gpx::ADAPTER_NAME,
            garmin_gpx::ADAPTER_VERSION
                .parse()
                .map_err(|error| definition("GPX adapter version", error))?,
        )
        .map_err(|error| definition("GPX adapter", error))?;
        self.persist(request, &identity, &document, &adapter).await
    }

    /// Commits a retained, contained parse without parsing the input a second time.
    /// # Errors
    /// [`RouteImportError`] for a changed input, ownership, selection, or persistence failure.
    pub async fn import_prepared(
        &self,
        request: RouteImportRequest<'_>,
        prepared: &garmin_gpx::worker::PreparedGpx,
    ) -> Result<RouteImportReceipt, RouteImportError> {
        let identity = request_identity(&request)?;
        if let Some(receipt) = self.retry(&request, &identity).await? {
            return Ok(receipt);
        }
        if request.bytes != prepared.bytes() {
            return Err(RouteImportError::PreparedInputMismatch);
        }
        self.persist(request, &identity, prepared.document(), prepared.parser())
            .await
    }

    async fn retry(
        &self,
        request: &RouteImportRequest<'_>,
        identity: &RouteImportIdentity,
    ) -> Result<Option<RouteImportReceipt>, RouteImportError> {
        self.storage
            .route_import(request.actor_id, request.operation_id)
            .await?
            .map(|existing| existing.retry(identity))
            .transpose()
            .map_err(Into::into)
    }

    async fn persist(
        &self,
        request: RouteImportRequest<'_>,
        identity: &RouteImportIdentity,
        document: &garmin_gpx::Document,
        adapter: &ComponentVersion,
    ) -> Result<RouteImportReceipt, RouteImportError> {
        let candidate = document
            .candidates()
            .iter()
            .find(|candidate| candidate.source() == request.candidate)
            .ok_or(RouteImportError::CandidateNotFound)?;
        let ids = ImportIds::from_operation(request.operation_id, request.candidate);
        let media_type = GPX_MEDIA_TYPE
            .parse::<MediaType>()
            .map_err(|error| definition("GPX media type", error))?;
        let artifact = Artifact::from_bytes(ids.artifact, media_type, request.bytes);
        let acquisition = Acquisition::from_source(
            ids.acquisition,
            request.operation_id,
            artifact.id(),
            request.source,
            request.source_identity,
            request.acquired_at,
        );
        let revision = RoutePlanRevision::from_parts(
            ids.revision,
            ids.plan,
            None,
            request.acquired_at,
            request.name,
            request.sport,
            candidate.shape().clone(),
            Vec::new(),
            RevisionProvenance::from_parts(
                RevisionSource::Artifact(artifact.id()),
                vec![Transformation::from_component(adapter.clone())],
            ),
        )?;
        let plan = RoutePlan::from_parts(ids.plan, request.actor_id, ids.revision);
        Ok(self
            .storage
            .save_selected_route_import(
                request.source,
                identity,
                adapter,
                RouteImport::from_parts(&artifact, request.bytes, &acquisition, &plan, &revision)?,
            )
            .await?)
    }
}

fn request_identity(
    request: &RouteImportRequest<'_>,
) -> Result<RouteImportIdentity, RouteImportError> {
    if request.actor_id != request.source.owner_id() {
        return Err(RouteImportError::ActorCannotUseSource);
    }
    Ok(RouteImportIdentity::from_parts(
        request.actor_id,
        request.source.id(),
        request.source_identity.clone(),
        ArtifactDigest::from_bytes(request.bytes),
        request.candidate,
        request.name.clone(),
        request.sport,
    ))
}

#[derive(Clone, Copy)]
struct ImportIds {
    artifact: ArtifactId,
    acquisition: AcquisitionId,
    plan: RoutePlanId,
    revision: RoutePlanRevisionId,
}

impl ImportIds {
    fn from_operation(operation: AcquisitionOperationId, candidate: CandidateSource) -> Self {
        let candidate = CandidateKey(candidate);
        Self {
            artifact: derived_id::<ArtifactIdKind>("route-import", operation, "artifact"),
            acquisition: derived_id::<AcquisitionIdKind>("route-import", operation, "acquisition"),
            plan: derived_id::<RoutePlanIdKind>("route-import", operation, candidate),
            revision: derived_id::<RoutePlanRevisionIdKind>(
                "route-import",
                operation,
                RevisionKey(candidate),
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct CandidateKey(CandidateSource);

impl fmt::Display for CandidateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            CandidateSource::TrackSegment { track, segment } => {
                write!(formatter, "track/{track}/segment/{segment}")
            }
            CandidateSource::Route { route } => write!(formatter, "route/{route}"),
        }
    }
}

struct RevisionKey(CandidateKey);

impl fmt::Display for RevisionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/revision", self.0)
    }
}

fn definition(field: &'static str, error: impl fmt::Display) -> RouteImportError {
    RouteImportError::InvalidDefinition {
        field,
        reason: error.to_string(),
    }
}

/// GPX import failure.
#[derive(Debug, Error)]
pub enum RouteImportError {
    #[error("prepared GPX belongs to different input bytes")]
    PreparedInputMismatch,
    #[error("acting user cannot import through another user's source")]
    ActorCannotUseSource,
    #[error("selected GPX candidate is not available")]
    CandidateNotFound,
    #[error("internal {field} is invalid: {reason}")]
    InvalidDefinition {
        /// Invalid definition.
        field: &'static str,
        /// Failure reason.
        reason: String,
    },
    #[error(transparent)]
    Gpx(#[from] garmin_gpx::Error),
    #[error(transparent)]
    Route(#[from] garmin_model::route::Error),
    #[error(transparent)]
    Persistence(#[from] garmin_storage::RoutePersistenceError),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
}

#[cfg(test)]
mod tests {
    use garmin_model::artifact::AcquisitionOperationId;

    use super::*;

    #[test]
    fn candidate_location_is_part_of_route_identity() {
        let operation = AcquisitionOperationId::from_u128(1);
        let first = ImportIds::from_operation(
            operation,
            CandidateSource::TrackSegment {
                track: 0,
                segment: 0,
            },
        );
        let second = ImportIds::from_operation(
            operation,
            CandidateSource::TrackSegment {
                track: 0,
                segment: 1,
            },
        );

        assert_eq!(first.artifact, second.artifact);
        assert_eq!(first.acquisition, second.acquisition);
        assert_ne!(first.plan, second.plan);
        assert_ne!(first.revision, second.revision);
    }
}
