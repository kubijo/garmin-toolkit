//! Application-owned FIT import orchestration.

mod avatar;
mod ids;
mod route;

use std::fmt::Display;

pub use avatar::{
    AvatarCrop, AvatarCropError, AvatarImportError, AvatarImportReceipt, AvatarImportRequest,
    AvatarImporter, AvatarPreview, MAX_AVATAR_BYTES, MAX_AVATAR_DIMENSION, THUMBNAIL_EDGE,
    avatar_preview,
};
pub use route::{RouteImportError, RouteImportReceipt, RouteImportRequest, RouteImporter};

use garmin_fit::SequencePosition;
use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionIdKind, AcquisitionOperationId, Artifact,
        ArtifactId, ArtifactIdKind, MediaType, NormalizationFailure, NormalizationOutcome,
        NormalizationRun, NormalizationRunId, NormalizationRunIdKind, SchemaVersion,
        SourceIdentity,
    },
    identity::{Source, UserId},
    observation::{Observation, ObservationId, ObservationIdKind, ObservationKind},
    value::{ComponentVersion, Timestamp},
};
use garmin_storage::{ActivityProjection, Ingestion, Storage};
use thiserror::Error;

use crate::ids::derived_id;

const FIT_MEDIA_TYPE: &str = "application/vnd.ant.fit";

/// One completed FIT acquisition.
pub struct FitImportRequest<'a> {
    actor_id: UserId,
    source: &'a Source,
    source_identity: SourceIdentity,
    operation_id: AcquisitionOperationId,
    acquired_at: Timestamp,
    bytes: &'a [u8],
}

impl<'a> FitImportRequest<'a> {
    #[must_use]
    pub const fn from_parts(
        actor_id: UserId,
        source: &'a Source,
        source_identity: SourceIdentity,
        operation_id: AcquisitionOperationId,
        acquired_at: Timestamp,
        bytes: &'a [u8],
    ) -> Self {
        Self {
            actor_id,
            source,
            source_identity,
            operation_id,
            acquired_at,
            bytes,
        }
    }

    #[must_use]
    pub const fn actor_id(&self) -> UserId {
        self.actor_id
    }

    #[must_use]
    pub const fn source(&self) -> &Source {
        self.source
    }

    #[must_use]
    pub const fn source_identity(&self) -> &SourceIdentity {
        &self.source_identity
    }

    #[must_use]
    pub const fn operation_id(&self) -> AcquisitionOperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn acquired_at(&self) -> Timestamp {
        self.acquired_at
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8] {
        self.bytes
    }
}

/// Record IDs produced by an import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReceipt {
    artifact_id: ArtifactId,
    acquisition_id: AcquisitionId,
    normalization_run_id: NormalizationRunId,
    observation_ids: Vec<ObservationId>,
}

impl ImportReceipt {
    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    #[must_use]
    pub const fn acquisition_id(&self) -> AcquisitionId {
        self.acquisition_id
    }

    #[must_use]
    pub const fn normalization_run_id(&self) -> NormalizationRunId {
        self.normalization_run_id
    }

    #[must_use]
    pub fn observation_ids(&self) -> &[ObservationId] {
        &self.observation_ids
    }
}

/// Persisted FIT import result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FitImportOutcome {
    Imported(ImportReceipt),
    Rejected {
        /// Stored record IDs.
        receipt: ImportReceipt,
        /// Stored parser failure.
        failure: NormalizationFailure,
    },
}

impl FitImportOutcome {
    #[must_use]
    pub const fn receipt(&self) -> &ImportReceipt {
        match self {
            Self::Imported(receipt) | Self::Rejected { receipt, .. } => receipt,
        }
    }
}

/// FIT import service.
pub struct FitImporter<'a> {
    storage: &'a Storage,
}

impl<'a> FitImporter<'a> {
    #[must_use]
    pub const fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    /// Normalizes and atomically stores an acquisition.
    ///
    /// Parser rejection is a stored [`FitImportOutcome::Rejected`] result.
    /// # Errors
    /// [`ImportError`] for ownership, definition, or persistence errors.
    pub async fn import(
        &self,
        request: FitImportRequest<'_>,
    ) -> Result<FitImportOutcome, ImportError> {
        if request.actor_id != request.source.owner_id() {
            return Err(ImportError::ActorCannotUseSource);
        }

        let ids = ImportIds::from_operation(request.operation_id);
        let media_type = FIT_MEDIA_TYPE
            .parse::<MediaType>()
            .map_err(|error| definition("FIT media type", error))?;
        let artifact = Artifact::from_bytes(ids.artifact, media_type, request.bytes);
        let acquisition = Acquisition::from_source(
            ids.acquisition,
            request.operation_id,
            artifact.id(),
            request.source,
            request.source_identity,
            request.acquired_at,
        );
        let parser = ComponentVersion::from_parts(
            garmin_fit::NORMALIZER_NAME,
            garmin_fit::NORMALIZER_VERSION
                .parse()
                .map_err(|error| definition("FIT normalizer version", error))?,
        )
        .map_err(|error| definition("FIT normalizer", error))?;
        let schema = SchemaVersion::from_u32(garmin_fit::NORMALIZATION_SCHEMA_VERSION)
            .map_err(|error| definition("FIT schema version", error))?;
        let prepared = PreparedImport {
            ids,
            bytes: request.bytes,
            artifact,
            acquisition,
            parser,
            schema,
        };

        match garmin_fit::normalize_activities(request.bytes) {
            Ok(normalized) => self.persist_success(prepared, normalized).await,
            Err(error) => {
                let failure = NormalizationFailure::from_string(error.to_string())
                    .map_err(|error| definition("FIT failure summary", error))?;
                let normalization = NormalizationRun::from_parts(
                    prepared.ids.normalization,
                    &prepared.acquisition,
                    prepared.parser,
                    prepared.schema,
                    Vec::new(),
                    NormalizationOutcome::Failed(failure.clone()),
                );
                let ingestion = Ingestion::from_parts(
                    &prepared.artifact,
                    prepared.bytes,
                    &prepared.acquisition,
                    &normalization,
                    &[],
                    &[],
                )?;
                self.storage.ingest(ingestion).await?;
                Ok(FitImportOutcome::Rejected {
                    receipt: prepared.ids.receipt(Vec::new()),
                    failure,
                })
            }
        }
    }

    async fn persist_success(
        &self,
        prepared: PreparedImport<'_>,
        normalized: garmin_fit::ActivityImport,
    ) -> Result<FitImportOutcome, ImportError> {
        let PreparedImport {
            ids,
            bytes,
            artifact,
            acquisition,
            parser,
            schema,
        } = prepared;
        let normalization = NormalizationRun::from_parts(
            ids.normalization,
            &acquisition,
            parser,
            schema,
            Vec::new(),
            NormalizationOutcome::Succeeded,
        );
        let positioned = normalized.positioned_activities().collect::<Vec<_>>();
        let observations = positioned
            .iter()
            .map(|(position, activity)| {
                Observation::from_run(
                    observation_id(ids.operation, *position),
                    &normalization,
                    ObservationKind::Activity,
                    Some(activity.activity().summary().time().start()),
                    activity.activity().semantic_fingerprint()?,
                )
                .map_err(ImportError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let projections = observations
            .iter()
            .zip(positioned)
            .map(|(observation, (position, activity))| {
                ActivityProjection::from_parts(observation, position, activity)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let observation_ids = observations.iter().map(Observation::id).collect::<Vec<_>>();
        let ingestion = Ingestion::from_parts(
            &artifact,
            bytes,
            &acquisition,
            &normalization,
            &observations,
            &projections,
        )?;
        self.storage.ingest(ingestion).await?;
        Ok(FitImportOutcome::Imported(ids.receipt(observation_ids)))
    }
}

#[derive(Clone, Copy)]
struct ImportIds {
    operation: AcquisitionOperationId,
    artifact: ArtifactId,
    acquisition: AcquisitionId,
    normalization: NormalizationRunId,
}

impl ImportIds {
    fn from_operation(operation: AcquisitionOperationId) -> Self {
        Self {
            operation,
            artifact: derived_id::<ArtifactIdKind>("fit-import", operation, "artifact"),
            acquisition: derived_id::<AcquisitionIdKind>("fit-import", operation, "acquisition"),
            normalization: derived_id::<NormalizationRunIdKind>(
                "fit-import",
                operation,
                "normalization",
            ),
        }
    }

    const fn receipt(self, observation_ids: Vec<ObservationId>) -> ImportReceipt {
        ImportReceipt {
            artifact_id: self.artifact,
            acquisition_id: self.acquisition,
            normalization_run_id: self.normalization,
            observation_ids,
        }
    }
}

struct PreparedImport<'a> {
    ids: ImportIds,
    bytes: &'a [u8],
    artifact: Artifact,
    acquisition: Acquisition,
    parser: ComponentVersion,
    schema: SchemaVersion,
}

fn observation_id(operation: AcquisitionOperationId, position: SequencePosition) -> ObservationId {
    derived_id::<ObservationIdKind>("fit-import", operation, format!("observation/{position}"))
}

fn definition(field: &'static str, error: impl Display) -> ImportError {
    ImportError::InvalidDefinition {
        field,
        reason: error.to_string(),
    }
}

/// Import failure outside parser rejection.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("acting user cannot import through another user's source")]
    ActorCannotUseSource,
    #[error("internal {field} is invalid: {reason}")]
    InvalidDefinition {
        /// Invalid definition.
        field: &'static str,
        /// Failure reason.
        reason: String,
    },
    #[error(transparent)]
    Observation(#[from] garmin_model::observation::Error),
    #[error(transparent)]
    Fingerprint(#[from] garmin_model::observation::SemanticFingerprintError),
    #[error(transparent)]
    Projection(#[from] garmin_storage::ActivityProjectionError),
    #[error(transparent)]
    Ingestion(#[from] garmin_storage::IngestionError),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_id_schema_has_a_golden_vector() {
        let operation = AcquisitionOperationId::from_u128(0x12345678_1234_5678_1234_567812345678);
        let ids = ImportIds::from_operation(operation);

        assert_eq!(
            [
                ids.artifact.to_string(),
                ids.acquisition.to_string(),
                ids.normalization.to_string(),
                observation_id(operation, SequencePosition::from_u32(2)).to_string(),
            ],
            [
                "54b7c8f0-ffb8-5754-ab62-9417e0d610c3",
                "0d1ef9c1-3032-5a2c-8b35-80c9b847be0e",
                "571ef38e-55c3-5967-873f-d00ce8fac026",
                "6df45ccb-0449-5687-870f-8f8a4cf7f17d",
            ]
        );
    }
}
