//! One independently addressable device storage.

use egui::{RichText, TextStyle, Ui};
use garmin_model::artifact::ByteCount;

pub struct Props<'a> {
    pub label: &'a str,
    pub detail: &'a str,
    /// Used and total bytes, when known.
    pub bytes: Option<(ByteCount, ByteCount)>,
}

pub fn show(ui: &mut Ui, props: &Props<'_>) {
    let label = crate::text::balanced(ui, props.label, &TextStyle::Body, true);
    ui.label(RichText::new(label).strong());
    if let Some((used, total)) = props.bytes.filter(|(used, total)| used <= total) {
        let palette = crate::theme::palette(ui);
        // Scale before conversion; byte counts must remain valid on WASM32.
        let fraction = used.ratio_scaled(total, 10_000);
        ui.add(
            crate::theme::progress_bar(
                f32::from(fraction) / 10_000.0,
                palette.interaction().interactive(),
            )
            .desired_width(ui.available_width())
            .desired_height(6.0),
        );
    }
    let detail = crate::text::balanced(ui, props.detail, &TextStyle::Small, false);
    ui.label(RichText::new(detail).small());
    ui.add_space(8.0);
}
