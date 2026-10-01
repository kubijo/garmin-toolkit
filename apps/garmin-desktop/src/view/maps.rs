//! Native connection to the same host-owned service used over RPC.

use eframe::egui;
use garmin_device::attachments::Candidate as _;
use garmin_service_api::maps::{Command, Request, State};
use garmin_services::{
    deployment::Deployment,
    maps::{Operations, Session},
};
use garmin_ui::maps::ClientError;
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

pub(super) struct Controller {
    pub operations: Arc<Operations>,
    runtime: tokio::runtime::Runtime,
    sessions: HashMap<String, Session>,
    pub error: Option<ClientError>,
}

impl Controller {
    pub fn show(
        &self,
        ui: &mut egui::Ui,
        intl: &garmin_i18n::Intl,
        device: &str,
    ) -> Option<(Uuid, Command)> {
        if let Some(error) = &self.error {
            error.show(ui, intl);
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        let state = self.snapshot(device)?;
        garmin_ui::maps::show(ui, intl, &state).map(|command| (state.revision, command))
    }

    pub fn new(deployment: Arc<Deployment>) -> std::io::Result<Self> {
        Ok(Self {
            operations: Arc::new(Operations::new(deployment)),
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()?,
            sessions: HashMap::new(),
            error: None,
        })
    }

    pub fn open(&mut self, candidate: &crate::device_backend::Candidate) {
        match self.runtime.block_on(self.operations.open(
            candidate.key().to_owned(),
            crate::device_backend::map_connector(candidate),
            cfg!(feature = "demo"),
        )) {
            Ok(session) => {
                self.sessions.insert(candidate.key().to_owned(), session);
                self.error = None;
            }
            Err(error) => self.error = Some(ClientError::Connection(format!("{error:#}"))),
        }
    }

    pub fn snapshot(&self, device: &str) -> Option<State> {
        self.sessions.get(device).map(Session::snapshot)
    }

    pub fn submit(&mut self, device: &str, revision: Uuid, command: Command) {
        let Some(session) = self.sessions.get(device) else {
            return;
        };
        let _runtime = self.runtime.enter();
        self.error = session
            .submit(Request::Change {
                request: Uuid::new_v4(),
                revision,
                command,
            })
            .err()
            .map(ClientError::Request);
    }
}
