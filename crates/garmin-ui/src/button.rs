//! Semantic action buttons.

use egui::{Response, RichText, Ui};
use garmin_color::theme;

use crate::{
    Size,
    icons::Icon,
    theme::{CONTROL_RADIUS, widget},
};

/// Visual and semantic importance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Kind {
    /// Main action in the current context.
    #[default]
    Primary,
    Secondary,
    Tertiary,
    Ghost,
    Danger,
}

impl Kind {
    fn states(self, ui: &Ui) -> &'static theme::ButtonStates {
        let buttons = crate::theme::palette(ui).buttons();
        match self {
            Self::Primary => buttons.primary(),
            Self::Secondary => buttons.secondary(),
            Self::Tertiary => buttons.tertiary(),
            Self::Ghost => buttons.ghost(),
            Self::Danger => buttons.danger(),
        }
    }
}

/// Button width within its parent layout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Width {
    /// Fit the button contents.
    #[default]
    Fit,
    Fill,
}

/// Inputs for one button.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub kind: Kind,
    pub size: Size,
    pub width: Width,
    pub enabled: bool,
}

/// One option in an inline button group.
#[derive(Clone, Copy, Debug)]
pub struct GroupChoice<'a, T> {
    label: &'a str,
    icon: Icon,
    value: T,
    enabled: bool,
}

impl<'a, T> GroupChoice<'a, T> {
    /// Creates an icon-and-label option.
    pub const fn new(label: &'a str, icon: Icon, value: T) -> Self {
        Self {
            label,
            icon,
            value,
            enabled: true,
        }
    }

    /// Controls whether this individual option can be selected.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Inline button-group configuration.
#[derive(Clone, Copy, Debug)]
pub struct GroupProps {
    pub size: Size,
    pub width: Width,
    pub enabled: bool,
}

#[must_use]
pub fn group<T>(
    ui: &mut Ui,
    selected: T,
    choices: &[GroupChoice<'_, T>],
    props: GroupProps,
) -> Option<T>
where
    T: Copy + Eq,
{
    row(ui, choices.len(), props.width, |ui, index| {
        let choice = &choices[index];
        let is_selected = choice.value == selected;
        let button = Props {
            label: choice.label,
            icon: Some(choice.icon),
            kind: if is_selected {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            size: props.size,
            width: props.width,
            enabled: props.enabled && choice.enabled,
        };
        if props.width == Width::Fit && button.natural_width(ui) > ui.available_size_before_wrap().x
        {
            ui.end_row();
        }
        let clicked = button.show(ui).clicked();
        (clicked && !is_selected).then_some(choice.value)
    })
}

pub(crate) fn row<T>(
    ui: &mut Ui,
    count: usize,
    width: Width,
    mut show: impl FnMut(&mut Ui, usize) -> Option<T>,
) -> Option<T> {
    if count == 0 {
        return None;
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if width == Width::Fill {
            ui.columns(count, |columns| {
                columns
                    .iter_mut()
                    .enumerate()
                    .fold(None, |action, (index, ui)| show(ui, index).or(action))
            })
        } else {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.horizontal_wrapped(|ui| {
                (0..count).fold(None, |action, index| show(ui, index).or(action))
            })
            .inner
        }
    })
    .inner
}

impl Props<'_> {
    pub(crate) fn natural_width(self, ui: &Ui) -> f32 {
        let metrics = metrics(self.size);
        let text = egui::WidgetText::from(RichText::new(self.label).size(metrics.font_size))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            );
        text.size().x
            + metrics.horizontal_padding * 2.0
            + self.icon.map_or(0.0, |_| metrics.icon_size + metrics.gap)
    }

    /// Renders the button.
    pub fn show(self, ui: &mut Ui) -> Response {
        let states = self.kind.states(ui);
        self.show_with_states(ui, states)
    }

    pub(crate) fn show_with_states(self, ui: &mut Ui, states: &theme::ButtonStates) -> Response {
        self.show_with_states_at_height(ui, states, metrics(self.size).height)
    }

    pub(crate) fn show_with_states_at_height(
        self,
        ui: &mut Ui,
        states: &theme::ButtonStates,
        height: f32,
    ) -> Response {
        let metrics = Metrics {
            height,
            ..metrics(self.size)
        };
        self.show_with_states_and_metrics(ui, states, metrics)
    }

    fn show_with_states_and_metrics(
        self,
        ui: &mut Ui,
        states: &theme::ButtonStates,
        metrics: Metrics,
    ) -> Response {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = metrics.height;
            let visuals = &mut ui.style_mut().visuals;
            visuals.widgets.inactive = state_visuals(states.rest());
            visuals.widgets.hovered = state_visuals(states.hover());
            visuals.widgets.active = state_visuals(states.active());
            visuals.widgets.open = visuals.widgets.active;
            visuals.widgets.noninteractive = state_visuals(states.disabled());
            visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
            ui.spacing_mut().button_padding = egui::vec2(metrics.horizontal_padding, 0.0);

            let label = RichText::new(self.label).size(metrics.font_size);
            let image = self.icon.map(|icon| {
                icon.mask()
                    .fit_to_exact_size(egui::Vec2::splat(metrics.icon_size))
            });
            let button = egui::Button::opt_image_and_text(image, Some(label.into()))
                .image_tint_follows_text_color(true)
                .gap(metrics.gap)
                .min_size(egui::vec2(
                    match self.width {
                        Width::Fit => 0.0,
                        Width::Fill => ui.available_width(),
                    },
                    metrics.height,
                ))
                .corner_radius(CONTROL_RADIUS);

            interaction_cursor(ui.add_enabled(self.enabled, button))
        })
        .inner
    }
}

/// Inputs for an icon-only button.
#[derive(Clone, Copy, Debug)]
pub struct IconProps<'a> {
    pub label: &'a str,
    pub icon: Icon,
    pub kind: Kind,
    pub size: Size,
    pub enabled: bool,
}

impl IconProps<'_> {
    /// Renders a square icon button.
    pub fn show(self, ui: &mut Ui) -> Response {
        let metrics = metrics(self.size);
        self.show_with_dimension(ui, metrics.height)
    }

    pub(crate) fn show_with_dimension(self, ui: &mut Ui, dimension: f32) -> Response {
        let states = self.kind.states(ui);
        let content = crate::theme::palette(ui).content();
        let icon_size = icon_button_size(self.size);
        let response = ui
            .scope(|ui| {
                ui.spacing_mut().interact_size = egui::Vec2::splat(dimension);
                ui.spacing_mut().button_padding =
                    egui::Vec2::splat(((dimension - icon_size) / 2.0).max(0.0));
                let visuals = &mut ui.style_mut().visuals;
                visuals.widgets.inactive =
                    icon_state_visuals(self.kind, states.rest(), content.icon_secondary());
                visuals.widgets.hovered =
                    icon_state_visuals(self.kind, states.hover(), content.icon_primary());
                visuals.widgets.active =
                    icon_state_visuals(self.kind, states.active(), content.icon_primary());
                visuals.widgets.open = visuals.widgets.active;
                visuals.widgets.noninteractive =
                    icon_state_visuals(self.kind, states.disabled(), content.icon_disabled());
                visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

                ui.add_enabled_ui(self.enabled, |ui| {
                    ui.add_sized(
                        egui::Vec2::splat(dimension),
                        egui::Button::new(
                            self.icon
                                .mask()
                                .fit_to_exact_size(egui::Vec2::splat(icon_size)),
                        )
                        .image_tint_follows_text_color(true)
                        .frame(true)
                        .min_size(egui::Vec2::splat(dimension)),
                    )
                })
                .inner
            })
            .inner;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, self.enabled, self.label)
        });
        interaction_cursor(response).on_hover_text(self.label)
    }
}

pub(crate) fn interaction_cursor(response: Response) -> Response {
    if !response.enabled() && response.contains_pointer() {
        response.ctx.set_cursor_icon(egui::CursorIcon::NotAllowed);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const fn icon_button_size(size: Size) -> f32 {
    match size {
        Size::Small | Size::Medium => 14.0,
        Size::Large => 16.0,
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    height: f32,
    horizontal_padding: f32,
    font_size: f32,
    icon_size: f32,
    gap: f32,
}

const fn metrics(size: Size) -> Metrics {
    match size {
        Size::Small => Metrics {
            height: 32.0,
            horizontal_padding: 12.0,
            font_size: 12.0,
            icon_size: 16.0,
            gap: 6.0,
        },
        Size::Medium => Metrics {
            height: 40.0,
            horizontal_padding: 16.0,
            font_size: 14.0,
            icon_size: 16.0,
            gap: 8.0,
        },
        Size::Large => Metrics {
            height: 48.0,
            horizontal_padding: 20.0,
            font_size: 16.0,
            icon_size: 20.0,
            gap: 8.0,
        },
    }
}

fn state_visuals(state: &theme::ButtonState) -> egui::style::WidgetVisuals {
    widget(
        state.background(),
        state.background(),
        state.border(),
        state.foreground(),
    )
}

fn icon_state_visuals(
    kind: Kind,
    state: &theme::ButtonState,
    neutral_foreground: garmin_color::Color,
) -> egui::style::WidgetVisuals {
    let foreground = match kind {
        Kind::Ghost => neutral_foreground,
        Kind::Primary | Kind::Secondary | Kind::Tertiary | Kind::Danger => state.foreground(),
    };
    widget(
        state.background(),
        state.background(),
        state.border(),
        foreground,
    )
}
