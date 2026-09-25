//! Native child-window discovery and isolated semantic drivers.
use super::Spec;
use crate::automation::Driver;
use egui::{Context, ViewportId};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Entry {
    spec: Spec,
    viewport: ViewportId,
    live: Arc<AtomicBool>,
    driver: Driver,
}

#[derive(Default)]
struct Registry {
    next: u64,
    entries: BTreeMap<String, Entry>,
    passes: Vec<ViewportId>,
}

pub(super) struct Registration {
    registry: egui::plugin::TypedPluginHandle<Registry>,
    id: String,
    live: Arc<AtomicBool>,
}

impl Registration {
    pub(super) fn live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        self.registry.lock().entries.remove(&self.id);
    }
}

pub(super) fn register(context: &Context, spec: &Spec) -> Option<Registration> {
    context.plugin_opt::<Driver>()?;
    let registry = context.plugin_or_default::<Registry>();
    let mut state = registry.lock();
    state.next = state.next.checked_add(1)?;
    let id = format!("window-{}", state.next);
    let viewport = ViewportId::from_hash_of(&spec.id);
    let live = Arc::new(AtomicBool::new(true));
    state.entries.insert(
        id.clone(),
        Entry {
            spec: spec.clone(),
            viewport,
            live: live.clone(),
            driver: Driver::for_window(viewport),
        },
    );
    drop(state);
    Some(Registration { registry, id, live })
}

impl egui::plugin::Plugin for Registry {
    fn debug_name(&self) -> &'static str {
        "child window control"
    }

    fn input_hook(&mut self, context: &Context, input: &mut egui::RawInput) {
        self.passes.push(input.viewport_id);
        if let Some(entry) = self
            .entries
            .values_mut()
            .find(|entry| entry.viewport == input.viewport_id && entry.live.load(Ordering::Acquire))
        {
            entry.driver.input_hook(context, input);
        }
    }

    fn output_hook(&mut self, context: &Context, output: &mut egui::FullOutput) {
        let Some(viewport) = self.passes.pop() else {
            return;
        };
        if let Some(entry) = self
            .entries
            .values_mut()
            .find(|entry| entry.viewport == viewport && entry.live.load(Ordering::Acquire))
        {
            entry.driver.output_hook(context, output);
        }
    }
}

/// Route a command to the root or an explicitly selected live child.
/// # Errors
/// Disabled automation, stale handles, unsupported commands, or invalid actions.
pub fn command(
    context: &Context,
    window: Option<&str>,
    operation: &str,
    argument: &Value,
) -> Result<Value, String> {
    context
        .plugin_opt::<Driver>()
        .ok_or("automation is disabled")?;
    let root = window.is_none_or(|id| id == "root");
    let registry = context.plugin_or_default::<Registry>();
    let mut registry = registry.lock();
    if operation == "windows" {
        if !root || !argument.is_null() {
            return Err("windows takes no argument or child selector".into());
        }
        let mut windows = vec![describe(
            context,
            "root",
            "application",
            "Application",
            ViewportId::ROOT,
            true,
        )];
        windows.extend(
            registry
                .entries
                .iter()
                .filter(|(_, entry)| entry.live.load(Ordering::Acquire))
                .map(|(id, entry)| {
                    describe(
                        context,
                        id,
                        &entry.spec.kind,
                        &entry.spec.title,
                        entry.viewport,
                        entry.driver.ready(),
                    )
                }),
        );
        return Ok(json!(windows));
    }
    if root {
        drop(registry);
        if operation == "window.focus" || operation == "window.close" {
            return Err("window focus/close requires an explicit child handle".into());
        }
        return crate::automation::command(context, operation, argument);
    }
    let entry = registry
        .entries
        .get_mut(window.unwrap_or_default())
        .filter(|entry| entry.live.load(Ordering::Acquire))
        .ok_or("window is closed or its handle is stale")?;
    match operation {
        "window.focus" | "window.close" => {
            if !argument.is_null() {
                return Err("window focus/close takes no argument".into());
            }
            let command = if operation == "window.close" {
                entry.live.store(false, Ordering::Release);
                egui::ViewportCommand::Close
            } else {
                egui::ViewportCommand::Focus
            };
            context.send_viewport_cmd_to(entry.viewport, command);
            context.request_repaint_of(ViewportId::ROOT);
            Ok(Value::Null)
        }
        _ => entry.driver.command(context, operation, argument),
    }
}

fn describe(
    context: &Context,
    id: &str,
    kind: &str,
    title: &str,
    viewport: ViewportId,
    ready: bool,
) -> Value {
    context.input_for(viewport, |input| {
        json!({"id":id,"kind":kind,"title":title,"ready":ready,
            "focused":input.focused,"screenshots":viewport == ViewportId::ROOT})
    })
}

/// Capture the root; reject unsupported native immediate child captures.
/// # Errors
/// Unknown windows, unsupported child capture, disabled automation, or capture admission failure.
pub fn capture(context: &Context, window: Option<&str>) -> Result<crate::capture::Ticket, String> {
    context
        .plugin_opt::<Driver>()
        .ok_or("automation is disabled")?;
    if window.is_none_or(|id| id == "root") {
        return crate::capture::request(context);
    }
    let registry = context.plugin_or_default::<Registry>();
    let registry = registry.lock();
    registry
        .entries
        .get(window.unwrap_or_default())
        .filter(|entry| entry.live.load(Ordering::Acquire))
        .ok_or("window is closed or its handle is stale")?;
    Err("screenshots are unsupported for native immediate child windows".into())
}

#[cfg(test)]
mod tests;
