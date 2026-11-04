use cint::ColorInterop;
use egui::{Align, Align2, Id, Layout, Order, RichText, Sense, Ui};
use garmin_color::theme;

use crate::{icons, theme as widget_theme, typography};

const CARD_WIDTH: f32 = 400.0;
const VIEWPORT_MARGIN: f32 = 32.0;

#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub elapsed: &'a str,
}

pub fn show(ui: &Ui, id: Id, props: &Props<'_>) {
    let bounds = ui.max_rect();
    let ctx = ui.ctx();
    let palette = widget_theme::palette(ui);
    let icon_color = palette.support().information();
    let overlay = widget_theme::color32(palette.overlay());
    let style = ui.style().clone();

    egui::Area::new(id.with("backdrop"))
        .order(Order::Foreground)
        .fixed_pos(bounds.min)
        .movable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(bounds.size(), Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, overlay);
        });

    egui::Area::new(id.with("surface"))
        .order(Order::Tooltip)
        .fixed_pos(bounds.center())
        .pivot(Align2::CENTER_CENTER)
        .constrain_to(bounds)
        .movable(false)
        .show(ctx, |ui| {
            ui.set_style(style);
            let surfaces = palette.surfaces();
            let content = palette.content();
            let frame = egui::Frame::new()
                .fill(widget_theme::color32(surfaces.layer(theme::Level::One)))
                .corner_radius(widget_theme::FLOATING_RADIUS)
                .stroke(egui::Stroke::new(
                    1.0,
                    widget_theme::color32(palette.borders().subtle()),
                ))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 32,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(128),
                })
                .inner_margin(32);
            let width = CARD_WIDTH
                .min((bounds.width() - VIEWPORT_MARGIN - frame.total_margin().sum().x).max(0.0));
            frame.show(ui, |ui| {
                ui.set_width(width);
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    icons::Props {
                        icon: icons::WIFI_SLASH,
                        size: 48.0,
                        color: icon_color,
                    }
                    .show(ui);
                    ui.add_space(24.0);
                    ui.add(
                        egui::Label::new(
                            typography::semibold(props.title)
                                .size(24.0)
                                .color(content.text_primary().into_cint()),
                        )
                        .selectable(false),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(props.message)
                                .color(content.text_secondary().into_cint()),
                        )
                        .selectable(false)
                        .wrap(),
                    );
                    ui.add_space(24.0);
                    ui.add(
                        egui::Label::new(
                            typography::semibold(props.elapsed)
                                .color(content.text_secondary().into_cint()),
                        )
                        .selectable(false),
                    );
                });
            });
        });
}

#[must_use]
pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn reconnect_card_stays_inside_narrow_viewports_in_both_themes() {
        for dark in [true, false] {
            for width in [320.0, 400.0, 960.0] {
                let context = egui::Context::default();
                crate::install(&context);
                context.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 600.0));
                let id = egui::Id::new("synthetic-offline");
                for tick in 0..4 {
                    context.run_ui(egui::RawInput {
                        screen_rect: Some(bounds),
                        time: Some(f64::from(tick) / 60.0),
                        ..Default::default()
                    }, |ui| {
                        super::show(ui, id, &super::Props {
                            title: "Spojení s aplikací bylo přerušeno",
                            message: "Připojení se automaticky obnovuje. Vaše aktivity zůstanou zachovány.",
                            elapsed: "Offline 01:42",
                        });
                    }).drop_without_applying_deltas();
                }
                let surface = context
                    .memory(|memory| memory.area_rect(id.with("surface")))
                    .expect("reconnect card was drawn");
                assert!(
                    bounds.contains_rect(surface),
                    "card {surface:?} overflows {bounds:?}, dark={dark}"
                );
            }
        }
    }

    #[test]
    fn elapsed_duration_uses_a_stable_clock() {
        assert_eq!(format_duration(0), "00:00");
        assert_eq!(format_duration(61), "01:01");
        assert_eq!(format_duration(3_661), "01:01:01");
    }
}
