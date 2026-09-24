//! Geometry and input regressions for the production developer panel.
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use gallery::egui;
use garmin_model::logging::{Level, Record};
use garmin_ui::developer;

struct Preview {
    panel: developer::State,
    bounds: egui::Rect,
}

fn panel(size: [f32; 2], dark: bool) -> Harness<'static, Preview> {
    let mut installed = false;
    let mut harness = Harness::builder().with_size(size).build_ui_state(
        move |ui, state: &mut Preview| {
            if !installed {
                garmin_ui::install(ui.ctx());
                ui.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
                ui.ctx().add_plugin(garmin_ui::automation::Driver::default());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            developer::contents(ui, &mut state.panel);
            state.bounds = ui.min_rect();
        },
        Preview {
            panel: developer::State {
                native: true,
                connected: true,
                logs: vec![Record {
                    sequence: 1, timestamp_ms: 1_790_166_467_209, level: Level::Info,
                    component: "app".into(), source: "desktop".into(), session: "linux-preview".into(),
                    source_sequence: 1, message: "Application started".into(),
                    fields: std::collections::BTreeMap::from([("details".into(), "A diagnostic value that must wrap inside the panel without adding a nested scroll area or widening the window.".into())]),
                }],
                ..Default::default()
            },
            bounds: egui::Rect::NOTHING,
        },
    );
    harness.run();
    harness
}

#[test]
fn action_rows_stay_compact_when_the_window_gets_taller() {
    let mut harness = panel([620.0, 800.0], true);
    let before = harness.get_by_label("stationary-arrival").rect();
    let toolbar_before = harness.get_by_label("Pause stream").rect();
    let description = harness
        .get_by_label("Run a scenario with simulated input. Use Stop or Esc to cancel.")
        .rect();
    assert!((before.height() - 32.0).abs() < 1.0);
    assert!((toolbar_before.height() - 32.0).abs() < 1.0);
    assert!((0.0..=12.0).contains(&(before.top() - description.bottom())));
    harness.set_size(egui::vec2(620.0, 1200.0));
    harness.run();
    assert_eq!(harness.get_by_label("stationary-arrival").rect(), before);
    assert_eq!(harness.get_by_label("Pause stream").rect(), toolbar_before);
    assert!(harness.get_by_label("Debug information").rect().top() < 800.0);
}

#[test]
fn narrow_panels_stack_actions_without_widening_the_window() {
    for dark in [true, false] {
        let harness = panel([380.0, 1600.0], dark);
        assert!(harness.state().bounds.right() <= 380.0);
        let mut bottom = 0.0;
        for label in garmin_ui::automation::SCENARIOS {
            let rect = harness.get_by_label(label).rect();
            assert!((rect.height() - 32.0).abs() < 1.0);
            assert!(rect.top() >= bottom);
            assert!(rect.right() <= 380.0);
            bottom = rect.bottom();
        }
        assert!(
            harness.get_by_label("Export logs").rect().top()
                >= harness.get_by_label("Pause stream").rect().bottom()
        );
    }
}

#[test]
fn responsive_scenario_button_fits_and_launches_from_the_panel() {
    let mut harness = panel([620.0, 800.0], true);
    let response = harness.get_by_label("responsive-layout");
    assert!(response.rect().right() <= 620.0);
    response.click();
    harness.run();
    assert_eq!(
        harness
            .ctx
            .plugin::<garmin_ui::automation::Driver>()
            .lock()
            .take_launch_request(),
        Some("responsive-layout")
    );
}

#[test]
fn remote_panel_queues_commands_without_starting_its_local_driver() {
    let mut harness = panel([720.0, 800.0], true);
    harness.state_mut().panel.native = false;
    harness.state_mut().panel.remote_automation = Some(developer::Automation {
        enabled: true,
        connected: true,
        ..Default::default()
    });
    harness.run();
    harness.get_by_label("responsive-layout").click();
    harness.run();
    let remote = harness
        .state()
        .panel
        .remote_automation
        .as_ref()
        .expect("rendering must retain the configured remote automation state");
    assert!(
        matches!(remote.requests.as_slice(), [developer::AutomationRequest::Start(name)] if name == "responsive-layout")
    );
    assert!(
        harness
            .ctx
            .plugin::<garmin_ui::automation::Driver>()
            .lock()
            .take_launch_request()
            .is_none()
    );
}

#[test]
fn disconnected_and_pending_remote_panels_disable_scenario_buttons() {
    let mut harness = panel([720.0, 800.0], true);
    for (connected, pending) in [(false, false), (true, true)] {
        harness.state_mut().panel.remote_automation = Some(developer::Automation {
            enabled: true,
            connected,
            pending,
            ..Default::default()
        });
        harness.run();
        assert!(
            harness
                .get_by_label("responsive-layout")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .state()
                .panel
                .remote_automation
                .as_ref()
                .expect("disabled controls must retain the configured remote automation state")
                .requests
                .is_empty()
        );
    }
}

#[test]
fn headers_and_tools_icon_respond_to_pointer_input() {
    let mut harness = panel([620.0, 800.0], true);
    harness.get_by_label("Automation").hover();
    harness.run();
    assert_eq!(
        harness.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    harness.get_by_label("Automation").click();
    harness.run();
    assert!(harness.query_by_label("stationary-arrival").is_none());
    harness.get_by_label("Automation").click();
    harness.run();
    assert!(harness.query_by_label("stationary-arrival").is_some());

    let mut icon = Harness::new_ui_state(
        |ui, open: &mut bool| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
            developer::header_button(ui, rect);
            *open = developer::state(ui.ctx()).lock().expect("panel state").open;
        },
        false,
    );
    icon.get_by_label("Developer tools").hover();
    icon.run();
    assert_eq!(
        icon.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    icon.get_by(|node| {
        node.role() == egui::accesskit::Role::Button
            && node.label().as_deref() == Some("Developer tools")
    })
    .click();
    icon.run();
    assert!(*icon.state());
}

#[test]
fn tools_button_stays_inside_its_header_cell() {
    let cell = egui::Rect::from_min_size(egui::pos2(46.0, 24.0), egui::vec2(34.0, 32.0));
    let mut harness = Harness::new_ui_state(
        move |ui, state: &mut (egui::Rect, bool, bool)| {
            ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
            let neighbor = egui::Rect::from_min_size(cell.right_top(), egui::vec2(32.0, 32.0));
            if ui
                .interact(neighbor, ui.id().with("minimize"), egui::Sense::click())
                .clicked()
            {
                state.1 = true;
            }
            state.0 = developer::header_button(ui, cell).rect;
            state.2 = developer::state(ui.ctx()).lock().expect("panel state").open;
        },
        (egui::Rect::NOTHING, false, false),
    );
    harness.run();
    assert!(
        cell.contains_rect(harness.state().0),
        "button escaped its header cell: {:?}",
        harness.state().0
    );
    let pos = egui::pos2(cell.right() + 1.0, cell.center().y);
    harness.hover_at(pos);
    harness.run();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
    }
    assert!(harness.state().1, "neighbor did not receive its click");
    assert!(
        !harness.state().2,
        "tools button stole the neighbor's click"
    );
}

#[test]
#[ignore = "renders browser control documentation with a headless GPU"]
fn capture_browser_controls() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/gallery/developer-interaction");
    std::fs::create_dir_all(&out)?;
    for dark in [true, false] {
        let mut harness = panel([620.0, 900.0], dark);
        harness.state_mut().panel.native = false;
        harness.run();
        harness.get_by_label("Automation").click();
        harness.run();
        harness.get_by_label("Browser controls").click();
        harness.run();
        let theme = if dark { "dark" } else { "light" };
        harness
            .render()?
            .save(out.join(format!("browser-controls-{theme}.png")))?;
    }
    Ok(())
}

#[test]
#[ignore = "renders maintained interaction evidence with a headless GPU"]
fn capture_expanded_log_details() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/gallery/developer-interaction");
    std::fs::create_dir_all(&out)?;
    for width in [380.0, 620.0] {
        for dark in [true, false] {
            let mut harness = panel([width, 660.0], dark);
            harness.get_by_label("Automation").click();
            harness.run();
            harness
                .get_by_label("Info · app · Application started")
                .scroll_to_me();
            harness.run();
            harness
                .get_by_label("Info · app · Application started")
                .click();
            harness.run();
            harness.get_by_label("Copy record as JSON").scroll_to_me();
            harness.run();
            assert!(harness.state().bounds.right() <= width);
            let theme = if dark { "dark" } else { "light" };
            harness
                .render()?
                .save(out.join(format!("details-{width}-{theme}.png")))?;
        }
    }
    Ok(())
}
