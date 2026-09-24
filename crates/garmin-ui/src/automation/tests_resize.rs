use super::*;

fn canvas_adapter(context: &Context, command: ResizeCommand) -> Result<(), String> {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<ResizeCommand>>(egui::Id::new("canvas-commands"))
            .push(command);
    });
    if context
        .data(|data| data.get_temp::<bool>(egui::Id::new("canvas-unavailable")))
        .unwrap_or(false)
    {
        return Err("canvas unavailable".into());
    }
    let size = match command {
        ResizeCommand::Save => return Ok(()),
        ResizeCommand::Set(size) | ResizeCommand::Restore(size) => size,
    };
    context.data_mut(|data| {
        let requests = data.get_temp_mut_or_default::<Vec<[f32; 2]>>(egui::Id::new("canvas-sizes"));
        requests.push(size);
    });
    Ok(())
}

#[test]
fn scenarios_restore_the_original_viewport_on_pass_failure_and_paused_cancellation() {
    for canvas in [false, true] {
        for outcome in ["passed", "failed", "cancelled"] {
            let context = Context::default();
            context.add_plugin(if canvas {
                Driver::default().with_resize_handler(canvas_adapter)
            } else {
                Driver::default()
            });
            context
                .plugin::<Driver>()
                .lock()
                .start("responsive-layout")
                .unwrap();
            let mut requests = Vec::new();
            for tick in 0..1000 {
                if outcome == "cancelled" && tick == 1 {
                    command(&context, "pause", &serde_json::json!(1.0 / 60.0))
                        .expect("pause the active scenario");
                    let plugin = context.plugin::<Driver>();
                    let driver = plugin.lock();
                    assert!(driver.viewport_restore.is_some());
                }
                if outcome == "cancelled" && tick == 2 {
                    let plugin = context.plugin::<Driver>();
                    let mut driver = plugin.lock();
                    assert!(
                        driver.viewport_restore.is_some(),
                        "pause must not restore sizing"
                    );
                    driver.cancel("cancelled while paused");
                    assert!(
                        driver.start("stationary-arrival").is_err(),
                        "cleanup is pending"
                    );
                }
                let output = tests::frame(
                    &context,
                    f64::from(tick) / 60.0,
                    vec![],
                    if outcome == "failed" {
                        "failed: map unavailable"
                    } else {
                        "ready"
                    },
                );
                if canvas {
                    requests = context
                        .data(|data| data.get_temp::<Vec<[f32; 2]>>(egui::Id::new("canvas-sizes")))
                        .unwrap_or_default();
                    if let Some(size) = requests.last() {
                        context.data_mut(|data| {
                            data.insert_temp(
                                egui::Id::new("test-viewport-size"),
                                egui::Vec2::from(*size),
                            );
                        });
                    }
                } else {
                    for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
                        if let egui::ViewportCommand::InnerSize(size) = command {
                            requests.push([size.x, size.y]);
                        }
                    }
                }
                let plugin = context.plugin::<Driver>();
                let driver = plugin.lock();
                if !driver.running() && driver.paused_at.is_none() {
                    break;
                }
            }
            let plugin = context.plugin::<Driver>();
            let driver = plugin.lock();
            let report = driver.report().unwrap();
            assert_eq!(report.state, outcome, "{:?}", report.failure);
            let size = requests.last().expect("restored viewport request");
            assert!((size[0] - 1000.0).abs() < f32::EPSILON);
            assert!((size[1] - 900.0).abs() < f32::EPSILON);
            assert!(requests.len() >= 2, "scenario resized before cleanup");
            assert!(driver.viewport_restore.is_none());
            if outcome == "passed" {
                assert!((report.viewport[0] - 1100.0).abs() < f32::EPSILON);
                assert_eq!(report.completed, report.total);
            }
            if canvas {
                let commands = context
                    .data(|data| {
                        data.get_temp::<Vec<ResizeCommand>>(egui::Id::new("canvas-commands"))
                    })
                    .unwrap();
                assert!(matches!(commands.first(), Some(ResizeCommand::Save)));
                assert!(matches!(commands.last(), Some(ResizeCommand::Restore(_))));
                assert_eq!(commands.len(), requests.len() + 1, "save once per run");
            }
        }
    }
}

#[test]
fn viewport_restore_errors_preserve_the_original_failure() {
    let context = Context::default();
    context.add_plugin(Driver::default().with_resize_handler(|context, command| {
        if matches!(command, ResizeCommand::Restore(_)) {
            Err("canvas removed".into())
        } else {
            canvas_adapter(context, command)
        }
    }));
    context
        .plugin::<Driver>()
        .lock()
        .start("responsive-layout")
        .unwrap();
    let _ = tests::frame(&context, 0.0, vec![], "ready");
    context.plugin::<Driver>().lock().cancel("user stopped");
    let _ = tests::frame(&context, 0.1, vec![], "ready");
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "cancelled");
    let failure = report.failure.as_deref().unwrap();
    assert!(failure.contains("user stopped"));
    assert!(failure.contains("Could not restore automation viewport: canvas removed"));
}

#[test]
fn resize_adapter_errors_end_the_sequence() {
    let context = Context::default();
    context.add_plugin(Driver::default().with_resize_handler(canvas_adapter));
    context.data_mut(|data| data.insert_temp(egui::Id::new("canvas-unavailable"), true));
    command(
        &context,
        "action",
        &serde_json::json!({"kind":"resize", "width":720, "height":480}),
    )
    .unwrap();
    let _ = tests::frame(&context, 0.0, vec![], "ready");
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    assert_eq!(driver.report().unwrap().state, "failed");
    assert_eq!(
        driver.report().unwrap().failure.as_deref(),
        Some("canvas unavailable")
    );
}

#[test]
fn responsive_sequence_preserves_state_and_uses_controls_in_both_layouts() {
    for canvas in [false, true] {
        let context = Context::default();
        let driver = if canvas {
            Driver::default().with_resize_handler(canvas_adapter)
        } else {
            Driver::default()
        };
        context.add_plugin(driver);
        command(
            &context,
            "sequence",
            &serde_json::json!([
                {"kind":"wait", "target":"setting"},
                {"kind":"click", "target":"setting"},
                {"kind":"assert_value", "target":"setting", "value":"on"},
                {"kind":"resize", "width":720, "height":480},
                {"kind":"assert_available", "target":"setting"},
                {"kind":"assert_value", "target":"setting", "value":"on"},
                {"kind":"click", "target":"setting"},
                {"kind":"assert_value", "target":"setting", "value":"off"},
                {"kind":"resize", "width":1100, "height":720},
                {"kind":"assert_available", "target":"setting"},
                {"kind":"assert_value", "target":"setting", "value":"off"},
            ]),
        )
        .unwrap();
        let mut size = egui::vec2(1100.0, 720.0);
        let mut checked = false;
        let mut requests = Vec::new();
        for tick in 0..300 {
            let mut output = context.run_ui(
                RawInput {
                    time: Some(f64::from(tick) / 60.0),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    let position = if size.x < 800.0 {
                        egui::pos2(30.0, 100.0)
                    } else {
                        egui::pos2(850.0, 30.0)
                    };
                    let rect = Rect::from_min_size(position, egui::vec2(180.0, 32.0));
                    let response = ui.put(
                        rect,
                        egui::Checkbox::new(&mut checked, "Persistent setting"),
                    );
                    crate::semantics::target(ui, &response, "setting");
                    crate::semantics::value(&response, if checked { "on" } else { "off" });
                },
            );
            if canvas {
                requests = context
                    .data(|data| data.get_temp::<Vec<[f32; 2]>>(egui::Id::new("canvas-sizes")))
                    .unwrap_or_default();
                if let Some(request) = requests.last() {
                    size = (*request).into();
                }
                assert!(
                    output.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .iter()
                        .all(|command| !matches!(command, egui::ViewportCommand::InnerSize(_)))
                );
            } else {
                for command in &output.viewport_output[&egui::ViewportId::ROOT].commands {
                    if let egui::ViewportCommand::InnerSize(request) = command {
                        size = *request;
                        requests.push([size.x, size.y]);
                    }
                }
            }
            output.textures_delta.clear();
            if !context.plugin::<Driver>().lock().running() {
                break;
            }
        }
        let plugin = context.plugin::<Driver>();
        let driver = plugin.lock();
        let report = driver.report().unwrap();
        assert_eq!(report.state, "passed", "{:?}", report.failure);
        assert_eq!(report.completed, 11);
        assert_eq!(report.actions.len(), 11);
        assert_eq!(requests.len(), 2);
        assert_eq!(report.viewport_changes.len(), 2);
        assert!(report.resize_request.is_none());
        assert!(!checked);
        assert!(!report.performance_eligible);
        let clicks: Vec<_> = report
            .actions
            .iter()
            .filter(|action| action.kind == "click")
            .collect();
        assert!(clicks[0].target_bounds[0] > 800.0);
        assert!(clicks[1].target_bounds[0] < 100.0);
    }
}

#[test]
fn ignored_resize_times_out_without_claiming_success() {
    let context = Context::default();
    context.add_plugin(Driver::default().with_resize_handler(|_, _| Ok(())));
    command(
        &context,
        "action",
        &serde_json::json!({"kind":"resize", "width":720, "height":480}),
    )
    .unwrap();
    for tick in 0..8 {
        let _ = tests::frame(&context, f64::from(tick), vec![], "ready");
    }
    let plugin = context.plugin::<Driver>();
    let driver = plugin.lock();
    let report = driver.report().unwrap();
    assert_eq!(report.state, "failed");
    assert!(
        report
            .failure
            .as_deref()
            .unwrap()
            .contains("resize timed out")
    );
    assert_eq!(report.completed, 0);
    assert!(report.resize_request.is_none());
}

#[test]
fn sequences_validate_every_action_before_starting() {
    let context = Context::default();
    context.add_plugin(Driver::default());
    for invalid in [
        serde_json::json!([]),
        serde_json::json!([{"kind":"resize", "width":0, "height":480}]),
        serde_json::json!([{"kind":"resize", "width":9000, "height":480}]),
        serde_json::json!([{"kind":"resize", "width":720, "height":480}, {"kind":"unknown"}]),
        serde_json::json!([{"kind":"assert_value", "target":"setting", "value":true}]),
    ] {
        assert!(command(&context, "sequence", &invalid).is_err());
        assert!(context.plugin::<Driver>().lock().report().is_none());
    }
}

#[test]
fn responsive_assertions_fail_if_state_or_controls_disappear() {
    for action in [
        serde_json::json!({"kind":"assert_available", "target":"missing"}),
        serde_json::json!({"kind":"assert_value", "target":"profile.0", "value":"incorrect"}),
    ] {
        let context = Context::default();
        context.add_plugin(Driver::default());
        let _ = tests::frame(&context, 0.0, vec![], "ready");
        command(&context, "sequence", &serde_json::json!([action])).unwrap();
        let _ = tests::frame(&context, 0.1, vec![], "ready");
        assert_eq!(
            context.plugin::<Driver>().lock().report().unwrap().state,
            "failed"
        );
    }
}
