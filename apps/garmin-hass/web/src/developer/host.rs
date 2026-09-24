//! Developer tools consumer of the browser window host.
use super::protocol::Snapshot;
use crate::window::BrowserWindow;
use eframe::egui::{self, Context};
use garmin_ui::{
    automation::Driver,
    developer::{Automation, AutomationRequest, state},
    window::{Event, Spec, WindowHost},
};

pub struct Host {
    context: Context,
    window: BrowserWindow<AutomationRequest, Snapshot>,
}

impl Host {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            window: BrowserWindow::default(),
        }
    }

    pub fn update(&mut self, intl: &garmin_i18n::Intl) {
        let requested = {
            let handle = state(&self.context);
            let mut state = handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.focus_requested = false;
            std::mem::take(&mut state.open)
        };
        if requested
            && let Err(error) = self.window.open(
                &self.context,
                Spec {
                    id: "developer-tools".into(),
                    kind: "developer-tools".into(),
                    title: "Developer tools".into(),
                    size: [720.0, 800.0],
                },
            )
        {
            tracing::warn!(%error, "Could not open Developer tools");
        }
        let mut window = std::mem::take(&mut self.window);
        let events = window.present(&self.context, intl, || self.snapshot(), |_| None);
        self.window = window;
        for event in events {
            if let Event::Command { id, command } = event {
                let error = match command {
                    AutomationRequest::Start(name) => crate::automation::launch(&name),
                    AutomationRequest::Stop => garmin_ui::automation::command(
                        &self.context,
                        "cancel",
                        &serde_json::Value::Null,
                    )
                    .map(|_| ()),
                }
                .err();
                self.window.reply(id, error);
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        let automation = self.context.plugin_opt::<Driver>().map_or_else(
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
        );
        let debug = state(&self.context)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .debug
            .clone();
        let renderer = self
            .context
            .data(|data| {
                data.get_temp::<garmin_ui::activity::map_diagnostics::RendererDiagnostics>(
                    egui::Id::new("map-renderer-diagnostics"),
                )
            })
            .map(|value| value.detail);
        let rect = self.context.viewport_rect();
        Snapshot {
            automation,
            debug,
            renderer,
            viewport: [rect.width(), rect.height()],
            scale: self.context.pixels_per_point(),
            dark: self.context.global_style().visuals.dark_mode,
        }
    }
}
