//! Platform-owned secondary windows with typed content and commands.
use egui::{Context, Ui};
use serde::{Deserialize, Serialize};

mod native;
pub use native::{NativeWindow, surface};
mod resize;
pub use resize::resize;
pub mod protocol;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Spec {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub size: [f32; 2],
}

pub enum Event<C> {
    Command { id: u32, command: C },
    Closed,
}

/// Hosts own window handles and delivery; consumers own state and operations.
pub trait WindowHost {
    type Command;
    type Snapshot;

    /// Open or focus this logical window.
    /// # Errors
    /// The platform could not create or focus the window.
    fn open(&mut self, context: &Context, spec: Spec) -> Result<(), String>;
    fn close(&mut self, context: &Context);
    fn is_open(&self) -> bool;

    /// Native hosts render locally; browser hosts lazily publish snapshots on request.
    fn present(
        &mut self,
        context: &Context,
        intl: &garmin_i18n::Intl,
        snapshot: impl FnOnce() -> Self::Snapshot,
        render: impl FnMut(&mut Ui) -> Option<Self::Command>,
    ) -> Vec<Event<Self::Command>>;

    /// Acknowledge a command after the consumer has admitted or rejected it.
    fn reply(&mut self, id: u32, error: Option<String>);
}
