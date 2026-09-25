//! Typed, session-scoped messages between an app tab and its tools window.
use garmin_ui::developer::Automation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub struct Snapshot {
    pub automation: Automation,
    pub debug: String,
    pub renderer: Option<String>,
    pub viewport: [f32; 2],
    pub scale: f32,
    pub dark: bool,
}
