//! User-scoped normalized assertions and non-destructive associations.

use std::{collections::HashSet, fmt};

use thiserror::Error;

use crate::{
    artifact::{NormalizationRun, NormalizationRunId},
    identity::UserId,
    value::{ComponentVersion, Timestamp},
};

define_id!(
    ObservationIdKind,
    ObservationId,
    "observation",
    "Type marker for normalized observation IDs.",
    "One normalized observation ID."
);
define_id!(
    AssociationGroupIdKind,
    AssociationGroupId,
    "association-group",
    "Type marker for association-group IDs.",
    "A non-destructive observation association-group ID."
);

/// The normalized assertion represented by an observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ObservationKind {
    Activity,
    Measurement,
    Device,
}

/// A versioned definition of canonical fingerprint semantics.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FingerprintSchema(ComponentVersion);

impl FingerprintSchema {
    #[must_use]
    pub const fn from_component(component: ComponentVersion) -> Self {
        Self(component)
    }

    #[must_use]
    pub const fn as_component(&self) -> &ComponentVersion {
        &self.0
    }

    #[must_use]
    pub fn into_component(self) -> ComponentVersion {
        self.0
    }
}

impl fmt::Display for FingerprintSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A digest of normalized values with known semantics.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SemanticFingerprint {
    schema: FingerprintSchema,
    digest: blake3::Hash,
}

/// A failure while producing a versioned semantic fingerprint.
#[derive(Debug, Error)]
pub enum SemanticFingerprintError {
    #[error("canonical semantic payload could not be encoded: {0}")]
    Encoding(#[from] postcard::Error),
    #[error("fingerprint schema identity was invalid: {0}")]
    Schema(#[from] crate::value::Error),
}

impl SemanticFingerprint {
    #[must_use]
    pub fn from_canonical_bytes(schema: FingerprintSchema, bytes: &[u8]) -> Self {
        Self {
            schema,
            digest: blake3::hash(bytes),
        }
    }

    #[must_use]
    pub const fn from_parts(schema: FingerprintSchema, digest: blake3::Hash) -> Self {
        Self { schema, digest }
    }

    #[must_use]
    pub const fn schema(&self) -> &FingerprintSchema {
        &self.schema
    }

    #[must_use]
    pub const fn digest(&self) -> &blake3::Hash {
        &self.digest
    }

    #[must_use]
    pub fn into_parts(self) -> (FingerprintSchema, blake3::Hash) {
        (self.schema, self.digest)
    }
}

impl fmt::Display for SemanticFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.schema, self.digest)
    }
}

/// One user-scoped projection produced by a successful normalization run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    id: ObservationId,
    owner_id: UserId,
    normalization_run_id: NormalizationRunId,
    kind: ObservationKind,
    observed_at: Option<Timestamp>,
    fingerprint: SemanticFingerprint,
}

impl Observation {
    /// Creates an observation from a successful normalization run.
    /// # Errors
    /// [`Error::FailedNormalization`] when the run produced no projection.
    pub fn from_run(
        id: ObservationId,
        run: &NormalizationRun,
        kind: ObservationKind,
        observed_at: Option<Timestamp>,
        fingerprint: SemanticFingerprint,
    ) -> Result<Self, Error> {
        if !run.succeeded() {
            return Err(Error::FailedNormalization);
        }
        Ok(Self {
            id,
            owner_id: run.owner_id(),
            normalization_run_id: run.id(),
            kind,
            observed_at,
            fingerprint,
        })
    }

    #[must_use]
    pub const fn id(&self) -> ObservationId {
        self.id
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
    pub const fn kind(&self) -> ObservationKind {
        self.kind
    }

    #[must_use]
    pub const fn observed_at(&self) -> Option<Timestamp> {
        self.observed_at
    }

    #[must_use]
    pub const fn fingerprint(&self) -> &SemanticFingerprint {
        &self.fingerprint
    }
}

/// Why distinct observations were associated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssociationBasis {
    EqualFingerprint(SemanticFingerprint),
    UserConfirmed,
}

/// A non-destructive group of observations for one user and kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssociationGroup {
    id: AssociationGroupId,
    owner_id: UserId,
    kind: ObservationKind,
    basis: AssociationBasis,
    members: Vec<ObservationId>,
}

impl AssociationGroup {
    /// Validates and groups distinct observations without merging them.
    /// # Errors
    /// Invalid size, ownership, kind, membership, or fingerprint basis.
    pub fn from_observations(
        id: AssociationGroupId,
        basis: AssociationBasis,
        observations: &[Observation],
    ) -> Result<Self, Error> {
        let Some(first) = observations.first() else {
            return Err(Error::TooFewAssociationMembers);
        };
        let mut members = Vec::with_capacity(observations.len());
        let mut distinct = HashSet::with_capacity(observations.len());
        for observation in observations {
            if observation.owner_id() != first.owner_id() {
                return Err(Error::MixedUsers);
            }
            if observation.kind() != first.kind() {
                return Err(Error::MixedKinds);
            }
            if let AssociationBasis::EqualFingerprint(fingerprint) = &basis
                && observation.fingerprint() != fingerprint
            {
                return Err(Error::FingerprintMismatch);
            }
            if distinct.insert(observation.id()) {
                members.push(observation.id());
            }
        }
        if members.len() < 2 {
            return Err(Error::TooFewAssociationMembers);
        }
        Ok(Self {
            id,
            owner_id: first.owner_id(),
            kind: first.kind(),
            basis,
            members,
        })
    }

    #[must_use]
    pub const fn id(&self) -> AssociationGroupId {
        self.id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn kind(&self) -> ObservationKind {
        self.kind
    }

    #[must_use]
    pub const fn basis(&self) -> &AssociationBasis {
        &self.basis
    }

    #[must_use]
    pub fn members(&self) -> &[ObservationId] {
        &self.members
    }
}

/// Invalid observation or association data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("failed normalization cannot produce an observation")]
    FailedNormalization,
    #[error("association requires at least two distinct observations")]
    TooFewAssociationMembers,
    #[error("association members must belong to one user")]
    MixedUsers,
    #[error("association members must have one observation kind")]
    MixedKinds,
    #[error("association fingerprint does not match every member")]
    FingerprintMismatch,
}
