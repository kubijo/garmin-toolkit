//! Database ownership, quiescence, and recoverable restore selection shared by hosts.

use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};

use garmin_storage::{
    Storage,
    snapshot::{Limits, Manifest, PreparedRestore},
};
use thiserror::Error;
use tokio::sync::{OwnedRwLockReadGuard, RwLock, RwLockReadGuard};
use uuid::Uuid;

use crate::{Application, UserContext};

mod files;
#[cfg(test)]
mod tests;

use files::{Location, Selection};

struct Live {
    application: Option<Application>,
    epoch: Uuid,
    selection: Selection,
}

/// One host's exclusive ownership of its database root.
/// Hosts acquire application leases for every job; leases cannot outlive restore quiescence.
pub struct Deployment {
    root: PathBuf,
    live: Arc<RwLock<Live>>,
    epoch: tokio::sync::watch::Sender<Uuid>,
    _lock: File,
    #[cfg(feature = "integration-hooks")]
    integration_gate: std::sync::Mutex<Option<Arc<crate::integration_gate::IntegrationGate>>>,
    #[cfg(test)]
    hook: std::sync::Mutex<Option<TestHook>>,
}

/// An owned application lease retained until an artifact delivery finishes or is dropped.
pub struct ApplicationLease(OwnedRwLockReadGuard<Live, Application>);

impl std::ops::Deref for ApplicationLease {
    type Target = Application;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
type TestHook = Box<dyn Fn(Checkpoint) -> Result<(), Error> + Send + Sync>;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Checkpoint {
    Staged,
    Journaled,
    Closed,
    Selected,
    Opened,
    Committed,
}

impl Deployment {
    /// Locks the deployment, recovers any interrupted switch, and opens the selected database.
    /// # Errors
    /// Fails closed for an occupied root, invalid journal, missing selected database, or database error.
    pub async fn open(root: impl AsRef<Path>) -> Result<Arc<Self>, Error> {
        Self::open_initialized(root, async |_| Ok(())).await
    }

    /// Initializes an unmanaged deployment while holding its process lock.
    /// The initializer must be idempotent: interrupted initialization is retried on next open.
    /// Selected databases, including restored generations, are never initialized again.
    /// # Errors
    /// Returns opening or initialization failures without publishing a selection.
    pub async fn open_initialized(
        root: impl AsRef<Path>,
        initialize: impl AsyncFnOnce(&Storage) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<Arc<Self>, Error> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("storage.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)?;
        files::recover(&root)?;
        let selection = files::read_selection(&root)?;
        let path = selection.active.path(&root);
        if (files::has_selection(&root)? || selection.active != Location::Original)
            && !path.is_file()
        {
            return Err(Error::Recovery("selected database is missing"));
        }
        let storage = Storage::open(path).await?;
        if !files::has_selection(&root)?
            && let Err(error) = initialize(&storage).await
        {
            storage.close().await;
            return Err(Error::Initialization(error));
        }
        if let Err(error) = files::housekeeping(&root, selection) {
            storage.close().await;
            return Err(error);
        }
        let application = Application::new(storage);
        let epoch = Uuid::new_v4();
        Ok(Arc::new(Self {
            root,
            live: Arc::new(RwLock::new(Live {
                application: Some(application),
                epoch,
                selection,
            })),
            _lock: lock,
            #[cfg(feature = "integration-hooks")]
            integration_gate: std::sync::Mutex::new(None),
            epoch: tokio::sync::watch::channel(epoch).0,
            #[cfg(test)]
            hook: std::sync::Mutex::new(None),
        }))
    }

    /// Returns the epoch that must accompany queued work.
    #[must_use]
    pub fn epoch(&self) -> Uuid {
        *self.epoch.borrow()
    }

    #[cfg(feature = "integration-hooks")]
    pub async fn set_integration_gate(&self, gate: Arc<crate::integration_gate::IntegrationGate>) {
        let live = self.live.read().await;
        *self
            .integration_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&gate));
        if let Some(application) = &live.application {
            application.set_integration_gate(gate);
        }
    }

    fn application_for_selected(&self, storage: Storage) -> Application {
        let application = Application::new(storage);
        #[cfg(feature = "integration-hooks")]
        if let Some(gate) = self
            .integration_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            application.set_integration_gate(gate);
        }
        application
    }

    pub(crate) fn watch_epoch(&self) -> tokio::sync::watch::Receiver<Uuid> {
        self.epoch.subscribe()
    }

    /// Leases the application only if the queued job belongs to the current database.
    /// # Errors
    /// Rejects jobs queued before a restore or restart, and deployments requiring recovery.
    pub async fn application(
        &self,
        epoch: Uuid,
    ) -> Result<RwLockReadGuard<'_, Application>, Error> {
        let guard = self.live.read().await;
        if guard.epoch != epoch {
            return Err(Error::Stale);
        }
        RwLockReadGuard::try_map(guard, |live| live.application.as_ref())
            .map_err(|_| Error::Unavailable)
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// Acquires a lease that can move into a response stream.
    /// # Errors
    /// Rejects stale epochs and deployments requiring recovery.
    pub async fn application_owned(&self, epoch: Uuid) -> Result<ApplicationLease, Error> {
        let guard = Arc::clone(&self.live).read_owned().await;
        if guard.epoch != epoch {
            return Err(Error::Stale);
        }
        OwnedRwLockReadGuard::try_map(guard, |live| live.application.as_ref())
            .map(ApplicationLease)
            .map_err(|_| Error::Unavailable)
    }

    pub(super) async fn backup(
        &self,
        epoch: Uuid,
        actor: UserContext,
        path: &Path,
        limits: Limits,
        cancelled: &garmin_progress::CancellationToken,
    ) -> Result<Manifest, Error> {
        let mut live = self.live.write().await;
        if live.epoch != epoch {
            return Err(Error::Stale);
        }
        if cancelled.is_cancelled() {
            return Err(Error::Recovery("snapshot creation cancelled"));
        }
        Ok(live
            .application
            .as_mut()
            .ok_or(Error::Unavailable)?
            .snapshot(actor, path, limits)
            .await?)
    }

    /// Consumes a verified stage after approval. Once started, caller disconnection cannot abort the switch.
    pub(super) async fn restore(
        self: &Arc<Self>,
        epoch: Uuid,
        actor: UserContext,
        prepared: PreparedRestore,
    ) -> Result<Uuid, Error> {
        let this = Arc::clone(self);
        match tokio::spawn(async move { this.restore_inner(epoch, actor, prepared).await }).await {
            Ok(result) => result,
            Err(error) => {
                self.close().await;
                Err(Error::Task(error))
            }
        }
    }

    async fn restore_inner(
        &self,
        epoch: Uuid,
        actor: UserContext,
        prepared: PreparedRestore,
    ) -> Result<Uuid, Error> {
        #[cfg(feature = "integration-hooks")]
        let gate = self
            .integration_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        #[cfg(feature = "integration-hooks")]
        if let Some(gate) = gate
            && self.live.try_write().is_err()
        {
            gate.signal("restore-blocked").await?;
        }
        let mut live = self.live.write().await;
        if live.epoch != epoch {
            return Err(Error::Stale);
        }
        live.application
            .as_ref()
            .ok_or(Error::Unavailable)?
            .require_snapshot_owner(actor)
            .await?;
        files::prune(&self.root, live.selection)?;
        let next = files::generation(&self.root)?;
        prepared.publish(&next.path(&self.root))?;
        #[cfg(test)]
        self.checkpoint(Checkpoint::Staged)?;
        let previous = live.selection;
        files::prepare(&self.root, previous, next)?;
        // Revoke all previously queued work even if this restore eventually rolls back.
        live.epoch = Uuid::new_v4();
        self.epoch.send_replace(live.epoch);
        let result = self.switch(&mut live, next).await;
        if let Err(error) = result {
            if let Some(application) = live.application.take() {
                application.close().await;
            }
            files::recover(&self.root)?;
            let selected = files::read_selection(&self.root)?;
            live.application =
                Some(self.application_for_selected(
                    Storage::open(selected.active.path(&self.root)).await?,
                ));
            live.selection = selected;
            return Err(error);
        }
        Ok(live.epoch)
    }

    async fn switch(&self, live: &mut Live, next: Location) -> Result<(), Error> {
        #[cfg(test)]
        self.checkpoint(Checkpoint::Journaled)?;
        if let Some(application) = live.application.take() {
            application.close().await;
        }
        #[cfg(test)]
        self.checkpoint(Checkpoint::Closed)?;
        let selected = Selection {
            active: next,
            rollback: Some(live.selection.active),
        };
        files::select(&self.root, selected)?;
        #[cfg(test)]
        self.checkpoint(Checkpoint::Selected)?;
        let storage = Storage::open(next.path(&self.root)).await?;
        if let Err(error) = storage.check_integrity().await {
            storage.close().await;
            return Err(error.into());
        }
        live.application = Some(self.application_for_selected(storage));
        #[cfg(test)]
        self.checkpoint(Checkpoint::Opened)?;
        files::finish(&self.root)?;
        live.selection = selected;
        #[cfg(test)]
        self.checkpoint(Checkpoint::Committed)?;
        Ok(())
    }

    #[cfg(test)]
    fn checkpoint(&self, checkpoint: Checkpoint) -> Result<(), Error> {
        if let Some(hook) = self
            .hook
            .lock()
            .expect("test hook mutex is healthy")
            .as_ref()
        {
            hook(checkpoint)?;
        }
        Ok(())
    }

    /// Closes the selected store. The root stays locked until the last deployment handle is dropped.
    pub async fn close(&self) {
        let mut live = self.live.write().await;
        if let Some(application) = live.application.take() {
            application.close().await;
        }
        live.epoch = Uuid::new_v4();
        self.epoch.send_replace(live.epoch);
    }
}

/// Restore coordination failures. A failed recovery never falls back to an empty database.
#[derive(Debug, Error)]
pub enum Error {
    #[error("database initialization failed: {0}")]
    Initialization(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("operation belongs to a previous database epoch")]
    Stale,
    #[error("deployment is closed or requires recovery")]
    Unavailable,
    #[error("restore recovery failed: {0}")]
    Recovery(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[error(transparent)]
    Snapshot(#[from] garmin_storage::snapshot::Error),
    #[error(transparent)]
    Authorization(#[from] crate::snapshots::Error),
    #[error("restore task failed: {0}")]
    Task(#[source] tokio::task::JoinError),
}
