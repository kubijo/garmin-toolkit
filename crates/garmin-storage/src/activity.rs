//! Activity persistence contract.

use garmin_fit::{CreatorDiagnostics, NormalizedActivity, SequencePosition};
use garmin_model::{
    activity::ActivitySummary,
    artifact::NormalizationRunId,
    identity::UserId,
    observation::{Observation, ObservationId, ObservationKind, SemanticFingerprintError},
};
use sqlx::{Sqlite, Transaction};
use thiserror::Error;

mod read;
mod write;

/// One activity observation and its complete normalized projection.
pub struct ActivityProjection<'a> {
    observation: &'a Observation,
    sequence_position: SequencePosition,
    normalized: &'a NormalizedActivity,
}

impl<'a> ActivityProjection<'a> {
    /// Validates a projection against its observation fingerprint.
    /// # Errors
    /// [`ActivityProjectionError`] for a non-activity observation or unequal semantics.
    pub fn from_parts(
        observation: &'a Observation,
        sequence_position: SequencePosition,
        normalized: &'a NormalizedActivity,
    ) -> Result<Self, ActivityProjectionError> {
        if observation.kind() != ObservationKind::Activity {
            return Err(ActivityProjectionError::WrongObservationKind);
        }
        if &normalized.activity().semantic_fingerprint()? != observation.fingerprint() {
            return Err(ActivityProjectionError::FingerprintMismatch);
        }
        Ok(Self {
            observation,
            sequence_position,
            normalized,
        })
    }

    #[must_use]
    pub const fn observation(&self) -> &Observation {
        self.observation
    }

    #[must_use]
    pub const fn sequence_position(&self) -> SequencePosition {
        self.sequence_position
    }

    #[must_use]
    pub const fn normalized(&self) -> &NormalizedActivity {
        self.normalized
    }
}

/// Invalid activity projection input.
#[derive(Debug, Error)]
pub enum ActivityProjectionError {
    #[error("activity projection requires an activity observation")]
    WrongObservationKind,
    #[error(transparent)]
    Fingerprint(#[from] SemanticFingerprintError),
    #[error("activity projection does not match its observation fingerprint")]
    FingerprintMismatch,
}

/// Metadata used by activity lists.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredActivitySummary {
    observation_id: ObservationId,
    owner_id: UserId,
    normalization_run_id: NormalizationRunId,
    sequence_position: SequencePosition,
    creator: CreatorDiagnostics,
    summary: ActivitySummary,
}

impl StoredActivitySummary {
    #[must_use]
    pub const fn observation_id(&self) -> ObservationId {
        self.observation_id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn normalization_run_id(&self) -> NormalizationRunId {
        self.normalization_run_id
    }

    #[must_use]
    pub const fn sequence_position(&self) -> SequencePosition {
        self.sequence_position
    }

    #[must_use]
    pub const fn creator(&self) -> &CreatorDiagnostics {
        &self.creator
    }

    #[must_use]
    pub fn into_creator(self) -> CreatorDiagnostics {
        self.creator
    }

    #[must_use]
    pub const fn summary(&self) -> ActivitySummary {
        self.summary
    }
}

/// A complete activity loaded from storage.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredActivity {
    observation_id: ObservationId,
    owner_id: UserId,
    normalization_run_id: NormalizationRunId,
    sequence_position: SequencePosition,
    normalized: NormalizedActivity,
}

impl StoredActivity {
    #[must_use]
    pub const fn observation_id(&self) -> ObservationId {
        self.observation_id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn normalization_run_id(&self) -> NormalizationRunId {
        self.normalization_run_id
    }

    #[must_use]
    pub const fn sequence_position(&self) -> SequencePosition {
        self.sequence_position
    }

    #[must_use]
    pub const fn normalized(&self) -> &NormalizedActivity {
        &self.normalized
    }

    #[must_use]
    pub fn into_normalized(self) -> NormalizedActivity {
        self.normalized
    }
}

pub async fn persist_activity_projections(
    transaction: &mut Transaction<'_, Sqlite>,
    projections: &[ActivityProjection<'_>],
) -> Result<(), crate::Error> {
    write::persist_activity_projections(transaction, projections).await
}
