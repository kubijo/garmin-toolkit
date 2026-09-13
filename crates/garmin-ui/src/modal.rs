//! Modal dialogs.

use cint::ColorInterop;
use egui::{Align, Align2, Id, Layout, Order, Response, RichText, Ui};
use garmin_color::theme;

use crate::{
    Size as ComponentSize, button,
    icons::Icon,
    theme::{FLOATING_RADIUS, color32},
};

const BODY_MARGIN: egui::Margin = egui::Margin {
    left: 24,
    right: 24,
    top: 24,
    bottom: 24,
};
const FOOTER_MARGIN: egui::Margin = egui::Margin::ZERO;
const ACTION_GAP: f32 = 2.0;
const VIEWPORT_MARGIN: f32 = 32.0;

/// Dialog placement and input scope.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Presentation {
    /// Center over the viewport and block input behind the dialog.
    #[default]
    Modal,
    Contained,
}

/// Dialog width.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Size {
    Small,
    /// Standard forms and decisions.
    #[default]
    Medium,
    Large,
}

impl Size {
    const fn width(self) -> f32 {
        match self {
            Self::Small => 384.0,
            Self::Medium => 512.0,
            Self::Large => 640.0,
        }
    }
}

/// Confirming action emphasis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrimaryKind {
    /// Normal confirmation.
    #[default]
    Confirm,
    /// Destructive confirmation.
    Danger,
}

impl PrimaryKind {
    const fn button_kind(self) -> button::Kind {
        match self {
            Self::Confirm => button::Kind::Primary,
            Self::Danger => button::Kind::Danger,
        }
    }
}

/// Confirming action inputs.
#[derive(Clone, Copy, Debug)]
pub struct Primary<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub kind: PrimaryKind,
    pub enabled: bool,
}

/// Dialog inputs.
pub struct Props<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub size: Size,
    pub presentation: Presentation,
    pub cancel_label: &'a str,
    pub backdrop_closes: Option<bool>,
    pub primary: Primary<'a>,
}

/// Dialog interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Cancel,
    Primary,
}

/// Dialog result and rendered body value.
pub struct Output<R> {
    pub action: Option<Action>,
    pub inner: R,
}

/// Shows a dialog in its configured presentation.
///
#[must_use]
pub fn show<R>(
    ui: &mut Ui,
    id: Id,
    props: &Props<'_>,
    body: impl FnOnce(&mut Ui) -> R,
) -> Output<R> {
    let bounds = ui.max_rect();
    match props.presentation {
        Presentation::Modal => {
            let ctx = ui.ctx().clone();
            let viewport = ctx.content_rect().size();
            let palette = crate::theme::palette(ui);
            let response = egui::Modal::new(id)
                .frame(egui::Frame::new())
                .backdrop_color(color32(palette.overlay()))
                .show(&ctx, |ui| render_surface(ui, props, viewport, body));
            let mut output = response.inner;
            let close_requested = response.response.should_close()
                || (props.backdrop_closes.unwrap_or_default()
                    && response.backdrop_response.clicked());
            let escaped = response.is_top_modal
                && !response.any_popup_open
                && ctx
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if output.action.is_none() && (close_requested || escaped) {
                output.action = Some(Action::Cancel);
            }
            output
        }
        Presentation::Contained => {
            let backdrop = contained_backdrop(ui, id.with("backdrop"), bounds);
            let mut output = egui::Area::new(id)
                .order(Order::Foreground)
                .fixed_pos(bounds.center())
                .pivot(Align2::CENTER_CENTER)
                .constrain_to(bounds)
                .show(ui.ctx(), |ui| {
                    render_surface(ui, props, bounds.size(), body)
                })
                .inner;
            if output.action.is_none()
                && props.backdrop_closes.unwrap_or_default()
                && backdrop.clicked()
            {
                output.action = Some(Action::Cancel);
            }
            output
        }
    }
}

fn render_surface<R>(
    ui: &mut Ui,
    props: &Props<'_>,
    bounds: egui::Vec2,
    body: impl FnOnce(&mut Ui) -> R,
) -> Output<R> {
    let width = props
        .size
        .width()
        .min((bounds.x - VIEWPORT_MARGIN).max(0.0));
    let max_body_height = bounds.y.mul_add(0.8, -180.0).max(96.0);

    let palette = crate::theme::palette(ui);
    egui::Frame::new()
        .fill(color32(palette.surfaces().layer(theme::Level::One)))
        .corner_radius(FLOATING_RADIUS)
        .stroke(egui::Stroke::new(1.0, color32(palette.borders().subtle())))
        .shadow(egui::Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: egui::Color32::from_black_alpha(128),
        })
        .show(ui, |ui| {
            ui.set_width(width);
            let inner = egui::Frame::new()
                .inner_margin(BODY_MARGIN)
                .show(ui, |ui| {
                    crate::theme::layer(ui, theme::Level::Two, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        header(ui, props);
                        egui::ScrollArea::vertical()
                            .max_height(max_body_height)
                            .show(ui, body)
                            .inner
                    })
                })
                .inner;
            let action = footer(ui, props);
            Output { action, inner }
        })
        .inner
}

fn contained_backdrop(ui: &Ui, id: Id, bounds: egui::Rect) -> Response {
    ui.painter()
        .rect_filled(bounds, 0.0, color32(crate::theme::palette(ui).overlay()));
    ui.interact(bounds, id, egui::Sense::click_and_drag())
}

fn header(ui: &mut Ui, props: &Props<'_>) {
    ui.add(
        egui::Label::new(crate::typography::semibold(props.title).size(22.0))
            .selectable(false)
            .wrap(),
    );
    if let Some(description) = props.description {
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(description).color(
                    crate::theme::palette(ui)
                        .content()
                        .text_secondary()
                        .into_cint(),
                ),
            )
            .selectable(false)
            .wrap(),
        );
    }
    ui.add_space(24.0);
}

fn footer(ui: &mut Ui, props: &Props<'_>) -> Option<Action> {
    let actions = crate::theme::palette(ui).modal_actions();
    let mut action = None;
    egui::Frame::new()
        .inner_margin(FOOTER_MARGIN)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = ACTION_GAP;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let primary = button::Props {
                    label: props.primary.label,
                    icon: props.primary.icon,
                    kind: props.primary.kind.button_kind(),
                    size: ComponentSize::Medium,
                    width: button::Width::Fit,
                    enabled: props.primary.enabled,
                }
                .show_with_states(
                    ui,
                    match props.primary.kind {
                        PrimaryKind::Confirm => actions.confirm(),
                        PrimaryKind::Danger => actions.danger(),
                    },
                );
                let cancel = button::Props {
                    label: props.cancel_label,
                    icon: None,
                    kind: button::Kind::Secondary,
                    size: ComponentSize::Medium,
                    width: button::Width::Fit,
                    enabled: true,
                }
                .show_with_states(ui, actions.cancel());
                if primary_activated(props.primary.kind, &primary) {
                    action = Some(Action::Primary);
                } else if cancel.clicked() {
                    action = Some(Action::Cancel);
                }
            });
        });
    action
}

fn primary_activated(_kind: PrimaryKind, response: &Response) -> bool {
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_primary_accepts_keyboard_activation() {
        let context = egui::Context::default();
        crate::install(&context);
        context
            .run_ui(egui::RawInput::default(), |ui| {
                ui.button("Remove").request_focus();
            })
            .drop_without_applying_deltas();

        let mut danger_response = None;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 200.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };
        context
            .run_ui(input, |ui| {
                let response = ui.button("Remove");
                danger_response = Some(response);
            })
            .drop_without_applying_deltas();

        let response = danger_response.expect("the button was shown");
        assert!(
            response.clicked(),
            "Enter produces a synthetic button click"
        );
        assert!(primary_activated(PrimaryKind::Danger, &response));
    }
}
