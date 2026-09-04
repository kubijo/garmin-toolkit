//! Semantic action buttons.

use egui::{Response, RichText, Ui};
use garmin_color::theme;

use crate::{Size, icons::Icon, theme::widget};

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
}

impl<'a, T> GroupChoice<'a, T> {
    /// Creates an icon-and-label option.
    pub const fn new(label: &'a str, icon: Icon, value: T) -> Self {
        Self { label, icon, value }
    }
}

/// Inline button-group configuration.
#[derive(Clone, Copy, Debug)]
pub struct GroupProps {
    pub size: Size,
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
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            choices.iter().find_map(|choice| {
                let is_selected = choice.value == selected;
                let clicked = Props {
                    label: choice.label,
                    icon: Some(choice.icon),
                    kind: if is_selected {
                        Kind::Primary
                    } else {
                        Kind::Secondary
                    },
                    size: props.size,
                    width: Width::Fit,
                    enabled: props.enabled,
                }
                .show(ui)
                .clicked();
                (clicked && !is_selected).then_some(choice.value)
            })
        })
        .inner
    })
    .inner
}

impl Props<'_> {
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
        let metrics = metrics(self.size);
        ui.scope(|ui| {
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
                    height,
                ))
                .corner_radius(egui::CornerRadius::ZERO);

            ui.add_enabled(self.enabled, button)
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
        response.on_hover_text(self.label)
    }
}

const fn icon_button_size(size: Size) -> f32 {
    match size {
        Size::Small | Size::Medium => 14.0,
        Size::Large => 16.0,
    }
}

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
