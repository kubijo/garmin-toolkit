//! Profile-owned GPX review and immutable Course delivery; no host paths or device writes.

#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC requests"
)]

use std::num::NonZeroU32;

use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, ArtifactId, ByteCount},
    route::{
        CourseGenerationId, CourseGenerationOperationId, RouteCandidateSource, RouteImportReceipt,
        RouteName, RoutePlanId, RoutePlanRevisionId, RoutePoint, RouteSport,
    },
    value::{ComponentVersion, Timestamp},
};
use remoc::rtc;

#[cfg(test)]
mod tests;

pub const MAX_UPLOAD_CHUNK: usize = 64 * 1024;
pub const MAX_GEOMETRY_CHUNK: u32 = 2048;
pub const PAGE_SIZE: u32 = 16;

#[garmin_macros::portable]
pub struct RouteSummary {
    pub id: RoutePlanId,
    pub revision: RoutePlanRevisionId,
    pub name: RouteName,
    pub sport: RouteSport,
    pub created_at: Timestamp,
    pub geometry: bool,
    pub point_count: u32,
}

#[garmin_macros::portable(eq)]
pub struct RouteSource {
    pub artifact: ArtifactId,
    pub digest: ArtifactDigest,
    pub byte_count: ByteCount,
    pub name: String,
}

#[garmin_macros::portable(eq)]
pub struct CourseVersion {
    pub id: CourseGenerationId,
    pub operation: CourseGenerationOperationId,
    pub revision: RoutePlanRevisionId,
    pub artifact: ArtifactId,
    pub version: NonZeroU32,
    pub serial: NonZeroU32,
    pub encoder: ComponentVersion,
    /// Whether the host would reuse this artifact with its current encoder.
    pub current_encoder: bool,
    pub generated_at: Timestamp,
    pub byte_count: ByteCount,
    pub digest: ArtifactDigest,
}

#[garmin_macros::portable]
pub struct GpxCandidate {
    pub source: RouteCandidateSource,
    pub suggested_name: Option<RouteName>,
    pub geometry: bool,
    pub point_count: u32,
    pub outline: Vec<OutlinePoint>,
}

#[garmin_macros::portable(copy)]
pub struct OutlinePoint {
    pub x: u8,
    pub y: u8,
}

#[garmin_macros::portable(eq)]
pub struct GpxRejected {
    pub source: RouteCandidateSource,
    pub reason: garmin_model::route::Error,
}

#[garmin_macros::portable(eq)]
pub enum GpxUploadPhase {
    Uploading,
    Parsing,
    Review {
        digest: ArtifactDigest,
        candidates: u32,
        rejected: u32,
    },
    Failed(String),
    InvalidFile(String),
}

#[garmin_macros::portable(eq)]
pub struct GpxUpload {
    pub operation: AcquisitionOperationId,
    pub file_name: String,
    pub received: ByteCount,
    pub total: ByteCount,
    pub phase: GpxUploadPhase,
}

#[garmin_macros::portable]
pub struct RouteSelection {
    pub digest: ArtifactDigest,
    pub candidate: RouteCandidateSource,
    pub name: RouteName,
    pub sport: RouteSport,
}

#[garmin_macros::portable]
pub enum RouteRequest {
    List {
        offset: u32,
    },
    Detail {
        plan: RoutePlanId,
    },
    RevisionPoints {
        revision: RoutePlanRevisionId,
        offset: u32,
        count: u32,
    },
    Versions {
        revision: RoutePlanRevisionId,
        offset: u32,
    },
    StartUpload {
        file_name: String,
        size: ByteCount,
    },
    Append {
        operation: AcquisitionOperationId,
        offset: u64,
        bytes: Vec<u8>,
    },
    Inspect {
        operation: AcquisitionOperationId,
    },
    UploadStatus {
        operation: AcquisitionOperationId,
    },
    Candidates {
        operation: AcquisitionOperationId,
        offset: u32,
    },
    PreviewPoints {
        operation: AcquisitionOperationId,
        candidate: RouteCandidateSource,
        offset: u32,
        count: u32,
    },
    Confirm {
        operation: AcquisitionOperationId,
        selection: RouteSelection,
    },
    Cancel {
        operation: AcquisitionOperationId,
    },
    PrepareGeneration {
        revision: RoutePlanRevisionId,
    },
    Generate {
        operation: CourseGenerationOperationId,
        revision: RoutePlanRevisionId,
    },
    DeleteCourse {
        revision: RoutePlanRevisionId,
        generation: CourseGenerationId,
    },
}

#[garmin_macros::portable]
pub enum RouteReply {
    Routes {
        items: Vec<RouteSummary>,
        next: Option<u32>,
    },
    Detail {
        route: RouteSummary,
        source: Option<RouteSource>,
    },
    Points {
        offset: u32,
        points: Vec<RoutePoint>,
        end: bool,
    },
    Versions {
        items: Vec<CourseVersion>,
        next: Option<u32>,
    },
    Upload(GpxUpload),
    Candidates {
        items: Vec<GpxCandidate>,
        rejected: Vec<GpxRejected>,
        next: Option<u32>,
    },
    Imported(RouteImportReceipt),
    Cancelled,
    GenerationReady(CourseGenerationOperationId),
    Generated(CourseVersion),
    CourseDeleted {
        revision: RoutePlanRevisionId,
    },
}

#[garmin_macros::portable(copy, eq)]
pub enum RouteFailureKind {
    Unauthorized,
    NotFound,
    Stale,
    Limit,
    InvalidState,
    Conflict,
    InvalidInput,
    Internal,
}

#[garmin_macros::portable(eq)]
pub struct RouteFailure {
    pub kind: RouteFailureKind,
    pub message: String,
}

#[rtc::remote]
pub trait RouteService {
    async fn execute(
        &self,
        request: RouteRequest,
    ) -> Result<Result<RouteReply, RouteFailure>, rtc::CallError>;
}
