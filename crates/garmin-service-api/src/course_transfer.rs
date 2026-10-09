//! Reviewed delivery of an immutable FIT Course to one selected device target.

#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC requests"
)]

use garmin_model::{
    artifact::{ArtifactDigest, ArtifactId, ByteCount},
    route::{CourseGenerationId, CourseGenerationOperationId},
};
use remoc::rtc;

#[garmin_macros::portable(eq)]
pub struct CourseTarget {
    pub device_key: String,
    pub device_name: String,
    pub device_digest: String,
    pub storage_id: String,
    pub storage_label: String,
    pub directory: String,
    pub free_bytes: Option<u64>,
}

#[garmin_macros::portable(eq)]
pub struct CourseTransferReview {
    pub approval: uuid::Uuid,
    pub transfer: uuid::Uuid,
    pub generation: CourseGenerationId,
    pub generation_operation: CourseGenerationOperationId,
    pub artifact: ArtifactId,
    pub version: u32,
    pub digest: ArtifactDigest,
    pub byte_count: ByteCount,
    pub target: CourseTarget,
    pub file_name: String,
}

#[garmin_macros::portable(eq)]
pub enum CourseTransferPhase {
    Running,
    Verified,
    Accepted,
    Missing,
    NeedsReview(String),
}

#[garmin_macros::portable(eq)]
pub struct CourseTransferStatus {
    pub transfer: uuid::Uuid,
    pub generation: CourseGenerationId,
    pub target: CourseTarget,
    pub file_name: String,
    pub phase: CourseTransferPhase,
}

#[garmin_macros::portable(eq)]
pub enum CourseTransferPreparation {
    Review(CourseTransferReview),
    AlreadyOnDevice(CourseTransferStatus),
}

#[garmin_macros::portable(eq)]
pub struct CourseCleanupReview {
    pub approval: uuid::Uuid,
    pub transfer: uuid::Uuid,
    pub target: CourseTarget,
    pub file_name: String,
    pub byte_count: u64,
}

#[rtc::remote]
pub trait CourseTransferService {
    async fn targets(
        &self,
        generation: CourseGenerationOperationId,
    ) -> Result<Result<Vec<CourseTarget>, String>, rtc::CallError>;
    async fn prepare(
        &self,
        generation: CourseGenerationOperationId,
        storage_id: String,
    ) -> Result<Result<CourseTransferPreparation, String>, rtc::CallError>;
    async fn approve(
        &self,
        approval: uuid::Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError>;
    async fn status(
        &self,
        transfer: uuid::Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError>;
    async fn list(
        &self,
        generation: CourseGenerationOperationId,
    ) -> Result<Result<Vec<CourseTransferStatus>, String>, rtc::CallError>;
    async fn cancel(&self, transfer: uuid::Uuid) -> Result<Result<(), String>, rtc::CallError>;
    async fn accept(
        &self,
        transfer: uuid::Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError>;
    async fn prepare_cleanup(
        &self,
        transfer: uuid::Uuid,
    ) -> Result<Result<CourseCleanupReview, String>, rtc::CallError>;
    async fn approve_cleanup(
        &self,
        approval: uuid::Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError>;
}
