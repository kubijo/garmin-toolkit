//! File-window commands cannot cross device, profile, or connection boundaries.
#[path = "../src/files/protocol.rs"]
mod protocol;

use garmin_model::identity::{LanguagePreference, UserId};
use garmin_service_api::{DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage};
use garmin_ui::{
    device_browser::{Action, Selection},
    window::protocol::Message,
};
use protocol::{Command, Identity, Request, Snapshot};

fn snapshot() -> Snapshot {
    Snapshot {
        identity: Identity {
            device_key: "watch-a".into(),
            generation: 4,
            profile: Some(UserId::new_v4()),
        },
        name: "Watch".into(),
        catalog: Some(DeviceCatalogSnapshot {
            device_key: "watch-a".into(),
            storages: vec![DeviceCatalogStorage {
                id: "storage".into(),
                label: "Internal".into(),
                entries: vec![],
            }],
        }),
        available: true,
        busy: false,
        language: LanguagePreference::English,
        dark: true,
        notice: None,
    }
}

fn command(snapshot: &Snapshot) -> Command {
    Command {
        identity: snapshot.identity.clone(),
        action: Request::Action(Action::ImportFit(Selection {
            storage_id: "storage".into(),
            storage_label: "Internal".into(),
            path: "Activities/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(123),
        })),
    }
}

#[test]
fn stale_commands_never_import_into_a_new_profile_or_device_session() {
    let mut view = snapshot();
    let request = command(&view);
    assert!(request.validate(&view, false).is_ok());
    assert!(request.validate(&view, true).is_err());
    view.available = false;
    assert!(request.validate(&view, false).is_err());
    view.available = true;
    view.identity.generation += 1;
    assert!(request.validate(&view, false).is_err());
    view.identity = request.identity.clone();
    view.identity.profile = None;
    assert!(request.validate(&view, false).is_err());
    view.identity.profile = Some(UserId::new_v4());
    assert!(request.validate(&view, false).is_err());
    view.identity = request.identity.clone();
    view.identity.device_key = "watch-b".into();
    assert!(request.validate(&view, false).is_err());
}

#[test]
fn transport_preserves_action_identity_and_parent_rejects_popup_owned_operations() {
    let mut view = snapshot();
    let envelope: Message<Command, Snapshot> = Message::Command {
        id: 7,
        request: command(&view),
    };
    let encoded = serde_json::to_string(&envelope).unwrap();
    let Message::Command { id, mut request } =
        serde_json::from_str::<Message<Command, Snapshot>>(&encoded).unwrap()
    else {
        panic!("expected a command")
    };
    assert_eq!(id, 7);
    assert!(request.validate(&view, false).is_ok());
    view.catalog = None;
    assert!(request.validate(&view, false).is_err());
    request.action = Request::Refresh;
    assert!(request.validate(&view, false).is_ok());
    request.action = Request::Action(Action::CreateDirectory {
        storage_id: "storage".into(),
        parent: "".into(),
        name: "Folder".into(),
    });
    assert!(request.validate(&view, false).is_err());
}
