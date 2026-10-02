//! A popup selection is admitted only for the current owner, connection, and directory.
#[path = "../src/backup/chooser/protocol.rs"]
mod protocol;

use garmin_model::identity::{ProfilePreferences, UserId};
use garmin_service_api::files::{Directory, Operation};
use garmin_ui::{device_browser::chooser::Action, window::protocol::Message};
use protocol::{Command, Identity, Request, Snapshot};

fn snapshot() -> Snapshot {
    Snapshot {
        identity: Identity {
            profile: UserId::new_v4(),
            generation: 4,
            chooser: "chooser-a".into(),
        },
        operation: Operation::Open,
        revision: "directory-1".into(),
        directory: Some(Ok(Directory {
            path: "/share".into(),
            entries: vec![],
        })),
        available: true,
        busy: false,
        preferences: ProfilePreferences::default(),
        dark: true,
        view: None,
    }
}

fn selection(view: &Snapshot) -> Command {
    Command {
        identity: view.identity.clone(),
        revision: view.revision.clone(),
        action: Request::Action(Action::Select {
            path: "/share/backup.tar.zst".into(),
            replace: false,
        }),
    }
}

#[test]
fn selections_cannot_cross_profile_connection_or_reopened_window_boundaries() {
    let mut view = snapshot();
    let command = selection(&view);
    assert!(command.validate(&view).is_ok());
    view.identity.profile = UserId::new_v4();
    assert!(command.validate(&view).is_err());
    view.identity = command.identity.clone();
    view.identity.generation += 1;
    assert!(command.validate(&view).is_err());
    view.identity = command.identity.clone();
    view.identity.chooser = "replacement-popup".into();
    assert!(command.validate(&view).is_err());
}

#[test]
fn directory_refresh_and_disconnect_reject_selection_without_blocking_close() {
    let mut view = snapshot();
    let mut command = selection(&view);
    view.busy = true;
    assert!(command.validate(&view).is_err());
    command.action = Request::Action(Action::Cancel);
    assert!(command.validate(&view).is_ok());
    command = selection(&view);
    view.busy = false;
    view.available = false;
    assert!(command.validate(&view).is_err());
    view.available = true;
    view.revision = "directory-2".into();
    assert!(command.validate(&view).is_err());
    command.revision.clone_from(&view.revision);
    view.directory = Some(Err("permission denied".into()));
    assert!(command.validate(&view).is_err());
    command.action = Request::Action(Action::Navigate("/".into()));
    assert!(command.validate(&view).is_ok());
}

#[test]
fn popup_transport_preserves_selection_and_hidden_file_preference() {
    let mut view = snapshot();
    view.operation = Operation::Save;
    view.preferences = view.preferences.with_show_hidden_files(true);
    let envelope: Message<Command, Snapshot> = Message::Snapshot(Box::new(view));
    let encoded = serde_json::to_string(&envelope).unwrap();
    let Message::Snapshot(view) =
        serde_json::from_str::<Message<Command, Snapshot>>(&encoded).unwrap()
    else {
        panic!("snapshot expected");
    };
    assert!(view.preferences.show_hidden_files());
    assert_eq!(view.operation, Operation::Save);
    let mut command = selection(&view);
    command.action = Request::Action(Action::Select {
        path: "/share/backup.tar.zst".into(),
        replace: true,
    });
    let envelope: Message<Command, Snapshot> = Message::Command {
        id: 7,
        request: command,
    };
    let encoded = serde_json::to_string(&envelope).unwrap();
    let Message::Command { id, request } =
        serde_json::from_str::<Message<Command, Snapshot>>(&encoded).unwrap()
    else {
        panic!("command expected");
    };
    assert_eq!(id, 7);
    assert!(request.validate(&view).is_ok());
    assert!(
        matches!(request.action, Request::Action(Action::Select { path, replace: true }) if path == "/share/backup.tar.zst")
    );
}
