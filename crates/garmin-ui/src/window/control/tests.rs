use super::*;
use egui::{Pos2, RawInput, Rect};

fn spec() -> Spec {
    Spec {
        id: "files".into(),
        kind: "device-files".into(),
        title: "Files".into(),
        size: [400.0, 300.0],
    }
}

fn frame(
    context: &Context,
    viewport: ViewportId,
    tick: u32,
    checked: &mut bool,
) -> egui::FullOutput {
    let mut input = RawInput {
        viewport_id: viewport,
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
        time: Some(f64::from(tick) / 60.0),
        focused: true,
        ..Default::default()
    };
    input.viewports.insert(
        viewport,
        egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            ..Default::default()
        },
    );
    context.run_ui(input, |ui| {
        let response = ui.checkbox(checked, "Setting");
        crate::semantics::target(ui, &response, "setting");
        crate::semantics::value(&response, if *checked { "on" } else { "off" });
    })
}

#[test]
fn child_actions_and_trees_are_isolated_from_the_root_workload() {
    let context = Context::default();
    crate::install(&context);
    context.add_plugin(Driver::default());
    let window = register(&context, &spec()).expect("registered child");
    let viewport = ViewportId::from_hash_of("files");
    let mut root = false;
    let mut child = false;
    frame(&context, ViewportId::ROOT, 0, &mut root).drop_without_applying_deltas();
    frame(&context, viewport, 0, &mut child).drop_without_applying_deltas();
    command(
        &context,
        None,
        "action",
        &json!({"kind":"click","target":"setting"}),
    )
    .expect("root action");
    let before = command(&context, None, "status", &Value::Null).expect("root status");
    command(&context, Some(&window.id), "sequence", &json!([
        {"kind":"click","target":"setting"}, {"kind":"assert_value","target":"setting","value":"on"}
    ])).expect("child sequence");
    for tick in 1..90 {
        frame(&context, viewport, tick, &mut child).drop_without_applying_deltas();
    }
    assert!(child);
    assert!(!root);
    assert_eq!(
        command(&context, None, "status", &Value::Null).expect("root status"),
        before
    );
    assert_eq!(
        command(&context, Some(&window.id), "result", &Value::Null).expect("child result")["state"],
        "passed"
    );
    assert!(
        command(
            &context,
            Some(&window.id),
            "start",
            &json!("stationary-arrival")
        )
        .is_err()
    );
    assert_eq!(
        command(&context, Some(&window.id), "targets", &Value::Null).expect("child targets")[0]["value"],
        "on"
    );
    assert_eq!(
        command(&context, None, "targets", &Value::Null).expect("root targets")[0]["value"],
        "off"
    );
}

#[test]
fn close_revokes_commands_and_reopen_never_reuses_a_handle() {
    let context = Context::default();
    crate::install(&context);
    context.add_plugin(Driver::default());
    let old = register(&context, &spec()).expect("registered child");
    let viewport = ViewportId::from_hash_of("files");
    frame(&context, viewport, 0, &mut false).drop_without_applying_deltas();
    assert!(
        matches!(capture(&context, Some(&old.id)), Err(error) if error.contains("unsupported"))
    );
    let windows = command(&context, None, "windows", &Value::Null).expect("windows");
    assert_eq!(windows[1]["screenshots"], false);
    command(&context, Some(&old.id), "window.focus", &Value::Null).expect("focus");
    command(&context, Some(&old.id), "window.close", &Value::Null).expect("close");
    assert!(!old.live());
    assert!(command(&context, Some(&old.id), "targets", &Value::Null).is_err());
    assert_eq!(
        command(&context, None, "windows", &Value::Null)
            .expect("windows")
            .as_array()
            .expect("list")
            .len(),
        1
    );
    let old_id = old.id.clone();
    drop(old);
    let new = register(&context, &spec()).expect("reopened child");
    assert_ne!(new.id, old_id);
    assert!(capture(&context, Some(&old_id)).is_err());
    let id = new.id.clone();
    drop(new); // The same revocation applies to owner logout or host destruction.
    assert!(command(&context, Some(&id), "targets", &Value::Null).is_err());
    assert!(command(&context, None, "window.close", &Value::Null).is_err());
}

#[test]
fn child_resize_commands_never_resize_the_root() {
    let context = Context::default();
    crate::install(&context);
    context.add_plugin(Driver::default());
    let window = register(&context, &spec()).expect("registered child");
    let viewport = ViewportId::from_hash_of("files");
    frame(&context, ViewportId::ROOT, 0, &mut false).drop_without_applying_deltas();
    frame(&context, viewport, 0, &mut false).drop_without_applying_deltas();
    command(
        &context,
        Some(&window.id),
        "action",
        &json!({"kind":"resize","width":720,"height":640}),
    )
    .expect("resize");
    let output = frame(&context, viewport, 1, &mut false);
    assert!(
        output.viewport_output[&viewport]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::InnerSize(_)))
    );
    assert!(
        output
            .viewport_output
            .get(&ViewportId::ROOT)
            .is_none_or(|output| !output
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::InnerSize(_))))
    );
    output.drop_without_applying_deltas();
}
