//! Pointer admission follows rendered geometry, including layout animation.
use super::*;

#[test]
fn pointer_actions_wait_for_moving_targets() {
    for action in [
        serde_json::json!({"kind":"click", "target":"moving"}),
        serde_json::json!({"kind":"drag", "target":"moving", "x":0.9, "y":0.5}),
        serde_json::json!({"kind":"scroll", "target":"moving", "delta":20}),
    ] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let mut clicks = 0;
        for tick in 0..90_u32 {
            context
                .run_ui(
                    RawInput {
                        time: Some(f64::from(tick) / 60.0),
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(800.0, 600.0),
                        )),
                        focused: true,
                        ..Default::default()
                    },
                    |ui| {
                        let position =
                            f32::from(u16::try_from(tick.min(12)).expect("bounded tick"));
                        let rect = Rect::from_min_size(
                            egui::pos2(20.0, 20.0 + position * 30.0),
                            egui::vec2(200.0, 24.0),
                        );
                        let response = ui.put(
                            rect,
                            egui::Button::new("Moving target").sense(egui::Sense::click_and_drag()),
                        );
                        crate::semantics::target(ui, &response, "moving");
                        clicks += usize::from(response.clicked());
                        if tick <= 13 {
                            assert!(
                                !ui.input(|input| input.events.iter().any(|event| match event {
                                    Event::PointerMoved(position) =>
                                        input.viewport_rect().contains(*position),
                                    Event::MouseWheel { .. }
                                    | Event::PointerButton { pressed: true, .. } => true,
                                    _ => false,
                                })),
                                "pointer input preceded stable layout at frame {tick}"
                            );
                        }
                    },
                )
                .drop_without_applying_deltas();
            if tick == 0 {
                command(&context, "action", &action).expect("moving target action");
            } else {
                let plugin = context.plugin::<Driver>();
                let driver = plugin.lock();
                if tick <= 13 {
                    let report = driver.report().expect("action report");
                    assert!(
                        report.actions.is_empty(),
                        "action started before stable layout"
                    );
                    assert_eq!(report.input_events, 0);
                }
                if !driver.running() {
                    break;
                }
            }
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().expect("action report");
        assert_eq!(report.state, "passed", "{:?}", report.failure);
        assert_eq!(report.actions.len(), 1);
        assert_eq!(clicks, usize::from(action["kind"] == "click"));
    }
}

#[test]
fn a_target_that_never_settles_fails_without_pointer_input() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    for tick in 0..180_u32 {
        context
            .run_ui(
                RawInput {
                    time: Some(f64::from(tick) / 60.0),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    let y = if tick % 2 == 0 { 20.0 } else { 100.0 };
                    let response = ui.put(
                        Rect::from_min_size(egui::pos2(20.0, y), egui::vec2(200.0, 24.0)),
                        egui::Button::new("Moving forever"),
                    );
                    crate::semantics::target(ui, &response, "moving");
                    assert!(!response.clicked());
                    assert!(!ui.input(|input| input.pointer.primary_down()));
                },
            )
            .drop_without_applying_deltas();
        if tick == 0 {
            command(
                &context,
                "action",
                &serde_json::json!({"kind":"click", "target":"moving"}),
            )
            .expect("moving target action");
        } else if !context.plugin::<Driver>().lock().running() {
            break;
        }
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().expect("action report");
    assert_eq!(report.state, "failed");
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("deadline"))
    );
    assert_eq!(report.input_events, 0);
    assert_eq!(report.completed, 0);
}
