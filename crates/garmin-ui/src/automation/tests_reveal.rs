use super::*;

fn frame(context: &Context, tick: u32, kind: &str, clicked: &mut bool) {
    context
        .run_ui(
            RawInput {
                time: Some(f64::from(tick) / 30.0),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 200.0))),
                focused: true,
                ..Default::default()
            },
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("outer")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.add_space(300.0);
                        if kind == "missing" {
                            return;
                        }
                        for _ in 0..if kind == "duplicate" { 2 } else { 1 } {
                            let response =
                                ui.add_enabled(kind != "disabled", egui::Button::new("Reveal me"));
                            crate::semantics::target(ui, &response, "reveal-target");
                            *clicked |= response.clicked();
                        }
                    });
            },
        )
        .drop_without_applying_deltas();
}

#[test]
fn scroll_into_view_reveals_clipped_targets_before_clicking() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let mut clicked = false;
    frame(&context, 0, "enabled", &mut clicked);
    command(
        &context,
        "sequence",
        &serde_json::json!([
            {"kind":"scroll_into_view", "target":"reveal-target"},
            {"kind":"click", "target":"reveal-target"},
        ]),
    )
    .unwrap();
    for tick in 1..300 {
        frame(&context, tick, "enabled", &mut clicked);
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        if !driver.running() {
            let report = driver.report().unwrap();
            assert_eq!(report.state, "passed", "{:?}", report.failure);
            assert!(clicked);
            assert_eq!(report.actions[0].kind, "scroll_into_view");
            return;
        }
    }
    panic!("scroll target never became clickable");
}

#[test]
fn scroll_into_view_rejects_missing_disabled_and_duplicate_targets() {
    for kind in ["missing", "disabled", "duplicate"] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        command(
            &context,
            "action",
            &serde_json::json!({
                "kind":"scroll_into_view", "target":"reveal-target",
            }),
        )
        .unwrap();
        let mut clicked = false;
        for tick in 0..1000 {
            frame(&context, tick, kind, &mut clicked);
            if !context.plugin::<Driver>().lock().running() {
                break;
            }
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().unwrap();
        assert_eq!(report.state, "failed", "{kind}: {:?}", report.failure);
        assert!(
            report
                .failure
                .as_deref()
                .unwrap()
                .contains(if kind == "duplicate" {
                    "ambiguous semantic target"
                } else {
                    "could not scroll target into view"
                })
        );
        assert!(!clicked);
    }
}
