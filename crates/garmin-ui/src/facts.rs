//! Compact facts with muted, right-aligned labels and semibold values.

use egui::{Align, Grid, Label, Layout, Rect, RichText, TextStyle, Ui, UiBuilder, vec2};

/// Render a standalone table from owned or borrowed labels and values.
pub fn show<L: AsRef<str>, V: AsRef<str>>(ui: &mut Ui, id: impl egui::AsIdSalt, rows: &[(L, V)]) {
    Table::new(ui, rows.iter().map(|(label, _)| label.as_ref())).show(ui, id, rows);
}

/// Shared column widths for one table or several related sections.
pub struct Table {
    label_width: f32,
    nonbreaking_label_width: f32,
}

impl Table {
    #[must_use]
    pub fn new<'a>(ui: &Ui, labels: impl IntoIterator<Item = &'a str>) -> Self {
        let secondary = crate::theme::color32(crate::theme::palette(ui).content().text_secondary());
        let mut nonbreaking_label_width = 0.0_f32;
        let label_width = labels
            .into_iter()
            .map(|label| {
                let width = ui
                    .painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        TextStyle::Body.resolve(ui.style()),
                        secondary,
                    )
                    .size()
                    .x;
                if label.contains('\u{a0}') {
                    nonbreaking_label_width = nonbreaking_label_width.max(width);
                }
                width
            })
            .fold(0.0_f32, f32::max);
        Self {
            label_width,
            nonbreaking_label_width,
        }
    }

    pub fn show<L: AsRef<str>, V: AsRef<str>>(
        &self,
        ui: &mut Ui,
        id: impl egui::AsIdSalt,
        rows: &[(L, V)],
    ) {
        let palette = crate::theme::palette(ui);
        let secondary = crate::theme::color32(palette.content().text_secondary());
        let primary = crate::theme::color32(palette.content().text_primary());
        let value_width = rows
            .iter()
            .map(|(_, value)| {
                egui::WidgetText::from(crate::typography::semibold(value.as_ref()))
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        TextStyle::Body,
                    )
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let available = (ui.available_width() - 8.0).max(0.0);
        let minimum_value = rows
            .iter()
            .flat_map(|(_, value)| value.as_ref().split_whitespace())
            .map(|word| {
                egui::WidgetText::from(crate::typography::semibold(word))
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        TextStyle::Body,
                    )
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let scale = (available / (self.label_width + minimum_value).max(1.0)).min(1.0);
        let label_width =
            (self.label_width * scale).max(self.nonbreaking_label_width.min(available));
        let wrapped = label_width + value_width > available || scale < 1.0;
        let value_width = value_width.min(available - label_width);
        Grid::new(id)
            .num_columns(2)
            .min_col_width(0.0)
            .min_row_height(0.0)
            .spacing(vec2(8.0, if wrapped { 8.0 } else { 2.0 }))
            .show(ui, |ui| {
                for (label, value) in rows {
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(Rect::from_min_size(
                                ui.available_rect_before_wrap().min,
                                vec2(label_width, 0.0),
                            ))
                            .layout(Layout::top_down(Align::Max)),
                        |ui| {
                            ui.set_width(label_width);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                Label::new(RichText::new(label.as_ref()).color(secondary)).wrap(),
                            );
                        },
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(Rect::from_min_size(
                                ui.available_rect_before_wrap().min,
                                vec2(value_width, 0.0),
                            ))
                            .layout(Layout::top_down(Align::Min)),
                        |ui| {
                            ui.set_width(value_width);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                Label::new(
                                    crate::typography::semibold(value.as_ref()).color(primary),
                                )
                                .wrap(),
                            );
                        },
                    );
                    ui.end_row();
                }
            });
    }
}
