//! Form inputs.

use cint::ColorInterop;
use egui::{Response, RichText, Ui, emath::Numeric};

use crate::{Size, button, icons};

/// Supporting or validation text below an input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Message<'a> {
    Helper(&'a str),
    Error(&'a str),
}

impl<'a> Message<'a> {
    const fn text(self) -> &'a str {
        match self {
            Self::Helper(text) | Self::Error(text) => text,
        }
    }

    const fn is_error(self) -> bool {
        matches!(self, Self::Error(_))
    }
}

/// Text input options.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    label: &'a str,
    placeholder: &'a str,
    message: Option<Message<'a>>,
    size: Option<Size>,
    disabled: Option<bool>,
}

impl<'a> Props<'a> {
    #[must_use]
    pub const fn new(label: &'a str) -> Self {
        Self {
            label,
            placeholder: "",
            message: None,
            size: None,
            disabled: None,
        }
    }

    #[must_use]
    pub const fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    #[must_use]
    pub fn message(mut self, message: impl Into<Option<Message<'a>>>) -> Self {
        self.message = message.into();
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
}

/// Number input options.
#[derive(Clone, Copy, Debug)]
pub struct NumberProps<'a, N> {
    label: &'a str,
    message: Option<Message<'a>>,
    size: Option<Size>,
    min: Option<N>,
    max: Option<N>,
    step: Option<N>,
    steppers: Option<bool>,
    readonly: Option<bool>,
    disabled: Option<bool>,
}

impl<'a, N> NumberProps<'a, N> {
    #[must_use]
    pub const fn new(label: &'a str) -> Self {
        Self {
            label,
            message: None,
            size: None,
            min: None,
            max: None,
            step: None,
            steppers: None,
            readonly: None,
            disabled: None,
        }
    }

    #[must_use]
    pub fn message(mut self, message: impl Into<Option<Message<'a>>>) -> Self {
        self.message = message.into();
        self
    }

    #[must_use]
    pub const fn size(mut self, size: Size) -> Self {
        self.size = Some(size);
        self
    }

    #[must_use]
    pub fn min(mut self, min: N) -> Self {
        self.min = Some(min);
        self
    }

    #[must_use]
    pub fn max(mut self, max: N) -> Self {
        self.max = Some(max);
        self
    }

    #[must_use]
    pub fn step(mut self, step: N) -> Self {
        self.step = Some(step);
        self
    }

    #[must_use]
    pub const fn steppers(mut self, steppers: bool) -> Self {
        self.steppers = Some(steppers);
        self
    }

    #[must_use]
    pub const fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = Some(readonly);
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }
}

/// Renders a controlled single-line text input.
pub fn show(ui: &mut Ui, value: &mut String, props: Props<'_>) -> Response {
    let size = props.size.unwrap_or_default();
    let disabled = props.disabled.unwrap_or_default();
    let metrics = metrics(size);
    field(
        ui,
        FieldProps {
            label: props.label,
            message: props.message,
            enabled: !disabled,
            readonly: false,
        },
        |ui, fill| {
            control_style(ui, fill, false);
            ui.add_enabled(
                !disabled,
                egui::TextEdit::singleline(value)
                    .hint_text(props.placeholder.to_owned())
                    .font(egui::FontId::proportional(metrics.font_size))
                    .desired_width(f32::INFINITY)
                    .min_size(egui::vec2(0.0, metrics.height))
                    .margin(egui::Margin::symmetric(
                        metrics.horizontal_padding,
                        metrics.vertical_padding,
                    ))
                    .background_color(fill),
            )
        },
    )
}

/// Renders a controlled numeric input.
/// # Panics
/// Panics when configured bounds are reversed or the step is not positive and finite.
pub fn show_number<N: Numeric>(ui: &mut Ui, value: &mut N, props: NumberProps<'_, N>) -> Response {
    let min = props.min.unwrap_or(N::MIN);
    let max = props.max.unwrap_or(N::MAX);
    let step = props.step.unwrap_or_else(|| N::from_f64(1.0));
    let size = props.size.unwrap_or_default();
    let steppers = props.steppers.unwrap_or(true);
    let readonly = props.readonly.unwrap_or_default();
    let disabled = props.disabled.unwrap_or_default();
    assert!(min <= max, "number input min must not exceed max");
    let step_value = step.to_f64();
    assert!(
        step_value.is_finite() && step_value > 0.0,
        "number input step must be positive and finite"
    );

    field(
        ui,
        FieldProps {
            label: props.label,
            message: props.message,
            enabled: !disabled,
            readonly,
        },
        |ui, fill| {
            number_control(
                ui,
                value,
                NumberControlProps {
                    size,
                    min,
                    max,
                    step,
                    steppers,
                    readonly,
                    disabled,
                },
                fill,
            )
        },
    )
}

#[derive(Clone, Copy)]
struct FieldProps<'a> {
    label: &'a str,
    message: Option<Message<'a>>,
    enabled: bool,
    readonly: bool,
}

fn field(
    ui: &mut Ui,
    props: FieldProps<'_>,
    control: impl FnOnce(&mut Ui, egui::Color32) -> Response,
) -> Response {
    let app_theme = crate::theme::palette(ui);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let label_color = if props.enabled {
            app_theme.content().text_primary()
        } else {
            app_theme.content().text_disabled()
        };
        ui.add(
            egui::Label::new(
                RichText::new(props.label)
                    .size(12.0)
                    .color(label_color.into_cint()),
            )
            .selectable(false),
        );

        let fill = if !props.enabled {
            ui.visuals().widgets.noninteractive.bg_fill
        } else if props.readonly {
            ui.visuals().widgets.noninteractive.weak_bg_fill
        } else {
            ui.visuals()
                .text_edit_bg_color
                .unwrap_or_else(|| ui.visuals().extreme_bg_color)
        };
        let response = control(ui, fill);
        let emphasized = response.has_focus() || props.message.is_some_and(Message::is_error);
        let border = if props.message.is_some_and(Message::is_error) {
            app_theme.support().error()
        } else if response.has_focus() {
            app_theme.borders().interactive()
        } else if props.enabled && !props.readonly {
            app_theme.borders().strong()
        } else {
            app_theme.borders().subtle()
        };
        let width = if emphasized { 2.0 } else { 1.0 };
        ui.painter().hline(
            response.rect.x_range(),
            response.rect.bottom() - width / 2.0,
            egui::Stroke::new(width, border.into_cint()),
        );

        if let Some(message) = props.message {
            let color = if message.is_error() {
                app_theme.support().error()
            } else {
                app_theme.content().text_helper()
            };
            ui.add(
                egui::Label::new(
                    RichText::new(message.text())
                        .size(12.0)
                        .color(color.into_cint()),
                )
                .selectable(false),
            );
        }
        response
    })
    .inner
}

fn number_control<N: Numeric>(
    ui: &mut Ui,
    value: &mut N,
    props: NumberControlProps<N>,
    fill: egui::Color32,
) -> Response {
    let editable = !props.disabled && !props.readonly;
    let metrics = metrics(props.size);
    ui.scope(|ui| {
        control_style(ui, fill, props.readonly);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::Vec2::ZERO;
        ui.style_mut().drag_value_text_style = egui::TextStyle::Body;
        egui::Frame::new()
            .fill(fill)
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), metrics.height),
                    egui::Layout::left_to_right(egui::Align::Center)
                        .with_main_align(egui::Align::Min),
                    |ui| {
                        let controls_width = if props.steppers {
                            metrics.height * 2.0
                        } else {
                            0.0
                        };
                        let value_width = (ui.available_width() - controls_width).max(0.0);
                        let editor = egui::DragValue::new(value)
                            .range(props.min..=props.max)
                            .speed(props.step.to_f64())
                            .update_while_editing(true);
                        let mut response = ui
                            .add_enabled_ui(editable, |ui| {
                                ui.spacing_mut().button_padding.x =
                                    f32::from(metrics.horizontal_padding);
                                ui.spacing_mut().interact_size =
                                    egui::vec2(value_width, metrics.height);
                                ui.add(editor)
                            })
                            .inner;

                        if props.steppers {
                            let decrease = stepper(
                                ui,
                                icons::MINUS,
                                "Decrease",
                                props.size,
                                editable && *value > props.min,
                            );
                            if decrease.clicked() {
                                *value = N::from_f64(
                                    (value.to_f64() - props.step.to_f64()).max(props.min.to_f64()),
                                );
                            }
                            let increase = stepper(
                                ui,
                                icons::PLUS,
                                "Increase",
                                props.size,
                                editable && *value < props.max,
                            );
                            if increase.clicked() {
                                *value = N::from_f64(
                                    (value.to_f64() + props.step.to_f64()).min(props.max.to_f64()),
                                );
                            }
                            response = response.union(decrease).union(increase);
                        }
                        response
                    },
                )
                .inner
            })
            .inner
    })
    .inner
}

#[derive(Clone, Copy)]
struct NumberControlProps<N> {
    size: Size,
    min: N,
    max: N,
    step: N,
    steppers: bool,
    readonly: bool,
    disabled: bool,
}

fn stepper(
    ui: &mut Ui,
    icon: icons::Icon,
    label: &'static str,
    size: Size,
    enabled: bool,
) -> Response {
    let res = button::IconProps {
        label,
        icon,
        kind: button::Kind::Ghost,
        size,
        enabled,
    }
    .show_with_dimension(ui, metrics(size).height);
    ui.painter().vline(
        res.rect.left(),
        res.rect.y_range(),
        egui::Stroke::new(
            1.0,
            crate::theme::palette(ui).borders().subtle().into_cint(),
        ),
    );
    res
}

#[derive(Clone, Copy)]
struct Metrics {
    height: f32,
    horizontal_padding: i8,
    vertical_padding: i8,
    font_size: f32,
}

const fn metrics(size: Size) -> Metrics {
    match size {
        Size::Small => Metrics {
            height: 40.0,
            horizontal_padding: 12,
            vertical_padding: 10,
            font_size: 14.0,
        },
        Size::Medium => Metrics {
            height: 48.0,
            horizontal_padding: 16,
            vertical_padding: 14,
            font_size: 14.0,
        },
        Size::Large => Metrics {
            height: 64.0,
            horizontal_padding: 16,
            vertical_padding: 22,
            font_size: 16.0,
        },
    }
}

fn control_style(ui: &mut Ui, fill: egui::Color32, readonly: bool) {
    let text_primary = crate::theme::color32(crate::theme::palette(ui).content().text_primary());
    let vis = &mut ui.style_mut().visuals;
    for state in [
        &mut vis.widgets.inactive,
        &mut vis.widgets.hovered,
        &mut vis.widgets.active,
        &mut vis.widgets.noninteractive,
    ] {
        state.bg_stroke = egui::Stroke::NONE;
    }
    vis.widgets.inactive.bg_fill = fill;
    vis.widgets.inactive.weak_bg_fill = fill;
    if readonly {
        vis.widgets.noninteractive.bg_fill = fill;
        vis.widgets.noninteractive.weak_bg_fill = fill;
        vis.widgets.noninteractive.fg_stroke.color = text_primary;
    }
    vis.selection.stroke = egui::Stroke::NONE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_options_start_unset() {
        let text = Props::new("Name");
        assert_eq!(text.size, None);
        assert_eq!(text.disabled, None);

        let number = NumberProps::<i32>::new("Count");
        assert_eq!(number.size, None);
        assert_eq!(number.min, None);
        assert_eq!(number.max, None);
        assert_eq!(number.step, None);
        assert_eq!(number.steppers, None);
        assert_eq!(number.readonly, None);
        assert_eq!(number.disabled, None);
    }
}
