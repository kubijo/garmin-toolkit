//! AccessKit-targeted input for demos and headless tests.

use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
use kittest::{By, Queryable as _};
use serde::{Deserialize, Serialize};

mod menu {
    use egui::{Rect, Ui};

    use super::{Driver, SCENARIOS};

    /// Size the menu beside the profile control with a two-point gap.
    #[must_use]
    pub fn header_rect(ui: &Ui, profile: Rect, left_limit: f32) -> Rect {
        let width = crate::header_selector::preferred_width(ui, 18.0, ["Automation"]);
        let right = (profile.left() - 2.0).max(left_limit);
        Rect::from_min_max(
            egui::pos2((right - width).max(left_limit), profile.top()),
            egui::pos2(right, profile.bottom()),
        )
    }

    /// Scenario menu contents.
    pub fn scenario_menu(ui: &mut Ui, running: bool, error: Option<&str>) -> Option<&'static str> {
        ui.label(egui::RichText::new("UI automation").strong());
        ui.label("Input is blocked during runs.");
        ui.label("Use Stop or Esc to cancel.");
        let mut selected = None;
        for name in SCENARIOS {
            let response = ui
                .add_enabled(!running, egui::Button::new(*name))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            crate::semantics::target(ui, &response, format!("automation.scenario.{name}"));
            if response.clicked() {
                selected = Some(*name);
                ui.close();
            }
        }
        if let Some(error) = error {
            ui.label(error);
        }
        selected
    }

    /// Show the menu for an installed driver.
    pub fn header_button(ui: &mut Ui, rect: Rect) {
        let plugin = ui.ctx().plugin::<Driver>();
        let (running, error) = {
            let driver = plugin.lock();
            (driver.running(), driver.launch_error.clone())
        };
        if let Some(name) = launcher(ui, rect, running, error.as_deref()) {
            plugin.lock().launch_request = Some(name);
            ui.ctx().request_repaint();
        }
    }

    /// Return the selected scenario for the caller to launch.
    pub fn launcher(
        ui: &mut Ui,
        rect: Rect,
        running: bool,
        error: Option<&str>,
    ) -> Option<&'static str> {
        let popup_id = ui.make_persistent_id("automation-menu");
        let response = crate::header_selector::control(
            ui,
            rect,
            ui.make_persistent_id("automation-button"),
            "UI automation scenarios",
            egui::Popup::is_id_open(ui.ctx(), popup_id),
            |ui, wide| {
                crate::icons::Props {
                    icon: crate::icons::PLAY,
                    size: 18.0,
                    color: crate::theme::palette(ui).content().icon_secondary(),
                }
                .show(ui);
                if wide {
                    ui.add(egui::Label::new("Automation").selectable(false).truncate());
                }
            },
        );
        crate::semantics::target(ui, &response, "automation.menu");
        let menu_style = ui.style().as_ref().clone();
        let selected = egui::Popup::menu(&response)
            .id(popup_id)
            .width(280.0)
            .style(egui::style::StyleModifier::new(move |style| {
                *style = menu_style.clone();
            }))
            .show(|ui| scenario_menu(ui, running, error))
            .and_then(|response| response.inner);
        response.on_hover_text("UI automation scenarios");
        selected
    }
}
mod actions;
mod control;
mod resize;
mod scenarios;
pub use control::command;
pub use resize::{ResizeCommand, ResizeHandler, ResizeRequest};
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_developer;
#[cfg(test)]
mod tests_resize;
#[cfg(test)]
mod tests_stability;
#[cfg(test)]
mod tests_workspace;

pub use menu::{header_button, header_rect, launcher, scenario_menu};
pub use scenarios::SCENARIOS;
use scenarios::{Action, Step};

#[derive(Clone, Debug)]
struct SemanticNode<'a>(kittest::AccessKitNode<'a>);

impl<'a> kittest::NodeT<'a> for SemanticNode<'a> {
    fn accesskit_node(&self) -> kittest::AccessKitNode<'a> {
        self.0
    }
    fn new_related(&self, node: kittest::AccessKitNode<'a>) -> Self {
        Self(node)
    }
}

/// Run report, retained until the next start.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub version: u8,
    pub scenario: String,
    pub state: String,
    pub phase: String,
    pub completed: usize,
    pub total: usize,
    pub failure: Option<String>,
    pub elapsed_seconds: f64,
    pub viewport: [f32; 2],
    pub pixels_per_point: f32,
    pub viewport_changes: Vec<ViewportChange>,
    pub resize_request: Option<ResizeRequest>,
    pub driver_milliseconds: f64,
    pub tree_milliseconds: f64,
    pub readiness: Option<String>,
    pub actions: Vec<ActionTiming>,
    pub input_events: usize,
    pub pauses: Vec<Pause>,
    pub interrupted_attempts: Vec<ActionTiming>,
    pub performance_eligible: bool,
}

/// Action timing relative to run start.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActionTiming {
    pub phase: String,
    pub kind: String,
    pub target: String,
    pub scheduled_seconds: f64,
    pub actual_seconds: f64,
    pub lateness_seconds: f64,
    pub target_bounds: [f32; 4],
    pub pointer_position: Option<[f32; 2]>,
}

impl ActionTiming {
    /// Paint the target and pointer without capturing input.
    pub fn highlight(&self, context: &Context) {
        let Some([pointer_x, pointer_y]) = self.pointer_position else {
            return;
        };
        let [left, top, right, bottom] = self.target_bounds;
        let rect = Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom));
        let cursor = egui::pos2(pointer_x, pointer_y);
        let painter = context.debug_painter();
        let color = context.global_style().visuals.warn_fg_color;
        painter.debug_rect(rect, color, "");
        painter.debug_text(
            rect.min + egui::vec2(4.0, 4.0),
            egui::Align2::LEFT_TOP,
            color,
            format!(
                "{} {} @ {pointer_x:.1}, {pointer_y:.1}",
                self.kind, self.target
            ),
        );
        painter.circle_stroke(cursor, 7.0, (2.0, color));
        for offset in [egui::vec2(11.0, 0.0), egui::vec2(0.0, 11.0)] {
            painter.line_segment([cursor - offset, cursor + offset], (2.0, color));
        }
    }
}

/// Show run status; return whether Stop was clicked.
pub fn status_view(
    ui: &mut egui::Ui,
    state: &str,
    phase: &str,
    completed: usize,
    total: usize,
    failure: Option<&str>,
) -> bool {
    ui.label(egui::RichText::new("UI automation").strong());
    ui.label(format!("{state} · {phase} · {completed}/{total}"));
    if let Some(failure) = failure {
        ui.label(failure);
    }
    if state != "running" && state != "paused" {
        return false;
    }
    ui.label("Input locked · Esc to stop");
    let stop = ui.button("Stop (Esc)");
    crate::semantics::target(ui, &stop, "automation.stop");
    stop.clicked()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pause {
    pub started_seconds: f64,
    pub duration_seconds: f64,
}

/// Geometry changes observed during a functional run.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ViewportChange {
    pub elapsed_seconds: f64,
    pub previous_viewport: [f32; 2],
    pub viewport: [f32; 2],
    pub previous_pixels_per_point: f32,
    pub pixels_per_point: f32,
}

struct Run {
    steps: Vec<Step>,
    report: Report,
    start: Option<f64>,
    due: f64,
    waiting_since: f64,
    gesture: Option<(Pos2, usize)>,
    target_geometry: Option<(Rect, bool)>,
    stabilizing_since: Option<f64>,
    ready_since: Option<f64>,
    recovering_layout: bool,
    resize_ready_since: Option<f64>,
    restore_viewport: bool,
}

impl Run {
    fn complete_step(&mut self, elapsed: f64, after: f64, waiting: bool) {
        self.report.completed += 1;
        self.gesture = None;
        self.target_geometry = None;
        self.stabilizing_since = None;
        self.ready_since = None;
        self.due = if waiting { elapsed } else { self.due } + after;
        self.waiting_since = elapsed;
        if self.report.completed == self.steps.len() {
            self.report.state = "passed".into();
        }
    }

    fn pointer_ready(&mut self, action: &Action, rect: Rect, elapsed: f64) -> bool {
        if !action.uses_pointer() || self.gesture.is_some() {
            return true;
        }
        // Bounds come from completed layout. Require two matching rendered
        // positions before pressing on an animated control.
        if !self
            .target_geometry
            .is_some_and(|(previous, stable)| stable && previous == rect)
        {
            self.stabilizing_since.get_or_insert(elapsed);
            return false;
        }
        if let Some(started) = self.stabilizing_since.take() {
            // Readiness waits shift scheduling without consuming later actions' deadlines.
            self.due += elapsed - started;
        }
        true
    }

    fn layout_ready(&mut self, action: &Action, value: Option<&str>, elapsed: f64) -> bool {
        if !self.recovering_layout {
            return true;
        }
        if matches!(action, Action::Observe(_))
            && value != Some("ready")
            && !value.is_some_and(|value| value.starts_with("failed:"))
            && elapsed - self.waiting_since <= 30.0
        {
            return false;
        }
        self.due = elapsed;
        self.recovering_layout = false;
        true
    }

    fn map_ready(&mut self, value: Option<&str>, elapsed: f64) -> Result<bool, String> {
        if let Some(failure) = value.filter(|value| value.starts_with("failed:")) {
            return Err(failure.into());
        }
        if value != Some("ready") {
            self.ready_since = None;
            if elapsed - self.waiting_since > 30.0 {
                return Err("visible map preparation/upload readiness timed out".into());
            }
            return Ok(false);
        }
        Ok(elapsed - *self.ready_since.get_or_insert(elapsed) >= 0.5)
    }

    fn recover_layout(&mut self, input: &mut RawInput, held: &mut Option<Pos2>, elapsed: f64) {
        // The target tree still describes the old layout.
        // Release outside controls, then retry the unfinished action after egui lays out the new size.
        if self.gesture.take().is_some()
            && let Some(attempt) = self.report.actions.pop()
        {
            self.report.interrupted_attempts.push(attempt);
        }
        if held.take().is_some() {
            let before = input.events.len();
            release_buttons(input, &[PointerButton::Primary]);
            self.report.input_events += input.events.len() - before;
        }
        self.due = elapsed + 1.0 / 60.0;
        self.waiting_since = elapsed;
        self.ready_since = None;
        self.target_geometry = None;
        self.stabilizing_since = None;
        self.resize_ready_since = None;
        self.recovering_layout = true;
    }

    fn observe_environment(
        &mut self,
        ctx: &Context,
        input: &RawInput,
    ) -> Result<(f64, Rect, bool), String> {
        let now = input
            .time
            .ok_or("automation requires a monotonic input clock")?;
        let epoch = *self.start.get_or_insert(now);
        let elapsed = now - epoch;
        if !elapsed.is_finite() || elapsed + 0.000_001 < self.report.elapsed_seconds {
            return Err("automation clock moved backwards or became invalid".into());
        }
        let elapsed = elapsed.max(self.report.elapsed_seconds);
        if elapsed - self.report.elapsed_seconds > 2.0 {
            return Err("automation frame missed its two second deadline".into());
        }
        self.report.elapsed_seconds = elapsed;
        if elapsed > 120.0 {
            return Err("scenario exceeded its 120 second deadline".into());
        }
        let screen = input.screen_rect.unwrap_or_else(|| {
            ctx.viewport_for(input.viewport_id, |viewport| viewport.input.viewport_rect())
        });
        let viewport = [screen.width(), screen.height()];
        let scale = ctx.zoom_factor() * input.viewport().native_pixels_per_point.unwrap_or(1.0);
        let changed = self.report.viewport[0] > 0.0
            && (self
                .report
                .viewport
                .iter()
                .zip(viewport)
                .any(|(a, b)| (a - b).abs() > 0.5)
                || (self.report.pixels_per_point - scale).abs() > f32::EPSILON);
        if changed {
            self.report.performance_eligible = false;
            self.report.viewport_changes.push(ViewportChange {
                elapsed_seconds: elapsed,
                previous_viewport: self.report.viewport,
                viewport,
                previous_pixels_per_point: self.report.pixels_per_point,
                pixels_per_point: scale,
            });
        }
        self.report.viewport = viewport;
        self.report.pixels_per_point = scale;
        Ok((elapsed, screen, changed))
    }
}

/// Install only in explicitly enabled demo/test sessions.
#[derive(Default)]
pub struct Driver {
    viewport: egui::ViewportId,
    window_scope: bool,
    resize_handler: Option<ResizeHandler>,
    viewport_restore: Option<resize::ViewportRestore>,
    // Hooks run outside the viewport pass.
    // Pair input/output identities explicitly;
    // a stack also handles immediate viewports nested inside the root pass.
    viewport_passes: Vec<egui::ViewportId>,
    tree: Option<kittest::State>,
    run: Option<Run>,
    held: Option<Pos2>,
    release: bool,
    launch_request: Option<&'static str>,
    pub launch_error: Option<String>,
    paused_at: Option<f64>,
    resumed_at: Option<f64>,
}

impl Driver {
    /// Create an independent driver for a child window.
    /// Built-in app scenarios stay on the root.
    #[must_use]
    pub fn for_window(viewport: egui::ViewportId) -> Self {
        Self {
            viewport,
            window_scope: true,
            ..Self::default()
        }
    }

    pub(crate) fn ready(&self) -> bool {
        self.tree.is_some()
    }

    /// Override native viewport resizing, e.g. with a browser canvas adapter.
    #[must_use]
    pub fn with_resize_handler(mut self, handler: ResizeHandler) -> Self {
        self.resize_handler = Some(handler);
        self
    }

    /// Start a built-in workload.
    ///
    /// # Errors
    /// Unknown names and concurrent runs are rejected.
    pub fn start(&mut self, name: &str) -> Result<(), String> {
        if self.window_scope {
            return Err("built-in scenarios require the root window".into());
        }
        let mut steps = scenarios::steps(name).ok_or_else(|| "unknown scenario".to_owned())?;
        if lookup(self.tree.as_ref(), "profile.toggle", Rect::EVERYTHING)?.is_some() {
            let open = lookup(self.tree.as_ref(), "profile.logout", Rect::EVERYTHING)?.is_some();
            steps.splice(0..0, scenarios::return_to_chooser(open));
        }
        let run = self.start_steps(name, steps)?;
        run.restore_viewport = run
            .steps
            .iter()
            .any(|step| matches!(step.action, Action::Resize { .. }));
        Ok(())
    }

    fn start_steps(&mut self, name: &str, steps: Vec<Step>) -> Result<&mut Run, String> {
        if self.running()
            || self.release
            || self.paused_at.is_some()
            || self.resumed_at.is_some()
            || self.viewport_restore.is_some()
        {
            return Err("an automation run is active or releasing input".into());
        }
        tracing::info!(scenario = name, "Automation started");
        self.launch_error = None;
        Ok(self.run.insert(Run {
            report: Report {
                version: 2,
                scenario: name.into(),
                state: "running".into(),
                phase: "setup".into(),
                completed: 0,
                total: steps.len(),
                failure: None,
                elapsed_seconds: 0.0,
                viewport: [0.0; 2],
                pixels_per_point: 1.0,
                viewport_changes: Vec::new(),
                resize_request: None,
                driver_milliseconds: 0.0,
                tree_milliseconds: 0.0,
                readiness: None,
                actions: Vec::with_capacity(steps.len()),
                input_events: 0,
                pauses: Vec::new(),
                interrupted_attempts: Vec::new(),
                performance_eligible: true,
            },
            steps,
            start: None,
            due: 0.0,
            waiting_since: 0.0,
            gesture: None,
            target_geometry: None,
            stabilizing_since: None,
            ready_since: None,
            recovering_layout: false,
            resize_ready_since: None,
            restore_viewport: false,
        }))
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| run.report.state == "running")
    }

    #[must_use]
    pub fn report(&self) -> Option<&Report> {
        self.run.as_ref().map(|run| &run.report)
    }

    pub fn request_launch(&mut self, name: &'static str) {
        self.launch_request = Some(name);
    }

    /// Take the pending menu selection.
    pub fn take_launch_request(&mut self) -> Option<&'static str> {
        self.launch_request.take()
    }

    /// Cancel and release held input on the next frame.
    pub fn cancel(&mut self, reason: &str) {
        self.finish("cancelled", Some(reason.into()));
    }

    fn finish(&mut self, state: &str, failure: Option<String>) {
        if self.running() || self.paused_at.is_some() || self.resumed_at.is_some() {
            self.paused_at = None;
            self.resumed_at = None;
            let run = self.run.as_mut().expect("active implies a run");
            tracing::info!(
                scenario = run.report.scenario,
                outcome = state,
                reason = failure,
                "Automation finished"
            );
            run.report.state = state.into();
            run.report.failure = failure;
            run.report.resize_request = None;
            self.release = self.held.is_some();
        }
    }

    fn release_input(&mut self, input: &mut RawInput) {
        if self.release {
            if self.held.take().is_some() {
                // Release outside controls to avoid click-through and a final drag delta.
                release_buttons(input, &[PointerButton::Primary]);
            }
            self.release = false;
        }
    }

    fn capture_user_input(
        &self,
        ctx: &Context,
        input: &mut RawInput,
    ) -> Result<Option<&'static str>, String> {
        let escape = input.events.iter().any(|event| {
            matches!(
                event,
                Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                }
            )
        });
        let reason = if escape {
            Ok(Some("stopped with Escape"))
        } else {
            let stops = ["automation.stop", "developer.automation.stop"]
                .into_iter()
                .map(|target| {
                    lookup(
                        self.tree.as_ref(),
                        target,
                        ctx.input_for(input.viewport_id, egui::InputState::viewport_rect),
                    )
                })
                .collect::<Result<Vec<_>, _>>();
            stops.map(|stops| {
                input.events.iter().find_map(|event| match event {
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        ..
                    }
                    | Event::Touch {
                        pos,
                        phase: egui::TouchPhase::Start,
                        ..
                    } if stops.iter().flatten().any(|(rect, _)| rect.contains(*pos)) => {
                        Some("stopped with Stop button")
                    }
                    _ => None,
                })
            })
        };
        input
            .events
            .retain(|event| matches!(event, Event::WindowFocused(_) | Event::Screenshot { .. }));
        input.events.push(Event::ModifiersChanged(Modifiers::NONE));
        input.hovered_files.clear();
        input.dropped_files.clear();
        if self.run.as_ref().is_some_and(|run| run.start.is_none()) {
            clear_inherited_input(ctx, input);
        }
        reason
    }

    fn drive(&mut self, ctx: &Context, input: &mut RawInput) -> Result<(), String> {
        let run = self.run.as_mut().expect("only drive an active run");
        let (elapsed, screen, changed) = run.observe_environment(ctx, input)?;
        if changed {
            run.recover_layout(input, &mut self.held, elapsed);
            return Ok(());
        }
        let step = run.steps[run.report.completed].clone();
        run.report.phase = step.phase.into();
        if elapsed < run.due {
            return Ok(());
        }
        if let Action::Resize { width, height } = step.action {
            return run.resize(
                ctx,
                input.viewport_id,
                self.resize_handler,
                [width, height],
                elapsed,
                screen,
            );
        }
        let bounds = lookup(self.tree.as_ref(), &step.target, screen)?;
        let waiting = matches!(
            step.action,
            Action::Wait | Action::Ready | Action::Observe(_)
        );
        let Some((rect, value)) = bounds else {
            if (run.recovering_layout || matches!(step.action, Action::Wait | Action::Ready))
                && elapsed - run.waiting_since <= 30.0
            {
                return Ok(());
            }
            return Err(format!(
                "target missing, disabled or clipped: {}",
                step.target
            ));
        };
        if step.target == "map" {
            run.report.readiness.clone_from(&value);
        }
        if !run.layout_ready(&step.action, value.as_deref(), elapsed) {
            return Ok(());
        }
        if matches!(step.action, Action::Ready) && !run.map_ready(value.as_deref(), elapsed)? {
            return Ok(());
        }
        if !waiting && elapsed - run.due > 2.0 {
            return Err(format!("action missed its deadline: {}", step.target));
        }
        if !run.pointer_ready(&step.action, rect, elapsed) {
            return Ok(());
        }
        let (start, frame) = run.gesture.unwrap_or((rect.center(), 0));
        if let Action::Observe(seconds) = step.action {
            if value.as_deref() != Some("ready") {
                return Err(format!(
                    "map lost readiness during stationary observation: {value:?}"
                ));
            }
            if elapsed - run.due < seconds {
                return Ok(());
            }
        }
        let before = input.events.len();
        let complete = step.input(input, &mut self.held, start, frame, rect, value.as_deref())?;
        run.report.input_events += input.events.len() - before;
        let pointer_position = last_pointer_position(&input.events[before..]);
        if frame == 0 {
            let scheduled = run.due
                + if let Action::Observe(seconds) = step.action {
                    seconds
                } else {
                    0.0
                };
            run.report.actions.push(ActionTiming {
                phase: step.phase.into(),
                kind: step.action.name().into(),
                target: step.target,
                scheduled_seconds: scheduled,
                actual_seconds: elapsed,
                lateness_seconds: (elapsed - scheduled).max(0.0),
                target_bounds: [rect.left(), rect.top(), rect.right(), rect.bottom()],
                pointer_position,
            });
        } else if let Some(action) = run.report.actions.last_mut() {
            action.pointer_position = pointer_position.or(action.pointer_position);
        }
        if complete {
            run.complete_step(elapsed, step.after, waiting);
        } else {
            run.gesture = Some((start, frame + 1));
            run.due += 1.0 / 60.0;
        }
        Ok(())
    }
}

impl Step {
    fn input(
        &self,
        input: &mut RawInput,
        held: &mut Option<Pos2>,
        start: Pos2,
        frame: usize,
        rect: Rect,
        value: Option<&str>,
    ) -> Result<bool, String> {
        let mut complete = true;
        match &self.action {
            Action::Wait | Action::Ready | Action::Observe(_) | Action::Available => {}
            Action::Resize { .. } => unreachable!("resize is handled before semantic input"),
            Action::Click => {
                input.events.push(Event::PointerMoved(start));
                input.events.push(pointer_button(start, frame == 0));
                *held = (frame == 0).then_some(start);
                complete = frame != 0;
            }
            Action::Drag { x, y } => {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "gesture has exactly thirty-two bounded samples"
                )]
                let fraction = frame.min(32) as f32 / 32.0;
                let end = rect.min + rect.size() * egui::vec2(*x, *y);
                let pos = start.lerp(end, fraction);
                input.events.push(Event::PointerMoved(pos));
                if frame == 0 || frame == 33 {
                    input.events.push(pointer_button(pos, frame == 0));
                }
                *held = (frame != 33).then_some(pos);
                complete = frame == 33;
            }
            Action::Wheel(delta) | Action::Scroll(delta) => {
                let point = if matches!(self.action, Action::Scroll(_)) {
                    rect.min + rect.size() * egui::vec2(0.5, 0.95)
                } else {
                    rect.center()
                };
                input.events.push(Event::PointerMoved(point));
                input.events.push(Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, *delta),
                    modifiers: Modifiers::NONE,
                });
            }
            Action::Text(text) => input.events.push(Event::Text(text.clone())),
            Action::Key(key) => {
                for pressed in [true, false] {
                    input.events.push(Event::Key {
                        key: *key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    });
                }
            }
            Action::Value(expected) => {
                if value != Some(expected.as_str()) {
                    return Err(format!(
                        "{}: expected {expected:?}, got {value:?}",
                        self.target
                    ));
                }
            }
        }
        Ok(complete)
    }
}

fn last_pointer_position(events: &[Event]) -> Option<[f32; 2]> {
    events.iter().rev().find_map(|event| match event {
        Event::PointerMoved(position) => Some([position.x, position.y]),
        _ => None,
    })
}

fn pointer_button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn release_buttons(input: &mut RawInput, buttons: &[PointerButton]) {
    let outside = egui::pos2(-10_000.0, -10_000.0);
    input.events.push(Event::PointerMoved(outside));
    for button in buttons {
        input.events.push(Event::PointerButton {
            pos: outside,
            button: *button,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }
    input.events.push(Event::PointerGone);
}

fn clear_inherited_input(ctx: &Context, input: &mut RawInput) {
    for key in ctx.input_for(input.viewport_id, |state| {
        state.keys_down.iter().copied().collect::<Vec<_>>()
    }) {
        input.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    let buttons = [
        PointerButton::Primary,
        PointerButton::Secondary,
        PointerButton::Middle,
        PointerButton::Extra1,
        PointerButton::Extra2,
    ];
    let held = ctx.input_for(input.viewport_id, |state| {
        buttons
            .into_iter()
            .filter(|button| state.pointer.button_down(*button))
            .collect::<Vec<_>>()
    });
    release_buttons(input, &held);
}

fn lookup(
    tree: Option<&kittest::State>,
    target: &str,
    screen: Rect,
) -> Result<Option<(Rect, Option<String>)>, String> {
    let Some(tree) = tree else { return Ok(None) };
    let root = SemanticNode(tree.root());
    let mut matches = root.query_all(By::new().predicate(|node| node.author_id() == Some(target)));
    let Some(node) = matches.next() else {
        return Ok(None);
    };
    let node = node.0;
    if matches.next().is_some() {
        return Err(format!("ambiguous semantic target: {target}"));
    }
    if node.is_disabled() || node.is_hidden() {
        return Ok(None);
    }
    let Some(bounds) = node.bounding_box() else {
        return Ok(None);
    };
    let bounds = tree
        .root()
        .direct_transform()
        .inverse()
        .transform_rect_bbox(bounds);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "egui bounds originate as f32 logical points"
    )]
    let rect = Rect::from_min_max(
        egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
        egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
    .intersect(screen);
    Ok((rect.is_finite() && rect.is_positive()).then(|| (rect, node.value())))
}

impl egui::plugin::Plugin for Driver {
    fn debug_name(&self) -> &'static str {
        "semantic automation"
    }

    fn setup(&mut self, ctx: &Context) {
        ctx.enable_accesskit();
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        self.viewport_passes.push(input.viewport_id);
        if input.viewport_id != self.viewport {
            return;
        }
        self.resume_clock(input);
        let started = web_time::Instant::now();
        let was_running = self.running();
        if was_running || self.release {
            match self.capture_user_input(ctx, input) {
                Ok(Some(reason)) => self.cancel(reason),
                Ok(None) => {}
                Err(reason) => self.finish("failed", Some(reason)),
            }
        }
        if self.running()
            && let Err(reason) = self.save_viewport(ctx, input)
        {
            self.finish("failed", Some(reason));
        }
        if self.running()
            && let Err(reason) = self.drive(ctx, input)
        {
            self.finish("failed", Some(reason));
        }
        self.release_input(input);
        self.restore_viewport(ctx);
        if self.viewport == egui::ViewportId::ROOT {
            crate::diagnostics::automation(ctx, "root", self.report());
        }
        if was_running && let Some(run) = &mut self.run {
            run.report.driver_milliseconds += started.elapsed().as_secs_f64() * 1000.0;
        }
        if self.running() || self.release || self.paused_at.is_some() {
            ctx.request_repaint_of(self.viewport);
        }
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut egui::FullOutput) {
        if self.viewport_passes.pop() != Some(self.viewport) {
            return;
        }
        let started = web_time::Instant::now();
        if let Some(update) = output.platform_output.accesskit_update.clone() {
            if let Some(tree) = &mut self.tree {
                tree.update(update);
            } else {
                self.tree = Some(kittest::State::new(update));
            }
        }
        if self.running()
            && let Some(run) = &mut self.run
        {
            let step = &run.steps[run.report.completed];
            if step.action.uses_pointer() && run.gesture.is_none() {
                let screen = ctx.input_for(self.viewport, egui::InputState::viewport_rect);
                run.target_geometry = lookup(self.tree.as_ref(), &step.target, screen)
                    .ok()
                    .flatten()
                    .map(|(rect, _)| {
                        let stable = run
                            .target_geometry
                            .is_some_and(|(previous, _)| previous == rect);
                        (rect, stable)
                    });
            }
            run.report.tree_milliseconds += started.elapsed().as_secs_f64() * 1000.0;
        }
    }
}

/// Draw outside the driver lock: widget creation can re-enter plugin hooks.
pub fn show_status(context: &Context) {
    let Some(plugin) = context.plugin_opt::<Driver>() else {
        return;
    };
    let status = plugin.lock().report().map(|report| {
        (
            report.state.clone(),
            report.phase.clone(),
            report.completed,
            report.total,
            report.failure.clone(),
            report
                .actions
                .iter()
                .rev()
                .find(|action| action.pointer_position.is_some())
                .cloned(),
        )
    });
    let Some((state, phase, completed, total, failure, attempt)) = status else {
        return;
    };
    if matches!(state.as_str(), "running" | "paused")
        && let Some(attempt) = attempt
    {
        attempt.highlight(context);
    }
    let cancel = egui::Window::new("Automation")
        .id(egui::Id::new("automation-status"))
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .collapsible(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, [-12.0, 48.0])
        .default_width(240.0)
        .show(context, |ui| {
            ui.set_max_width(240.0);
            status_view(ui, &state, &phase, completed, total, failure.as_deref())
        })
        .and_then(|response| response.inner)
        .unwrap_or(false);
    if cancel {
        plugin.lock().cancel("cancelled in status view");
    }
}
