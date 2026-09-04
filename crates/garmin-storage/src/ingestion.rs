//! Transactional artifact ingestion and immutable provenance.

use std::collections::HashSet;

use garmin_model::{
    artifact::{
        Acquisition, AcquisitionOperationId, Artifact, ArtifactDigest, ArtifactId, ByteCount,
        NormalizationOutcome, NormalizationRun,
    },
    identity::UserId,
    observation::{AssociationGroupId, Observation, ObservationKind},
    value::Timestamp,
};
use sqlx::{Sqlite, Transaction};
use thiserror::Error;

use crate::Storage;
use crate::{ActivityProjection, activity::persist_activity_projections};

/// A complete artifact acquisition and normalization attempt.
pub struct Ingestion<'a> {
    artifact: &'a Artifact,
    bytes: &'a [u8],
    acquisition: &'a Acquisition,
    normalization: &'a NormalizationRun,
    observations: &'a [Observation],
    activity_projections: &'a [ActivityProjection<'a>],
}

impl<'a> Ingestion<'a> {
    /// Validates one complete ingestion boundary.
    /// # Errors
    /// [`IngestionError`] when bytes or related records disagree.
    pub fn from_parts(
        artifact: &'a Artifact,
        bytes: &'a [u8],
        acquisition: &'a Acquisition,
        normalization: &'a NormalizationRun,
        observations: &'a [Observation],
        activity_projections: &'a [ActivityProjection<'a>],
    ) -> Result<Self, IngestionError> {
        if artifact.digest() != ArtifactDigest::from_bytes(bytes)
            || artifact.byte_count() != ByteCount::from_u64(bytes.len() as u64)
        {
            return Err(IngestionError::ArtifactBytesMismatch);
        }
        if acquisition.artifact_id() != artifact.id() {
            return Err(IngestionError::AcquisitionArtifactMismatch);
        }
        if normalization.acquisition_id() != acquisition.id() {
            return Err(IngestionError::NormalizationAcquisitionMismatch);
        }
        if normalization.artifact_id() != artifact.id() {
            return Err(IngestionError::NormalizationArtifactMismatch);
        }
        if normalization.owner_id() != acquisition.owner_id() {
            return Err(IngestionError::NormalizationOwnerMismatch);
        }
        if !normalization.succeeded() && !observations.is_empty() {
            return Err(IngestionError::FailedNormalizationHasObservations);
        }

        let mut observation_ids = HashSet::with_capacity(observations.len());
        for observation in observations {
            if observation.normalization_run_id() != normalization.id() {
                return Err(IngestionError::ObservationRunMismatch);
            }
            if observation.owner_id() != normalization.owner_id() {
                return Err(IngestionError::ObservationOwnerMismatch);
            }
            if !observation_ids.insert(observation.id()) {
                return Err(IngestionError::DuplicateObservation);
            }
        }

        let activity_observation_ids = observations
            .iter()
            .filter(|observation| observation.kind() == ObservationKind::Activity)
            .map(Observation::id)
            .collect::<HashSet<_>>();
        let mut projection_observation_ids = HashSet::with_capacity(activity_projections.len());
        let mut sequence_positions = HashSet::with_capacity(activity_projections.len());
        for projection in activity_projections {
            let Some(observation) = observations
                .iter()
                .find(|observation| observation.id() == projection.observation().id())
            else {
                return Err(IngestionError::UnexpectedActivityProjection);
            };
            if observation != projection.observation() {
                return Err(IngestionError::ActivityProjectionObservationMismatch);
            }
            if !projection_observation_ids.insert(projection.observation().id()) {
                return Err(IngestionError::DuplicateActivityProjection);
            }
            if !sequence_positions.insert(projection.sequence_position()) {
                return Err(IngestionError::DuplicateActivitySequencePosition);
            }
        }
        if projection_observation_ids != activity_observation_ids {
            return Err(IngestionError::MissingActivityProjection);
        }

        Ok(Self {
            artifact,
            bytes,
            acquisition,
            normalization,
            observations,
            activity_projections,
        })
    }

    #[must_use]
    pub const fn artifact(&self) -> &Artifact {
        self.artifact
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8] {
        self.bytes
    }

    #[must_use]
    pub const fn acquisition(&self) -> &Acquisition {
        self.acquisition
    }

    #[must_use]
    pub const fn normalization(&self) -> &NormalizationRun {
        self.normalization
    }

    #[must_use]
    pub const fn observations(&self) -> &[Observation] {
        self.observations
    }

    #[must_use]
    pub const fn activity_projections(&self) -> &[ActivityProjection<'a>] {
        self.activity_projections
    }
}

impl Storage {
    /// Loads the completion time retained for an acquisition retry.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted data.
    pub async fn acquisition_time(
        &self,
        owner_id: UserId,
        operation_id: AcquisitionOperationId,
    ) -> Result<Option<Timestamp>, crate::Error> {
        let owner_id = owner_id.to_string();
        let operation_id = operation_id.to_string();
        sqlx::query_file_scalar!("queries/acquisition-time.sql", owner_id, operation_id)
            .fetch_optional(&self.pool)
            .await?
            .map(|value| {
                value
                    .parse::<Timestamp>()
                    .map_err(|error| crate::Error::InvalidData {
                        field: "acquisition timestamp",
                        reason: error.to_string(),
                    })
            })
            .transpose()
    }

    /// Persists one complete ingestion atomically and idempotently.
    ///
    /// Exact bytes are shared across artifact records. Existing immutable IDs must retain
    /// their original values.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or conflicting immutable records.
    pub async fn ingest(&self, ingestion: Ingestion<'_>) -> Result<(), crate::Error> {
        let mut transaction = self.pool.begin().await?;
        persist_blob(&mut transaction, ingestion.artifact, ingestion.bytes).await?;
        persist_artifact(&mut transaction, ingestion.artifact).await?;
        persist_acquisition(&mut transaction, ingestion.acquisition).await?;
        persist_normalization(&mut transaction, ingestion.normalization).await?;
        persist_observations(
            &mut transaction,
            ingestion.normalization,
            ingestion.observations,
        )
        .await?;
        persist_activity_projections(&mut transaction, ingestion.activity_projections).await?;
        associate_equal_fingerprints(&mut transaction, ingestion.observations).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Loads the exact bytes for one immutable artifact record.
    /// # Errors
    /// [`enum@crate::Error`] when SQLite cannot complete the read.
    pub async fn artifact_bytes(&self, id: ArtifactId) -> Result<Option<Vec<u8>>, crate::Error> {
        let id = id.to_string();
        Ok(sqlx::query_file_scalar!("queries/artifact-bytes.sql", id)
            .fetch_optional(&self.pool)
            .await?)
    }
}

pub async fn persist_blob(
    transaction: &mut Transaction<'_, Sqlite>,
    artifact: &Artifact,
    bytes: &[u8],
) -> Result<(), crate::Error> {
    let artifact_digest = artifact.digest();
    let digest: &[u8] = artifact_digest.as_blake3().as_bytes();
    let byte_count = i64::try_from(artifact.byte_count().as_u64())
        .map_err(|_| IngestionError::ArtifactTooLarge)?;
    let stored = sqlx::query_file!("queries/persist-blob.sql", digest, byte_count, bytes,)
        .fetch_optional(&mut **transaction)
        .await?;
    if stored.is_none() {
        return Err(IngestionError::ArtifactDigestConflict.into());
    }
    Ok(())
}

pub async fn persist_artifact(
    transaction: &mut Transaction<'_, Sqlite>,
    artifact: &Artifact,
) -> Result<(), crate::Error> {
    let id = artifact.id().to_string();
    let artifact_digest = artifact.digest();
    let digest: &[u8] = artifact_digest.as_blake3().as_bytes();
    let media_type = artifact.media_type().to_string();
    let stored = sqlx::query_file!("queries/persist-artifact.sql", id, digest, media_type,)
        .fetch_optional(&mut **transaction)
        .await?;
    if stored.is_none() {
        return Err(IngestionError::ArtifactConflict.into());
    }
    Ok(())
}

pub async fn persist_acquisition(
    transaction: &mut Transaction<'_, Sqlite>,
    acquisition: &Acquisition,
) -> Result<(), crate::Error> {
    let id = acquisition.id().to_string();
    let operation_id = acquisition.operation_id().to_string();
    let artifact_id = acquisition.artifact_id().to_string();
    let owner_id = acquisition.owner_id().to_string();
    let source_id = acquisition.source_id().to_string();
    let source_identity = acquisition.source_identity().as_str();
    let acquired_at = acquisition.acquired_at().to_string();
    let stored = sqlx::query_file!(
        "queries/persist-acquisition.sql",
        id,
        operation_id,
        artifact_id,
        owner_id,
        source_id,
        source_identity,
        acquired_at,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::AcquisitionConflict.into());
    }
    Ok(())
}

async fn persist_normalization(
    transaction: &mut Transaction<'_, Sqlite>,
    normalization: &NormalizationRun,
) -> Result<(), crate::Error> {
    let id = normalization.id().to_string();
    let acquisition_id = normalization.acquisition_id().to_string();
    let artifact_id = normalization.artifact_id().to_string();
    let owner_id = normalization.owner_id().to_string();
    let parser_name = normalization.parser().name();
    let parser_version = normalization.parser().version().to_string();
    let schema_version = i64::from(normalization.schema().as_u32());
    let (outcome, failure) = match normalization.outcome() {
        NormalizationOutcome::Succeeded => ("succeeded", None),
        NormalizationOutcome::Failed(failure) => ("failed", Some(failure.as_str())),
    };
    let stored = sqlx::query_file!(
        "queries/persist-normalization.sql",
        id,
        acquisition_id,
        artifact_id,
        owner_id,
        parser_name,
        parser_version,
        schema_version,
        outcome,
        failure,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::NormalizationConflict.into());
    }

    for (position, transformation) in normalization.transformations().iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| IngestionError::TooManyRecords)?;
        let component = transformation.as_component();
        let component_name = component.name();
        let component_version = component.version().to_string();
        let stored = sqlx::query_file!(
            "queries/persist-transformation.sql",
            id,
            position,
            component_name,
            component_version,
        )
        .fetch_optional(&mut **transaction)
        .await?;
        if stored.is_none() {
            return Err(IngestionError::TransformationConflict.into());
        }
    }

    let transformation_count = sqlx::query_file_scalar!("queries/transformation-count.sql", id,)
        .fetch_one(&mut **transaction)
        .await?;
    let expected_count = i64::try_from(normalization.transformations().len())
        .map_err(|_| IngestionError::TooManyRecords)?;
    if transformation_count != expected_count {
        return Err(IngestionError::TransformationConflict.into());
    }
    Ok(())
}

async fn persist_observations(
    transaction: &mut Transaction<'_, Sqlite>,
    normalization: &NormalizationRun,
    observations: &[Observation],
) -> Result<(), crate::Error> {
    let normalization_run_id = normalization.id().to_string();
    for observation in observations {
        let id = observation.id().to_string();
        let owner_id = observation.owner_id().to_string();
        let kind = encode_observation_kind(observation.kind());
        let observed_at = observation.observed_at().map(|value| value.to_string());
        let fingerprint = observation.fingerprint();
        let schema = fingerprint.schema().as_component();
        let schema_name = schema.name();
        let schema_version = schema.version().to_string();
        let digest: &[u8] = fingerprint.digest().as_bytes();
        let stored = sqlx::query_file!(
            "queries/persist-observation.sql",
            id,
            owner_id,
            normalization_run_id,
            kind,
            observed_at,
            schema_name,
            schema_version,
            digest,
        )
        .fetch_optional(&mut **transaction)
        .await?;
        if stored.is_none() {
            return Err(IngestionError::ObservationConflict.into());
        }
    }

    let observation_count =
        sqlx::query_file_scalar!("queries/observation-count.sql", normalization_run_id,)
            .fetch_one(&mut **transaction)
            .await?;
    let expected_count =
        i64::try_from(observations.len()).map_err(|_| IngestionError::TooManyRecords)?;
    if observation_count != expected_count {
        return Err(IngestionError::ObservationConflict.into());
    }
    Ok(())
}

async fn associate_equal_fingerprints(
    transaction: &mut Transaction<'_, Sqlite>,
    observations: &[Observation],
) -> Result<(), crate::Error> {
    for observation in observations {
        let owner_id = observation.owner_id().to_string();
        let kind = encode_observation_kind(observation.kind());
        let fingerprint = observation.fingerprint();
        let schema = fingerprint.schema().as_component();
        let schema_name = schema.name();
        let schema_version = schema.version().to_string();
        let digest: &[u8] = fingerprint.digest().as_bytes();
        let artifact_count = sqlx::query_file_scalar!(
            "queries/matching-artifact-count.sql",
            owner_id,
            kind,
            schema_name,
            schema_version,
            digest,
        )
        .fetch_one(&mut **transaction)
        .await?;
        if artifact_count < 2 {
            continue;
        }

        let candidate_id = AssociationGroupId::new_v4().to_string();
        let group = sqlx::query_file!(
            "queries/upsert-equal-fingerprint-group.sql",
            candidate_id,
            owner_id,
            kind,
            schema_name,
            schema_version,
            digest,
        )
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query_file!(
            "queries/add-equal-fingerprint-members.sql",
            group.id,
            owner_id,
            kind,
            schema_name,
            schema_version,
            digest,
        )
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

const fn encode_observation_kind(kind: ObservationKind) -> &'static str {
    match kind {
        ObservationKind::Activity => "activity",
        ObservationKind::Measurement => "measurement",
        ObservationKind::Device => "device",
    }
}

/// Invalid ingestion data or a conflict with an immutable stored record.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IngestionError {
    #[error("artifact metadata does not match its bytes")]
    ArtifactBytesMismatch,
    #[error("acquisition and artifact IDs differ")]
    AcquisitionArtifactMismatch,
    #[error("normalization and acquisition IDs differ")]
    NormalizationAcquisitionMismatch,
    #[error("normalization and artifact IDs differ")]
    NormalizationArtifactMismatch,
    #[error("normalization and acquisition owners differ")]
    NormalizationOwnerMismatch,
    #[error("failed normalization cannot persist observations")]
    FailedNormalizationHasObservations,
    #[error("observation and normalization run IDs differ")]
    ObservationRunMismatch,
    #[error("observation and normalization owners differ")]
    ObservationOwnerMismatch,
    #[error("ingestion contains a duplicate observation ID")]
    DuplicateObservation,
    #[error("activity observation has no activity projection")]
    MissingActivityProjection,
    #[error("activity projection has no matching activity observation")]
    UnexpectedActivityProjection,
    #[error("ingestion contains duplicate activity projections")]
    DuplicateActivityProjection,
    #[error("activity projection observation disagrees with the ingestion observation")]
    ActivityProjectionObservationMismatch,
    #[error("ingestion contains duplicate activity sequence positions")]
    DuplicateActivitySequencePosition,
    #[error("artifact is too large for SQLite")]
    ArtifactTooLarge,
    #[error("ingestion contains too many records")]
    TooManyRecords,
    #[error("activity value is outside SQLite's integer range")]
    ActivityValueOutOfRange,
    #[error("activity projection conflicts with persisted data")]
    ActivityProjectionConflict,
    #[error("artifact digest already identifies different bytes")]
    ArtifactDigestConflict,
    #[error("artifact ID already identifies different data")]
    ArtifactConflict,
    #[error("acquisition identity already describes different provenance")]
    AcquisitionConflict,
    #[error("normalization run ID already describes a different attempt")]
    NormalizationConflict,
    #[error("normalization transformations conflict with stored data")]
    TransformationConflict,
    #[error("observation ID already describes different data")]
    ObservationConflict,
}
