//! Selected GPX input identity and durable import results.

use crate::{
    artifact::{AcquisitionId, ArtifactDigest, ArtifactId, SourceIdentity},
    identity::{SourceId, UserId},
};

use super::{RouteName, RoutePlanId, RoutePlanRevisionId, RouteSport};

/// Location of a track segment or control-point route in an imported document.
#[garmin_macros::portable(copy, hash)]
pub enum RouteCandidateSource {
    TrackSegment {
        /// Zero-based track index.
        track: usize,
        /// Zero-based segment index within that track.
        segment: usize,
    },
    Route {
        /// Zero-based route index.
        route: usize,
    },
}

/// Immutable arguments bound to an import operation; host time is not a retry argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteImportIdentity {
    owner_id: UserId,
    source_id: SourceId,
    source_identity: SourceIdentity,
    digest: ArtifactDigest,
    candidate: RouteCandidateSource,
    name: RouteName,
    sport: RouteSport,
}

impl RouteImportIdentity {
    #[must_use]
    pub const fn from_parts(
        owner_id: UserId,
        source_id: SourceId,
        source_identity: SourceIdentity,
        digest: ArtifactDigest,
        candidate: RouteCandidateSource,
        name: RouteName,
        sport: RouteSport,
    ) -> Self {
        Self {
            owner_id,
            source_id,
            source_identity,
            digest,
            candidate,
            name,
            sport,
        }
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }
    #[must_use]
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    #[must_use]
    pub const fn source_identity(&self) -> &SourceIdentity {
        &self.source_identity
    }
    #[must_use]
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }
    #[must_use]
    pub const fn candidate(&self) -> RouteCandidateSource {
        self.candidate
    }
    #[must_use]
    pub const fn name(&self) -> &RouteName {
        &self.name
    }
    #[must_use]
    pub const fn sport(&self) -> RouteSport {
        self.sport
    }
}

/// Records produced by one selected-candidate import.
#[garmin_macros::portable(copy, eq)]
pub struct RouteImportReceipt {
    artifact: ArtifactId,
    acquisition: AcquisitionId,
    plan: RoutePlanId,
    revision: RoutePlanRevisionId,
}

impl RouteImportReceipt {
    #[must_use]
    pub const fn from_parts(
        artifact: ArtifactId,
        acquisition: AcquisitionId,
        plan: RoutePlanId,
        revision: RoutePlanRevisionId,
    ) -> Self {
        Self {
            artifact,
            acquisition,
            plan,
            revision,
        }
    }

    #[must_use]
    pub const fn artifact_id(self) -> ArtifactId {
        self.artifact
    }
    #[must_use]
    pub const fn acquisition_id(self) -> AcquisitionId {
        self.acquisition
    }
    #[must_use]
    pub const fn plan_id(self) -> RoutePlanId {
        self.plan
    }
    #[must_use]
    pub const fn revision_id(self) -> RoutePlanRevisionId {
        self.revision
    }
}
