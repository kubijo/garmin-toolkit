//! Inline and transient operation notifications.

use std::time::Duration;

use cint::ColorInterop;
use egui::{Align, Align2, Id, Layout, Order, Rect, Response, RichText, Ui};
use garmin_color::{Color, theme};

use crate::{Size, button, icons, theme as widget_theme};

const ANIMATION_TIME: f32 = 0.24;
const DEFAULT_DURATION: Duration = Duration::from_secs(4);
const STACK_GAP: f32 = 8.0;
const STACK_MARGIN: f32 = 16.0;
const STACK_PEEK: f32 = 8.0;
const TOAST_WIDTH: f32 = 320.0;
const TOAST_SHADOW_CLIP_MARGIN: f32 = 20.0;
const SEVERITY_RAIL_WIDTH: f32 = 3.0;
const CLOSE_ICON_SIZE: f32 = 14.0;
const CLOSE_TARGET_SIZE: f32 = 24.0;
const TOAST_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;
const SEVERITY_RAIL_RADIUS: egui::CornerRadius = egui::CornerRadius::ZERO;

/// Notification severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Information,
    Success,
    Warning,
    Error,
}

impl Kind {
    fn color(self, ui: &Ui) -> Color {
        let support = crate::theme::palette(ui).support();
        match self {
            Self::Information => support.information(),
            Self::Success => support.success(),
            Self::Warning => support.warning(),
            Self::Error => support.error(),
        }
    }

    const fn icon(self) -> icons::Icon {
        match self {
            Self::Information => icons::INFO,
            Self::Success => icons::CHECK,
            Self::Warning | Self::Error => icons::WARNING,
        }
    }
}

/// Inline-notification inputs.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a> {
    pub kind: Kind,
    pub title: &'a str,
    pub detail: Option<&'a str>,
}

/// Actionable-notification inputs.
#[derive(Clone, Copy, Debug)]
pub struct ActionableProps<'a> {
    pub kind: Kind,
    pub title: &'a str,
    pub detail: Option<&'a str>,
    pub action: Option<&'a str>,
    pub closable: bool,
}

/// Interaction with an actionable notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Invoke,
    Dismiss,
}

/// Renders an inline notification.
pub fn show(ui: &mut Ui, props: &Props<'_>) {
    let _ = surface(
        ui,
        &ActionableProps {
            kind: props.kind,
            title: props.title,
            detail: props.detail,
            action: None,
            closable: false,
        },
    );
}

#[must_use]
pub fn actionable(ui: &mut Ui, props: &ActionableProps<'_>) -> Option<Action> {
    surface(ui, props)
}

/// Removes repeated transport-layer prefixes while preserving the underlying diagnostic.
#[must_use]
pub fn normalize_error_detail(detail: &str) -> String {
    let mut detail = detail.trim().to_owned();
    for prefix in ["receive error:", "send error:", "connection error:"] {
        let repeated = format!("{prefix} {prefix}");
        while detail.contains(&repeated) {
            detail = detail.replace(&repeated, prefix);
        }
    }
    detail
}

/// Stable identity assigned by [`Toasts`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ToastId(u64);

/// Owned content queued in a [`Toasts`] host.
#[derive(Clone, Debug)]
pub struct Toast {
    kind: Kind,
    title: String,
    detail: Option<String>,
    action: Option<String>,
    duration: Option<Duration>,
}

impl Toast {
    /// Creates an automatically expiring toast.
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: None,
            action: None,
            duration: Some(DEFAULT_DURATION),
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn action(mut self, label: impl Into<String>) -> Self {
        self.action = Some(label.into());
        self.duration = None;
        self
    }

    #[must_use]
    pub const fn persistent(mut self) -> Self {
        self.duration = None;
        self
    }
}

struct Entry {
    id: ToastId,
    toast: Toast,
    age: f32,
    remaining: Option<f32>,
    dismissing: bool,
    position: f32,
}

impl Entry {
    fn new(id: ToastId, toast: Toast) -> Self {
        let remaining = toast.duration.map(|duration| duration.as_secs_f32());
        Self {
            id,
            toast,
            age: 0.0,
            remaining,
            dismissing: false,
            position: 0.0,
        }
    }

    fn visibility(&self) -> f32 {
        let progress = if self.dismissing {
            1.0 - self.age / ANIMATION_TIME
        } else {
            self.age / ANIMATION_TIME
        };
        egui::emath::easing::cubic_out(progress.clamp(0.0, 1.0))
    }
}

/// Toast-host event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastEvent {
    Invoked(ToastId),
    Dismissed(ToastId),
}

/// Toast-stack expansion behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StackMode {
    /// Collapse until the pointer enters the stack.
    #[default]
    Automatic,
    Collapsed,
    Expanded,
}

/// Application-owned transient notification queue.
#[derive(Default)]
pub struct Toasts {
    entries: Vec<Entry>,
    next_id: u64,
    hover_rect: Option<Rect>,
}

impl Toasts {
    /// Adds a toast and returns its stable identity.
    /// # Panics
    /// Panics after exhausting the 64-bit toast identifier space.
    pub fn push(&mut self, toast: Toast) -> ToastId {
        let id = ToastId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("toast identifier space is exhausted");
        self.entries.push(Entry::new(id, toast));
        id
    }

    /// Begins dismissing a toast if it is still present.
    pub fn dismiss(&mut self, id: ToastId) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.dismissing)
        {
            entry.dismissing = true;
            entry.age = 0.0;
        }
    }

    #[must_use]
    pub fn show(&mut self, ctx: &egui::Context, id: Id) -> Vec<ToastEvent> {
        self.show_in(ctx, id, ctx.content_rect(), StackMode::Automatic)
    }

    #[must_use]
    pub fn show_in(
        &mut self,
        ctx: &egui::Context,
        id: Id,
        bounds: Rect,
        mode: StackMode,
    ) -> Vec<ToastEvent> {
        if self.entries.is_empty() {
            self.hover_rect = None;
            return Vec::new();
        }

        let (delta, focused, pointer) =
            ctx.input(|input| (input.stable_dt, input.focused, input.pointer.hover_pos()));
        let hovered = pointer.is_some_and(|position| {
            self.hover_rect
                .is_some_and(|rect| rect.expand(STACK_GAP).contains(position))
        });
        let expansion = match mode {
            StackMode::Automatic => ctx.animate_bool_with_time_and_easing(
                id.with("expanded"),
                hovered,
                ANIMATION_TIME,
                egui::emath::easing::cubic_out,
            ),
            StackMode::Collapsed => 0.0,
            StackMode::Expanded => 1.0,
        };

        self.tick(delta, focused && !hovered && mode == StackMode::Automatic);
        let output = self.render_stack(ctx, id, bounds, mode, expansion, delta);
        self.hover_rect = Some(output.rect);

        let mut events = Vec::new();
        for (toast_id, action) in output.interactions {
            match action {
                Action::Invoke => events.push(ToastEvent::Invoked(toast_id)),
                Action::Dismiss => events.push(ToastEvent::Dismissed(toast_id)),
            }
            self.dismiss(toast_id);
        }

        let timer_running = focused && !hovered && mode == StackMode::Automatic;
        let moving = self.entries.iter().any(|entry| {
            entry.age < ANIMATION_TIME
                || entry.dismissing
                || (entry.remaining.is_some() && timer_running)
        }) || output.positions_moving;
        self.entries
            .retain(|entry| !(entry.dismissing && entry.age >= ANIMATION_TIME));
        if moving {
            ctx.request_repaint();
        }
        events
    }

    fn tick(&mut self, delta: f32, timer_running: bool) {
        for entry in &mut self.entries {
            entry.age += delta;
            if !entry.dismissing
                && timer_running
                && let Some(remaining) = &mut entry.remaining
            {
                *remaining -= delta;
                if *remaining <= 0.0 {
                    entry.dismissing = true;
                    entry.age = 0.0;
                }
            }
        }
    }

    fn render_stack(
        &mut self,
        ctx: &egui::Context,
        id: Id,
        bounds: Rect,
        mode: StackMode,
        expansion: f32,
        delta: f32,
    ) -> StackOutput {
        let display: Vec<_> = (0..self.entries.len()).rev().collect();
        let output = egui::Area::new(id)
            .order(Order::Tooltip)
            .fixed_pos(bounds.right_top() + egui::vec2(-STACK_MARGIN, STACK_MARGIN))
            .pivot(Align2::RIGHT_TOP)
            .movable(false)
            .fade_in(false)
            .show(ctx, |ui| {
                ui.set_width(TOAST_WIDTH);
                let heights: Vec<_> = display
                    .iter()
                    .map(|index| measure_surface(ui, &self.entries[*index]))
                    .collect();
                let gaps = points(heights.len().saturating_sub(1));
                let expanded_height = STACK_GAP.mul_add(gaps, heights.iter().sum::<f32>());
                let collapsed_height = STACK_PEEK.mul_add(gaps, heights[0]);
                let stack_height = egui::lerp(collapsed_height..=expanded_height, expansion);
                ui.set_min_size(egui::vec2(TOAST_WIDTH, stack_height));
                let origin = ui.min_rect().min;
                let mut interactions = Vec::new();
                let mut positions_moving = false;

                for index in (0..display.len()).rev() {
                    let entry = &mut self.entries[display[index]];
                    let collapsed_y = STACK_PEEK * points(index);
                    let expanded_y = expanded_y_for(&heights, index);
                    let target_y = egui::lerp(collapsed_y..=expanded_y, expansion);
                    entry.position = if mode == StackMode::Automatic {
                        approach(entry.position, target_y, delta)
                    } else {
                        target_y
                    };
                    positions_moving |= (entry.position - target_y).abs() > 0.1;
                    let inset = (1.0 - expansion) * 6.0 * points(index);
                    let height = egui::lerp(heights[0]..=heights[index], expansion);
                    let visibility = if mode == StackMode::Automatic {
                        entry.visibility()
                    } else {
                        1.0
                    };
                    let offset = egui::vec2((1.0 - visibility) * 24.0, -(1.0 - visibility) * 16.0);
                    let rect = Rect::from_min_size(
                        origin + egui::vec2(inset, entry.position) + offset,
                        egui::vec2(TOAST_WIDTH - inset * 2.0, height),
                    );
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    child.set_clip_rect(rect.expand(TOAST_SHADOW_CLIP_MARGIN));
                    child.set_opacity(visibility);
                    if let Some(action) = render_surface(
                        &mut child,
                        &ActionableProps {
                            kind: entry.toast.kind,
                            title: &entry.toast.title,
                            detail: entry.toast.detail.as_deref(),
                            action: entry.toast.action.as_deref(),
                            closable: true,
                        },
                        if index == 0 { 1.0 } else { expansion },
                    )
                    .action
                    {
                        interactions.push((entry.id, action));
                    }
                }
                (interactions, positions_moving)
            });
        let (interactions, positions_moving) = output.inner;
        StackOutput {
            interactions,
            positions_moving,
            rect: output.response.rect,
        }
    }
}

struct StackOutput {
    interactions: Vec<(ToastId, Action)>,
    positions_moving: bool,
    rect: Rect,
}

fn expanded_y_for(heights: &[f32], index: usize) -> f32 {
    STACK_GAP.mul_add(points(index), heights[..index].iter().sum::<f32>())
}

fn points(value: usize) -> f32 {
    f32::from(u16::try_from(value).expect("toast stack contains more than 65535 entries"))
}

fn approach(current: f32, target: f32, delta: f32) -> f32 {
    let progress = egui::emath::easing::cubic_out((delta / ANIMATION_TIME).clamp(0.0, 1.0));
    let next = egui::lerp(current..=target, progress);
    if (next - target).abs() <= 0.1 {
        target
    } else {
        next
    }
}

fn measure_surface(ui: &mut Ui, entry: &Entry) -> f32 {
    let rect = Rect::from_min_size(ui.min_rect().min, egui::vec2(TOAST_WIDTH, 512.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("toast-measure", entry.id.0))
            .max_rect(rect)
            .invisible(),
    );
    render_surface(
        &mut child,
        &ActionableProps {
            kind: entry.toast.kind,
            title: &entry.toast.title,
            detail: entry.toast.detail.as_deref(),
            action: entry.toast.action.as_deref(),
            closable: true,
        },
        1.0,
    )
    .rect
    .height()
}

fn surface(ui: &mut Ui, props: &ActionableProps<'_>) -> Option<Action> {
    render_surface(ui, props, 1.0).action
}

struct SurfaceOutput {
    action: Option<Action>,
    rect: Rect,
    #[cfg(test)]
    action_target: Option<(Id, Rect)>,
    #[cfg(test)]
    close_target: Option<(Id, Rect)>,
}

struct SurfaceContentOutput {
    action: Option<Action>,
    #[cfg(test)]
    action_target: Option<(Id, Rect)>,
    #[cfg(test)]
    close_target: Option<(Id, Rect)>,
}

fn render_surface(ui: &mut Ui, props: &ActionableProps<'_>, content_opacity: f32) -> SurfaceOutput {
    let palette = crate::theme::palette(ui);
    let accent = props.kind.color(ui);
    let output = egui::Frame::new()
        .fill(widget_theme::color32(
            palette.surfaces().layer(theme::Level::Two),
        ))
        .corner_radius(TOAST_RADIUS)
        .stroke(egui::Stroke::new(
            1.0,
            widget_theme::color32(palette.borders().subtle()),
        ))
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(96),
        })
        .inner_margin(egui::Margin {
            left: 16,
            right: 8,
            top: 8,
            bottom: 12,
        })
        .show(ui, |ui| render_surface_content(ui, props, content_opacity));
    ui.painter().rect_filled(
        severity_rail_rect(output.response.rect),
        SEVERITY_RAIL_RADIUS,
        accent.into_cint(),
    );
    SurfaceOutput {
        action: output.inner.action,
        rect: output.response.rect,
        #[cfg(test)]
        action_target: output.inner.action_target,
        #[cfg(test)]
        close_target: output.inner.close_target,
    }
}

fn render_surface_content(
    ui: &mut Ui,
    props: &ActionableProps<'_>,
    content_opacity: f32,
) -> SurfaceContentOutput {
    let palette = crate::theme::palette(ui);
    let accent = props.kind.color(ui);
    #[cfg(test)]
    let mut action_target = None;
    #[cfg(test)]
    let mut close_target = None;
    ui.multiply_opacity(content_opacity);
    ui.set_min_width(ui.available_width());
    ui.spacing_mut().item_spacing.y = 8.0;
    let mut action = None;
    ui.horizontal_top(|ui| {
        icons::Props {
            icon: props.kind.icon(),
            size: 18.0,
            color: accent,
        }
        .show(ui);
        ui.add_space(4.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.add(
                egui::Label::new(
                    crate::typography::semibold(props.title)
                        .color(palette.content().text_primary().into_cint()),
                )
                .selectable(false),
            );
            if let Some(detail) = props.detail {
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .size(12.0)
                            .color(palette.content().text_secondary().into_cint()),
                    )
                    .selectable(false)
                    .wrap(),
                );
            }
        });
        if props.closable {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let response = close_button(ui, palette.content().icon_primary());
                #[cfg(test)]
                {
                    close_target = Some((response.id, response.rect));
                }
                if response.clicked() {
                    action = Some(Action::Dismiss);
                }
            });
        }
    });
    if let Some(label) = props.action {
        ui.add_space(4.0);
        let response = ui
            .horizontal(|ui| {
                ui.add_space(30.0);
                button::Props {
                    label,
                    icon: None,
                    kind: button::Kind::Tertiary,
                    size: Size::Small,
                    width: button::Width::Fit,
                    enabled: true,
                }
                .show(ui)
            })
            .inner;
        #[cfg(test)]
        {
            action_target = Some((response.id, response.rect));
        }
        if response.clicked() {
            action = Some(Action::Invoke);
        }
    }
    SurfaceContentOutput {
        action,
        #[cfg(test)]
        action_target,
        #[cfg(test)]
        close_target,
    }
}

fn severity_rail_rect(surface: Rect) -> Rect {
    Rect::from_min_max(
        surface.min,
        egui::pos2(surface.min.x + SEVERITY_RAIL_WIDTH, surface.max.y),
    )
}

fn close_button(ui: &mut Ui, color: Color) -> Response {
    let (id, icon_rect) = ui.allocate_space(egui::Vec2::splat(CLOSE_ICON_SIZE));
    let target_expansion = (CLOSE_TARGET_SIZE - CLOSE_ICON_SIZE) / 2.0;
    let response = ui.interact(icon_rect.expand(target_expansion), id, egui::Sense::click());
    icons::X
        .mask()
        .tint(color.into_cint())
        .fit_to_exact_size(egui::Vec2::splat(CLOSE_ICON_SIZE))
        .paint_at(ui, icon_rect);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Dismiss"));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_make_toasts_persistent() {
        let toast = Toast::new(Kind::Information, "Device connected").action("Review");

        assert_eq!(toast.action.as_deref(), Some("Review"));
        assert_eq!(toast.duration, None);
    }

    #[test]
    fn repeated_transport_prefixes_are_collapsed() {
        assert_eq!(
            normalize_error_detail(
                "Application service call failed: receive error: receive error: connection closed"
            ),
            "Application service call failed: receive error: connection closed"
        );
        assert_eq!(
            normalize_error_detail(" send error: send error: unavailable "),
            "send error: unavailable"
        );
    }

    #[test]
    fn timers_pause_and_expire_without_losing_the_exit_animation() {
        let mut toasts = Toasts::default();
        toasts.push(Toast::new(Kind::Success, "Saved"));

        toasts.tick(DEFAULT_DURATION.as_secs_f32() + 1.0, false);
        assert!(!toasts.entries[0].dismissing);

        toasts.tick(DEFAULT_DURATION.as_secs_f32() + 1.0, true);
        assert!(toasts.entries[0].dismissing);
        assert!(toasts.entries[0].age.abs() < f32::EPSILON);
    }

    #[test]
    fn dismissing_one_toast_does_not_touch_its_siblings() {
        let mut toasts = Toasts::default();
        let first = toasts.push(Toast::new(Kind::Information, "First"));
        toasts.push(Toast::new(Kind::Information, "Second"));

        toasts.dismiss(first);

        assert!(toasts.entries[0].dismissing);
        assert!(!toasts.entries[1].dismissing);
    }

    #[test]
    fn expanded_mixed_height_toasts_keep_the_configured_gap() {
        let heights = [84.0, 50.0, 112.0];

        let first_gap = expanded_y_for(&heights, 1) - heights[0];
        let first_two_gaps = expanded_y_for(&heights, 2) - heights[0] - heights[1];
        assert!((first_gap - STACK_GAP).abs() < f32::EPSILON);
        assert!((first_two_gaps - STACK_GAP * 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn hover_focus_and_press_preserve_toast_geometry_and_stack_gap() {
        fn render(context: &egui::Context, events: Vec<egui::Event>) -> SurfaceOutput {
            let mut output = None;
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(TOAST_WIDTH, 200.0),
                        )),
                        events,
                        ..egui::RawInput::default()
                    },
                    |ui| {
                        ui.set_width(TOAST_WIDTH);
                        output = Some(render_surface(
                            ui,
                            &ActionableProps {
                                kind: Kind::Warning,
                                title: "One FIT file needs attention",
                                detail: Some("The source file could not be imported."),
                                action: Some("Review"),
                                closable: true,
                            },
                            1.0,
                        ));
                    },
                )
                .drop_without_applying_deltas();
            output.expect("egui rendered one pass")
        }

        fn geometry(output: &SurfaceOutput) -> (Rect, Rect, Rect) {
            (
                output.rect,
                output.action_target.expect("action target").1,
                output.close_target.expect("close target").1,
            )
        }

        let context = egui::Context::default();
        crate::install(&context);
        let rest = render(&context, Vec::new());
        let rest_geometry = geometry(&rest);
        let (action_id, action_rect) = rest.action_target.expect("action target");

        let hovered = render(
            &context,
            vec![egui::Event::PointerMoved(action_rect.center())],
        );
        assert_eq!(geometry(&hovered), rest_geometry);

        context.memory_mut(|memory| memory.request_focus(action_id));
        let focused = render(&context, Vec::new());
        assert_eq!(geometry(&focused), rest_geometry);

        let pressed = render(
            &context,
            vec![
                egui::Event::PointerMoved(action_rect.center()),
                egui::Event::PointerButton {
                    pos: action_rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(geometry(&pressed), rest_geometry);

        let gap =
            expanded_y_for(&[rest.rect.height(), hovered.rect.height()], 1) - rest.rect.height();
        assert!((gap - STACK_GAP).abs() < f32::EPSILON);
    }

    #[test]
    fn severity_rail_spans_the_complete_toast_height() {
        let surface = Rect::from_min_max(egui::pos2(12.0, 24.0), egui::pos2(332.0, 120.0));

        let rail = severity_rail_rect(surface);

        assert!((rail.top() - surface.top()).abs() < f32::EPSILON);
        assert!((rail.bottom() - surface.bottom()).abs() < f32::EPSILON);
        assert!((rail.left() - surface.left()).abs() < f32::EPSILON);
        assert!((rail.width() - SEVERITY_RAIL_WIDTH).abs() < f32::EPSILON);
    }

    #[test]
    fn title_only_toast_measurement_matches_normal_rendering() {
        let context = egui::Context::default();
        crate::install(&context);
        let entry = Entry::new(
            ToastId(0),
            Toast::new(Kind::Success, "Profile settings saved").persistent(),
        );
        let mut heights = None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(TOAST_WIDTH, 200.0),
            )),
            ..egui::RawInput::default()
        };
        context
            .run_ui(input, |ui| {
                ui.set_width(TOAST_WIDTH);
                let measured = measure_surface(ui, &entry);
                let rendered = render_surface(
                    ui,
                    &ActionableProps {
                        kind: entry.toast.kind,
                        title: &entry.toast.title,
                        detail: entry.toast.detail.as_deref(),
                        action: entry.toast.action.as_deref(),
                        closable: true,
                    },
                    1.0,
                )
                .rect
                .height();
                heights = Some((measured, rendered));
            })
            .drop_without_applying_deltas();

        let (measured, rendered) = heights.expect("egui rendered one pass");
        assert!((measured - rendered).abs() < f32::EPSILON);
    }
}
