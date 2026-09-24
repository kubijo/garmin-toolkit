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
                crate::developer::contents(ui, &mut crate::developer::State::default());
            }
        },
    );
    output.textures_delta.clear();
}

#[test]
fn both_stop_controls_have_unique_targets_and_capture_mouse_and_touch() {
    for target in ["automation.stop", "developer.automation.stop"] {
        for touch in [false, true] {
            let context = context();
            start(&context);
            for tick in 0..4 {
                render(&context, tick, vec![], false);
            }
            let point = {
                let plugin = context.plugin::<Driver>();
                let driver = plugin.lock();
                assert!(driver.running(), "{:?}", driver.report().unwrap().failure);
                lookup(driver.tree.as_ref(), target, Rect::EVERYTHING)
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
            render(&context, 4, vec![event], false);
            assert_eq!(
                context.plugin::<Driver>().lock().report().unwrap().state,
                "cancelled"
            );
            assert!(
                !context.input(|input| input.pointer.primary_down() || input.pointer.any_click())
            );
        }
    }
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
