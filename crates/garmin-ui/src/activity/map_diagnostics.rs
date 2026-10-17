//! Technical renderer identity supplied by the browser host, separate from map performance counters.

use egui::{Context, Id, Ui};
use serde::Deserialize;

/// The same renderer identity shown in browser startup diagnostics.
#[derive(Clone, Deserialize)]
pub struct RendererDiagnostics {
    pub label: String,
    pub detail: String,
}

impl RendererDiagnostics {
    /// Publish the latest host state for the map footer and developer panel.
    pub fn install(self, context: &Context) {
        crate::diagnostics::publish(
            context,
            garmin_model::diagnostics::Observation {
                kind: "renderer".into(),
                window: "root".into(),
                removed: false,
                fields: std::collections::BTreeMap::from([
                    ("label".into(), self.label.clone()),
                    ("detail".into(), self.detail.clone()),
                ]),
            },
        );
        context.data_mut(|data| data.insert_temp(Id::new("map-renderer-diagnostics"), self));
    }

    /// Render a compact technical identity with full details on hover.
    pub fn show(&self, ui: &mut Ui) {
        ui.add(egui::Label::new(egui::RichText::new(&self.label).small().weak()).truncate())
            .on_hover_text(&self.detail);
    }

    pub(super) fn show_installed(ui: &mut Ui) {
        let diagnostics =
            ui.data(|data| data.get_temp::<Self>(Id::new("map-renderer-diagnostics")));
        if let Some(diagnostics) = diagnostics {
            diagnostics.show(ui);
        }
    }
}
