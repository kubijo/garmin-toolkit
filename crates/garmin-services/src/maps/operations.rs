//! Host/device ownership is independent of the profile and client connection.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use super::{MutationLocks, Session, Settings};

#[derive(Default)]
pub struct Operations {
    sessions: Mutex<HashMap<(uuid::Uuid, String), Session>>,
    mutations: Arc<MutationLocks>,
    deployment: Option<Arc<crate::deployment::Deployment>>,
    simulator: tokio::sync::OnceCell<garmin_simulator::MockServer>,
    simulation_write_bytes_per_second: Option<std::num::NonZeroU64>,
}

impl Operations {
    #[must_use]
    pub fn new(deployment: Arc<crate::deployment::Deployment>) -> Self {
        Self {
            deployment: Some(deployment),
            ..Self::default()
        }
    }

    /// Pace retained-copy uploads for acceptance tests.
    /// Physical writes are unaffected.
    #[must_use]
    pub const fn with_simulation_write_rate(
        mut self,
        bytes_per_second: Option<std::num::NonZeroU64>,
    ) -> Self {
        self.simulation_write_bytes_per_second = bytes_per_second;
        self
    }

    /// Compose a deployment's workflow with the host-selected device and service.
    /// # Errors
    /// Missing deployment or failure to start the loopback simulator.
    pub async fn open(
        &self,
        device: String,
        connector: Arc<dyn super::device::Connector>,
        simulated: bool,
    ) -> anyhow::Result<Session> {
        use anyhow::Context as _;
        let deployment = self
            .deployment
            .as_ref()
            .context("map host has no deployment")?;
        let source = if simulated {
            let server = self
                .simulator
                .get_or_try_init(garmin_simulator::MockServer::start)
                .await?;
            super::CatalogSource::Loopback(server.base_url().clone())
        } else {
            super::CatalogSource::Garmin
        };
        let root = deployment.root().join("maps");
        let mut receipts =
            super::pending_recovery::PendingRecoveryStore::new(root.join("pending-recoveries"));
        if !simulated {
            receipts = receipts
                .with_registry(super::pending_recovery::PendingRecoveryStore::application()?);
        }
        Ok(self.connect(
            device,
            Settings {
                connector,
                source,
                cache: root.join("cache"),
                captures: root.join("captures"),
                receipts,
                concurrency: 2,
                simulation_write_bytes_per_second: self.simulation_write_bytes_per_second,
            },
        ))
    }

    /// Join the device's existing workflow, or create its first host session.
    pub fn connect(&self, device: String, settings: Settings) -> Session {
        let epoch = self
            .deployment
            .as_ref()
            .map_or(uuid::Uuid::nil(), |host| host.epoch());
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((epoch, device.clone()))
            .or_insert_with(|| {
                Session::start(
                    device,
                    settings,
                    Arc::clone(&self.mutations),
                    self.deployment
                        .as_ref()
                        .map(|host| (Arc::clone(host), epoch)),
                )
            })
            .clone()
    }

    /// Browser writes must acquire this same device gate.
    #[must_use]
    pub fn mutations(&self) -> Arc<MutationLocks> {
        Arc::clone(&self.mutations)
    }
}
