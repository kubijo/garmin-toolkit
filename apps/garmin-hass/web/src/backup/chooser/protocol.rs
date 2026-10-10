//! Parent-owned chooser identity and admission for popup commands.
use garmin_model::identity::{ProfilePreferences, UserId};
use garmin_service_api::files::{Directory, Operation};
use garmin_ui::device_browser::chooser::Action;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Identity {
    pub profile: UserId,
    pub generation: u64,
    pub chooser: String,
}

#[derive(Deserialize, Serialize)]
pub struct Command {
    pub identity: Identity,
    pub revision: String,
    pub action: Request,
}

#[derive(Deserialize, Serialize)]
pub enum Request {
    Action(Action),
    Embed(Box<garmin_ui::device_browser::chooser::ViewState>),
}

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    pub identity: Identity,
    pub operation: Operation,
    pub revision: String,
    pub directory: Option<Result<Directory, String>>,
    pub available: bool,
    pub busy: bool,
    pub preferences: ProfilePreferences,
    pub dark: bool,
    pub view: Option<garmin_ui::device_browser::chooser::ViewState>,
}

impl Command {
    pub fn validate(&self, snapshot: &Snapshot) -> Result<(), String> {
        if self.identity != snapshot.identity {
            return Err(
                "The profile or connection changed. Reopen the chooser from the app.".into(),
            );
        }
        if matches!(
            self.action,
            Request::Action(Action::Cancel) | Request::Embed(_)
        ) {
            return Ok(());
        }
        if !snapshot.available || snapshot.busy || self.revision != snapshot.revision {
            return Err("The chooser is updating or unavailable. Wait for it to reconnect.".into());
        }
        if matches!(self.action, Request::Action(Action::Select { .. }))
            && !matches!(snapshot.directory, Some(Ok(_)))
        {
            return Err("Load a directory before selecting a backup.".into());
        }
        Ok(())
    }
}
