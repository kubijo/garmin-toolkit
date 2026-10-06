use garmin_model::{
    identity::UserId,
    route::{CourseGenerationId, RoutePlanRevisionId},
};
use sqlx::SqliteConnection;

use super::CoursePersistenceError;
use crate::Storage;

impl Storage {
    /// Deletes one owned Course version and reclaims bytes no remaining artifact references.
    /// A tombstone makes retries idempotent and prevents version reuse or regeneration by an old operation.
    /// # Errors
    /// Returns storage failures or an absent/foreign generation without changing any data.
    pub async fn delete_course_generation(
        &self,
        owner: UserId,
        revision: RoutePlanRevisionId,
        generation: CourseGenerationId,
    ) -> Result<(), crate::Error> {
        let mut transaction = self
            .pool
            .begin_with(include_str!("../../queries/begin-immediate.sql"))
            .await?;
        let owner = owner.to_string();
        let revision = revision.to_string();
        let generation = generation.to_string();
        let deleted = sqlx::query_file!(
            "queries/delete-course-generation.sql",
            generation,
            owner,
            revision
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(deleted) = deleted {
            sqlx::query_file!(
                "queries/insert-course-deletion.sql",
                generation,
                deleted.operation_id,
                owner,
                revision,
                deleted.version
            )
            .execute(&mut *transaction)
            .await?;
            let digest =
                sqlx::query_file_scalar!("queries/delete-course-artifact.sql", deleted.artifact_id)
                    .fetch_one(&mut *transaction)
                    .await?;
            sqlx::query_file!("queries/delete-unreferenced-blob.sql", digest)
                .execute(&mut *transaction)
                .await?;
        } else if sqlx::query_file_scalar!(
            "queries/course-deletion-retry.sql",
            generation,
            owner,
            revision
        )
        .fetch_optional(&mut *transaction)
        .await?
        .is_none()
        {
            return Err(CoursePersistenceError::RevisionNotFound.into());
        }
        transaction.commit().await?;
        Ok(())
    }
}

pub(super) async fn check_operation(
    connection: &mut SqliteConnection,
    owner: UserId,
    operation: &str,
) -> Result<(), crate::Error> {
    if let Some(deleted_owner) =
        sqlx::query_file_scalar!("queries/course-deleted-operation.sql", operation)
            .fetch_optional(connection)
            .await?
    {
        return Err(if deleted_owner == owner.to_string() {
            CoursePersistenceError::Deleted
        } else {
            CoursePersistenceError::RevisionNotFound
        }
        .into());
    }
    Ok(())
}
