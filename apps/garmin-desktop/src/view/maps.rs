//! Native connection to the same host-owned service used over RPC.

use eframe::egui;
use garmin_device::attachments::Candidate as _;
use garmin_service_api::firmware::{Action as FirmwareAction, AvailableUpdate};
use garmin_service_api::maps::{Command, Request, State};
use garmin_services::firmware::Session as FirmwareSession;
use garmin_services::{
    deployment::Deployment,
    maps::{Operations, Session},
};
use garmin_ui::maps::ClientError;
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

#[cfg(all(test, feature = "demo"))]
#[test]
fn desktop_attachment_publishes_the_firmware_version() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    let (_directory, mut desktop) = super::file_window_tests::desktop()?;
    let context = desktop.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        desktop.process_devices(&context);
        if let Some(update) = desktop
            .device_snapshots()
            .iter()
            .find_map(|device| device.firmware_update.clone())
        {
            assert_eq!(update.version, "24.20");
            return Ok(());
        }
        assert!(
            Instant::now() < deadline,
            "the attached simulator offered no firmware"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) struct Controller {
    pub operations: Arc<Operations>,
    runtime: tokio::runtime::Runtime,
    sessions: HashMap<String, Session>,
    firmware_error: Option<String>,
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
            firmware_error: None,
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

    fn connect_firmware(
        &self,
        candidate: &crate::device_backend::Candidate,
    ) -> Result<FirmwareSession, String> {
        if let Some(session) = self.operations.firmware_session(candidate.key()) {
            return Ok(session);
        }
        self.runtime
            .block_on(self.operations.open_firmware(
                candidate.key().to_owned(),
                crate::device_backend::map_connector(candidate),
                cfg!(feature = "demo"),
            ))
            .map_err(|error| error.to_string())
    }

    pub fn open_firmware(&mut self, candidate: &crate::device_backend::Candidate) {
        self.firmware_error = self.connect_firmware(candidate).err();
    }

    pub fn check_attached_firmware(&self, candidate: &crate::device_backend::Candidate) {
        match self.connect_firmware(candidate) {
            Ok(session) => {
                let _runtime = self.runtime.enter();
                session.check_on_attachment();
            }
            Err(reason) => tracing::warn!(%reason, "could not open firmware session"),
        }
    }

    pub fn firmware_update(&self, key: &str) -> Option<AvailableUpdate> {
        self.operations.firmware_session(key)?.available_update()
    }

    pub fn show_firmware(
        &self,
        ui: &mut egui::Ui,
        intl: &garmin_i18n::Intl,
        key: &str,
    ) -> Option<(Uuid, FirmwareAction)> {
        if let Some(error) = &self.firmware_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let state = self.operations.firmware_session(key)?.snapshot();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        garmin_ui::firmware::show(ui, intl, &state).map(|action| (state.revision, action))
    }

    pub fn submit_firmware(&mut self, key: &str, revision: Uuid, action: FirmwareAction) {
        let Some(session) = self.operations.firmware_session(key) else {
            return;
        };
        let _runtime = self.runtime.enter();
        self.firmware_error = session
            .submit(revision, action)
            .err()
            .map(|error| error.to_string());
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
