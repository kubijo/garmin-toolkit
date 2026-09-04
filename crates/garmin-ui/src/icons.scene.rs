use gallery::prelude::*;
use garmin_color::{Color, theme};
use garmin_ui::icons;

scene_meta! { title: "Desktop / Components / Icons" }

const TINT_NAMES: &[&str] = &[
    "primary",
    "secondary",
    "accent",
    "success",
    "warning",
    "error",
];
const TILE_WIDTH: f32 = 104.0;
const TILE_PADDING: f32 = 6.0;
const LABEL_GAP: f32 = 5.0;
const LABEL_HEIGHT: f32 = 15.0;

struct CatalogProps {
    query: String,
    size: f32,
    color: Color,
}

#[scene(default)]
fn catalog(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let query = ctx.text("filter", "");
    let size = ctx.slider("size", 32.0, 16.0, 64.0, 1.0);
    let tint = ctx.buttons("tint", TINT_NAMES, 0);
    let props = CatalogProps {
        query: query.trim().to_ascii_uppercase(),
        size,
        color: tint_color(tint),
    };

    ui.label(format!(
        "{} icons. Click one to copy its `icons::NAME` reference.",
        icons::ALL.len()
    ));
    stage!(ctx, ui, scroll, |ui| show_catalog(ui, &props));
}

fn show_catalog(ui: &mut egui::Ui, props: &CatalogProps) {
    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
    ui.horizontal_wrapped(|ui| {
        for &icon in icons::ALL {
            if props.query.is_empty() || icon.as_name().contains(&props.query) {
                show_icon(ui, icon, props);
            }
        }
    });
}

fn show_icon(ui: &mut egui::Ui, icon: icons::Icon, props: &CatalogProps) {
    let copied_id = ui.id().with("copied-icon");
    let selected = ui.data(|data| data.get_temp::<&'static str>(copied_id)) == Some(icon.as_name());
    let tile_size = egui::vec2(
        TILE_WIDTH,
        TILE_PADDING.mul_add(2.0, props.size + LABEL_GAP + LABEL_HEIGHT),
    );
    let (rect, response) = ui.allocate_exact_size(tile_size, egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let visuals = ui.style().interact_selectable(&response, selected);
    if response.hovered() || selected {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), visuals.bg_fill);
    }

    let icon_center = egui::pos2(
        rect.center().x,
        rect.top() + TILE_PADDING + props.size / 2.0,
    );
    icons::Props {
        icon,
        size: props.size,
        color: props.color,
    }
    .paint_at(ui, icon_center);
    ui.painter().text(
        egui::pos2(
            rect.center().x,
            icon_center.y + props.size / 2.0 + LABEL_GAP,
        ),
        egui::Align2::CENTER_TOP,
        icon.as_name().to_ascii_lowercase().replace('_', "-"),
        egui::FontId::proportional(11.0),
        visuals.text_color(),
    );

    if response.clicked() {
        let reference = format!("icons::{}", icon.as_name());
        ui.data_mut(|data| data.insert_temp(copied_id, icon.as_name()));
        ui.ctx().copy_text(reference.clone());
        action(format!("Copied `{reference}`"));
    }
}

const fn tint_color(index: usize) -> Color {
    match index {
        1 => theme::GRAY_100.content().icon_secondary(),
        2 => theme::GRAY_100.interaction().interactive(),
        3 => theme::GRAY_100.support().success(),
        4 => theme::GRAY_100.support().warning(),
        5 => theme::GRAY_100.support().error(),
        _ => theme::GRAY_100.content().icon_primary(),
    }
}
