//! One panel for local drivers and remote browser sessions.
use egui::Ui;
use serde::{Deserialize, Serialize};

use super::{State, panel};
use crate::{
    Size,
    automation::{Driver, Report, SCENARIOS},
    button, icons,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum AutomationRequest {
    Start(String),
    Stop,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Automation {
    pub enabled: bool,
    pub connected: bool,
    pub pending: bool,
    pub report: Option<Report>,
    pub error: Option<String>,
    #[serde(skip)]
    pub requests: Vec<AutomationRequest>,
}

pub(super) fn show(ui: &mut Ui, state: &mut State) {
    if let Some(remote) = &mut state.remote_automation {
        controls(ui, remote);
        return;
    }
    if ui.ctx().plugin_opt::<Driver>().is_none() {
        panel::note(
            ui,
            "Enable --ui-automation in a demo build to run scenarios and individual actions.",
        );
        return;
    }
    let mut view = snapshot(ui.ctx());
    controls(ui, &mut view);
    for request in view.requests {
        dispatch(ui.ctx(), request);
    }
}

pub(super) fn snapshot(context: &egui::Context) -> Automation {
    context.plugin_opt::<Driver>().map_or_else(
        || Automation {
            connected: true,
            ..Default::default()
        },
        |plugin| {
            let driver = plugin.lock();
            Automation {
                enabled: true,
                connected: true,
                report: driver.report().cloned(),
                error: driver.launch_error.clone(),
                ..Default::default()
            }
        },
    )
}

pub(super) fn dispatch(context: &egui::Context, request: AutomationRequest) {
    let Some(plugin) = context.plugin_opt::<Driver>() else {
        return;
    };
    match request {
        AutomationRequest::Start(name) => {
            if let Some(&name) = SCENARIOS
                .iter()
                .find(|&&candidate| candidate == name.as_str())
            {
                plugin.lock().request_launch(name);
            }
        }
        AutomationRequest::Stop => plugin.lock().cancel("stopped from Developer tools"),
    }
    context.request_repaint_of(egui::ViewportId::ROOT);
}

fn controls(ui: &mut Ui, view: &mut Automation) {
    let running = view
        .report
        .as_ref()
        .is_some_and(|report| matches!(report.state.as_str(), "running" | "paused"));
    if !view.connected {
        panel::note(
            ui,
            "App tab is not responding. Return to it, or reopen Developer tools from the app if that tab was reloaded or closed.",
        );
    } else if !view.enabled {
        panel::note(
            ui,
            "Enable --ui-automation in a demo build to run scenarios and individual actions.",
        );
    } else {
        panel::note(
            ui,
            "Run a scenario with simulated input. Use Stop or Esc to cancel.",
        );
    }
    panel::actions(ui, 500.0, |ui| {
        for name in SCENARIOS {
            let response = button::Props {
                label: name,
                icon: Some(icons::PLAY),
                kind: button::Kind::Secondary,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: view.connected && view.enabled && !view.pending && !running,
            }
            .show(ui);
            crate::semantics::target(ui, &response, format!("automation.scenario.{name}"));
            if response.clicked() {
                view.requests.push(AutomationRequest::Start((*name).into()));
            }
        }
    });
    if let Some(error) = &view.error {
        panel::error(ui, error);
    }
    if let Some(report) = &view.report {
        ui.label(format!(
            "{} · {} · {}/{}",
            report.state, report.phase, report.completed, report.total
        ));
        if let Some(failure) = &report.failure {
            panel::error(ui, failure);
        }
        ui.horizontal_wrapped(|ui| {
            if panel::action(ui, "Copy report", button::Kind::Secondary).clicked() {
                ui.ctx()
                    .copy_text(serde_json::to_string_pretty(report).unwrap_or_default());
            }
            if running {
                let stop = ui
                    .add_enabled_ui(view.connected && !view.pending, |ui| {
                        panel::action(ui, "Stop", button::Kind::Danger)
                    })
                    .inner;
                crate::semantics::target(ui, &stop, "developer.automation.stop");
                if stop.clicked()
                    || (view.connected && ui.input(|input| input.key_pressed(egui::Key::Escape)))
                {
                    view.requests.push(AutomationRequest::Stop);
                }
            }
        });
    }
}
