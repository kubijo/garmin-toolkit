use gallery::prelude::*;
use garmin_ui::image_crop;

scene_meta! { title: "Components / Media / Image crop" }

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let zoom = ctx.slider("zoom", 1.25, 1.0, 4.0, 0.05);
    let center_x = ctx.slider("horizontal", 0.5, 0.0, 1.0, 0.01);
    let center_y = ctx.slider("vertical", 0.45, 0.0, 1.0, 0.01);
    let disabled = ctx.toggle("disabled", false);
    stage!(ctx, ui, |ui| {
        ui.set_width(360.0);
        let texture = texture(ui);
        let mut state = image_crop::State::from_center_zoom([center_x, center_y], zoom);
        let _ = image_crop::show(
            ui,
            &mut state,
            image_crop::Props {
                texture: egui::load::SizedTexture::from_handle(&texture),
                source_size: [640, 400],
                zoom_label: "Zoom",
                disabled,
            },
        );
    });
}

#[scene]
fn dialog(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, (640, 680), |ui| {
        let texture = texture(ui);
        let mut state = image_crop::State::default();
        let _ = garmin_ui::modal::show(
            ui,
            egui::Id::new("gallery-avatar-crop-dialog"),
            &garmin_ui::modal::Props {
                title: "Adjust profile picture",
                description: Some("Drag to reposition. Scroll or use the slider to zoom."),
                size: garmin_ui::modal::Size::Medium,
                presentation: garmin_ui::modal::Presentation::Contained,
                cancel_label: "Cancel",
                backdrop_closes: Some(true),
                primary: garmin_ui::modal::Primary {
                    label: "Save picture",
                    icon: Some(garmin_ui::icons::CHECK),
                    kind: garmin_ui::modal::PrimaryKind::Confirm,
                    enabled: true,
                },
            },
            |ui| {
                ui.vertical_centered(|ui| {
                    let _ = image_crop::show(
                        ui,
                        &mut state,
                        image_crop::Props {
                            texture: egui::load::SizedTexture::from_handle(&texture),
                            source_size: [640, 400],
                            zoom_label: "Zoom",
                            disabled: false,
                        },
                    );
                });
            },
        );
    });
}

fn texture(ui: &egui::Ui) -> egui::TextureHandle {
    let id = egui::Id::new("gallery-image-crop-fixture");
    if let Some(texture) = ui.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let texture = ui.ctx().load_texture(
        "gallery-image-crop-fixture",
        fixture(),
        egui::TextureOptions::LINEAR,
    );
    ui.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

fn fixture() -> egui::ColorImage {
    let size = [640, 400];
    let mut image = egui::ColorImage::filled(size, egui::Color32::from_rgb(38, 38, 38));
    for y in 0..size[1] {
        for x in 0..size[0] {
            let sky = y < 210;
            let color = if sky {
                egui::Color32::from_rgb(69, 137, 255)
            } else if (x / 64 + y / 48) % 2 == 0 {
                egui::Color32::from_rgb(36, 161, 72)
            } else {
                egui::Color32::from_rgb(24, 124, 58)
            };
            image[(x, y)] = color;
        }
    }
    image
}
