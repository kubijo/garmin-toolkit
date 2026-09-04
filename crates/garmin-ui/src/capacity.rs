//! One independently addressable device storage.

use egui::{RichText, TextStyle, Ui};

pub struct Props<'a> {
    pub label: &'a str,
    pub detail: &'a str,
    /// Used and total bytes, when known.
    pub bytes: Option<(u64, u64)>,
}

pub fn show(ui: &mut Ui, props: &Props<'_>) {
    let label = crate::text::balanced(ui, props.label, &TextStyle::Body, true);
    ui.label(RichText::new(label).strong());
    if let Some((used, total)) = props.bytes.filter(|(used, total)| used <= total) {
        // Scale before conversion; byte counts must remain valid on WASM32.
        let fraction = if total == 0 {
            0
        } else {
            u128::from(used) * 10_000 / u128::from(total)
        };
        let fraction = u16::try_from(fraction).unwrap_or(10_000);
        ui.add(
            egui::ProgressBar::new(f32::from(fraction) / 10_000.0)
                .desired_width(ui.available_width())
                .desired_height(6.0),
        );
    }
    let detail = crate::text::balanced(ui, props.detail, &TextStyle::Small, false);
    ui.label(RichText::new(detail).small());
    ui.add_space(8.0);
}
