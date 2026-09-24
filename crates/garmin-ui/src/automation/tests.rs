use super::*;

#[test]
fn menu_selection_queues_the_named_scenario_without_starting_a_second_runner() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let render = |events| {
        let mut output = context.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                egui::Popup::open_id(ui.ctx(), ui.make_persistent_id("automation-menu"));
                let profile = Rect::from_min_size(egui::pos2(600.0, 0.0), egui::vec2(120.0, 32.0));
                let rect = header_rect(ui, profile, 0.0);
                assert!((profile.left() - rect.right() - 2.0).abs() < f32::EPSILON);
                assert!(rect.width() > 100.0);
                header_button(ui, rect);
            },
        );
        output.textures_delta.clear();
    };
    render(vec![]);
    render(vec![]);
    let point = {
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        for name in SCENARIOS {
            assert!(
                lookup(
                    driver.tree.as_ref(),
                    &format!("automation.scenario.{name}"),
                    Rect::EVERYTHING
                )
                .unwrap()
                .is_some()
            );
        }
        lookup(
            driver.tree.as_ref(),
            "automation.scenario.activity-smoke",
            Rect::EVERYTHING,
        )
        .unwrap()
        .unwrap()
        .0
        .center()
    };
    render(vec![
        Event::PointerMoved(point),
        pointer_button(point, true),
    ]);
    render(vec![pointer_button(point, false)]);
    let plugin = context.plugin::<Driver>();
    let mut driver = plugin.lock();
    assert_eq!(driver.take_launch_request(), Some("activity-smoke"));
    assert!(!driver.running());
}

#[test]
fn scenario_buttons_show_a_pointer_only_when_enabled() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let render = |running, events| {
        let mut output = context.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                scenario_menu(ui, running, None);
            },
        );
        output.textures_delta.clear();
        output.platform_output.cursor_icon
    };
    render(false, vec![]);
    render(false, vec![]);
    let points = {
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        SCENARIOS
            .iter()
            .map(|name| {
                lookup(
                    driver.tree.as_ref(),
                    &format!("automation.scenario.{name}"),
                    Rect::EVERYTHING,
                )
                .unwrap()
                .unwrap()
                .0
                .center()
            })
            .collect::<Vec<_>>()
    };
    for point in points {
        assert_eq!(
            render(false, vec![Event::PointerMoved(point)]),
            egui::CursorIcon::PointingHand
        );
        assert_eq!(
            render(true, vec![Event::PointerMoved(point)]),
            egui::CursorIcon::Default
        );
        render(false, vec![]);
    }
}

#[test]
fn an_active_profile_returns_to_the_chooser_using_normal_clicks() {
    for initially_open in [false, true] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let mut signed_in = true;
        let mut expanded = initially_open;
        for tick in 0..200 {
            let mut output = context.run_ui(
                RawInput {
                    time: Some(f64::from(tick) / 60.0),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    if signed_in {
                        let response = ui.button("Profile");
                        crate::semantics::target(ui, &response, "profile.toggle");
                        if response.clicked() {
                            expanded = !expanded;
                        }
                        if expanded {
                            let response = ui.button("Log out");
                            crate::semantics::target(ui, &response, "profile.logout");
                            if response.clicked() {
                                signed_in = false;
                            }
                        }
                    } else {
                        let response = ui.button("Alex Rider");
                        crate::semantics::target(ui, &response, "profile.0");
                    }
                },
            );
            output.textures_delta.clear();
            if tick == 0 {
                let plugin = context.plugin::<Driver>();
                let mut driver = plugin.lock();
                driver.start("stationary-arrival").unwrap();
                let run = driver.run.as_mut().unwrap();
                let reset = scenarios::return_to_chooser(initially_open);
                assert_eq!(run.steps[0].target, reset[0].target);
                run.steps = reset;
                run.report.total = run.steps.len();
            } else if !context.plugin::<Driver>().lock().running() {
                break;
            }
        }
        assert!(!signed_in);
        assert_eq!(
            context.plugin::<Driver>().lock().report().unwrap().state,
            "passed"
        );
    }
}

pub(super) fn frame(
    context: &Context,
    time: f64,
    events: Vec<Event>,
    value: &str,
) -> egui::FullOutput {
    frame_at_scale(context, time, events, value, 1.0)
}

fn frame_at_scale(
    context: &Context,
    time: f64,
    events: Vec<Event>,
    value: &str,
    scale: f32,
) -> egui::FullOutput {
    let size = context
        .data(|data| data.get_temp::<egui::Vec2>(egui::Id::new("test-viewport-size")))
        .unwrap_or(egui::vec2(1000.0, 900.0));
    let mut input = RawInput {
        time: Some(time),
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        focused: true,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(scale);
    let mut output = context.run_ui(input, |ui| {
        show_status(ui.ctx());
        let responsive = context
            .plugin::<Driver>()
            .lock()
            .report()
            .is_some_and(|report| report.scenario == "responsive-layout");
        if responsive {
            responsive_controls(ui, size);
        }
        for target in [
            "profile.0",
            "activity.0",
            "map",
            "map.fit",
            "playback.toggle",
            "playback.speed.2",
            "playback.speed",
            "lap.0",
            "map.full-activity",
            "chart.0",
            "activity.1",
            "activity.viewer",
            "map.empty",
        ] {
            if responsive
                && size.x < 1000.0
                && target == "activity.0"
                && !ui
                    .data(|data| data.get_temp::<bool>(egui::Id::new("activity.list.toggle")))
                    .unwrap_or(false)
            {
                continue;
            }
            let response = ui.add_sized([200.0, 40.0], egui::Button::new("Translated label"));
            let playing_id = egui::Id::new("test-playing");
            let mut playing = ui
                .data(|data| data.get_temp::<bool>(playing_id))
                .unwrap_or(false);
            if target == "playback.toggle" && response.clicked() {
                playing = !playing;
                ui.data_mut(|data| data.insert_temp(playing_id, playing));
            }
            crate::semantics::target(ui, &response, target);
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_value(match target {
                    "playback.speed" => "2×",
                    "playback.toggle" => {
                        if playing {
                            "playing"
                        } else {
                            "stopped"
                        }
                    }
                    "activity.0" | "activity.1" => "selected",
                    "map.empty" => "empty",
                    _ => value,
                });
            });
        }
    });
    for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
        if let egui::ViewportCommand::InnerSize(size) = command {
            context.data_mut(|data| data.insert_temp(egui::Id::new("test-viewport-size"), *size));
        }
    }
    output.textures_delta.clear();
    output
}

fn responsive_controls(ui: &mut egui::Ui, size: egui::Vec2) {
    let response = ui.put(
        Rect::from_min_size(egui::pos2(size.x - 100.0, 0.0), egui::vec2(90.0, 24.0)),
        egui::Button::new("Profile"),
    );
    crate::semantics::target(ui, &response, "profile.toggle");
    if size.x >= 1000.0 {
        return;
    }
    for (target, x) in [
        ("activity.list.toggle", 230.0),
        ("activity.details.toggle", 380.0),
    ] {
        let id = egui::Id::new(target);
        let mut open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let response = ui.put(
            Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(130.0, 24.0)),
            egui::Button::new(target),
        );
        if response.clicked() {
            open = !open;
            ui.data_mut(|data| data.insert_temp(id, open));
        }
        crate::semantics::target(ui, &response, target);
        crate::semantics::value(&response, if open { "open" } else { "closed" });
    }
}

#[test]
fn secondary_window_frames_do_not_drive_or_replace_the_root_workload() {
    for name in SCENARIOS {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let _ = frame(&context, 0.0, vec![], "ready");
        context.plugin::<Driver>().lock().start(name).unwrap();
        let secondary = egui::ViewportId::from_hash_of("developer-tools-window");
        for tick in 1..2000 {
            let now = f64::from(tick) / 60.0;
            let _ = frame(&context, now, vec![], "ready");
            let before =
                serde_json::to_string(&context.plugin::<Driver>().lock().report()).unwrap();
            let event = Event::PointerMoved(egui::pos2(20.0, 20.0));
            let mut input = RawInput {
                viewport_id: secondary,
                time: Some(now + 1.0 / 120.0),
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(if tick % 2 == 0 { 680.0 } else { 720.0 }, 640.0),
                )),
                events: vec![event.clone()],
                ..Default::default()
            };
            input.viewports.insert(
                secondary,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    native_pixels_per_point: Some(2.0),
                    ..Default::default()
                },
            );
            let mut received = Vec::new();
            context
                .run_ui(input, |ui| {
                    received = ui.input(|input| input.raw.events.clone());
                    let response = ui.button("Secondary window target");
                    crate::semantics::target(ui, &response, "secondary.only");
                })
                .drop_without_applying_deltas();
            let plugin = context.plugin::<Driver>();
            let driver = plugin.lock();
            assert_eq!(
                received,
                vec![event],
                "secondary input must remain untouched"
            );
            assert_eq!(serde_json::to_string(&driver.report()).unwrap(), before);
            assert!(
                lookup(driver.tree.as_ref(), "secondary.only", Rect::EVERYTHING)
                    .unwrap()
                    .is_none()
            );
            if !driver.running() {
                assert_eq!(
                    driver.report().unwrap().state,
                    "passed",
                    "{name}: {:?}",
                    driver.report().unwrap().failure
                );
                break;
            }
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().unwrap();
        assert_eq!(report.state, "passed", "{name}: {:?}", report.failure);
        let expected = if *name == "responsive-layout" {
            [1100.0, 720.0]
        } else {
            [1000.0, 900.0]
        };
        assert!((report.viewport[0] - expected[0]).abs() < f32::EPSILON);
        assert!((report.viewport[1] - expected[1]).abs() < f32::EPSILON);
        assert!((report.pixels_per_point - 1.0).abs() < f32::EPSILON);
        assert_eq!(report.completed, report.total);
    }
}

#[test]
fn built_in_workloads_use_the_same_plugin_and_complete_every_action() {
    for name in SCENARIOS {
        let context = Context::default();
        context.add_plugin(Driver::default());
        context.plugin::<Driver>().lock().start(name).unwrap();
        for tick in 0..2000 {
            let _ = frame(&context, f64::from(tick) / 60.0, vec![], "ready");
            if !context.plugin::<Driver>().lock().running() {
                break;
            }
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().unwrap();
        assert_eq!(report.state, "passed", "{name}: {:?}", report.failure);
        assert_eq!(report.completed, report.total);
        assert_eq!(report.actions.len(), report.total);
        if *name == "warm-interaction" {
            assert!(report.input_events > 170);
        }
    }
}

#[test]
fn hidpi_clicks_use_logical_bounds_and_reach_the_target() {
    for scale in [1.0, 1.5, 2.0] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let target = Rect::from_min_size(egui::pos2(350.0, 250.0), egui::vec2(200.0, 50.0));
        let mut clicked = false;
        for tick in 0..2000 {
            let mut input = RawInput {
                time: Some(f64::from(tick) / 60.0),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                focused: true,
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(scale);
            let mut output = context.run_ui(input, |ui| {
                if clicked {
                    let response = ui.button("Activity");
                    crate::semantics::target(ui, &response, "activity.0");
                } else {
                    let response = ui.put(target, egui::Button::new("Profile"));
                    crate::semantics::target(ui, &response, "profile.0");
                    clicked = response.clicked();
                }
            });
            output.textures_delta.clear();
            if tick == 0 {
                let plugin = context.plugin::<Driver>();
                let mut driver = plugin.lock();
                driver.start("stationary-arrival").unwrap();
                let run = driver.run.as_mut().unwrap();
                run.steps.truncate(3);
                run.report.total = run.steps.len();
            } else if !context.plugin::<Driver>().lock().running() {
                break;
            }
        }
        assert!(clicked, "profile click missed at scale {scale}");
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().unwrap();
        assert_eq!(
            report.state, "passed",
            "scale {scale}: {:?}",
            report.failure
        );
        let click = &report.actions[1];
        for (actual, expected) in click
            .target_bounds
            .into_iter()
            .zip([350.0, 250.0, 550.0, 300.0])
        {
            assert!((actual - expected).abs() < 0.001);
        }
        let [pointer_x, pointer_y] = click.pointer_position.unwrap();
        assert!(egui::pos2(pointer_x, pointer_y).distance(target.center()) < 0.001);
    }
}

#[test]
fn target_highlight_does_not_capture_clicks() {
    let context = Context::default();
    let target = Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(200.0, 50.0));
    let attempt = ActionTiming {
        phase: "setup".into(),
        kind: "click".into(),
        target: "profile.0".into(),
        scheduled_seconds: 0.0,
        actual_seconds: 0.0,
        lateness_seconds: 0.0,
        target_bounds: [100.0, 100.0, 300.0, 150.0],
        pointer_position: Some([200.0, 125.0]),
    };
    let mut clicked = false;
    for events in [
        vec![],
        vec![
            Event::PointerMoved(target.center()),
            pointer_button(target.center(), true),
        ],
        vec![pointer_button(target.center(), false)],
    ] {
        let mut output = context.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                clicked |= ui.put(target, egui::Button::new("Profile")).clicked();
                attempt.highlight(ui.ctx());
            },
        );
        output.textures_delta.clear();
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::epaint::Shape::Circle(_)))
        );
    }
    assert!(clicked);
}

#[test]
fn duplicate_and_unknown_starts_do_not_replace_the_active_workload() {
    let mut driver = Driver::default();
    assert!(driver.start("arbitrary code").is_err());
    driver.start("activity-smoke").unwrap();
    assert!(driver.start("stationary-arrival").is_err());
    assert_eq!(driver.report().unwrap().scenario, "activity-smoke");
}

#[test]
fn escape_cancels_without_completing_a_synthetic_click_or_drag() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.0, vec![], "ready");
    let _ = frame(&context, 0.1, vec![], "ready");
    let _ = frame(&context, 0.2, vec![], "ready");
    assert!(context.plugin::<Driver>().lock().held.is_some());
    let _ = frame(
        &context,
        0.3,
        vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        "ready",
    );
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    assert_eq!(driver.report().unwrap().state, "cancelled");
    assert!(driver.held.is_none());
    assert!(!context.input(|input| input.pointer.primary_down()));
    assert!(!context.input(|input| input.pointer.primary_down()));
    assert!(!context.input(|input| input.pointer.any_click()));
    assert_eq!(
        context.input(|input| input.pointer.delta()),
        egui::Vec2::ZERO
    );
}

#[test]
fn hovering_status_widgets_does_not_reenter_a_locked_plugin() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.0, vec![], "ready");
    for tick in 1..4 {
        let _ = frame(
            &context,
            f64::from(tick) / 60.0,
            vec![Event::PointerMoved(egui::pos2(850.0, 70.0))],
            "ready",
        );
    }
    assert_eq!(
        context.plugin::<Driver>().lock().report().unwrap().state,
        "running"
    );
}

#[test]
fn stop_button_captures_mouse_and_touch_without_clicking_through() {
    for (touch, scale) in [(false, 1.0), (true, 1.0), (false, 2.0), (true, 2.0)] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let render = |time, events| frame_at_scale(&context, time, events, "ready", scale);
        let _ = render(0.0, vec![]);
        context
            .plugin::<Driver>()
            .lock()
            .start("stationary-arrival")
            .unwrap();
        for tick in 0..2 {
            let _ = render(f64::from(tick) / 10.0, vec![]);
        }
        let point = {
            let plugin = context.plugin::<Driver>();
            let driver = plugin.lock();
            assert!(driver.held.is_some());
            lookup(driver.tree.as_ref(), "automation.stop", Rect::EVERYTHING)
                .unwrap()
                .unwrap()
                .0
                .center()
        };
        let event = if touch {
            Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(0),
                phase: egui::TouchPhase::Start,
                pos: point,
                force: None,
            }
        } else {
            pointer_button(point, true)
        };
        let _ = render(0.3, vec![event]);
        assert_eq!(
            context.plugin::<Driver>().lock().report().unwrap().state,
            "cancelled"
        );
        assert!(!context.input(|input| input.pointer.primary_down() || input.pointer.any_click()));
        assert_eq!(
            context.input(|input| input.pointer.delta()),
            egui::Vec2::ZERO
        );
        let _ = render(0.4, vec![pointer_button(point, false)]);
        assert!(!context.input(|input| input.pointer.any_click()));
        let _ = render(0.5, vec![Event::Text("input resumed".into())]);
        assert!(context.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, Event::Text(_)))
        }));
    }
}

#[test]
fn starting_a_run_releases_inherited_physical_keys_and_buttons() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let point = egui::pos2(100.0, 100.0);
    let _ = frame(
        &context,
        0.0,
        vec![
            Event::PointerMoved(point),
            pointer_button(point, true),
            Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ],
        "ready",
    );
    assert!(context.input(|input| input.pointer.primary_down() && input.key_down(egui::Key::A)));
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.1, vec![], "ready");
    context.input(|input| {
        assert!(!input.pointer.primary_down());
        assert!(!input.pointer.any_click());
        assert!(!input.key_down(egui::Key::A));
        assert_eq!(input.pointer.delta(), egui::Vec2::ZERO);
    });
    assert!(context.plugin::<Driver>().lock().running());
}

#[test]
fn real_interaction_is_blocked_while_the_synthetic_workload_completes() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("warm-interaction")
        .unwrap();
    for tick in 0..2000 {
        let events = vec![
            Event::PointerMoved(Pos2::ZERO),
            pointer_button(Pos2::ZERO, true),
            pointer_button(Pos2::ZERO, false),
            Event::Text("blocked".into()),
            Event::Paste("blocked".into()),
            Event::ModifiersChanged(Modifiers::CTRL),
            Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::CTRL,
            },
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 999.0),
                modifiers: Modifiers::CTRL,
                phase: egui::TouchPhase::Move,
            },
            Event::Zoom(9.0),
            Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(0),
                phase: egui::TouchPhase::Start,
                pos: Pos2::ZERO,
                force: None,
            },
        ];
        let _ = frame(&context, f64::from(tick) / 60.0, events, "ready");
        context.input(|input| {
            assert!(!input.modifiers.ctrl);
            assert!(!input.key_down(egui::Key::A));
            assert!(!input.events.iter().any(|event| matches!(
                event,
                Event::Text(_) | Event::Paste(_) | Event::Touch { .. } | Event::Zoom(_)
            )));
        });
        if !context.plugin::<Driver>().lock().running() {
            break;
        }
    }
    assert_eq!(
        context.plugin::<Driver>().lock().report().unwrap().state,
        "passed"
    );
}

#[test]
fn preparation_failure_and_timeout_cannot_pass() {
    for value in ["failed:tile decoder", "pending"] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        context
            .plugin::<Driver>()
            .lock()
            .start("stationary-arrival")
            .unwrap();
        for tick in 0..400 {
            let _ = frame(&context, f64::from(tick) / 10.0, vec![], value);
        }
        assert_eq!(
            context.plugin::<Driver>().lock().report().unwrap().state,
            "failed"
        );
    }
}

#[test]
fn deadlines_fail_without_skipping_actions() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.0, vec![], "ready");
    let _ = frame(&context, 0.1, vec![], "ready");
    let _ = frame(&context, 5.0, vec![], "ready");
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    assert_eq!(driver.report().unwrap().state, "failed");
    assert_eq!(driver.report().unwrap().completed, 1);
}

#[test]
fn semantic_lookup_rejects_duplicates_and_clipped_targets() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let mut output = context.run_ui(RawInput::default(), |ui| {
        for _ in 0..2 {
            let response = ui.button("Any translation");
            crate::semantics::target(ui, &response, "duplicate");
        }
        let response = ui.button("Clipped");
        ui.set_clip_rect(Rect::NOTHING);
        crate::semantics::target(ui, &response, "clipped");
        ui.set_clip_rect(Rect::from_min_max(
            egui::pos2(0.0, 200.0),
            egui::pos2(400.0, 300.0),
        ));
        let response = ui.button("Finite clip");
        crate::semantics::target(ui, &response, "finite-clip");
    });
    output.textures_delta.clear();
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    assert!(lookup(driver.tree.as_ref(), "duplicate", Rect::EVERYTHING).is_err());
    assert!(
        lookup(driver.tree.as_ref(), "clipped", Rect::EVERYTHING)
            .unwrap()
            .is_none()
    );
    assert!(
        lookup(driver.tree.as_ref(), "missing", Rect::EVERYTHING)
            .unwrap()
            .is_none()
    );
    assert!(
        lookup(driver.tree.as_ref(), "finite-clip", Rect::EVERYTHING)
            .unwrap()
            .is_none()
    );
}

#[test]
fn author_ids_survive_translation_and_bounds_follow_layout() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let mut previous = None;
    for (label, offset) in [("Play", 0.0), ("Přehrát", 60.0)] {
        let mut output = context.run_ui(RawInput::default(), |ui| {
            ui.add_space(offset);
            let response = ui.button(label);
            crate::semantics::target(ui, &response, "play");
        });
        output.textures_delta.clear();
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let rect = lookup(driver.tree.as_ref(), "play", Rect::EVERYTHING)
            .unwrap()
            .unwrap()
            .0;
        if let Some(top) = previous {
            assert!(rect.top() > top + 50.0);
        }
        previous = Some(rect.top());
    }
}

#[test]
fn explicit_cancel_releases_held_input_even_without_focus() {
    for focused in [true, false] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        context
            .plugin::<Driver>()
            .lock()
            .start("stationary-arrival")
            .unwrap();
        for time in [0.0, 0.1, 0.2] {
            let _ = frame(&context, time, vec![], "ready");
        }
        context.plugin::<Driver>().lock().cancel("API cancellation");
        let mut input = RawInput {
            time: Some(0.3),
            focused,
            ..Default::default()
        };
        egui::plugin::Plugin::input_hook(
            &mut *context.plugin::<Driver>().lock(),
            &context,
            &mut input,
        );
        assert!(
            input
                .events
                .iter()
                .any(|event| matches!(event, Event::PointerButton { pressed: false, .. }))
        );
        assert_eq!(
            context.plugin::<Driver>().lock().report().unwrap().state,
            "cancelled"
        );
    }
}

#[test]
fn focus_loss_keeps_the_workload_running() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.0, vec![], "ready");
    let mut input = RawInput {
        time: Some(0.1),
        focused: false,
        ..Default::default()
    };
    egui::plugin::Plugin::input_hook(
        &mut *context.plugin::<Driver>().lock(),
        &context,
        &mut input,
    );
    assert!(context.plugin::<Driver>().lock().running());
}

#[test]
fn individual_actions_use_semantic_targets_and_reject_overlapping_work() {
    let context = Context::default();
    assert!(command(&context, "targets", &serde_json::Value::Null).is_err());
    context.add_plugin(Driver::default());
    let _ = frame(&context, 0.0, vec![], "ready");
    let targets = command(&context, "targets", &serde_json::Value::Null).unwrap();
    assert!(
        targets
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["id"] == "profile.0")
    );
    let action = serde_json::json!({"kind":"click", "target":"profile.0"});
    command(&context, "action", &action).unwrap();
    assert!(command(&context, "action", &action).is_err());
    for time in [0.1, 0.2, 0.3] {
        let _ = frame(&context, time, vec![], "ready");
    }
    let result = command(&context, "result", &serde_json::Value::Null).unwrap();
    assert_eq!(result["state"], "passed");
    assert_eq!(result["completed"], 1);
    assert!(
        command(
            &context,
            "action",
            &serde_json::json!({"kind":"click", "target":"missing"})
        )
        .is_err()
    );
}

#[test]
fn hidden_pause_releases_a_click_and_excludes_suspended_time() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    for time in [0.0, 0.1, 0.2] {
        let _ = frame(&context, time, vec![], "ready");
    }
    command(&context, "pause", &serde_json::json!(0.2)).unwrap();
    assert_eq!(
        context.plugin::<Driver>().lock().report().unwrap().state,
        "paused"
    );
    assert!(command(&context, "start", &serde_json::json!("stationary-arrival")).is_err());
    command(&context, "resume", &serde_json::json!(200.2)).unwrap();
    let _ = frame(&context, 200.2, vec![], "ready");
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "running", "{:?}", report.failure);
    assert_eq!(report.pauses.len(), 1);
    assert!(!report.performance_eligible);
    assert!(report.elapsed_seconds < 1.0);
    assert!(driver.held.is_none());
    assert!(!context.input(|input| input.pointer.primary_down()));
}

#[test]
fn resizing_and_dpi_changes_reanchor_clicks_and_drags() {
    for (action, change_at) in [
        (Action::Click, 1),
        (Action::Click, 3),
        (Action::Drag { x: 0.9, y: 0.8 }, 1),
        (Action::Drag { x: 0.9, y: 0.8 }, 8),
    ] {
        for (resize, scale) in [(true, 1.0), (false, 2.0), (true, 2.0)] {
            check_layout_change(&action, change_at, resize, scale);
        }
    }
}

fn check_layout_change(action: &Action, change_at: usize, resize: bool, scale: f32) {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start_steps(
            "resize",
            vec![
                Step {
                    phase: "test",
                    target: "target".into(),
                    action: Action::Wait,
                    after: 0.0,
                },
                Step {
                    phase: "test",
                    target: "target".into(),
                    action: action.clone(),
                    after: 0.0,
                },
            ],
        )
        .unwrap();
    let mut clicks = 0;
    let mut latest_target = Rect::NOTHING;
    for tick in 0..150_u32 {
        let changed = usize::try_from(tick).unwrap() >= change_at;
        let size = if changed && resize {
            egui::vec2(600.0, 500.0)
        } else {
            egui::vec2(1000.0, 900.0)
        };
        let mut input = RawInput {
            time: Some(f64::from(tick) / 60.0),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            focused: true,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(if changed { scale } else { 1.0 });
        context
            .run_ui(input, |ui| {
                latest_target = Rect::from_min_size(
                    (size - egui::vec2(240.0, 160.0)).to_pos2(),
                    egui::vec2(200.0, 100.0),
                );
                let response = ui.put(
                    latest_target,
                    egui::Button::new("Target").sense(egui::Sense::click_and_drag()),
                );
                crate::semantics::target(ui, &response, "target");
                clicks += usize::from(response.clicked());
            })
            .drop_without_applying_deltas();
        if usize::try_from(tick).unwrap() == change_at {
            assert!(!context.input(|input| input.pointer.primary_down()));
            assert_eq!(clicks, 0, "releasing during resize must not click through");
        }
        if !context.plugin::<Driver>().lock().running() {
            break;
        }
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "passed", "{:?}", report.failure);
    assert!(!report.performance_eligible);
    assert_eq!(report.viewport_changes.len(), 1);
    assert_eq!(report.completed, 2);
    assert_eq!(report.actions.len(), 2);
    assert_eq!(
        report.interrupted_attempts.len(),
        usize::from(change_at > 1)
    );
    assert!(driver.held.is_none());
    assert!(!context.input(|input| input.pointer.primary_down()));
    let pointer = report.actions.last().unwrap().pointer_position.unwrap();
    let expected = if matches!(action, Action::Click) {
        latest_target.center()
    } else {
        latest_target.min + latest_target.size() * egui::vec2(0.9, 0.8)
    };
    assert!((egui::pos2(pointer[0], pointer[1]) - expected).length() < 0.1);
    assert_eq!(clicks, usize::from(matches!(action, Action::Click)));
}

#[test]
fn resized_stationary_observation_waits_for_readiness_and_restarts_its_interval() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    let _ = frame(&context, 0.0, vec![], "ready");
    context
        .plugin::<Driver>()
        .lock()
        .start_steps(
            "observe-resize",
            vec![Step {
                phase: "stationary",
                target: "map".into(),
                action: Action::Observe(0.2),
                after: 0.0,
            }],
        )
        .unwrap();
    for tick in 1..60 {
        let _ = frame_at_scale(
            &context,
            f64::from(tick) / 60.0,
            vec![],
            if (3..10).contains(&tick) {
                "loading"
            } else {
                "ready"
            },
            if tick >= 3 { 2.0 } else { 1.0 },
        );
        if !context.plugin::<Driver>().lock().running() {
            break;
        }
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "passed", "{:?}", report.failure);
    assert_eq!(report.viewport_changes.len(), 1);
    assert!(!report.performance_eligible);
    assert_eq!(report.actions.len(), 1);
    assert!(report.elapsed_seconds >= 0.35);
}

#[test]
fn terminal_reports_stop_accumulating_overhead() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    context
        .plugin::<Driver>()
        .lock()
        .start("stationary-arrival")
        .unwrap();
    let _ = frame(&context, 0.0, vec![], "ready");
    context
        .plugin::<Driver>()
        .lock()
        .cancel("test cancellation");
    let before = serde_json::to_string(&context.plugin::<Driver>().lock().report()).unwrap();
    assert_eq!(
        context.plugin::<Driver>().lock().report().unwrap().state,
        "cancelled"
    );
    let _ = frame(&context, 0.2, vec![], "ready");
    assert_eq!(
        serde_json::to_string(&context.plugin::<Driver>().lock().report()).unwrap(),
        before
    );
}
