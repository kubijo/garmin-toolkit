//! Run the popup protocol and liveness checks without a browser renderer.
#[path = "../src/developer/protocol.rs"]
mod protocol;

use garmin_ui::developer::{Automation, AutomationRequest};
use garmin_ui::window::protocol::{KIND_PARAMETER, Link, SESSION_PARAMETER, valid_session};
use protocol::Snapshot;
type Message = garmin_ui::window::protocol::Message<AutomationRequest, Snapshot>;

#[test]
fn session_names_are_exactly_one_random_hex_token() {
    assert_eq!(SESSION_PARAMETER, "app-window");
    assert_eq!(KIND_PARAMETER, "window-kind");
    assert!(valid_session("0123456789abcdef0123456789abcdef"));
    for invalid in [
        "",
        "123",
        "../another-session",
        "0123456789abcdef0123456789abcdeg",
    ] {
        assert!(!valid_session(invalid));
    }
}

#[test]
fn connection_loss_disables_commands_until_the_original_tab_responds() {
    let mut link = Link::default();
    assert!(link.begin(0.0).is_none());
    link.observe(1.0);
    assert!(link.connected(1.0));
    assert!(!link.connected(7.0));
    assert!(link.begin(7.0).is_none());
    link.observe(8.0);
    assert!(link.begin(8.0).is_some());
}

#[test]
fn only_matching_acknowledgements_release_commands_and_timeouts_never_retry() {
    let mut link = Link::default();
    link.observe(0.0);
    let first = link.begin(0.0).unwrap();
    assert!(link.begin(0.1).is_none());
    link.reply(first + 1, None, 0.2);
    assert!(link.pending());
    link.observe(6.0);
    link.expire(6.0);
    assert!(!link.pending());
    assert!(link.error.as_deref().unwrap().contains("acknowledgement"));
    let second = link.begin(6.0).unwrap();
    assert_ne!(first, second);
    link.reply(first, None, 6.1);
    assert!(link.pending());
    link.reply(second, Some("automation disabled".into()), 6.2);
    assert!(!link.pending());
    assert_eq!(link.error.as_deref(), Some("automation disabled"));
}

#[test]
fn protocol_roundtrips_typed_commands_and_does_not_replay_ui_requests() {
    for message in [
        Message::Poll,
        Message::Command {
            id: 1,
            request: AutomationRequest::Start("responsive-layout".into()),
        },
        Message::Command {
            id: 2,
            request: AutomationRequest::Stop,
        },
        Message::Reply { id: 2, error: None },
    ] {
        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
    }
    let snapshot = Message::Snapshot(Box::new(Snapshot {
        automation: Automation {
            enabled: true,
            connected: true,
            requests: vec![AutomationRequest::Stop],
            ..Default::default()
        },
        debug: "app debug".into(),
        renderer: Some("worker-gl".into()),
        viewport: [720.0, 640.0],
        scale: 2.0,
        dark: true,
    }));
    let encoded = serde_json::to_string(&snapshot).unwrap();
    let Message::Snapshot(decoded) = serde_json::from_str(&encoded).unwrap() else {
        panic!("snapshot");
    };
    assert!(decoded.automation.requests.is_empty());
    // Serialization must preserve these viewport values exactly.
    assert_eq!(
        decoded.viewport.map(f32::to_bits),
        [720.0_f32, 640.0].map(f32::to_bits)
    );
    assert!(
        serde_json::from_str::<Message>(r#"{"Command":{"id":1,"request":{"Delete":"all"}}}"#)
            .is_err()
    );
}
