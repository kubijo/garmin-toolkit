//! Single-value selection control.

use cint::ColorInterop;
use egui::{Id, Popup, Response, RichText, ScrollArea, Sense, Ui};
use garmin_color::theme;

use crate::{Size, icons, images, theme::color32};

const MAX_VISIBLE_CHOICES: usize = 8;
const CONTENT_INSET: f32 = 12.0;
const IMAGE_HEIGHT: f32 = 16.0;
const IMAGE_SLOT_WIDTH: f32 = 24.0;
const IMAGE_GAP: f32 = 8.0;
const CARET_SIZE: f32 = 16.0;

/// One selectable value.
#[derive(Clone, Copy, Debug)]
pub struct Choice<'a> {
    label: &'a str,
    image: Option<images::Image>,
}

impl<'a> Choice<'a> {
    #[must_use]
    pub const fn new(label: &'a str) -> Self {
        Self { label, image: None }
    }

    #[must_use]
    pub const fn image(mut self, image: images::Image) -> Self {
        self.image = Some(image);
        self
    }
}

/// Select configuration.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    label: &'a str,
    helper: Option<&'a str>,
    size: Option<Size>,
    disabled: Option<bool>,
    width: Option<f32>,
}

impl<'a> Props<'a> {
    #[must_use]
    pub const fn new(label: &'a str) -> Self {
        Self {
            label,
            helper: None,
            size: None,
            disabled: None,
            width: None,
        }
    }

    #[must_use]
    pub const fn helper(mut self, helper: &'a str) -> Self {
        self.helper = Some(helper);
        self
    }

    #[must_use]
    pub const fn size(mut self, size: Size) -> Self {
        self.size = Some(size);
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

/// Renders a labelled single-value selector.
pub fn show(
    ui: &mut Ui,
    id: Id,
    selected: &mut usize,
    choices: &[Choice<'_>],
    props: Props<'_>,
) -> Response {
    let size = props.size.unwrap_or_default();
    let enabled = !props.disabled.unwrap_or(false) && !choices.is_empty();
    let selected_choice = choices.get(*selected).or_else(|| choices.first()).copied();

    ui.add(egui::Label::new(RichText::new(props.label).size(label_size(size))).selectable(false));
    let output = ui.add_enabled_ui(enabled, |ui| {
        let row_height = height(size);
        let popup_id = id.with("popup");
        let open = Popup::is_id_open(ui.ctx(), popup_id);
        let response = control(
            ui,
            id,
            selected_choice,
            props.width.unwrap_or_else(|| ui.available_width()),
            row_height,
            open,
        );
        let mut changed = false;
        let _ = Popup::menu(&response)
            .id(popup_id)
            .width(response.rect.width())
            .style(egui::style::StyleModifier::new(|style| {
                style.spacing.menu_margin = egui::Margin::ZERO;
                style.spacing.item_spacing.y = 0.0;
            }))
            .show(|ui| {
                ui.set_min_width(response.rect.width());
                ScrollArea::vertical()
                    .max_height(menu_height(choices.len(), row_height))
                    .show(ui, |ui| {
                        for (index, choice) in choices.iter().enumerate() {
                            let choice_response = choice_row(
                                ui,
                                *choice,
                                *selected == index,
                                index + 1 == choices.len(),
                                row_height,
                            );
                            if choice_response.clicked() && *selected != index {
                                *selected = index;
                                changed = true;
                            }
                        }
                    });
            });
        (response, changed)
    });
    let (mut response, changed) = output.inner;
    if changed {
        response.mark_changed();
    }
    if let Some(helper) = props.helper {
        ui.add(
            egui::Label::new(RichText::new(helper).weak().size(12.0))
                .selectable(false)
                .wrap(),
        );
    }
    response
}

fn control(
    ui: &mut Ui,
    id: Id,
    choice: Option<Choice<'_>>,
    width: f32,
    row_height: f32,
    open: bool,
) -> Response {
    let (_, rect) = ui.allocate_space(egui::vec2(width, row_height));
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::WidgetType::ComboBox);
        info.enabled = ui.is_enabled();
        info.current_text_value = choice.map(|choice| choice.label.to_owned());
        info
    });
    if response.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let visuals = if open {
        &ui.visuals().widgets.open
    } else {
        ui.style().interact(&response)
    };
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    if let Some(choice) = choice {
        paint_choice(ui, rect, choice, visuals.fg_stroke.color);
    }
    let caret_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.right() - CONTENT_INSET - CARET_SIZE / 2.0,
            rect.center().y,
        ),
        egui::Vec2::splat(CARET_SIZE),
    );
    icons::CARET_DOWN
        .mask()
        .tint(visuals.fg_stroke.color)
        .paint_at(ui, caret_rect);
    response
}

fn paint_choice(ui: &Ui, rect: egui::Rect, choice: Choice<'_>, color: egui::Color32) {
    let mut label_x = rect.left() + CONTENT_INSET;
    if let Some(image) = choice.image {
        let image_size = image.size_for_height(IMAGE_HEIGHT);
        let image_rect = egui::Rect::from_center_size(
            egui::pos2(label_x + IMAGE_SLOT_WIDTH / 2.0, rect.center().y),
            image_size,
        );
        image.paint_at(ui, image_rect);
        label_x += IMAGE_SLOT_WIDTH + IMAGE_GAP;
    }
    ui.painter().text(
        egui::pos2(label_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        choice.label,
        egui::TextStyle::Button.resolve(ui.style()),
        color,
    );
}

const fn height(size: Size) -> f32 {
    match size {
        Size::Small => 32.0,
        Size::Medium => 40.0,
        Size::Large => 48.0,
    }
}

const fn label_size(size: Size) -> f32 {
    match size {
        Size::Small => 11.0,
        Size::Medium | Size::Large => 12.0,
    }
}

fn menu_height(choice_count: usize, row_height: f32) -> f32 {
    let visible = u8::try_from(choice_count.min(MAX_VISIBLE_CHOICES))
        .expect("the visible choice count is capped at eight");
    row_height * f32::from(visible)
}

fn choice_row(
    ui: &mut Ui,
    choice: Choice<'_>,
    selected: bool,
    last: bool,
    row_height: f32,
) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::click());
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::WidgetType::SelectableLabel, true, choice.label);
        info.selected = Some(selected);
        info
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let palette = crate::theme::palette(ui);
    let fill = if selected {
        palette.interaction().interactive()
    } else if response.hovered() {
        palette.surfaces().layer_hover(theme::Level::One)
    } else {
        palette.surfaces().layer(theme::Level::One)
    };
    ui.painter().rect_filled(rect, 0.0, fill.into_cint());
    if !last {
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, palette.borders().subtle().into_cint()),
        );
    }
    paint_choice(ui, rect, choice, color32(palette.content().text_primary()));
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0, palette.interaction().focus().into_cint()),
            egui::StrokeKind::Inside,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_configuration_starts_unset() {
        let props = Props::new("Language");

        assert_eq!(props.helper, None);
        assert_eq!(props.size, None);
        assert_eq!(props.disabled, None);
        assert_eq!(props.width, None);
    }

    #[test]
    fn menu_height_only_caps_long_lists() {
        let row_height = 40.0;
        let capped = menu_height(MAX_VISIBLE_CHOICES, row_height);

        assert!(menu_height(MAX_VISIBLE_CHOICES - 1, row_height) < capped);
        assert_eq!(
            menu_height(MAX_VISIBLE_CHOICES + 1, row_height).to_bits(),
            capped.to_bits()
        );
    }
}
