//! Profile-authorized Course bytes sent to an explicitly selected device.

use std::sync::Arc;

use garmin_model::{identity::UserId, route::CourseGenerationOperationId};
use garmin_service_api::course_transfer::{
    CourseCleanupReview, CourseTarget, CourseTransferPreparation, CourseTransferService,
    CourseTransferStatus,
};
use garmin_services::{UserContext, course_transfer::CourseTransfers, maps::device::Connector};
use remoc::rtc;
use uuid::Uuid;

pub(super) struct Session {
    pub actor: UserId,
    pub device_key: String,
    pub connector: Arc<dyn Connector>,
    pub operations: Arc<CourseTransfers>,
}

impl Session {
    fn actor(&self) -> UserContext {
        UserContext::new(self.actor)
    }
}

impl CourseTransferService for Session {
    async fn targets(
        &self,
        generation: CourseGenerationOperationId,
    ) -> Result<Result<Vec<CourseTarget>, String>, rtc::CallError> {
        Ok(self
            .operations
            .targets(
                self.actor(),
                generation,
                &self.device_key,
                self.connector.as_ref(),
            )
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn prepare(
        &self,
        generation: CourseGenerationOperationId,
        storage_id: String,
    ) -> Result<Result<CourseTransferPreparation, String>, rtc::CallError> {
        Ok(self
            .operations
            .prepare(
                self.actor(),
                generation,
                self.device_key.clone(),
                &storage_id,
                Arc::clone(&self.connector),
            )
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn approve(
        &self,
        approval: Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError> {
        Ok(self
            .operations
            .approve(self.actor(), approval)
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn status(
        &self,
        transfer: Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError> {
        Ok(self
            .operations
            .status(self.actor(), transfer, self.connector.as_ref())
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn list(
        &self,
        generation: CourseGenerationOperationId,
    ) -> Result<Result<Vec<CourseTransferStatus>, String>, rtc::CallError> {
        Ok(self
            .operations
            .list(self.actor(), generation, &self.device_key)
            .await
            .map_err(|error| format!("{error:#}")))
    }

    fn cancel(
        &self,
        transfer: Uuid,
    ) -> impl std::future::Future<Output = Result<Result<(), String>, rtc::CallError>> {
        std::future::ready(Ok(self
            .operations
            .cancel(self.actor(), transfer)
            .map_err(|error| format!("{error:#}"))))
    }

    async fn accept(
        &self,
        transfer: Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError> {
        Ok(self
            .operations
            .accept(self.actor(), transfer, self.connector.as_ref())
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn prepare_cleanup(
        &self,
        transfer: Uuid,
    ) -> Result<Result<CourseCleanupReview, String>, rtc::CallError> {
        Ok(self
            .operations
            .prepare_cleanup(self.actor(), transfer, self.connector.as_ref())
            .await
            .map_err(|error| format!("{error:#}")))
    }

    async fn approve_cleanup(
        &self,
        approval: Uuid,
    ) -> Result<Result<CourseTransferStatus, String>, rtc::CallError> {
        Ok(self
            .operations
            .approve_cleanup(self.actor(), approval, self.connector.as_ref())
            .await
            .map_err(|error| format!("{error:#}")))
    }
}
