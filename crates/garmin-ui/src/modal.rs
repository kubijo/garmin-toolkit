//! Modal dialogs.

use cint::ColorInterop;
use egui::{Align, Align2, Id, Layout, Order, Response, RichText, Ui};
use garmin_color::theme;

use crate::{Size as ComponentSize, button, icons::Icon, theme::color32};

const BODY_PADDING: i8 = 24;
const FOOTER_HEIGHT: f32 = 56.0;
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
        .show(ui, |ui| {
            ui.set_width(width);
            let inner = egui::Frame::new()
                .inner_margin(BODY_PADDING)
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
    let cell_width = ui.available_width() / 2.0;
    let mut action = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            let cancel = footer_button(
                ui,
                cell_width,
                button::Props {
                    label: props.cancel_label,
                    icon: None,
                    kind: button::Kind::Secondary,
                    size: ComponentSize::Medium,
                    width: button::Width::Fill,
                    enabled: true,
                },
                actions.cancel(),
            );
            let primary = footer_button(
                ui,
                cell_width,
                button::Props {
                    label: props.primary.label,
                    icon: props.primary.icon,
                    kind: props.primary.kind.button_kind(),
                    size: ComponentSize::Medium,
                    width: button::Width::Fill,
                    enabled: props.primary.enabled,
                },
                match props.primary.kind {
                    PrimaryKind::Confirm => actions.confirm(),
                    PrimaryKind::Danger => actions.danger(),
                },
            );
            if cancel.clicked() {
                action = Some(Action::Cancel);
            } else if primary.clicked() {
                action = Some(Action::Primary);
            }
        });
    });
    action
}

fn footer_button(
    ui: &mut Ui,
    width: f32,
    props: button::Props<'_>,
    states: &theme::ButtonStates,
) -> Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, FOOTER_HEIGHT),
        Layout::top_down(Align::Min),
        |ui| props.show_with_states_at_height(ui, states, FOOTER_HEIGHT),
    )
    .inner
}
