//! Browser tools and the run overlay share one semantic tree.
use super::*;

fn context() -> Context {
    let context = Context::default();
    crate::install(&context);
    context.add_plugin(Driver::default());
    context
}

fn start(context: &Context) {
    context
        .plugin::<Driver>()
        .lock()
        .start_steps(
            "developer-controls",
            vec![Step {
                phase: "setup",
                target: "pending-target".into(),
                action: Action::Wait,
                after: 0.1,
            }],
        )
        .unwrap();
}

fn render(context: &Context, tick: u32, events: Vec<Event>, window: bool) {
    let intl = garmin_i18n::Translations::bundled()
        .expect("bundled translations")
        .formatter(garmin_i18n::Language::English)
        .expect("English formatter");
    let mut output = context.run_ui(
        RawInput {
            time: Some(f64::from(tick) / 60.0),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 900.0))),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            show_status(ui.ctx());
            if window {
                crate::developer::show(ui.ctx(), false, &intl);
            } else {
                ui.set_max_width(620.0);
                let state = crate::developer::state(ui.ctx());
                crate::developer::contents(ui, &mut state.lock().expect("panel state"));
            }
        },
    );
    output.textures_delta.clear();
}

#[test]
fn assertion_failures_describe_values_without_debug_wrappers() {
    let context = context();
    render(&context, 0, vec![], false);
    command(
        &context,
        "action",
        &serde_json::json!({
            "kind": "assert_value", "target": "developer.section.automation", "value": "closed"
        }),
    )
    .unwrap();
    render(&context, 1, vec![], false);
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    assert_eq!(
        driver.report().unwrap().failure.as_deref(),
        Some("developer.section.automation: expected 'closed', got 'open'")
    );
}

#[test]
fn section_targets_can_collapse_and_reopen_automation() {
    let context = context();
    for tick in 0..4 {
        render(&context, tick, vec![], false);
    }
    for (start, value) in [(4, "closed"), (34, "open")] {
        command(
            &context,
            "sequence",
            &serde_json::json!([
                {"kind": "click", "target": "developer.section.automation"},
                {"kind": "assert_value", "target": "developer.section.automation", "value": value}
            ]),
        )
        .expect("section sequence");
        for tick in start..start + 30 {
            render(&context, tick, vec![], false);
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().expect("section report");
        assert_eq!(report.state, "passed", "{:?}", report.failure);
        let scenario = lookup(
            driver.tree.as_ref(),
            "automation.scenario.stationary-arrival",
            Rect::EVERYTHING,
        )
        .expect("unique scenario target");
        assert_eq!(scenario.is_some(), value == "open");
    }
}

#[test]
fn section_sequence_waits_for_headers_to_stop_moving() {
    let context = context();
    // Match the native control section from the reported transition.
    crate::developer::state(&context)
        .lock()
        .expect("panel state")
        .native = true;
    for tick in 0..4 {
        render(&context, tick, vec![], false);
    }
    command(
        &context,
        "sequence",
        &serde_json::json!([
            {"kind": "click", "target": "developer.section.control"},
            {"kind": "assert_value", "target": "developer.section.control", "value": "open"},
            {"kind": "click", "target": "developer.section.control"},
            {"kind": "assert_value", "target": "developer.section.control", "value": "closed"},
            {"kind": "click", "target": "developer.section.debug"},
            {"kind": "assert_value", "target": "developer.section.debug", "value": "open"},
            {"kind": "click", "target": "developer.section.debug"},
            {"kind": "assert_value", "target": "developer.section.debug", "value": "closed"}
        ]),
    )
    .expect("section sequence");
    for tick in 4..124 {
        render(&context, tick, vec![], false);
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().expect("section report");
    assert_eq!(report.state, "passed", "{:?}", report.failure);
    assert_eq!(report.completed, 8);
}

#[test]
fn both_stop_controls_capture_mouse_and_touch_while_running_or_paused() {
    for target in ["automation.stop", "developer.automation.stop"] {
        for (paused, touch) in [(false, false), (false, true), (true, false), (true, true)] {
            let context = context();
            start(&context);
            for tick in 0..4 {
                render(&context, tick, vec![], false);
            }
            if paused {
                command(&context, "pause", &serde_json::json!(4.0 / 60.0)).unwrap();
                // Let any input release finish before testing cancellation of a settled pause.
                for tick in 4..6 {
                    render(&context, tick, vec![], false);
                }
            }
            let point = {
                let plugin = context.plugin::<Driver>();
                let driver = plugin.lock();
                assert_eq!(
                    driver.report().unwrap().state,
                    if paused { "paused" } else { "running" }
                );
                assert!(!driver.release);
                lookup(driver.tree.as_ref(), target, Rect::EVERYTHING)
                    .unwrap()
                    .unwrap()
                    .0
                    .center()
            };
            let mut events = Vec::new();
            if touch {
                events.push(Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::Start,
                    pos: point,
                    force: None,
                });
            } else {
                events.push(pointer_button(point, true));
            }
            render(&context, 6, events, false);
            assert_eq!(
                context.plugin::<Driver>().lock().report().unwrap().state,
                "cancelled"
            );
            assert!(
                !context.input(|input| input.pointer.primary_down() || input.pointer.any_click())
            );
            assert!(context.input(|input| {
                input.raw.events.iter().all(|event| {
                    !matches!(event, Event::PointerButton { .. } | Event::Touch { .. })
                })
            }));
            let mut events = Vec::new();
            if touch {
                events.push(Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::End,
                    pos: point,
                    force: None,
                });
            } else {
                events.push(pointer_button(point, false));
            }
            render(&context, 7, events, false);
            assert!(!context.input(|input| input.pointer.any_click()));
        }
    }
}

#[test]
fn escape_cancels_a_settled_pause() {
    let context = context();
    start(&context);
    for tick in 0..4 {
        render(&context, tick, vec![], false);
    }
    command(&context, "pause", &serde_json::json!(4.0 / 60.0)).unwrap();
    render(&context, 4, vec![], false);
    assert!(!context.plugin::<Driver>().lock().release);
    render(
        &context,
        5,
        vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        false,
    );
    assert_eq!(
        context.plugin::<Driver>().lock().report().unwrap().state,
        "cancelled"
    );
    assert!(!context.input(|input| input.key_pressed(egui::Key::Escape)));
}

#[test]
fn automated_click_reaches_a_button_under_the_running_status_overlay() {
    let context = context();
    let mut clicks = 0;
    start(&context);
    for tick in 0..40_u32 {
        context
            .run_ui(
                RawInput {
                    time: Some(f64::from(tick) / 60.0),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 900.0))),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    let response = ui.put(
                        Rect::from_min_size(egui::pos2(1000.0, 64.0), egui::vec2(80.0, 40.0)),
                        egui::Button::new("Import"),
                    );
                    crate::semantics::target(ui, &response, "test.import");
                    clicks += usize::from(response.clicked());
                    show_status(ui.ctx());
                },
            )
            .drop_without_applying_deltas();
        if tick == 3 {
            let overlay = context
                .memory(|memory| memory.area_rect(egui::Id::new("automation-status")))
                .expect("running status overlay");
            assert!(overlay.contains(egui::pos2(1040.0, 84.0)));
            context
                .plugin::<Driver>()
                .lock()
                .cancel("overlay positioned");
            command(
                &context,
                "action",
                &serde_json::json!({"kind": "click", "target": "test.import"}),
            )
            .expect("click under overlay");
        }
    }
    assert_eq!(clicks, 1, "the app must receive the reported click");
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().expect("click report");
    assert_eq!(report.state, "passed", "{:?}", report.failure);
}

#[test]
fn browser_tools_hide_during_automation_and_return_afterwards() {
    let context = context();
    let state = crate::developer::state(&context);
    state.lock().unwrap().open = true;
    let panel_present = || {
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        lookup(
            driver.tree.as_ref(),
            "automation.scenario.stationary-arrival",
            Rect::EVERYTHING,
        )
        .unwrap()
        .is_some()
    };
    for tick in 0..4 {
        render(&context, tick, vec![], true);
    }
    assert!(panel_present());
    start(&context);
    for tick in 4..8 {
        render(&context, tick, vec![], true);
    }
    assert!(context.plugin::<Driver>().lock().running());
    assert!(!panel_present());
    assert!(state.lock().unwrap().open);
    context.plugin::<Driver>().lock().cancel("test complete");
    for tick in 8..12 {
        render(&context, tick, vec![], true);
    }
    assert!(panel_present());
}
