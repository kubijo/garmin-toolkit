//! One plotting vocabulary for compact bars and cumulative comparisons.

#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Plot sample positions, bounded tick counts, and pixel coordinates use floating values."
)]

use egui::{Color32, Shape, Stroke, Ui};
use egui_plot::{Bar, BarChart, GridMark, Line, LineStyle, Plot, Points};

pub struct Series<'a> {
    pub name: &'a str,
    pub values: &'a [Option<f64>],
    pub color: Color32,
    pub dashed: bool,
    pub dates: Option<&'a [String]>,
}

#[derive(Clone, Copy)]
pub struct Props<'a> {
    pub labels: &'a [String],
    pub series: &'a [Series<'a>],
    pub kind: Kind,
    pub height: f32,
    pub integral: bool,
    pub intl: &'a garmin_i18n::Intl,
    pub unit: &'a str,
}

#[derive(Clone, Copy)]
pub enum Kind {
    Bars,
    Lines,
}

/// Bounds and labels use the same step; count axes never introduce fractional ticks.
#[must_use]
pub fn scale(maximum: f64, integral: bool) -> (f64, f64) {
    let peak = if maximum > 0.0 { maximum } else { 1.0 };
    let raw = peak / 2.0;
    let magnitude = 10_f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|step| step * magnitude)
        .find(|step| *step >= raw)
        .unwrap_or(magnitude * 10.0);
    let step = if integral { step.ceil().max(1.0) } else { step };
    ((peak / step).ceil() * step, step)
}

pub fn show(ui: &mut Ui, id: impl egui::AsIdSalt, props: Props<'_>) -> egui::Rect {
    let Props {
        labels,
        series,
        kind,
        height,
        integral,
        intl,
        unit,
    } = props;
    let maximum = series
        .iter()
        .flat_map(|series| series.values)
        .flatten()
        .copied()
        .fold(0.0, f64::max);
    let length = labels.len().max(1);
    let scale_maximum = if matches!(kind, Kind::Lines) && length == 1 {
        maximum * 1.01
    } else {
        maximum
    };
    let (upper, step) = scale(scale_maximum, integral);
    let grid = crate::theme::color32(crate::theme::palette(ui).borders().subtle());
    let background = ui.painter().add(Shape::Noop);
    let missing_background = ui.painter().add(Shape::Noop);
    let plot = Plot::new(id)
        .height(height)
        .show_background(false)
        .show_grid(false)
        .show_axes(true)
        .custom_y_axes(vec![egui_plot::AxisHints::new_y().min_thickness(32.0)])
        .custom_x_axes(vec![
            egui_plot::AxisHints::new_x()
                .min_thickness(20.0)
                .formatter(|_, _| String::new()),
        ])
        .label_formatter(move |hover| hover_label(hover, series, labels, intl, unit))
        .allow_drag(false)
        .allow_axis_zoom_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show_crosshair(false)
        .cursor_color(Color32::TRANSPARENT)
        .set_margin_fraction(egui::Vec2::ZERO)
        .include_x(-0.5)
        .include_x(length as f64 - 0.5)
        .include_y(0.0)
        .include_y(upper)
        .x_grid_spacer(move |_| {
            (0..length)
                .map(|index| GridMark {
                    value: index as f64,
                    step_size: 1.0,
                })
                .collect()
        })
        .y_grid_spacer(move |_| {
            (0..=(upper / step).round() as usize)
                .map(|index| GridMark {
                    value: index as f64 * step,
                    step_size: step,
                })
                .collect()
        })
        .y_axis_formatter(move |mark, _| tick_label(mark.value, step, integral))
        .show(ui, |plot| paint_series(plot, series, kind));
    paint_grid(ui, background, &plot, length, upper, step, grid);
    paint_axis_labels(ui, &plot, labels);
    paint_missing(ui, missing_background, &plot, series, labels, intl);
    plot.response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Other,
            true,
            series.iter().map(|s| s.name).collect::<Vec<_>>().join(", "),
        )
    });
    *plot.transform.frame()
}

pub fn legend(ui: &mut Ui, frame: egui::Rect, series: &[Series<'_>]) {
    let padding = (frame.left() - ui.max_rect().left()).max(0.0).round() as i32;
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 16.0;
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: i8::try_from(padding).unwrap_or(32),
                ..egui::Margin::ZERO
            })
            .show(ui, |ui| {
                let layout = if ui.available_width() >= 640.0 {
                    egui::Layout::left_to_right(egui::Align::Center)
                } else {
                    egui::Layout::top_down(egui::Align::Min)
                };
                ui.with_layout(layout, |ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for entry in series {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(24.0, 12.0), egui::Sense::hover());
                            let points = [rect.left_center(), rect.right_center()];
                            if entry.dashed {
                                ui.painter().extend(Shape::dashed_line(
                                    &points,
                                    Stroke::new(2.0, entry.color),
                                    4.0,
                                    3.0,
                                ));
                            } else {
                                ui.painter()
                                    .line_segment(points, Stroke::new(2.0, entry.color));
                            }
                            ui.add(
                                egui::Label::new(egui::RichText::new(entry.name).size(12.0)).wrap(),
                            );
                        });
                    }
                });
            });
    });
}

fn paint_axis_labels(ui: &Ui, plot: &egui_plot::PlotResponse<()>, labels: &[String]) {
    if labels.is_empty() {
        return;
    }
    let frame = *plot.transform.frame();
    let stroke = Stroke::new(
        1.0,
        crate::theme::color32(crate::theme::palette(ui).borders().subtle()),
    );
    let mut indices = vec![0, labels.len() / 2, labels.len() - 1];
    indices.sort_unstable();
    indices.dedup();
    for index in indices {
        let x = plot
            .transform
            .position_from_point(&egui_plot::PlotPoint::new(index as f64, 0.0))
            .x;
        ui.painter().line_segment(
            [
                egui::pos2(x, frame.bottom()),
                egui::pos2(x, frame.bottom() + 4.0),
            ],
            stroke,
        );
        let anchor = if index == 0 {
            egui::Align2::LEFT_TOP
        } else if index + 1 == labels.len() {
            egui::Align2::RIGHT_TOP
        } else {
            egui::Align2::CENTER_TOP
        };
        ui.painter().text(
            egui::pos2(x, frame.bottom() + 6.0),
            anchor,
            axis_label(labels, index as f64),
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
    }
}

fn axis_label(labels: &[String], value: f64) -> String {
    let index = value.round() as usize;
    if (value - index as f64).abs() > 0.01
        || !(index == 0 || index + 1 == labels.len() || index == labels.len() / 2)
    {
        return String::new();
    }
    labels
        .get(index)
        .map(|label| {
            label.parse::<jiff::civil::Date>().map_or_else(
                |_| label.clone(),
                |date| {
                    if labels.len() > 1
                        && labels.iter().all(|label| {
                            label
                                .parse::<jiff::civil::Date>()
                                .is_ok_and(|other| other.day() == 1)
                        })
                    {
                        date.month().to_string()
                    } else {
                        date.day().to_string()
                    }
                },
            )
        })
        .unwrap_or_default()
}

fn tick_label(value: f64, step: f64, integral: bool) -> String {
    let decimals = if integral {
        0
    } else {
        (0..=6)
            .find(|digits| {
                let scaled = step * 10_f64.powi(*digits);
                (scaled - scaled.round()).abs() < 0.000_001
            })
            .unwrap_or(6)
    };
    format!("{:.*}", decimals as usize, value)
}

fn hover_label(
    hover: &egui_plot::HoverPosition<'_>,
    series: &[Series<'_>],
    labels: &[String],
    intl: &garmin_i18n::Intl,
    unit: &str,
) -> Option<String> {
    let egui_plot::HoverPosition::NearDataPoint {
        plot_name,
        position,
        ..
    } = hover
    else {
        return None;
    };
    let index = position.x.round() as usize;
    let source = series.iter().find(|series| series.name == *plot_name)?;
    let date = source.dates.unwrap_or(labels).get(index)?;
    let label = date
        .parse::<jiff::civil::Date>()
        .map_or_else(|_| date.clone(), |date| intl.dates().date(date));
    let value = if unit.is_empty() {
        format!("{:.0}", position.y)
    } else {
        format!("{:.2} {}", position.y, unit)
    };
    Some(format!("{plot_name}\n{label}\n{value}"))
}

fn paint_missing(
    ui: &Ui,
    background: egui::layers::ShapeIdx,
    plot: &egui_plot::PlotResponse<()>,
    series: &[Series<'_>],
    labels: &[String],
    intl: &garmin_i18n::Intl,
) {
    let frame = *plot.transform.frame();
    let mut shapes = Vec::new();
    let color = crate::theme::color32(crate::theme::palette(ui).borders().strong());
    for (series_index, series) in series.iter().enumerate() {
        for (index, value) in series.values.iter().enumerate() {
            if value.is_some() {
                continue;
            }
            let left = plot
                .transform
                .position_from_point(&egui_plot::PlotPoint::new(index as f64 - 0.32, 0.0))
                .x;
            let right = plot
                .transform
                .position_from_point(&egui_plot::PlotPoint::new(index as f64 + 0.32, 0.0))
                .x;
            let rect = egui::Rect::from_min_max(
                egui::pos2(left, frame.top()),
                egui::pos2(right, frame.bottom()),
            )
            .intersect(frame);
            shapes.push(Shape::rect_filled(rect, 0, color.gamma_multiply(0.12)));
            for step in 0..=((rect.height() + rect.width()) / 6.0).ceil() as usize {
                let start = rect.top() - rect.width() + step as f32 * 6.0;
                let p1 = egui::pos2(rect.left(), start.max(rect.top()));
                let p2 = egui::pos2(rect.right(), (start + rect.width()).min(rect.bottom()));
                if p1.y <= rect.bottom() && p2.y >= rect.top() {
                    shapes.push(Shape::line_segment(
                        [p1, p2],
                        Stroke::new(1.0, color.gamma_multiply(0.25)),
                    ));
                }
            }
            let date = series
                .dates
                .unwrap_or(labels)
                .get(index)
                .and_then(|label| label.parse::<jiff::civil::Date>().ok())
                .map(|date| intl.dates().date(date))
                .unwrap_or_default();
            let missing =
                garmin_i18n::format_message!(intl, default_message: "Missing measurement");
            let label = format!("{date} · {missing}");
            let response = ui.interact(
                rect,
                ui.id().with(("missing-measurement", series_index, index)),
                egui::Sense::hover(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    true,
                    format!("{}: {}", series.name, label),
                )
            });
            response.on_hover_text(format!("{}: {}", series.name, label));
        }
    }
    ui.painter().set(background, Shape::Vec(shapes));
}

fn paint_series(plot: &mut egui_plot::PlotUi<'_>, series: &[Series<'_>], kind: Kind) {
    let count = series.len().max(1) as f64;
    for (series_index, series) in series.iter().enumerate() {
        match kind {
            Kind::Bars => {
                let bars = series
                    .values
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        let width = 0.64 / count;
                        let offset = ((series_index as f64 + 0.5) / count - 0.5) * 0.64;
                        value.map(|value| {
                            Bar::new(index as f64 + offset, value)
                                .width(width)
                                .fill(series.color)
                                .stroke(Stroke::NONE)
                        })
                    })
                    .collect();
                plot.bar_chart(BarChart::new(series.name, bars).color(series.color));
            }
            Kind::Lines => {
                let mut points = Vec::new();
                for (index, value) in series
                    .values
                    .iter()
                    .enumerate()
                    .chain(std::iter::once((series.values.len(), &None)))
                {
                    if let Some(value) = value {
                        points.push([index as f64, *value]);
                    } else if !points.is_empty() {
                        if points.len() == 1 {
                            plot.points(
                                Points::new(series.name, std::mem::take(&mut points))
                                    .color(series.color)
                                    .radius(3.0),
                            );
                            continue;
                        }
                        let mut line = Line::new(series.name, std::mem::take(&mut points))
                            .color(series.color)
                            .width(2.0);
                        if series.dashed {
                            line = line.style(LineStyle::dashed_dense());
                        }
                        plot.line(line);
                    }
                }
            }
        }
    }
}

fn paint_grid(
    ui: &Ui,
    background: egui::layers::ShapeIdx,
    plot: &egui_plot::PlotResponse<()>,
    length: usize,
    upper: f64,
    step: f64,
    grid: Color32,
) {
    let transform = &plot.transform;
    let frame = *transform.frame();
    let mut shapes = Vec::new();
    let stroke = Stroke::new(1.0, grid);
    for index in 0..length {
        let x = transform
            .position_from_point(&egui_plot::PlotPoint::new(index as f64, 0.0))
            .x;
        shapes.extend(Shape::dashed_line(
            &[egui::pos2(x, frame.top()), egui::pos2(x, frame.bottom())],
            stroke,
            2.0,
            3.0,
        ));
    }
    for index in 0..=(upper / step).round() as usize {
        let y = transform
            .position_from_point(&egui_plot::PlotPoint::new(0.0, index as f64 * step))
            .y;
        shapes.extend(Shape::dashed_line(
            &[egui::pos2(frame.left(), y), egui::pos2(frame.right(), y)],
            stroke,
            2.0,
            3.0,
        ));
        shapes.push(Shape::line_segment(
            [
                egui::pos2(frame.left() - 4.0, y),
                egui::pos2(frame.left(), y),
            ],
            stroke,
        ));
    }
    shapes.push(Shape::line_segment(
        [frame.left_top(), frame.left_bottom()],
        stroke,
    ));
    shapes.push(Shape::line_segment(
        [frame.left_bottom(), frame.right_bottom()],
        stroke,
    ));
    ui.painter().set(background, Shape::Vec(shapes));
}

#[cfg(test)]
mod tests {
    #[test]
    fn ticks_have_exact_positions_and_integral_count_steps() {
        assert_eq!(super::scale(3.05, false), (4.0, 2.0));
        assert_eq!(super::scale(4.6, true), (6.0, 3.0));
        assert_eq!(super::scale(0.0, true), (1.0, 1.0));
    }
}
