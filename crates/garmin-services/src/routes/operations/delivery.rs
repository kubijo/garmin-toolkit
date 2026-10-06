//! Artifact tickets defer leasing until delivery, so unused tickets cannot stall restore.

use std::sync::Arc;

use garmin_model::artifact::ArtifactId;
use tokio::sync::OwnedSemaphorePermit;

use super::{Kind, Result, RouteSession, failure, internal, stale};
use crate::deployment::ApplicationLease;

/// Prepared immutable bytes, authorized for one profile and database epoch.
pub struct RouteDownload {
    session: RouteSession,
    bytes: Arc<[u8]>,
    file_name: String,
    media_type: &'static str,
    permit: OwnedSemaphorePermit,
}

/// Holds restore quiescence through the last delivered byte or cancellation.
pub struct ArtifactDelivery {
    bytes: Arc<[u8]>,
    offset: usize,
    _lease: ApplicationLease,
    _permit: OwnedSemaphorePermit,
}

impl RouteSession {
    /// Authorizes an original GPX or saved FIT Course, without re-encoding it.
    /// # Errors
    /// Rejects stale sessions, foreign artifacts, integrity failures, and exhausted capacity.
    pub async fn download(&self, artifact: ArtifactId) -> Result<RouteDownload> {
        let permit = Arc::clone(&self.operations.deliveries)
            .try_acquire_owned()
            .map_err(|_| {
                failure(
                    Kind::Limit,
                    "too many route downloads are pending or active",
                )
            })?;
        let app = self
            .operations
            .deployment
            .application(self.epoch)
            .await
            .map_err(stale)?;
        let (bytes, extension, media_type) = if let Some(bytes) = app
            .course_artifact(self.actor, artifact)
            .await
            .map_err(internal)?
        {
            (bytes, "fit", "application/vnd.ant.fit")
        } else if let Some(bytes) = app
            .storage
            .route_source_artifact(self.actor.user_id(), artifact)
            .await
            .map_err(internal)?
        {
            (bytes, "gpx", "application/gpx+xml")
        } else {
            return Err(failure(Kind::NotFound, "route artifact was not found"));
        };
        Ok(RouteDownload {
            session: self.clone(),
            bytes: bytes.into(),
            file_name: format!("route-{artifact}.{extension}"),
            media_type,
            permit,
        })
    }
}

impl RouteDownload {
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
    #[must_use]
    pub const fn media_type(&self) -> &'static str {
        self.media_type
    }
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Starts delivery only if its original epoch is still current.
    /// # Errors
    /// Rejects a ticket issued before a restore or deployment closure.
    pub async fn begin(self) -> Result<ArtifactDelivery> {
        let lease = self
            .session
            .operations
            .deployment
            .application_owned(self.session.epoch)
            .await
            .map_err(stale)?;
        Ok(ArtifactDelivery {
            bytes: self.bytes,
            offset: 0,
            _lease: lease,
            _permit: self.permit,
        })
    }
}

impl ArtifactDelivery {
    /// Advances by a bounded chunk; the delivery retains its lease until dropped.
    pub fn next_chunk(&mut self) -> Option<&[u8]> {
        if self.offset == self.bytes.len() {
            return None;
        }
        let start = self.offset;
        self.offset = self.offset.saturating_add(64 * 1024).min(self.bytes.len());
        Some(&self.bytes[start..self.offset])
    }
}
