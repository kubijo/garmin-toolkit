use super::*;
use egui::{Event, Pos2, RawInput, Rect, Vec2, ViewportId, ViewportInfo};

struct Child {
    context: Context,
    callback: Arc<egui::DeferredViewportUiCallback>,
    tick: u32,
}

impl Child {
    fn new() -> Self {
        let context = Context::default();
        crate::install(&context);
        context.set_embed_viewports(false);
        context.add_plugin(crate::automation::Driver::default());
        state(&context).lock().unwrap().open = true;
        state(&context).lock().unwrap().logs = (1..=20)
            .map(|sequence| garmin_model::logging::Record {
                sequence,
                timestamp_ms: 0,
                level: garmin_model::logging::Level::Info,
                component: "test".into(),
                source: "desktop".into(),
                session: "covered-root".into(),
                source_sequence: sequence,
                message: "A retained log record".into(),
                fields: std::collections::BTreeMap::default(),
            })
            .collect();
        let intl = garmin_i18n::Translations::bundled()
            .unwrap()
            .formatter(garmin_i18n::Language::English)
            .unwrap();
        let mut output = context.run_ui(RawInput::default(), |ui| show(ui.ctx(), &intl));
        output.textures_delta.clear();
        let viewport = &output.viewport_output[&ViewportId::from_hash_of("developer-tools-window")];
        assert!(viewport.class == egui::ViewportClass::Deferred);
        let callback = viewport
            .viewport_ui_cb
            .clone()
            .expect("independent callback");
        Self {
            context,
            callback,
            tick: 0,
        }
    }

    // Deliberately never run the root UI after creating the child.
    fn frame(&mut self, events: Vec<Event>, close: bool) -> egui::PlatformOutput {
        self.tick += 1;
        let child = ViewportId::from_hash_of("developer-tools-window");
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(680.0, 640.0));
        let input = RawInput {
            viewport_id: child,
            viewports: [
                (
                    ViewportId::ROOT,
                    ViewportInfo {
                        occluded: Some(true),
                        ..Default::default()
                    },
                ),
                (
                    child,
                    ViewportInfo {
                        parent: Some(ViewportId::ROOT),
                        inner_rect: Some(rect),
                        focused: Some(true),
                        events: if close {
                            vec![egui::ViewportEvent::Close]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            screen_rect: Some(rect),
            time: Some(f64::from(self.tick) / 60.0),
            focused: true,
            events,
            ..Default::default()
        };
        let mut output = self.context.run_ui(input, |ui| (self.callback)(ui));
        output.textures_delta.clear();
        output.platform_output
    }

    fn target(&self, name: &str) -> serde_json::Value {
        let targets = crate::window::control::command(
            &self.context,
            Some("window-1"),
            "targets",
            &serde_json::Value::Null,
        )
        .unwrap();
        targets
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["id"] == name)
            .unwrap()
            .clone()
    }

    fn center(&self, name: &str) -> Pos2 {
        let [left, top, right, bottom]: [f32; 4] =
            serde_json::from_value(self.target(name)["bounds"].clone()).unwrap();
        egui::pos2(left.midpoint(right), top.midpoint(bottom))
    }

    fn click(&mut self, name: &str) {
        let position = self.center(name);
        for pressed in [true, false] {
            self.frame(
                vec![
                    Event::PointerMoved(position),
                    Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                false,
            );
        }
        self.frame(vec![], false);
    }
}

#[test]
fn covered_root_does_not_stall_tools_cursor_clicks_or_scrolling() {
    let mut child = Child::new();
    child.frame(vec![], false);
    child.frame(vec![], false);
    assert_eq!(child.target("developer.section.logs")["value"], "closed");
    let pointer = child.center("developer.section.logs");
    let output = child.frame(vec![Event::PointerMoved(pointer)], false);
    assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand);
    child.click("developer.section.logs");
    assert_eq!(child.target("developer.section.logs")["value"], "open");
    child.click("developer.section.logs");
    assert_eq!(child.target("developer.section.logs")["value"], "closed");
    child.click("developer.section.logs");
    assert_eq!(child.target("developer.section.logs")["value"], "open");
    let before = child.center("developer.section.logs").y;
    child.frame(
        vec![
            Event::PointerMoved(egui::pos2(350.0, 500.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
        false,
    );
    for _ in 0..10 {
        child.frame(vec![], false);
    }
    assert!(child.center("developer.section.logs").y < before);
}

#[test]
fn child_close_revokes_its_handle_without_a_root_frame() {
    let mut child = Child::new();
    child.frame(vec![], false);
    child.frame(vec![], true);
    assert!(!state(&child.context).lock().unwrap().open);
    assert!(
        crate::window::control::command(
            &child.context,
            Some("window-1"),
            "targets",
            &serde_json::Value::Null,
        )
        .is_err()
    );
    state(&child.context).lock().unwrap().open = true;
    let intl = garmin_i18n::Translations::bundled()
        .unwrap()
        .formatter(garmin_i18n::Language::English)
        .unwrap();
    let mut output = child
        .context
        .run_ui(RawInput::default(), |ui| show(ui.ctx(), &intl));
    output.textures_delta.clear();
    let viewport = &output.viewport_output[&ViewportId::from_hash_of("developer-tools-window")];
    assert!(!viewport.commands.contains(&egui::ViewportCommand::Close));
    let windows =
        crate::window::control::command(&child.context, None, "windows", &serde_json::Value::Null)
            .unwrap();
    assert!(
        windows
            .as_array()
            .unwrap()
            .iter()
            .any(|window| window["id"] == "window-2")
    );
}
