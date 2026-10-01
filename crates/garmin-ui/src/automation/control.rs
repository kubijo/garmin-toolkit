//! One command dispatcher for browser hooks and the desktop HTTP adapter.
use super::{Action, Driver, Pause, SCENARIOS, SemanticNode, actions};
use egui::Context;
use kittest::{By, Queryable as _};
use serde_json::{Value, json};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    argument: Value,
    run_in_background: bool,
}

/// Keep eframe UI passes running for explicitly enabled background automation.
/// Call from the host's raw-input hook, before eframe checks viewport visibility.
pub fn prepare_background_input(context: &Context, input: &mut egui::RawInput) {
    let Some(plugin) = context.plugin_opt::<Driver>() else {
        return;
    };
    let driver = plugin.lock();
    if driver.needs_background_frames()
        && let Some(viewport) = input.viewports.get_mut(&driver.viewport)
    {
        viewport.occluded = Some(false);
    }
}

/// Execute a control command on the UI thread.
/// # Errors
/// Disabled automation, malformed requests, or an active workload.
pub fn command(context: &Context, operation: &str, argument: &Value) -> Result<Value, String> {
    let plugin = context
        .plugin_opt::<Driver>()
        .ok_or("Enable --ui-automation in a demo build")?;
    let mut driver = plugin.lock();
    let result = driver.command(context, operation, argument);
    crate::diagnostics::automation(context, "root", driver.report());
    result
}

impl Driver {
    fn needs_background_frames(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.background)
            && (self.running()
                || self.resumed_at.is_some()
                || self.release
                || self.viewport_restore.is_some())
    }

    fn control_report(&self) -> Value {
        let mut report = json!(self.report());
        if let Some(fields) = report.as_object_mut() {
            // The browser keeps delivering frames through input release and viewport
            // restoration, then returns to its ordinary animation-frame scheduling.
            fields.insert(
                "needs_background_frames".into(),
                self.needs_background_frames().into(),
            );
        }
        report
    }

    pub(crate) fn command(
        &mut self,
        context: &Context,
        operation: &str,
        argument: &Value,
    ) -> Result<Value, String> {
        let launching = matches!(operation, "start" | "action" | "sequence");
        let request: Option<RunRequest> =
            if launching && argument.get("run_in_background").is_some() {
                Some(
                    serde_json::from_value(argument.clone())
                        .map_err(|error| format!("invalid automation run options: {error}"))?,
                )
            } else {
                None
            };
        let argument = request
            .as_ref()
            .map_or(argument, |request| &request.argument);
        let screen = context.input_for(self.viewport, egui::InputState::viewport_rect);
        let driver = self;
        let result = match operation {
            "list" => Ok(if driver.window_scope {
                json!([])
            } else {
                json!(SCENARIOS)
            }),
            "start" => driver
                .start(argument.as_str().ok_or("scenario must be a string")?)
                .map(|()| Value::Null),
            "status" => Ok(driver.control_report()),
            "result" => Ok(if driver.running() || driver.paused_at.is_some() {
                Value::Null
            } else {
                driver.control_report()
            }),
            "cancel" => {
                driver.cancel(argument.as_str().unwrap_or("cancelled through control API"));
                Ok(Value::Null)
            }
            "pause" => {
                driver.pause(
                    argument
                        .as_f64()
                        .ok_or("pause requires monotonic seconds")?,
                );
                Ok(Value::Null)
            }
            "resume" => {
                driver.resume(
                    argument
                        .as_f64()
                        .ok_or("resume requires monotonic seconds")?,
                );
                Ok(Value::Null)
            }
            "targets" => {
                let targets = driver.tree.as_ref().map_or_else(Vec::new, |tree| {
                SemanticNode(tree.root()).query_all(By::new().predicate(|node| node.author_id().is_some())).map(|node| {
                    let node = node.0;
                    let id = node.author_id().unwrap_or_default();
                    let bounds = super::lookup(Some(tree), id, screen).ok().flatten().map(|(rect, _)| [rect.left(), rect.top(), rect.right(), rect.bottom()]);
                    json!({"id": id, "label": node.label(), "role": format!("{:?}", node.role()), "value": node.value(), "enabled": !node.is_disabled(), "bounds": bounds})
                }).collect()
            });
                Ok(json!(targets))
            }
            "action" => {
                let steps = actions::parse(argument)?;
                for step in &steps {
                    if !matches!(step.action, Action::Resize { .. }) {
                        super::lookup(driver.tree.as_ref(), &step.target, screen)?
                            .ok_or("target missing, disabled or clipped")?;
                    }
                }
                driver.start_steps("individual-action", steps)?;
                Ok(Value::Null)
            }
            "sequence" => {
                let steps = actions::sequence(argument)?;
                driver.start_steps("custom-sequence", steps)?;
                Ok(Value::Null)
            }
            _ => Err("unknown automation command".into()),
        };
        if result.is_ok() && launching {
            let run = driver.run.as_mut().expect("successfully started run");
            run.background = request.is_some_and(|request| request.run_in_background);
            run.report.performance_eligible &= !run.background;
        }
        context.request_repaint_of(driver.viewport);
        result
    }
}

impl Driver {
    fn pause(&mut self, now: f64) {
        if !now.is_finite() || !self.running() {
            return;
        }
        self.paused_at = Some(now);
        self.release = self.held.is_some();
        let run = self.run.as_mut().expect("running run");
        run.report.state = "paused".into();
        run.report.performance_eligible = false;
        if run.gesture.take().is_some()
            && let Some(attempt) = run.report.actions.pop()
        {
            run.report.interrupted_attempts.push(attempt);
        }
        run.ready_since = None;
        run.target_geometry = None;
        run.stabilizing_since = None;
    }

    fn resume(&mut self, now: f64) {
        if !now.is_finite() {
            return;
        }
        if let Some(start) = self.paused_at.take() {
            let run = self.run.as_mut().expect("paused run");
            run.report.pauses.push(Pause {
                started_seconds: run.report.elapsed_seconds,
                duration_seconds: (now - start).max(0.0),
            });
            self.resumed_at = Some(now);
        }
    }

    pub(super) fn resume_clock(&mut self, input: &mut egui::RawInput) {
        if self.resumed_at.take().is_some() {
            // Release first, then retry the interrupted action on a subsequent frame.
            if let Some(run) = &mut self.run {
                run.start = input.time.map(|now| now - run.report.elapsed_seconds);
                run.due = run.report.elapsed_seconds + 0.05;
                run.waiting_since = run.report.elapsed_seconds;
                run.report.state = "running".into();
            }
        }
    }
}
