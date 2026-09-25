//! Stable AccessKit target identifiers.

/// Publish a control's state on its accessibility node.
pub fn value(response: &egui::Response, value: impl Into<String>) {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_value(value.into()));
}

/// Attach a locale-independent identifier and clipped bounds to an accessible response.
pub fn target(ui: &egui::Ui, response: &egui::Response, name: impl Into<String>) {
    let clipped = response.rect.intersect(ui.clip_rect());
    let visible = ui.is_visible();
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_author_id(name.into());
        // AccessKit transforms must not turn an inverted clip into a clickable rectangle.
        let bounds = if visible && clipped.is_positive() && clipped.is_finite() {
            clipped
        } else {
            node.set_hidden();
            egui::Rect::ZERO
        };
        node.set_bounds(egui::accesskit::Rect {
            x0: f64::from(bounds.left()),
            y0: f64::from(bounds.top()),
            x1: f64::from(bounds.right()),
            y1: f64::from(bounds.bottom()),
        });
    });
}
