//! Typed file-window messages and parent-side command admission.
use garmin_model::identity::{ProfilePreferences, UserId};
use garmin_service_api::DeviceCatalogSnapshot;
use garmin_ui::device_browser;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Identity {
    pub device_key: String,
    pub generation: u64,
    pub profile: Option<UserId>,
}

#[derive(Deserialize, Serialize)]
pub struct Command {
    pub identity: Identity,
    pub action: Request,
}

#[derive(Deserialize, Serialize)]
pub enum Request {
    Action(device_browser::Action),
    Refresh,
    Embed(Box<device_browser::ViewState>),
}

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    pub identity: Identity,
    pub name: String,
    pub catalog: Option<DeviceCatalogSnapshot>,
    pub available: bool,
    pub busy: bool,
    pub preferences: ProfilePreferences,
    pub dark: bool,
    pub notice: Option<String>,
    pub view: Option<device_browser::ViewState>,
}

impl Command {
    pub fn validate(&self, snapshot: &Snapshot, busy: bool) -> Result<(), String> {
        if !snapshot.available || self.identity != snapshot.identity {
            return Err(
                "The device or profile session changed. Reopen device files from the app.".into(),
            );
        }
        if matches!(self.action, Request::Embed(_)) {
            return Ok(());
        }
        if busy {
            return Err("A file operation is already in progress.".into());
        }
        match self.action {
            Request::Refresh | Request::Embed(_) => Ok(()),
            Request::Action(
                device_browser::Action::Open(_)
                | device_browser::Action::ImportFit(_)
                | device_browser::Action::ShowHiddenFiles(_),
            ) => {
                if snapshot.catalog.is_some() {
                    Ok(())
                } else {
                    Err("The device catalogue is not available yet.".into())
                }
            }
            Request::Action(_) => Err("This operation belongs to the file window.".into()),
        }
    }
}
