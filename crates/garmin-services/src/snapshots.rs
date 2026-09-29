//! Deployment-wide snapshots require an existing owner actor.

use std::{io::Read, path::Path};

use garmin_model::identity::Role;
use garmin_storage::snapshot::{Limits, Manifest, PreparedRestore};
use thiserror::Error;

use crate::{Application, UserContext};

mod operations;
pub use operations::{SnapshotDownload, SnapshotOperations, SnapshotSession};

impl Application {
    /// Publishes a verified snapshot without replacing an existing file.
    /// # Errors
    /// Rejects missing/member actors and any snapshot validation or persistence failure.
    pub async fn snapshot(
        &mut self,
        actor: UserContext,
        destination: &Path,
        limits: Limits,
    ) -> Result<Manifest, Error> {
        self.require_snapshot_owner(actor).await?;
        Ok(self.storage.snapshot(destination, limits).await?)
    }

    /// Verifies a snapshot for a new deployment root without changing this application.
    /// Dropping the returned stage cancels restoration; the current root remains available for rollback.
    /// # Errors
    /// Rejects missing/member actors and corrupt, incompatible, or oversized input.
    pub async fn prepare_restore(
        &self,
        actor: UserContext,
        input: impl Read,
        staging_parent: &Path,
        limits: Limits,
    ) -> Result<PreparedRestore, Error> {
        self.require_snapshot_owner(actor).await?;
        Ok(PreparedRestore::read(input, staging_parent, limits).await?)
    }

    pub(crate) async fn require_snapshot_owner(&self, actor: UserContext) -> Result<(), Error> {
        match self.storage.user(actor.user_id()).await? {
            Some(user) if user.role() == Role::Owner => Ok(()),
            _ => Err(Error::OwnerRequired),
        }
    }
}

/// Authorization, storage, or snapshot failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("deployment snapshots require an existing owner profile")]
    OwnerRequired,
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[error(transparent)]
    Snapshot(#[from] garmin_storage::snapshot::Error),
}
