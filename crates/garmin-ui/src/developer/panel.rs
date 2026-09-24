//! Shared developer panel layout and controls.
use egui::{RichText, Ui};
use garmin_color::theme::Level;

use super::{Request, State};
use crate::{Size, button, icons, theme::color32};

pub(super) fn show(ui: &mut Ui, state: &mut State) {
    let previous_requests = state.requests.len();
    let palette = crate::theme::palette(ui);
    crate::theme::apply_palette(ui.style_mut(), palette);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    egui::Frame::new()
        .fill(color32(palette.surfaces().background()))
        .inner_margin(0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("developer-sections")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    automation(ui, state);
                    control(ui, state);
                    super::logs_view::show(ui, state);
                    debug(ui, state);
                });
        });
    if state.requests.len() != previous_requests {
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }
}

pub(super) fn section(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title)
    });
    if response.clicked() {
        state.toggle(ui);
    }
    let palette = crate::theme::palette(ui);
    let fill = if response.hovered() || response.is_pointer_button_down_on() {
        palette.surfaces().layer_hover(Level::One)
    } else {
        palette.surfaces().layer(Level::One)
    };
    ui.painter().rect_filled(rect, 0.0, color32(fill));
    icons::Props {
        icon: if state.is_open() {
            icons::CARET_DOWN
        } else {
            icons::CARET_RIGHT
        },
        size: 16.0,
        color: palette.content().icon_primary(),
    }
    .paint_at(ui, egui::pos2(rect.left() + 20.0, rect.center().y));
    let galley = egui::WidgetText::from(crate::typography::semibold(title)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() - 48.0).max(0.0),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 36.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color32(palette.content().text_primary()),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0, color32(palette.interaction().focus())),
            egui::StrokeKind::Inside,
        );
    }
    state.show_body_unindented(ui, |ui| {
        egui::Frame::new().inner_margin(12).show(ui, |ui| {
            ui.set_width((rect.width() - 24.0).max(0.0));
            body(ui);
        });
    });
}

/// Action groups consume their controls' height, never the remaining viewport height.
pub(super) fn actions(ui: &mut Ui, row_min_width: f32, body: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        if ui.available_width() >= row_min_width {
            ui.horizontal_wrapped(body);
        } else {
            ui.vertical(body);
        }
    });
}

pub(super) fn action(ui: &mut Ui, label: &str, kind: button::Kind) -> egui::Response {
    let wrap = ui.style().wrap_mode;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    let response = button::Props {
        label,
        icon: None,
        kind,
        size: Size::Small,
        width: button::Width::Fit,
        enabled: true,
    }
    .show(ui);
    ui.style_mut().wrap_mode = wrap;
    response
}

pub(super) fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
}

pub(super) fn error(ui: &mut Ui, text: &str) {
    let color = color32(crate::theme::palette(ui).support().error());
    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
}

pub(super) fn code(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
}

fn automation(ui: &mut Ui, state: &mut State) {
    section(ui, "automation", "Automation", true, |ui| {
        #[cfg(any(test, feature = "automation"))]
        super::automation::show(ui, state);
        #[cfg(not(any(test, feature = "automation")))]
        {
            let _ = state;
            note(
                ui,
                "Enable --ui-automation in a demo build to run scenarios and individual actions.",
            );
        }
    });
}

fn control(ui: &mut Ui, state: &mut State) {
    section(
        ui,
        "control",
        if state.native {
            "Control server"
        } else {
            "Browser controls"
        },
        false,
        |ui| {
            if state.native {
                if let Some(url) = &state.server {
                    code(ui, url);
                    ui.horizontal_wrapped(|ui| {
                        if action(ui, "Copy URL", button::Kind::Secondary).clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                        if action(ui, "Stop server", button::Kind::Secondary).clicked() {
                            state.requests.push(Request::StopServer);
                        }
                    });
                } else if action(ui, "Start control server", button::Kind::Secondary).clicked() {
                    state.requests.push(Request::StartServer);
                }
                if let Some(message) = &state.server_error {
                    error(ui, message);
                }
                note(
                    ui,
                    "POST /api/control with operation and argument. GET /api/capabilities and /api/debug.",
                );
            } else {
                note(
                    ui,
                    "Use browser automation tools of your choice, including browser MCP. JavaScript hooks are available through window.garminAutomation in the application tab when --ui-automation is enabled.",
                );
                note(
                    ui,
                    "Resize actions size the application canvas inside the application tab. Sequences can verify state and control availability across layouts.",
                );
                for example in [
                    "window.garminAutomation.list()",
                    "window.garminAutomation.start('activity-smoke')",
                    "window.garminAutomation.targets()",
                    "window.garminAutomation.action({kind: 'click', target: 'map.fit'})",
                    "window.garminAutomation.sequence([{kind: 'resize', width: 720, height: 640}, {kind: 'assert_available', target: 'map.fit'}])",
                    "window.garminAutomation.status()",
                    "window.garminAutomation.result()",
                    "window.garminAutomation.cancel()",
                ] {
                    ui.horizontal(|ui| {
                        if action(ui, "Copy", button::Kind::Secondary).clicked() {
                            ui.ctx().copy_text(example.into());
                        }
                        code(ui, example);
                    });
                }
            }
        },
    );
}

fn debug(ui: &mut Ui, state: &State) {
    section(ui, "debug", "Debug information", false, |ui| {
        ui.label(format!(
            "Viewport: {:.0} × {:.0} · Scale: {:.2}",
            state.viewport[0], state.viewport[1], state.scale
        ));
        if let Some(renderer) = ui.ctx().data(|data| {
            data.get_temp::<crate::activity::map_diagnostics::RendererDiagnostics>(egui::Id::new(
                "map-renderer-diagnostics",
            ))
        }) {
            code(ui, &renderer.detail);
        }
        note(
            ui,
            &format!(
                "Log stream: {} · Cursor: {:?}",
                if state.connected {
                    "connected"
                } else {
                    "disconnected"
                },
                state.cursor.map(|cursor| cursor.sequence)
            ),
        );
        code(ui, &state.debug);
        if action(ui, "Copy debug info", button::Kind::Secondary).clicked() {
            ui.ctx().copy_text(format!(
                "{}\nViewport: {:?}\nScale: {}\nLogs connected: {}\nLog cursor: {:?}",
                state.debug, state.viewport, state.scale, state.connected, state.cursor
            ));
        }
    });
}
