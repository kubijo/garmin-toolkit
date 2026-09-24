use super::{Event, NativeWindow, Spec, WindowHost};
use egui::{Context, RawInput};

#[test]
fn native_host_delivers_actions_without_constructing_remote_snapshots() {
    let context = Context::default();
    crate::install(&context);
    let intl = garmin_i18n::Translations::bundled()
        .expect("bundled translations")
        .formatter(garmin_i18n::Language::English)
        .expect("English formatter");
    let mut host = NativeWindow::<String, String>::default();
    host.open(
        &context,
        Spec {
            id: "files-watch-a".into(),
            kind: "device-files".into(),
            title: "Files".into(),
            size: [640.0, 480.0],
        },
    )
    .unwrap();
    let mut commands = Vec::new();
    let mut output = context.run_ui(RawInput::default(), |ui| {
        commands.extend(host.present(
            ui.ctx(),
            &intl,
            || panic!("native views do not serialize snapshots"),
            |ui| {
                ui.label("Device files");
                Some("open-fit".into())
            },
        ));
    });
    // This headless test has no renderer to apply texture updates.
    output.textures_delta.clear();
    assert!(
        commands
            .iter()
            .any(|event| matches!(event, Event::Command { command, .. } if command == "open-fit"))
    );
    assert!(host.is_open());
    host.close(&context);
    assert!(!host.is_open());
    assert!(
        host.present(
            &context,
            &intl,
            || panic!("closed"),
            |_| panic!("closed views must not render")
        )
        .is_empty()
    );
}
