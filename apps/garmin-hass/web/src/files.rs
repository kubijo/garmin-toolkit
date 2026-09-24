//! File-browser controller; the platform host only transports its view and actions.
use crate::window::BrowserWindow;
use garmin_model::identity::{LanguagePreference, UserId};
use garmin_ui::{
    device_browser,
    window::{Event, Spec, WindowHost},
};
pub(crate) mod catalog;
mod protocol;
use protocol::{Command, Identity, Request, Snapshot};

pub mod popup;

#[derive(Default)]
pub struct Host {
    window: BrowserWindow<Command, Snapshot>,
    pub device_key: Option<String>,
    owner: Option<UserId>,
}

impl crate::App {
    pub(super) fn open_file_window(&mut self, key: String) {
        self.sync_file_window_owner();
        let Some(owner) = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| profile.user.id())
        else {
            return;
        };
        let busy = crate::device_browser_busy(&self.shared.borrow());
        let same_window =
            self.files.owner == Some(owner) && self.files.device_key.as_ref() == Some(&key);
        // Raising the current owner's view must not depend on FIT operation admission.
        // Switching devices still waits for the active operation to finish.
        if busy && !same_window {
            return;
        }
        let name = self
            .shared
            .borrow()
            .snapshots
            .iter()
            .find(|device| device.key == key)
            .map_or_else(|| key.clone(), |device| device.name.clone());
        if self.files.device_key.as_ref() != Some(&key) {
            self.device_browser = None;
        }
        self.files.device_key = Some(key.clone());
        self.files.owner = Some(owner);
        // Open synchronously from the click, before requesting the catalogue.
        if let Err(error) = self.files.window.open(
            &self.context,
            Spec {
                id: format!("device-files-{key}"),
                kind: "device-files".into(),
                title: garmin_i18n::format_message!(&self.intl, default_message: "Files on {device}", values: { device: name.as_str() }),
                size: [1000.0, 720.0],
            },
        ) {
            self.shared.borrow_mut().notice = Some(crate::Notice::error(
                "Could not open device files".into(),
                error,
            ));
        }
        if busy {
            return;
        }
        crate::spawn_device_catalog(self.shared.clone(), self.context.clone(), key);
    }

    pub(super) fn sync_file_window_owner(&mut self) {
        let selected = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| profile.user.id());
        if self.files.owner.is_none() || self.files.owner == selected {
            return;
        }
        self.files.window.close(&self.context);
        self.files.owner = None;
        self.files.device_key = None;
        self.device_browser = None;
        self.device_fit_preview = None;
        let mut state = self.shared.borrow_mut();
        // Late results must not restore another user's catalogue or FIT preview.
        state.device_catalog_request.invalidate();
        state.device_catalog = None;
        state.device_browser_refresh = None;
        state.active_device_browser_operation = None;
        state.device_browser_interrupted = false;
        state.device_fit_preview = None;
        state.device_fit_import = None;
        state.notice = None;
    }

    pub(super) fn update_file_window(&mut self) {
        if !self.files.window.is_open() {
            return;
        }
        if self.files.device_key.is_none() {
            return;
        }
        // Snapshot construction borrows app state only when a popup actually polls.
        let mut window = std::mem::take(&mut self.files.window);
        let events = window.present(
            &self.context,
            &self.intl,
            || self.file_snapshot().expect("file window has a device"),
            |_| None,
        );
        self.files.window = window;
        for event in events {
            if let Event::Command { id, command } = event {
                let snapshot = self.file_snapshot().expect("file window has a device");
                let result = self.file_command(command, &snapshot);
                self.files.window.reply(id, result.err());
            }
        }
    }

    fn file_snapshot(&self) -> Option<Snapshot> {
        let key = self.files.device_key.as_ref()?;
        let state = self.shared.borrow();
        let profile = self
            .selected_profile
            .and_then(|index| self.profiles.get(index));
        let device = state.snapshots.iter().find(|device| &device.key == key);
        Some(Snapshot {
            identity: Identity {
                device_key: key.clone(),
                generation: state.connection_generation,
                profile: profile.map(|profile| profile.user.id()),
            },
            name: device.map_or_else(|| key.clone(), |device| device.name.clone()),
            catalog: self
                .device_browser
                .as_ref()
                .filter(|browser| browser.device_key() == key)
                .map(|browser| browser.catalog().clone()),
            available: state.client.is_some() && device.is_some() && profile.is_some(),
            busy: crate::device_browser_busy(&state)
                || state.device_catalog_request.loading().is_some(),
            language: profile.map_or(LanguagePreference::English, |profile| {
                profile.user.profile().preferences().language()
            }),
            dark: self.context.global_style().visuals.dark_mode,
            notice: state.notice.as_ref().map(|notice| {
                notice.detail.as_ref().map_or_else(
                    || notice.title.clone(),
                    |detail| format!("{}: {detail}", notice.title),
                )
            }),
        })
    }

    fn file_command(&mut self, command: Command, snapshot: &Snapshot) -> Result<(), String> {
        // Recheck busy for every command, including multiple deliveries in one frame.
        command.validate(snapshot, crate::device_browser_busy(&self.shared.borrow()))?;
        match command.action {
            Request::Refresh => crate::spawn_device_catalog(
                self.shared.clone(),
                self.context.clone(),
                command.identity.device_key,
            ),
            Request::Action(
                action @ (device_browser::Action::Open(_) | device_browser::Action::ImportFit(_)),
            ) => self.handle_device_browser_action(action),
            Request::Action(_) => return Err("This operation belongs to the file window.".into()),
        }
        Ok(())
    }
}
