//! One command dispatcher for browser hooks and the desktop HTTP adapter.
use super::{Action, Driver, Pause, SCENARIOS, SemanticNode, actions};
use egui::Context;
use kittest::{By, Queryable as _};
use serde_json::{Value, json};

/// Execute a control command on the UI thread.
/// # Errors
/// Disabled automation, malformed requests, or an active workload.
pub fn command(context: &Context, operation: &str, argument: &Value) -> Result<Value, String> {
    let plugin = context
        .plugin_opt::<Driver>()
        .ok_or("Enable --ui-automation in a demo build")?;
    let mut driver = plugin.lock();
    driver.command(context, operation, argument)
}

impl Driver {
    pub(crate) fn command(
        &mut self,
        context: &Context,
        operation: &str,
        argument: &Value,
    ) -> Result<Value, String> {
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
            "status" => Ok(json!(driver.report())),
            "result" => Ok(if driver.running() || driver.paused_at.is_some() {
                Value::Null
            } else {
                json!(driver.report())
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
