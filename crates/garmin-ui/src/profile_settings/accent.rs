//! Profile accent selection using the shared egui-elegance picker.
use super::{Action, Props};
use crate::{Size, button, semantics, theme, typography};
use egui::{Color32, Ui};
use garmin_color::{Color, swatch};
use garmin_i18n::format_message;

#[derive(Clone)]
struct Draft {
    source: Option<Color>,
    color: Color32,
}

impl Draft {
    fn load(ui: &Ui, props: &Props<'_>) -> Self {
        ui.data_mut(|data| data.get_temp::<Self>(props.id.with("profile-accent-draft")))
            .filter(|draft| draft.source == props.accent)
            .unwrap_or_else(|| Self {
                source: props.accent,
                color: theme::color32(props.accent.unwrap_or(swatch::ACTION)),
            })
    }

    fn accent(&self) -> Color {
        let [r, g, b, a] = self.color.to_srgba_unmultiplied();
        Color::from_rgba(r, g, b, a)
    }
}

pub(super) fn preview(ui: &Ui, props: &Props<'_>) -> Color {
    Draft::load(ui, props).accent()
}

pub(super) fn show(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let intl = props.intl;
    ui.label(format_message!(intl, default_message: "Profile accent color"));
    typography::body(
        ui,
        &format_message!(intl, default_message: "Identifies your profile in avatars and profile lists."),
    );
    let id = props.id.with("profile-accent-draft");
    let mut draft = Draft::load(ui, props);
    let mut action = None;
    ui.add_enabled_ui(!props.disabled, |ui| {
        install_picker_theme(ui);
        // The upstream picker does not expose popover width. Bound the Area's
        // default only during this widget so swatches stay compact.
        let style = ui.ctx().global_style();
        ui.ctx()
            .global_style_mut(|style| style.spacing.default_area_size.x = 288.0);
        let response = button::interaction_cursor(
            ui.add(
                elegance::ColorPicker::new(id, &mut draft.color)
                    .palette(presets())
                    .palette_columns(8)
                    .recents_max(8),
            ),
        );
        ui.ctx().set_global_style(style);
        semantics::target(ui, &response, "profile.accent.picker");
        if response.changed() {
            // The avatar appears above the picker and was already painted this frame.
            ui.ctx().request_repaint();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let apply = format_message!(intl, default_message: "Apply color");
            let response = button::Props {
                label: &apply,
                icon: None,
                kind: button::Kind::Secondary,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: draft.color != theme::color32(props.accent.unwrap_or(swatch::ACTION)),
            }
            .show(ui);
            semantics::target(ui, &response, "profile.accent.apply");
            if response.clicked() {
                action = Some(Action::UpdateAccent(Some(draft.accent())));
            }
            let reset = format_message!(intl, default_message: "Use default color");
            let response = button::Props {
                label: &reset,
                icon: None,
                kind: button::Kind::Ghost,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: props.accent.is_some(),
            }
            .show(ui);
            semantics::target(ui, &response, "profile.accent.reset");
            if response.clicked() {
                action = Some(Action::UpdateAccent(None));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(id, draft));
    action
}

fn presets() -> [Color32; 8] {
    [
        swatch::ACTION,
        swatch::purple::G50,
        swatch::magenta::G50,
        swatch::red::G50,
        swatch::orange::G50,
        swatch::green::G50,
        swatch::teal::G50,
        swatch::cyan::G50,
    ]
    .map(theme::color32)
}

// Elegance reads its own context theme. Map its picker to our semantic palette,
// and preserve the application's global style when installing that adapter.
fn install_picker_theme(ui: &Ui) {
    let palette = theme::palette(ui);
    let mut picker = if palette.is_dark() {
        elegance::Theme::charcoal()
    } else {
        elegance::Theme::paper()
    };
    let colors = &mut picker.palette;
    colors.bg = theme::color32(palette.surfaces().background());
    colors.card = ui.visuals().window_fill;
    colors.input_bg = ui.visuals().extreme_bg_color;
    colors.border = theme::color32(palette.borders().subtle());
    colors.text = theme::color32(palette.content().text_primary());
    colors.text_muted = theme::color32(palette.content().text_secondary());
    colors.text_faint = colors.text_muted;
    colors.focus = theme::color32(palette.interaction().interactive());
    picker.control_radius = 0.0;
    picker.card_radius = 0.0;
    picker.card_padding = 16.0;
    picker.typography.body = 14.0;
    picker.typography.small = 12.0;
    if elegance::Theme::current(ui.ctx()) != picker {
        let style = ui.ctx().global_style();
        picker.install(ui.ctx());
        ui.ctx().set_global_style(style);
    }
}
