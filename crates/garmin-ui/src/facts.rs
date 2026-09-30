//! Compact facts with muted, right-aligned labels and semibold values.

use egui::{Align, Grid, Label, Layout, RichText, TextStyle, Ui, vec2};

/// Shared column widths for one table or several related sections.
pub struct Table {
    label_width: f32,
    value_width: f32,
}

impl Table {
    #[must_use]
    pub fn new<'a>(ui: &Ui, labels: impl IntoIterator<Item = &'a str>) -> Self {
        let secondary = crate::theme::color32(crate::theme::palette(ui).content().text_secondary());
        let label_width = labels
            .into_iter()
            .map(|label| {
                ui.painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        TextStyle::Body.resolve(ui.style()),
                        secondary,
                    )
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
            .min(ui.available_width() * 0.4);
        Self {
            label_width,
            value_width: (ui.available_width() - label_width - 8.0).max(0.0),
        }
    }

    pub fn show(&self, ui: &mut Ui, id: impl egui::AsIdSalt, rows: &[(&str, &str)]) {
        let palette = crate::theme::palette(ui);
        let secondary = crate::theme::color32(palette.content().text_secondary());
        let primary = crate::theme::color32(palette.content().text_primary());
        Grid::new(id)
            .num_columns(2)
            .min_col_width(0.0)
            .min_row_height(0.0)
            .spacing(vec2(8.0, 2.0))
            .show(ui, |ui| {
                for &(label, value) in rows {
                    ui.allocate_ui_with_layout(
                        vec2(self.label_width, 0.0),
                        Layout::top_down(Align::Max),
                        |ui| {
                            ui.set_min_width(self.label_width);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(Label::new(RichText::new(label).color(secondary)).wrap());
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(self.value_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_min_width(self.value_width);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                Label::new(crate::typography::semibold(value).color(primary))
                                    .wrap(),
                            );
                        },
                    );
                    ui.end_row();
                }
            });
    }
}
