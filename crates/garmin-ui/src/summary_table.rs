//! Read-only summary tables with wrapping cells and a total in the final column.

use egui::{Align, Color32, Frame, Label, Layout, RichText, Stroke, Ui, accesskit::Role};
use garmin_color::theme::Level;

use crate::{theme, typography};

/// Column width is proportional to its share of the table.
#[derive(Clone, Copy)]
pub struct Column<'a> {
    label: &'a str,
    share: u8,
    align: Align,
}

impl<'a> Column<'a> {
    #[must_use]
    pub const fn new(label: &'a str, share: u8, align: Align) -> Self {
        Self {
            label,
            share,
            align,
        }
    }
}

pub struct Cell {
    pub text: RichText,
    pub detail: Option<String>,
    pub fraction: Option<f32>,
}

impl Cell {
    #[must_use]
    pub fn new(text: impl Into<RichText>) -> Self {
        Self {
            text: text.into(),
            detail: None,
            fraction: None,
        }
    }
}

pub fn show<const N: usize>(
    ui: &mut Ui,
    label: &str,
    columns: [Column<'_>; N],
    rows: impl IntoIterator<Item = [Cell; N]>,
    total: Option<(&str, &str)>,
) {
    const {
        assert!(N >= 2, "a summary table needs a label and a value column");
    }
    let palette = theme::palette(ui);
    let rule = Stroke::new(1.0, theme::color32(palette.borders().subtle()));
    let header_fill = theme::color32(palette.surfaces().layer_hover(Level::One));
    let stripe = theme::table_stripe(ui);
    Frame::NONE.stroke(rule).show(ui, |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Table);
            node.set_label(label);
            node.set_column_count(N);
        });
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        let width = ui.available_width();
        let shares: f32 = columns.iter().map(|col| f32::from(col.share.max(1))).sum();
        let widths = columns.map(|col| width * f32::from(col.share.max(1)) / shares);
        let layout = std::array::from_fn::<_, N, _>(|i| (widths[i], columns[i].align, 1));
        let headings = columns.map(|col| Cell::new(typography::semibold(col.label)));
        row(ui, header_fill, rule, Role::ColumnHeader, layout, headings);
        for (index, cells) in rows.into_iter().enumerate() {
            let fill = if index % 2 == 1 {
                stripe
            } else {
                Color32::TRANSPARENT
            };
            row(ui, fill, rule, Role::Cell, layout, cells);
        }
        let Some(total) = total else { return };
        let last = widths[N - 1];
        row(
            ui,
            Color32::TRANSPARENT,
            Stroke::NONE,
            Role::Cell,
            [(width - last, Align::Min, N - 1), (last, Align::Max, 1)],
            [
                Cell::new(typography::semibold(total.0).size(14.0)),
                Cell::new(typography::semibold(total.1).size(16.0)),
            ],
        );
    });
}

fn row<const N: usize>(
    ui: &mut Ui,
    fill: Color32,
    border: Stroke,
    role: Role,
    layout: [(f32, Align, usize); N],
    cells: [Cell; N],
) {
    let mut progress_fills = Vec::new();
    let rect = Frame::NONE
        .fill(fill)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.ctx()
                    .accesskit_node_builder(ui.unique_id(), |node| node.set_role(Role::Row));
                for ((width, align, span), cell) in layout.into_iter().zip(cells) {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        Layout::top_down(align),
                        |ui| {
                            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                                node.set_role(role);
                                node.set_column_span(span);
                            });
                            if let Some(fraction) =
                                cell.fraction.filter(|v| v.is_finite() && *v > 0.0)
                            {
                                let background = ui.painter().add(egui::Shape::Noop);
                                let left = ui.max_rect().left();
                                progress_fills.push((background, left, width * fraction.min(1.0)));
                            }
                            Frame::NONE.inner_margin(8).show(ui, |ui| {
                                ui.set_width((width - 16.0).max(0.0));
                                ui.add(Label::new(cell.text).wrap().selectable(false));
                                if let Some(detail) = cell.detail {
                                    ui.add_space(2.0);
                                    typography::helper(ui, &detail);
                                }
                            });
                        },
                    );
                }
            });
        })
        .response
        .rect;
    for (background, left, width) in progress_fills {
        let filled = egui::Rect::from_x_y_ranges(left..=left + width, rect.y_range());
        ui.painter().set(
            background,
            egui::Shape::rect_filled(filled, 0, theme::table_progress_fill(ui)),
        );
    }
    if border != Stroke::NONE {
        ui.painter().hline(rect.x_range(), rect.bottom(), border);
        let mut x = rect.left();
        for (width, _, _) in layout.into_iter().take(N - 1) {
            x += width;
            ui.painter().vline(x, rect.y_range(), border);
        }
    }
}
