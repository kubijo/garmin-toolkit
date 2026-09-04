//! Profile identity and selection components.

use cint::ColorInterop;
use egui::{
    Align, Align2, FontId, Image, ImageSource, Layout, Response, RichText, Sense, Ui, UiBuilder,
    Vec2,
    load::{Bytes, SizedTexture},
};
use garmin_color::{Color, theme};
use garmin_i18n::{Intl, format_message};
use std::borrow::Cow;

use crate::{header_selector, icons, theme::color32};

const AVATAR_SIZE: f32 = 36.0;
const ROW_PADDING: f32 = 8.0;
const CHOOSER_MAX_WIDTH: f32 = 440.0;
const CHOOSER_AVATAR_SIZE: f32 = 48.0;
const CHOOSER_ROW_HEIGHT: f32 = 72.0;
const CHOOSER_ROW_PADDING: f32 = 12.0;
const HEADER_AVATAR_SIZE: f32 = 28.0;

/// An avatar image already owned by the presentation boundary.
#[derive(Clone)]
pub struct AvatarImage(ImageSource<'static>);

impl AvatarImage {
    #[must_use]
    pub fn encoded(uri: impl Into<Cow<'static, str>>, bytes: impl Into<Bytes>) -> Self {
        Self(ImageSource::Bytes {
            uri: uri.into(),
            bytes: bytes.into(),
        })
    }

    #[must_use]
    pub const fn texture(texture: SizedTexture) -> Self {
        Self(ImageSource::Texture(texture))
    }

    fn source(&self) -> ImageSource<'static> {
        self.0.clone()
    }
}

/// Inputs for one avatar.
pub struct AvatarProps<'a> {
    pub display_name: &'a str,
    pub accent: Color,
    pub image: Option<&'a AvatarImage>,
    pub size: f32,
}

impl AvatarProps<'_> {
    /// Renders the avatar.
    pub fn show(&self, ui: &mut Ui) -> Response {
        let palette = crate::theme::palette(ui);
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::hover());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), self.size / 2.0, self.accent.into_cint());

        let image_rect = rect.shrink(2.0);
        painter.circle_filled(
            image_rect.center(),
            image_rect.width() / 2.0,
            palette.surfaces().layer(theme::Level::One).into_cint(),
        );
        if let Some(image) = self.image {
            Image::new(image.source())
                .fit_to_exact_size(image_rect.size())
                .corner_radius((image_rect.width() / 2.0).round())
                .paint_at(ui, image_rect);
        } else {
            painter.text(
                image_rect.center(),
                Align2::CENTER_CENTER,
                initials(self.display_name),
                FontId::proportional(self.size * 0.34),
                color32(palette.content().text_primary()),
            );
        }
        response
    }
}

/// Presentation data for one profile.
pub struct ProfileProps<'a> {
    pub display_name: &'a str,
    pub accent: Color,
    pub avatar: Option<&'a AvatarImage>,
}

/// Inputs for the profile selector.
pub struct SelectorProps<'a> {
    pub intl: &'a Intl,
    pub profiles: &'a [ProfileProps<'a>],
    pub selected: Option<usize>,
    pub expanded: bool,
}

/// Inputs for account actions below a separate trigger.
pub struct MenuProps<'a> {
    pub intl: &'a Intl,
}

/// Inputs for the application-entry profile chooser.
pub struct ChooserProps<'a> {
    pub intl: &'a Intl,
    pub profiles: &'a [ProfileProps<'a>],
}

/// A profile selector interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Toggle,
    Select(usize),
    Create,
    Settings,
    Logout,
}

/// Paints the active-profile control into caller-owned header geometry.
pub fn header(ui: &mut Ui, rect: egui::Rect, props: &SelectorProps<'_>, label: &str) -> Response {
    let selected = props.selected.and_then(|index| props.profiles.get(index));
    let palette = crate::theme::palette(ui);
    header_selector::control(
        ui,
        rect,
        ui.make_persistent_id("active-profile"),
        label,
        props.expanded,
        |ui, wide| {
            if let Some(profile) = selected {
                AvatarProps {
                    display_name: profile.display_name,
                    accent: profile.accent,
                    image: profile.avatar,
                    size: HEADER_AVATAR_SIZE,
                }
                .show(ui);
                if wide {
                    ui.add(
                        egui::Label::new(profile.display_name)
                            .selectable(false)
                            .truncate(),
                    );
                }
            } else {
                icons::Props {
                    icon: icons::USER_CIRCLE,
                    size: HEADER_AVATAR_SIZE,
                    color: palette.content().icon_secondary(),
                }
                .show(ui);
            }
        },
    )
}

pub(crate) fn preferred_header_width(ui: &Ui, props: &SelectorProps<'_>) -> f32 {
    props
        .selected
        .and_then(|index| props.profiles.get(index))
        .map_or(48.0, |profile| {
            header_selector::preferred_width(
                ui,
                HEADER_AVATAR_SIZE,
                std::iter::once(profile.display_name),
            )
        })
}

#[must_use]
pub fn chooser(ui: &mut Ui, props: &ChooserProps<'_>) -> Option<Action> {
    let heading = format_message!(
        props.intl,
        default_message: "Choose a profile",
    );
    let empty = format_message!(
        props.intl,
        default_message: "No profiles yet",
    );
    let create = format_message!(
        props.intl,
        default_message: "Create a profile",
    );
    let available = ui.available_size_before_wrap();
    let content_height = chooser_content_height(props.profiles.len());
    let top_padding = if available.y.is_finite() {
        ((available.y - content_height) / 2.0).max(24.0)
    } else {
        24.0
    };
    let width = available.x.min(CHOOSER_MAX_WIDTH);
    let mut action = None;

    ui.vertical_centered(|ui| {
        ui.add_space(top_padding);
        ui.allocate_ui_with_layout(
            egui::vec2(width, content_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(crate::typography::semibold(heading).size(28.0));
                });
                ui.add_space(32.0);

                if props.profiles.is_empty() {
                    ui.label(RichText::new(empty).weak());
                    ui.add_space(16.0);
                } else {
                    for (index, profile) in props.profiles.iter().enumerate() {
                        if chooser_profile_row(ui, profile).clicked() {
                            action = Some(Action::Select(index));
                        }
                        ui.add_space(8.0);
                    }
                    ui.add_space(8.0);
                }

                if chooser_create_row(ui, &create).clicked() {
                    action = Some(Action::Create);
                }
            },
        );
    });
    action
}

fn chooser_content_height(profile_count: usize) -> f32 {
    let profiles_height = std::iter::repeat_n(CHOOSER_ROW_HEIGHT + 8.0, profile_count).sum::<f32>();
    let empty_height = if profile_count == 0 { 36.0 } else { 8.0 };
    40.0 + 32.0 + profiles_height + empty_height + 56.0
}

fn chooser_profile_row(ui: &mut Ui, profile: &ProfileProps<'_>) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), CHOOSER_ROW_HEIGHT),
        Sense::click(),
    );
    let response = row_response(ui, response);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            profile.display_name,
        )
    });
    let palette = crate::theme::palette(ui);
    let fill = if response.highlighted() {
        palette.surfaces().layer_hover(theme::Level::One)
    } else {
        palette.surfaces().layer(theme::Level::One)
    };
    let border = if response.highlighted() {
        palette.borders().strong()
    } else {
        palette.borders().subtle()
    };
    ui.painter().rect_filled(rect, 0.0, fill.into_cint());
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, border.into_cint()),
        egui::StrokeKind::Inside,
    );

    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink(CHOOSER_ROW_PADDING))
            .layout(Layout::left_to_right(Align::Center)),
    );
    avatar(&mut child, profile, CHOOSER_AVATAR_SIZE);
    child
        .add(egui::Label::new(crate::typography::semibold(profile.display_name)).selectable(false));
    child.with_layout(Layout::right_to_left(Align::Center), |ui| {
        icons::Props {
            icon: icons::CARET_RIGHT,
            size: 20.0,
            color: palette.content().icon_secondary(),
        }
        .show(ui);
    });
    paint_focus_ring(ui, &response);
    response
}

fn chooser_create_row(ui: &mut Ui, label: &str) -> Response {
    const HEIGHT: f32 = 56.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), Sense::click());
    let response = row_response(ui, response);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let visuals = ui.style().interact(&response);
    ui.painter().rect_filled(rect, 0.0, visuals.weak_bg_fill);
    ui.painter()
        .rect_stroke(rect, 0.0, visuals.bg_stroke, egui::StrokeKind::Inside);

    let icon_center = egui::pos2(
        rect.left() + CHOOSER_ROW_PADDING + CHOOSER_AVATAR_SIZE / 2.0,
        rect.center().y,
    );
    icons::Props {
        icon: icons::PLUS,
        size: 20.0,
        color: crate::theme::palette(ui).content().icon_primary(),
    }
    .paint_at(ui, icon_center);
    ui.painter().text(
        egui::pos2(
            rect.left() + CHOOSER_ROW_PADDING + CHOOSER_AVATAR_SIZE + ui.spacing().item_spacing.x,
            rect.center().y,
        ),
        Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        color32(crate::theme::palette(ui).content().text_primary()),
    );
    paint_focus_ring(ui, &response);
    response
}

#[must_use]
pub fn menu(ui: &mut Ui, props: &MenuProps<'_>) -> Option<Action> {
    let settings = format_message!(
        props.intl,
        default_message: "Profile settings",
    );
    let logout = format_message!(
        props.intl,
        default_message: "Log out",
    );
    let mut action = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        header_selector::menu(ui, |ui| {
            if action_row(
                ui,
                &settings,
                icons::GEAR,
                header_selector::RowKind::Default,
                false,
            )
            .clicked()
            {
                action = Some(Action::Settings);
            }
            if action_row(
                ui,
                &logout,
                icons::SIGN_OUT,
                header_selector::RowKind::Danger,
                true,
            )
            .clicked()
            {
                action = Some(Action::Logout);
            }
        });
    });
    action
}

fn row_response(ui: &Ui, response: Response) -> Response {
    if ui.rect_contains_pointer(response.rect) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.highlight()
    } else {
        response
    }
}

fn action_row(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    kind: header_selector::RowKind,
    divided: bool,
) -> Response {
    let response = header_selector::row(ui, 44.0, divided, true, kind, |ui, rect, foreground| {
        let icon_center = egui::pos2(
            rect.left() + ROW_PADDING + AVATAR_SIZE / 2.0,
            rect.center().y,
        );
        icons::Props {
            icon,
            size: 18.0,
            color: foreground,
        }
        .paint_at(ui, icon_center);
        let label_position = egui::pos2(
            rect.left() + ROW_PADDING + AVATAR_SIZE + ui.spacing().item_spacing.x,
            rect.center().y,
        );
        ui.painter().text(
            label_position,
            Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Button.resolve(ui.style()),
            color32(foreground),
        );
    });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
}

fn paint_focus_ring(ui: &Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            0.0,
            egui::Stroke::new(
                2.0,
                crate::theme::palette(ui)
                    .borders()
                    .interactive()
                    .into_cint(),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

fn avatar(ui: &mut Ui, profile: &ProfileProps<'_>, size: f32) {
    AvatarProps {
        display_name: profile.display_name,
        accent: profile.accent,
        image: profile.avatar,
        size,
    }
    .show(ui);
}

fn initials(display_name: &str) -> String {
    let mut words = display_name.split_whitespace();
    let first = words.next().and_then(|word| word.chars().next());
    let last = words.next_back().and_then(|word| word.chars().next());
    match (first, last) {
        (Some(first), Some(last)) => first.to_uppercase().chain(last.to_uppercase()).collect(),
        (Some(first), None) => first.to_uppercase().collect(),
        (None, _) => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_use_outer_name_parts() {
        assert_eq!(initials("Alex Morgan Rider"), "AR");
        assert_eq!(initials("sam"), "S");
        assert_eq!(initials("  "), "?");
    }
}
