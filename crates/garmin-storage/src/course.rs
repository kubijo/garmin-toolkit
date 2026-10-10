//! Immutable Course generation receipts and artifact persistence.

use std::num::NonZeroU32;

use garmin_fit::course::SerialNumber;
use garmin_model::{
    artifact::{Artifact, ArtifactDigest, ArtifactId, ByteCount},
    identity::UserId,
    route::{
        CourseGenerationId, CourseGenerationOperationId, RoutePlanId, RoutePlanRevision,
        RoutePlanRevisionId,
    },
    value::{ComponentVersion, Timestamp},
};
use sqlx::{Sqlite, Transaction};
use thiserror::Error;

use crate::{Storage, ingestion, route};

mod deletion;
#[cfg(test)]
mod tests;

/// Metadata for immutable, already persisted FIT Course bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedCourse {
    id: CourseGenerationId,
    operation_id: CourseGenerationOperationId,
    plan_id: RoutePlanId,
    revision_id: RoutePlanRevisionId,
    artifact_id: ArtifactId,
    version: NonZeroU32,
    serial: SerialNumber,
    encoder: ComponentVersion,
    generated_at: Timestamp,
    byte_count: ByteCount,
    digest: ArtifactDigest,
}

impl GeneratedCourse {
    #[must_use]
    pub const fn id(&self) -> CourseGenerationId {
        self.id
    }
    #[must_use]
    pub const fn operation_id(&self) -> CourseGenerationOperationId {
        self.operation_id
    }
    #[must_use]
    pub const fn plan_id(&self) -> RoutePlanId {
        self.plan_id
    }
    #[must_use]
    pub const fn revision_id(&self) -> RoutePlanRevisionId {
        self.revision_id
    }
    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }
    #[must_use]
    pub const fn version(&self) -> NonZeroU32 {
        self.version
    }
    #[must_use]
    pub const fn serial(&self) -> SerialNumber {
        self.serial
    }
    #[must_use]
    pub const fn encoder(&self) -> &ComponentVersion {
        &self.encoder
    }
    #[must_use]
    pub const fn generated_at(&self) -> Timestamp {
        self.generated_at
    }
    #[must_use]
    pub const fn byte_count(&self) -> ByteCount {
        self.byte_count
    }
    #[must_use]
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }
}

/// A committed retry or an exclusive allocation awaiting encoded bytes.
pub enum CourseGenerationStart {
    Existing(Box<GeneratedCourse>),
    Pending(Box<PendingCourseGeneration>),
}

/// Holds the write transaction until encoded bytes and their receipt commit together.
/// Dropping this value rolls back both version and serial allocation.
pub struct PendingCourseGeneration {
    transaction: Transaction<'static, Sqlite>,
    owner_id: UserId,
    operation_id: CourseGenerationOperationId,
    revision: RoutePlanRevision,
    version: NonZeroU32,
    serial: SerialNumber,
    encoder: ComponentVersion,
    generated_at: Timestamp,
}

impl PendingCourseGeneration {
    #[must_use]
    pub const fn revision(&self) -> &RoutePlanRevision {
        &self.revision
    }
    #[must_use]
    pub const fn serial(&self) -> SerialNumber {
        self.serial
    }

    /// Saves the service's encoded result and its generation receipt atomically.
    /// The caller must encode `revision()` with `serial()` using the recorded encoder.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or immutable artifact conflicts.
    pub async fn commit(mut self, bytes: &[u8]) -> Result<GeneratedCourse, crate::Error> {
        let artifact = Artifact::from_bytes(
            ArtifactId::new_v4(),
            "application/vnd.ant.fit".parse().map_err(invalid)?,
            bytes,
        );
        let record = GeneratedCourse {
            id: CourseGenerationId::new_v4(),
            operation_id: self.operation_id,
            plan_id: self.revision.plan_id(),
            revision_id: self.revision.id(),
            artifact_id: artifact.id(),
            version: self.version,
            serial: self.serial,
            encoder: self.encoder,
            generated_at: self.generated_at,
            byte_count: artifact.byte_count(),
            digest: artifact.digest(),
        };
        ingestion::persist_blob(&mut self.transaction, &artifact, bytes).await?;
        ingestion::persist_artifact(&mut self.transaction, &artifact).await?;
        insert_generation(&mut self.transaction, self.owner_id, &record).await?;
        insert_operation(
            &mut self.transaction,
            self.owner_id,
            self.operation_id,
            record.id,
        )
        .await?;
        self.transaction.commit().await?;
        Ok(record)
    }
}

impl Storage {
    /// Reads a bounded page with the current encoder's newest artifact first, then history.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid metadata.
    pub async fn generated_courses_page(
        &self,
        owner: UserId,
        revision: RoutePlanRevisionId,
        encoder: &ComponentVersion,
        offset: u32,
        count: u32,
    ) -> Result<Vec<GeneratedCourse>, crate::Error> {
        let owner = owner.to_string();
        let revision = revision.to_string();
        let offset = i64::from(offset);
        let count = i64::from(count);
        let encoder_name = encoder.name();
        let encoder_version = encoder.version().to_string();
        sqlx::query_file_as!(
            CourseRow,
            "queries/course-versions-page.sql",
            owner,
            revision,
            encoder_name,
            encoder_version,
            count,
            offset
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(CourseRow::decode)
        .collect()
    }

    /// Reads an owned, committed generation receipt without allocating another version.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted values.
    pub async fn course_generation(
        &self,
        owner: UserId,
        operation: CourseGenerationOperationId,
    ) -> Result<Option<GeneratedCourse>, crate::Error> {
        let operation = operation.to_string();
        deletion::check_operation(&mut *self.pool.acquire().await?, owner, &operation).await?;
        let row = sqlx::query_file_as!(
            CourseRow,
            "queries/course-generation-receipt.sql",
            operation
        )
        .fetch_optional(&self.pool)
        .await?;
        row.filter(|row| row.owner_id == owner.to_string())
            .map(CourseRow::decode)
            .transpose()
    }

    /// Reuses an exact revision/encoder artifact, or reserves a new version and serial.
    /// `operation_id` binds the revision; a changed host timestamp or encoder after a restart
    /// does not replace an already committed result. Hold the deployment lease through commit.
    /// # Errors
    /// [`enum@crate::Error`] for absent/foreign revisions, changed retry arguments, or exhaustion.
    pub async fn prepare_course_generation(
        &self,
        owner_id: UserId,
        operation_id: CourseGenerationOperationId,
        revision_id: RoutePlanRevisionId,
        encoder: ComponentVersion,
        generated_at: Timestamp,
    ) -> Result<CourseGenerationStart, crate::Error> {
        let mut transaction = self
            .pool
            .begin_with(include_str!("../queries/begin-immediate.sql"))
            .await?;
        let operation = operation_id.to_string();
        deletion::check_operation(&mut transaction, owner_id, &operation).await?;
        if let Some(row) = sqlx::query_file_as!(
            CourseRow,
            "queries/course-generation-receipt.sql",
            operation
        )
        .fetch_optional(&mut *transaction)
        .await?
        {
            if row.owner_id != owner_id.to_string() {
                return Err(CoursePersistenceError::RevisionNotFound.into());
            }
            let record = row.decode()?;
            if record.revision_id != revision_id {
                return Err(CoursePersistenceError::OperationConflict.into());
            }
            transaction.commit().await?;
            return Ok(CourseGenerationStart::Existing(Box::new(record)));
        }
        let revision = route::read_revision(&mut transaction, owner_id, revision_id)
            .await?
            .ok_or(CoursePersistenceError::RevisionNotFound)?;
        let revision_key = revision_id.to_string();
        let owner = owner_id.to_string();
        let encoder_name = encoder.name();
        let encoder_version = encoder.version().to_string();
        if let Some(row) = sqlx::query_file_as!(
            CourseRow,
            "queries/reusable-course.sql",
            owner,
            revision_key,
            encoder_name,
            encoder_version
        )
        .fetch_optional(&mut *transaction)
        .await?
        {
            let record = row.decode()?;
            insert_operation(&mut transaction, owner_id, operation_id, record.id).await?;
            transaction.commit().await?;
            return Ok(CourseGenerationStart::Existing(Box::new(record)));
        }
        let next_version =
            sqlx::query_file_scalar!("queries/next-course-version.sql", revision_key)
                .fetch_one(&mut *transaction)
                .await?;
        let version = u32::try_from(next_version)
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(CoursePersistenceError::VersionExhausted)?;
        let serial = sqlx::query_file_scalar!("queries/allocate-course-serial.sql")
            .fetch_optional(&mut *transaction)
            .await?
            .and_then(|value| u32::try_from(value).ok())
            .and_then(NonZeroU32::new)
            .map(SerialNumber::from_nonzero)
            .ok_or(CoursePersistenceError::SerialExhausted)?;
        Ok(CourseGenerationStart::Pending(Box::new(
            PendingCourseGeneration {
                transaction,
                owner_id,
                operation_id,
                revision,
                version,
                serial,
                encoder,
                generated_at,
            },
        )))
    }

    /// Lists immutable generations of one owned revision, newest version first.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted metadata.
    pub async fn generated_courses(
        &self,
        owner_id: UserId,
        revision_id: RoutePlanRevisionId,
    ) -> Result<Vec<GeneratedCourse>, crate::Error> {
        let owner_id = owner_id.to_string();
        let revision_id = revision_id.to_string();
        sqlx::query_file_as!(
            CourseRow,
            "queries/course-generations.sql",
            owner_id,
            revision_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(CourseRow::decode)
        .collect()
    }

    /// Reads the exact saved bytes after authorizing through the owning route.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or byte integrity failures.
    pub async fn course_artifact(
        &self,
        owner_id: UserId,
        artifact_id: ArtifactId,
    ) -> Result<Option<Vec<u8>>, crate::Error> {
        let owner_id = owner_id.to_string();
        let artifact_id = artifact_id.to_string();
        let Some(row) = sqlx::query_file!("queries/course-artifact.sql", owner_id, artifact_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let digest = decode_digest(row.digest)?;
        if u64::try_from(row.byte_count).ok() != Some(row.bytes.len() as u64)
            || ArtifactDigest::from_bytes(&row.bytes) != digest
        {
            return Err(CoursePersistenceError::ArtifactIntegrity.into());
        }
        Ok(Some(row.bytes))
    }
}

async fn insert_operation(
    transaction: &mut Transaction<'_, Sqlite>,
    owner: UserId,
    operation: CourseGenerationOperationId,
    generation: CourseGenerationId,
) -> Result<(), crate::Error> {
    let owner = owner.to_string();
    let operation = operation.to_string();
    let generation = generation.to_string();
    sqlx::query_file!(
        "queries/insert-course-operation.sql",
        operation,
        generation,
        owner
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_generation(
    transaction: &mut Transaction<'_, Sqlite>,
    owner_id: UserId,
    record: &GeneratedCourse,
) -> Result<(), crate::Error> {
    let id = record.id.to_string();
    let operation_id = record.operation_id.to_string();
    let owner_id = owner_id.to_string();
    let plan_id = record.plan_id.to_string();
    let revision_id = record.revision_id.to_string();
    let artifact_id = record.artifact_id.to_string();
    let version = i64::from(record.version.get());
    let serial = i64::from(record.serial.as_u32());
    let encoder_name = record.encoder.name();
    let encoder_version = record.encoder.version().to_string();
    let generated_at = record.generated_at.to_string();
    sqlx::query_file!(
        "queries/insert-course-generation.sql",
        id,
        operation_id,
        owner_id,
        plan_id,
        revision_id,
        artifact_id,
        version,
        serial,
        encoder_name,
        encoder_version,
        generated_at,
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

struct CourseRow {
    id: String,
    operation_id: String,
    owner_id: String,
    plan_id: String,
    revision_id: String,
    artifact_id: String,
    version: i64,
    serial: i64,
    encoder_name: String,
    encoder_version: String,
    generated_at: String,
    byte_count: i64,
    digest: Vec<u8>,
}

impl CourseRow {
    fn decode(self) -> Result<GeneratedCourse, crate::Error> {
        Ok(GeneratedCourse {
            id: self.id.parse().map_err(invalid)?,
            operation_id: self.operation_id.parse().map_err(invalid)?,
            plan_id: self.plan_id.parse().map_err(invalid)?,
            revision_id: self.revision_id.parse().map_err(invalid)?,
            artifact_id: self.artifact_id.parse().map_err(invalid)?,
            version: NonZeroU32::new(u32::try_from(self.version).map_err(invalid)?)
                .ok_or_else(|| invalid("zero Course version"))?,
            serial: SerialNumber::from_u32(u32::try_from(self.serial).map_err(invalid)?)
                .map_err(invalid)?,
            encoder: ComponentVersion::from_parts(
                self.encoder_name,
                self.encoder_version.parse().map_err(invalid)?,
            )
            .map_err(invalid)?,
            generated_at: self.generated_at.parse().map_err(invalid)?,
            byte_count: ByteCount::from_u64(u64::try_from(self.byte_count).map_err(invalid)?),
            digest: decode_digest(self.digest)?,
        })
    }
}

fn decode_digest(bytes: Vec<u8>) -> Result<ArtifactDigest, crate::Error> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid("invalid digest length"))?;
    Ok(ArtifactDigest::from_blake3(blake3::Hash::from_bytes(bytes)))
}

fn invalid(error: impl std::fmt::Display) -> crate::Error {
    crate::Error::InvalidData {
        field: "Course generation",
        reason: error.to_string(),
    }
}

/// Rejected Course persistence or access.
#[derive(Debug, Error)]
pub enum CoursePersistenceError {
    #[error("the generated Course was deleted")]
    Deleted,
    #[error("route revision does not exist in this profile")]
    RevisionNotFound,
    #[error("Course generation operation was already used for another revision")]
    OperationConflict,
    #[error("Course generation version space is exhausted")]
    VersionExhausted,
    #[error("FIT Course serial space is exhausted")]
    SerialExhausted,
    #[error("stored Course bytes failed integrity verification")]
    ArtifactIntegrity,
}
