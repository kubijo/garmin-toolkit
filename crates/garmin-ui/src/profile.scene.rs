use gallery::prelude::*;
use garmin_color::{Color, swatch};
use garmin_ui::profile;

scene_meta! { title: "Application / Profiles" }

const AVATAR_SAMPLE_WIDTH: f32 = 96.0;
const AVATAR_SAMPLE_LABEL_HEIGHT: f32 = 24.0;

struct ChooserSceneProps {
    dataset: usize,
    width: f32,
    height: f32,
}

#[scene]
fn chooser(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let props = ChooserSceneProps {
        dataset: ctx.buttons("profiles", &["household", "single", "empty"], 0),
        width: ctx.slider("width", 720.0, 320.0, 960.0, 1.0),
        height: ctx.slider("height", 520.0, 400.0, 720.0, 1.0),
    };
    let texture = avatar_texture(ui);
    let image = profile::AvatarImage::texture(egui::load::SizedTexture::from_handle(&texture));
    let profiles = [
        profile::ProfileProps {
            display_name: "Alex Rider",
            accent: swatch::cyan::G40,
            avatar: Some(&image),
        },
        profile::ProfileProps {
            display_name: "Sam Runner",
            accent: swatch::magenta::G40,
            avatar: None,
        },
        profile::ProfileProps {
            display_name: "Taylor Cyclist",
            accent: swatch::green::G40,
            avatar: None,
        },
    ];
    let profiles = match props.dataset {
        1 => &profiles[..1],
        2 => &profiles[..0],
        _ => &profiles[..],
    };
    let intl = globals.intl();

    stage!(ctx, ui, |ui| {
        ui.set_width(props.width);
        ui.set_height(props.height);
        let _ = profile::chooser(
            ui,
            &profile::ChooserProps {
                intl: &intl,
                profiles,
            },
        );
    });
}

#[scene]
fn avatars(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let size = ctx.slider("size", 48.0, 24.0, 96.0, 1.0);
    let texture = avatar_texture(ui);
    let image = profile::AvatarImage::texture(egui::load::SizedTexture::from_handle(&texture));
    let samples = [
        ("Alex Rider", swatch::cyan::G40, Some(&image)),
        ("Sam Runner", swatch::magenta::G40, None),
        ("Taylor Cyclist", swatch::green::G40, None),
        (
            "",
            garmin_ui::theme::palette(ui)
                .interaction()
                .interactive(),
            None,
        ),
    ];

    stage!(ctx, ui, |ui| {
        ui.horizontal(|ui| {
            for (name, accent, image) in samples {
                avatar_sample(ui, name, accent, image, size);
            }
        });
    });
}

fn avatar_sample(
    ui: &mut egui::Ui,
    name: &str,
    accent: Color,
    image: Option<&profile::AvatarImage>,
    size: f32,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(
            size.max(AVATAR_SAMPLE_WIDTH),
            size + AVATAR_SAMPLE_LABEL_HEIGHT,
        ),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            profile::AvatarProps {
                display_name: name,
                accent,
                image,
                size,
            }
            .show(ui);
            ui.label(if name.is_empty() { "blank" } else { name });
        },
    );
}

fn avatar_fixture() -> egui::ColorImage {
    const SIDE: usize = 64;
    let mut image = egui::ColorImage::filled([SIDE, SIDE], egui::Color32::from_rgb(0, 67, 83));
    for y in 0..SIDE {
        for x in 0..SIDE {
            let head = x.abs_diff(32).pow(2) + y.abs_diff(25).pow(2) < 16_usize.pow(2);
            let shoulders = y > 43 && x.abs_diff(32) < (y - 40) * 2;
            if head {
                image[(x, y)] = egui::Color32::from_rgb(255, 216, 179);
            }
            if shoulders {
                image[(x, y)] = egui::Color32::from_rgb(69, 137, 255);
            }
        }
    }
    image
}

fn avatar_texture(ui: &egui::Ui) -> egui::TextureHandle {
    let id = egui::Id::new("gallery-avatar-fixture");
    if let Some(texture) = ui.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let texture = ui.ctx().load_texture(
        "gallery-avatar-fixture",
        avatar_fixture(),
        egui::TextureOptions::LINEAR,
    );
    ui.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}
