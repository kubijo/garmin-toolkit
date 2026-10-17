//! Generation and retrieval of persisted FIT Course artifacts.

pub mod operations;

use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactId},
    route::{CourseGenerationOperationId, RoutePlanRevisionId},
    value::{ComponentVersion, Timestamp},
};
use garmin_storage::{CourseGenerationStart, GeneratedCourse, StoredRouteImport};

use crate::{Application, Error, UserContext};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod import_tests;

impl Application {
    /// Confirms a retained worker parse without re-entering the GPX parser in the host.
    /// The host must bind its preview to the selected profile and deployment lease.
    /// # Errors
    /// [`enum@Error`] for input mismatch, ownership, selection, or persistence failure.
    pub async fn import_prepared_route(
        &self,
        request: crate::RouteImportRequest<'_>,
        prepared: &garmin_gpx::worker::PreparedGpx,
    ) -> Result<garmin_model::route::RouteImportReceipt, Error> {
        Ok(garmin_importer::RouteImporter::new(&self.storage)
            .import_prepared(
                garmin_importer::RouteImportRequest::from_parts(
                    request.user.user_id(),
                    request.source,
                    request.source_identity,
                    request.operation_id,
                    request.acquired_at,
                    request.bytes,
                    request.candidate,
                    request.name,
                    request.sport,
                ),
                prepared,
            )
            .await?)
    }

    /// Recovers a committed import without requiring a live preview or parsing its source again.
    /// # Errors
    /// [`enum@Error`] for database failures or invalid persisted provenance.
    pub async fn route_import(
        &self,
        user: UserContext,
        operation: AcquisitionOperationId,
    ) -> Result<Option<StoredRouteImport>, Error> {
        Ok(self.storage.route_import(user.user_id(), operation).await?)
    }

    /// Generates one immutable Course version, or returns the original committed receipt.
    /// The operation ID is issued by the host and binds the selected revision, never its head.
    /// The caller holds its deployment lease until this call finishes.
    /// # Errors
    /// [`enum@Error`] for invalid ownership, conflicting retries, encoding, or persistence failure.
    pub async fn generate_course(
        &self,
        user: UserContext,
        operation_id: CourseGenerationOperationId,
        revision_id: RoutePlanRevisionId,
        generated_at: Timestamp,
    ) -> Result<GeneratedCourse, Error> {
        let encoder = encoder()?;
        match self
            .storage
            .prepare_course_generation(
                user.user_id(),
                operation_id,
                revision_id,
                encoder,
                generated_at,
            )
            .await?
        {
            CourseGenerationStart::Existing(record) => Ok(*record),
            CourseGenerationStart::Pending(pending) => {
                #[cfg(feature = "integration-hooks")]
                let integration_gate = self
                    .integration_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                #[cfg(feature = "integration-hooks")]
                if let Some(gate) = &integration_gate {
                    gate.checkpoint("course-generation").await?;
                }
                #[cfg(test)]
                let gate = self
                    .course_generation_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.started.notify_one();
                    gate.resume.notified().await;
                }
                let bytes = garmin_fit::course::encode(pending.revision(), pending.serial())?;
                #[cfg(feature = "integration-hooks")]
                if let Some(gate) = &integration_gate {
                    gate.checkpoint("course-encoded").await?;
                }
                Ok(pending.commit(&bytes).await?)
            }
        }
    }

    /// Lists saved versions of an exact revision in the selected profile.
    /// # Errors
    /// [`enum@Error`] for invalid persisted data or database failure.
    pub async fn generated_courses(
        &self,
        user: UserContext,
        revision_id: RoutePlanRevisionId,
    ) -> Result<Vec<GeneratedCourse>, Error> {
        Ok(self
            .storage
            .generated_courses(user.user_id(), revision_id)
            .await?)
    }

    /// Retrieves saved bytes for delivery after authorizing through the owning route.
    /// This never regenerates the Course. Host download tickets must remain bound to
    /// the same deployment lease and profile.
    /// # Errors
    /// [`enum@Error`] for database or stored-byte integrity failures.
    pub async fn course_artifact(
        &self,
        user: UserContext,
        artifact_id: ArtifactId,
    ) -> Result<Option<Vec<u8>>, Error> {
        Ok(self
            .storage
            .course_artifact(user.user_id(), artifact_id)
            .await?)
    }
}

fn encoder() -> Result<ComponentVersion, Error> {
    ComponentVersion::from_parts(
        garmin_fit::course::ENCODER_NAME,
        garmin_fit::course::ENCODER_VERSION
            .parse()
            .map_err(invalid_encoder)?,
    )
    .map_err(invalid_encoder)
}

fn invalid_encoder(error: impl std::fmt::Display) -> Error {
    garmin_storage::Error::InvalidData {
        field: "Course encoder identity",
        reason: error.to_string(),
    }
    .into()
}
