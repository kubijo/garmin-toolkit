//! Application shell and primary navigation.

use cint::ColorInterop;
use egui::{Align, Align2, Layout, Order, Rect, Response, Sense, Ui, UiBuilder};
use garmin_color::theme;

use crate::{icons, profile, theme::color32};

const HEADER_HEIGHT: f32 = 36.0;
const NAV_ITEM_HEIGHT: f32 = 48.0;
const EXPANDED_NAV_WIDTH: f32 = 224.0;
const RAIL_WIDTH: f32 = 56.0;
const ACTIVE_MARKER_WIDTH: f32 = 3.0;
const ICON_SIZE: f32 = 20.0;
const CONTENT_PADDING: f32 = 24.0;
const PROFILE_MENU_WIDTH: f32 = 280.0;
const PROFILE_CONTROL_GAP: f32 = 8.0;
const COMPACT_HEADER_WIDTH: f32 = 600.0;
const NAV_GROUP_HEIGHT: f32 = 32.0;
const WINDOW_ACTION_WIDTH: f32 = 32.0;
const WINDOW_CONTROLS_WIDTH: f32 = WINDOW_ACTION_WIDTH * 3.0;

/// Primary-navigation presentation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Navigation {
    /// Icons and labels in a side navigation.
    #[default]
    Expanded,
    Rail,
}

impl Navigation {
    const fn width(self) -> f32 {
        match self {
            Self::Expanded => EXPANDED_NAV_WIDTH,
            Self::Rail => RAIL_WIDTH,
        }
    }
}

/// One primary destination.
#[derive(Clone, Copy, Debug)]
pub struct Destination<'a> {
    pub label: &'a str,
    pub icon: icons::Icon,
}

/// Related primary destinations.
#[derive(Clone, Copy, Debug)]
pub struct NavigationGroup<'a> {
    pub label: Option<&'a str>,
    pub destinations: &'a [Destination<'a>],
}

/// Optional native-window controls integrated into the shell header.
pub struct WindowControls<'a> {
    pub maximized: bool,
    pub minimize_label: &'a str,
    pub maximize_label: &'a str,
    pub restore_label: &'a str,
    pub close_label: &'a str,
}

/// Shell inputs.
pub struct Props<'a> {
    pub product_name: &'a str,
    pub navigation_groups: &'a [NavigationGroup<'a>],
    pub active: Option<usize>,
    pub navigation: Navigation,
    pub profile_selector: Option<&'a profile::SelectorProps<'a>>,
    pub toggle_label: &'a str,
    pub profile_label: &'a str,
    pub window_controls: Option<&'a WindowControls<'a>>,
}

/// Native-window interaction emitted by the shell header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowAction {
    Drag,
    Minimize,
    ToggleMaximize,
    Close,
}

/// Shell interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ToggleNavigation,
    Navigate(usize),
    Profile(profile::Action),
    Window(WindowAction),
}

/// Shell result and rendered page value.
pub struct Output<R> {
    pub action: Option<Action>,
    pub inner: R,
}

#[derive(Clone, Copy)]
struct HeaderSelectors {
    profile: Rect,
}

#[must_use]
pub fn show<R>(ui: &mut Ui, props: &Props<'_>, page: impl FnOnce(&mut Ui) -> R) -> Output<R> {
    let size = ui.available_size_before_wrap();
    let (root, _) = ui.allocate_exact_size(size, Sense::hover());
    let nav_width = if props.navigation_groups.is_empty() {
        0.0
    } else {
        props.navigation.width().min(root.width())
    };
    let header_bottom = (root.top() + HEADER_HEIGHT).min(root.bottom());
    let header = Rect::from_min_max(root.min, egui::pos2(root.right(), header_bottom));
    let navigation = Rect::from_min_max(
        egui::pos2(root.left(), header.bottom()),
        egui::pos2(root.left() + nav_width, root.bottom()),
    );
    let content = Rect::from_min_max(
        egui::pos2(navigation.right(), header.bottom()),
        root.right_bottom(),
    );

    paint_chrome(ui, root, header, navigation, nav_width > 0.0);

    let selectors = header_selectors(ui, header, props);
    let mut action = header_contents(ui, header, nav_width, selectors, props);
    let nav_action = navigation_contents(ui, navigation, props);
    if action.is_none() {
        action = nav_action;
    }

    let page_padding = egui::vec2(
        CONTENT_PADDING.min(content.width() / 2.0),
        CONTENT_PADDING.min(content.height() / 2.0),
    );
    let page_rect = content.shrink2(page_padding);
    let mut page_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(page_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    let inner = page(&mut page_ui);

    if let Some(profile_action) = profile_menu(ui, selectors.profile, props) {
        action = Some(Action::Profile(profile_action));
    }
    paint_window_border(ui, root);
    Output { action, inner }
}

fn paint_chrome(ui: &Ui, root: Rect, header: Rect, navigation: Rect, show_navigation: bool) {
    let theme = crate::theme::palette(ui);
    ui.painter()
        .rect_filled(root, 0.0, theme.surfaces().background().into_cint());
    ui.painter()
        .rect_filled(header, 0.0, theme.surfaces().chrome().into_cint());
    let border = egui::Stroke::new(1.0, theme.borders().strong().into_cint());
    ui.painter()
        .hline(header.x_range(), header.bottom(), border);
    if show_navigation {
        ui.painter()
            .rect_filled(navigation, 0.0, theme.surfaces().chrome().into_cint());
        ui.painter()
            .vline(navigation.right(), navigation.y_range(), border);
    }
}

fn paint_window_border(ui: &Ui, root: Rect) {
    ui.painter().rect_stroke(
        root,
        0.0,
        egui::Stroke::new(
            1.0,
            crate::theme::palette(ui).borders().subtle().into_cint(),
        ),
        egui::StrokeKind::Inside,
    );
}

fn header_contents(
    ui: &mut Ui,
    header: Rect,
    nav_width: f32,
    selectors: HeaderSelectors,
    props: &Props<'_>,
) -> Option<Action> {
    let show_navigation = !props.navigation_groups.is_empty();
    let toggle_rect = show_navigation
        .then(|| Rect::from_min_size(header.min, egui::vec2(RAIL_WIDTH, HEADER_HEIGHT)));
    let toggle_clicked = toggle_rect.is_some_and(|rect| {
        let response = shell_response(
            ui,
            rect,
            ui.make_persistent_id("shell-navigation-toggle"),
            props.toggle_label,
        );
        paint_header_action_background(ui, &response);
        icons::Props {
            icon: icons::LIST,
            size: ICON_SIZE,
            color: crate::theme::palette(ui).content().icon_primary(),
        }
        .paint_at(ui, rect.center());
        paint_focus_ring(ui, &response);
        let clicked = response.clicked();
        response.on_hover_text(props.toggle_label);
        clicked
    });

    let product_left = if !show_navigation {
        header.left() + CONTENT_PADDING
    } else if props.navigation == Navigation::Expanded {
        toggle_rect.map_or_else(|| header.left(), |rect| rect.right())
    } else {
        nav_width + 16.0
    };
    let product_rect = Rect::from_min_max(
        egui::pos2(product_left, header.top()),
        egui::pos2(selectors.profile.left().max(product_left), header.bottom()),
    );
    ui.painter().with_clip_rect(product_rect).text(
        egui::pos2(product_rect.left(), product_rect.center().y),
        Align2::LEFT_CENTER,
        props.product_name,
        egui::TextStyle::Button.resolve(ui.style()),
        color32(crate::theme::palette(ui).content().text_primary()),
    );

    let window_action = header_window_action(ui, header, product_rect, props.window_controls);

    let profile_clicked = props.profile_selector.is_some_and(|selector| {
        profile::header(ui, selectors.profile, selector, props.profile_label).clicked()
    });

    if toggle_clicked {
        Some(Action::ToggleNavigation)
    } else if profile_clicked {
        Some(Action::Profile(profile::Action::Toggle))
    } else {
        window_action
    }
}

fn header_selectors(ui: &Ui, header: Rect, props: &Props<'_>) -> HeaderSelectors {
    let controls_width = props.window_controls.map_or(0.0, |_| WINDOW_CONTROLS_WIDTH);
    let controls_left = header.right() - controls_width;
    let profile_right = controls_left - props.profile_selector.map_or(0.0, |_| PROFILE_CONTROL_GAP);
    let preferred_profile_width = preferred_profile_width(ui, header.width(), props);
    let profile_width = if props.profile_selector.is_some() {
        preferred_profile_width.min((profile_right - header.left()).max(0.0))
    } else {
        0.0
    };
    let profile = Rect::from_min_max(
        egui::pos2(profile_right - profile_width, header.top()),
        egui::pos2(profile_right, header.bottom()),
    );
    HeaderSelectors { profile }
}

fn header_window_action(
    ui: &Ui,
    header: Rect,
    drag_rect: Rect,
    controls: Option<&WindowControls<'_>>,
) -> Option<Action> {
    controls.and_then(|controls| {
        let controls_action = window_controls(ui, header, controls);
        let drag = ui.interact(
            drag_rect,
            ui.make_persistent_id("shell-window-drag"),
            Sense::click_and_drag(),
        );
        if drag.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if drag.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if drag.double_clicked() {
            Some(Action::Window(WindowAction::ToggleMaximize))
        } else if drag.drag_started() {
            Some(Action::Window(WindowAction::Drag))
        } else {
            controls_action.map(Action::Window)
        }
    })
}

fn preferred_profile_width(ui: &Ui, header_width: f32, props: &Props<'_>) -> f32 {
    let Some(selector) = props.profile_selector else {
        return 0.0;
    };
    if header_width < COMPACT_HEADER_WIDTH {
        return 48.0;
    }
    profile::preferred_header_width(ui, selector)
}

fn window_controls(ui: &Ui, header: Rect, props: &WindowControls<'_>) -> Option<WindowAction> {
    let minimize_rect = Rect::from_min_size(
        egui::pos2(header.right() - WINDOW_CONTROLS_WIDTH, header.top()),
        egui::vec2(WINDOW_ACTION_WIDTH, HEADER_HEIGHT),
    );
    let maximize_rect = minimize_rect.translate(egui::vec2(WINDOW_ACTION_WIDTH, 0.0));
    let close_rect = maximize_rect.translate(egui::vec2(WINDOW_ACTION_WIDTH, 0.0));
    let minimize = window_control(
        ui,
        minimize_rect,
        "shell-window-minimize",
        props.minimize_label,
        icons::MINUS,
        false,
    );
    let maximize = window_control(
        ui,
        maximize_rect,
        "shell-window-maximize",
        if props.maximized {
            props.restore_label
        } else {
            props.maximize_label
        },
        if props.maximized {
            icons::WINDOW_RESTORE
        } else {
            icons::WINDOW_MAXIMIZE
        },
        false,
    );
    let close = window_control(
        ui,
        close_rect,
        "shell-window-close",
        props.close_label,
        icons::X,
        true,
    );
    if minimize.clicked() {
        Some(WindowAction::Minimize)
    } else if maximize.clicked() {
        Some(WindowAction::ToggleMaximize)
    } else if close.clicked() {
        Some(WindowAction::Close)
    } else {
        None
    }
}

fn window_control(
    ui: &Ui,
    rect: Rect,
    id: &str,
    label: &str,
    icon: icons::Icon,
    danger: bool,
) -> Response {
    let response = shell_response(ui, rect, ui.make_persistent_id(id), label);
    let palette = crate::theme::palette(ui);
    let (background, foreground) = if response.highlighted() {
        let states = if danger {
            palette.buttons().danger()
        } else {
            palette.buttons().primary()
        };
        let state = if response.is_pointer_button_down_on() {
            states.active()
        } else {
            states.rest()
        };
        (state.background(), state.foreground())
    } else {
        (
            palette.surfaces().chrome(),
            palette.content().icon_secondary(),
        )
    };
    ui.painter().rect_filled(rect, 0.0, background.into_cint());
    icons::Props {
        icon,
        size: 12.0,
        color: foreground,
    }
    .paint_at(ui, rect.center());
    paint_focus_ring(ui, &response);
    response
}

fn profile_menu(ui: &Ui, selector_rect: Rect, props: &Props<'_>) -> Option<profile::Action> {
    let selector = props.profile_selector?;
    if !selector.expanded {
        return None;
    }
    let geometry = crate::header_selector::menu_geometry(
        selector_rect,
        PROFILE_MENU_WIDTH,
        ui.min_rect().left(),
    );
    egui::Area::new(ui.make_persistent_id("shell-profile-menu"))
        .order(Order::Foreground)
        .fixed_pos(geometry.position)
        .show(ui.ctx(), |ui| {
            ui.set_width(geometry.width);
            profile::menu(
                ui,
                &profile::MenuProps {
                    intl: selector.intl,
                },
            )
        })
        .inner
}

fn navigation_contents(ui: &Ui, rect: Rect, props: &Props<'_>) -> Option<Action> {
    let mut action = None;
    let mut top = rect.top();
    let mut destination_index = 0;
    for group in props.navigation_groups {
        if let Some(label) = group.label
            && props.navigation == Navigation::Expanded
        {
            let group_rect = Rect::from_min_size(
                egui::pos2(rect.left(), top),
                egui::vec2(rect.width(), NAV_GROUP_HEIGHT),
            );
            if group_rect.bottom() > rect.bottom() {
                break;
            }
            ui.painter().text(
                egui::pos2(group_rect.left() + 16.0, group_rect.center().y),
                Align2::LEFT_CENTER,
                label,
                egui::TextStyle::Small.resolve(ui.style()),
                color32(crate::theme::palette(ui).content().text_secondary()),
            );
            top += NAV_GROUP_HEIGHT;
        }
        for destination in group.destinations {
            let item_rect = Rect::from_min_size(
                egui::pos2(rect.left(), top),
                egui::vec2(rect.width(), NAV_ITEM_HEIGHT),
            );
            top += NAV_ITEM_HEIGHT;
            if item_rect.bottom() > rect.bottom() {
                break;
            }
            let active = props.active == Some(destination_index);
            let response = shell_response(
                ui,
                item_rect,
                ui.make_persistent_id(("shell-destination", destination_index)),
                destination.label,
            );
            paint_destination(
                ui,
                item_rect,
                *destination,
                props.navigation,
                active,
                &response,
            );
            let clicked = response.clicked();
            if props.navigation == Navigation::Rail {
                response.on_hover_text(destination.label);
            }
            if clicked {
                action = Some(Action::Navigate(destination_index));
            }
            destination_index += 1;
        }
    }
    action
}

fn paint_destination(
    ui: &Ui,
    rect: Rect,
    destination: Destination<'_>,
    navigation: Navigation,
    active: bool,
    response: &Response,
) {
    let theme = crate::theme::palette(ui);
    if response.highlighted() || active {
        let level = if response.highlighted() {
            theme::Level::Two
        } else {
            theme::Level::One
        };
        ui.painter()
            .rect_filled(rect, 0.0, theme.surfaces().layer_hover(level).into_cint());
    }
    if active {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(ACTIVE_MARKER_WIDTH, rect.height())),
            0.0,
            theme.interaction().interactive().into_cint(),
        );
    }

    let icon_center = egui::pos2(rect.left() + RAIL_WIDTH / 2.0, rect.center().y);
    icons::Props {
        icon: destination.icon,
        size: ICON_SIZE,
        color: if active {
            theme.content().icon_primary()
        } else {
            theme.content().icon_secondary()
        },
    }
    .paint_at(ui, icon_center);
    if navigation == Navigation::Expanded {
        ui.painter().text(
            egui::pos2(rect.left() + RAIL_WIDTH, rect.center().y),
            Align2::LEFT_CENTER,
            destination.label,
            egui::TextStyle::Button.resolve(ui.style()),
            color32(theme.content().text_primary()),
        );
    }
    paint_focus_ring(ui, response);
}

fn shell_response(ui: &Ui, rect: Rect, id: egui::Id, label: &str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.highlight()
    } else {
        response
    }
}

fn paint_header_action_background(ui: &Ui, response: &Response) {
    if response.highlighted() {
        ui.painter().rect_filled(
            response.rect,
            0.0,
            crate::theme::palette(ui)
                .surfaces()
                .layer_hover(theme::Level::One)
                .into_cint(),
        );
    }
}

fn paint_focus_ring(ui: &Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            0.0,
            egui::Stroke::new(
                2.0,
                crate::theme::palette(ui).interaction().focus().into_cint(),
            ),
            egui::StrokeKind::Inside,
        );
    }
}
