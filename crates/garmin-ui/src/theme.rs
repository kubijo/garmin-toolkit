//! egui application styling.

use cint::ColorInterop;
use egui::{Color32, CornerRadius, Stroke, Style, Ui, style::WidgetVisuals};
use garmin_color::{Color, theme};

/// Applies the default dark application theme.
pub fn apply(style: &mut Style) {
    apply_palette(style, &theme::GRAY_100);
}

/// Applies a semantic application theme.
pub fn apply_palette(style: &mut Style, palette: &theme::Theme) {
    crate::typography::apply(style);
    let surfaces = palette.surfaces();
    let content = palette.content();
    let interaction = palette.interaction();
    let support = palette.support();
    let borders = palette.borders();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    apply_layer_with(style, palette, theme::Level::One);

    let visuals = &mut style.visuals;

    visuals.dark_mode = palette.is_dark();
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(color32(content.text_secondary()));
    visuals.widgets.active = widget(
        interaction.brand(),
        interaction.brand(),
        interaction.focus(),
        content.text_primary(),
    );
    visuals.widgets.open = visuals.widgets.active;
    visuals.selection.bg_fill = color32(interaction.brand());
    visuals.selection.stroke = Stroke::new(1.0, color32(interaction.focus()));
    visuals.hyperlink_color = color32(interaction.link());
    visuals.faint_bg_color = color32(surfaces.background_hover());
    visuals.warn_fg_color = color32(support.warning());
    visuals.error_fg_color = color32(support.error());
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.window_fill = color32(surfaces.layer(theme::Level::One));
    visuals.window_stroke = Stroke::new(1.0, color32(borders.subtle()));
    visuals.menu_corner_radius = CornerRadius::ZERO;
    visuals.panel_fill = color32(surfaces.background());
    visuals.button_frame = true;
    visuals.collapsing_header_frame = false;
    visuals.indent_has_left_vline = false;
    visuals.striped = false;
}

#[must_use]
pub fn palette(ui: &Ui) -> &'static theme::Theme {
    if ui.visuals().dark_mode {
        &theme::GRAY_100
    } else {
        &theme::GRAY_10
    }
}

/// Applies one semantic surface level to nested controls.
pub fn layer<R>(ui: &mut Ui, level: theme::Level, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let palette = palette(ui);
        apply_layer_with(ui.style_mut(), palette, level);
        add(ui)
    })
    .inner
}

fn apply_layer_with(style: &mut Style, palette: &theme::Theme, level: theme::Level) {
    let surfaces = palette.surfaces();
    let content = palette.content();
    let borders = palette.borders();
    let visuals = &mut style.visuals;

    visuals.widgets.noninteractive = widget(
        surfaces.layer(level),
        surfaces.layer(level),
        borders.subtle(),
        content.text_primary(),
    );
    visuals.widgets.inactive = widget(
        surfaces.field(level),
        surfaces.layer(level),
        borders.subtle(),
        content.text_primary(),
    );
    visuals.widgets.hovered = widget(
        surfaces.field_hover(level),
        surfaces.layer_hover(level),
        borders.strong(),
        content.text_primary(),
    );
    visuals.extreme_bg_color = color32(surfaces.field(level));
    visuals.text_edit_bg_color = Some(color32(surfaces.field(level)));
    visuals.code_bg_color = color32(surfaces.layer(level));
}

pub(crate) fn color32(color: Color) -> Color32 {
    Color32::from(color.into_cint())
}

pub(crate) fn progress_bar(progress: f32, fill: Color) -> egui::ProgressBar {
    egui::ProgressBar::new(progress).fill(color32(fill))
}

pub(crate) fn widget(fill: Color, weak: Color, border: Color, foreground: Color) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: color32(fill),
        weak_bg_fill: color32(weak),
        bg_stroke: Stroke::new(1.0, color32(border)),
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0, color32(foreground)),
        expansion: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_style_uses_gray_100_roles_and_square_widgets() {
        let mut style = Style::default();

        apply(&mut style);

        assert_eq!(
            style.visuals.panel_fill,
            color32(theme::GRAY_100.surfaces().background())
        );
        assert_eq!(
            style.visuals.selection.bg_fill,
            color32(theme::GRAY_100.interaction().brand())
        );
        assert_eq!(
            style.visuals.widgets.inactive.corner_radius,
            CornerRadius::ZERO
        );

        apply_layer_with(&mut style, &theme::GRAY_100, theme::Level::Two);
        assert_eq!(
            style.visuals.text_edit_bg_color,
            Some(color32(theme::GRAY_100.surfaces().field(theme::Level::Two)))
        );
    }
}
