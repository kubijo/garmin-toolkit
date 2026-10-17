//! Resize actions complete only after the host reports the requested layout.
use super::{ActionTiming, Driver, Run};
use egui::{Context, RawInput, Rect};
use serde::{Deserialize, Serialize};

/// Host adapter for resizing the root view in logical egui points.
pub type ResizeHandler = fn(&Context, ResizeCommand) -> Result<(), String>;

/// Built-in scenarios save and restore host sizing; explicit actions only set it.
#[derive(Clone, Copy)]
pub enum ResizeCommand {
    Save,
    Set([f32; 2]),
    Restore([f32; 2]),
}

pub(super) struct ViewportRestore {
    size: [f32; 2],
    maximized: Option<bool>,
}

impl Driver {
    pub(super) fn save_viewport(
        &mut self,
        context: &Context,
        input: &RawInput,
    ) -> Result<(), String> {
        if self.viewport_restore.is_some()
            || !self.run.as_ref().is_some_and(|run| run.restore_viewport)
        {
            return Ok(());
        }
        let screen = input
            .screen_rect
            .unwrap_or_else(|| context.input_for(self.viewport, egui::InputState::viewport_rect));
        if let Some(handler) = self.resize_handler {
            handler(context, ResizeCommand::Save)?;
        }
        self.viewport_restore = Some(ViewportRestore {
            size: [screen.width(), screen.height()],
            maximized: input.viewport().maximized,
        });
        Ok(())
    }

    pub(super) fn restore_viewport(&mut self, context: &Context) {
        if self.running() || self.paused_at.is_some() || self.resumed_at.is_some() {
            return;
        }
        let Some(saved) = self.viewport_restore.take() else {
            return;
        };
        let result = if let Some(handler) = self.resize_handler {
            handler(context, ResizeCommand::Restore(saved.size))
        } else {
            let result = resize_viewport(context, self.viewport, None, saved.size);
            if let Some(maximized) = saved.maximized {
                context.send_viewport_cmd_to(
                    self.viewport,
                    egui::ViewportCommand::Maximized(maximized),
                );
            }
            result
        };
        if let Err(error) = result {
            let reason = format!("Could not restore automation viewport: {error}");
            tracing::error!(%reason);
            if let Some(run) = &mut self.run {
                if run.report.state == "passed" {
                    run.report.state = "failed".into();
                }
                run.report.failure = Some(
                    run.report
                        .failure
                        .as_ref()
                        .map_or_else(|| reason.clone(), |failure| format!("{failure}; {reason}")),
                );
            }
        }
        context.request_repaint_of(self.viewport);
    }
}

/// Resize awaiting acknowledgement from the rendered root viewport.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResizeRequest {
    pub viewport: [f32; 2],
    pub started_seconds: f64,
}

impl Run {
    pub(super) fn resize(
        &mut self,
        context: &Context,
        viewport: egui::ViewportId,
        handler: Option<ResizeHandler>,
        size: [f32; 2],
        elapsed: f64,
        screen: Rect,
    ) -> Result<(), String> {
        if self.report.resize_request.is_none() {
            resize_viewport(context, viewport, handler, size)?;
            self.report.resize_request = Some(ResizeRequest {
                viewport: size,
                started_seconds: elapsed,
            });
            self.report.performance_eligible = false;
            self.resize_ready_since = None;
            return Ok(());
        }
        let request = self
            .report
            .resize_request
            .as_ref()
            .expect("resize requested");
        if elapsed - request.started_seconds > 5.0 {
            return Err(format!(
                "resize timed out: requested {} × {}, observed {} × {} logical points",
                size[0],
                size[1],
                screen.width(),
                screen.height(),
            ));
        }
        if (screen.width() - size[0]).abs() > 0.5 || (screen.height() - size[1]).abs() > 0.5 {
            self.resize_ready_since = None;
            return Ok(());
        }
        if elapsed - *self.resize_ready_since.get_or_insert(elapsed) < 0.05 {
            return Ok(());
        }
        self.report.actions.push(ActionTiming {
            phase: self.steps[self.report.completed].phase.into(),
            kind: "resize".into(),
            target: "viewport".into(),
            scheduled_seconds: request.started_seconds,
            actual_seconds: elapsed,
            lateness_seconds: elapsed - request.started_seconds,
            target_bounds: [screen.left(), screen.top(), screen.right(), screen.bottom()],
            pointer_position: None,
        });
        let after = self.steps[self.report.completed].after;
        self.report.completed += 1;
        self.report.resize_request = None;
        self.resize_ready_since = None;
        self.recovering_layout = false;
        self.due = elapsed + after;
        self.waiting_since = elapsed;
        if self.report.completed == self.steps.len() {
            self.report.state = "passed".into();
        }
        Ok(())
    }
}

fn resize_viewport(
    context: &Context,
    viewport: egui::ViewportId,
    handler: Option<ResizeHandler>,
    size: [f32; 2],
) -> Result<(), String> {
    if let Some(handler) = handler {
        return handler(context, ResizeCommand::Set(size));
    }
    if cfg!(target_arch = "wasm32") {
        return Err("browser canvas resize adapter is unavailable".into());
    }
    context.send_viewport_cmd_to(viewport, egui::ViewportCommand::InnerSize(size.into()));
    Ok(())
}
