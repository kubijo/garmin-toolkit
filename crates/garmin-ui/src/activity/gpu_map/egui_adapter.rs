//! eframe installation and logical-to-physical placement for the shared map renderer.

use std::sync::Arc;

use egui::{Rect, Shape, epaint::PaintCallbackInfo};
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor};

use super::{
    Frame, WgpuMapHandle,
    renderer::{DrawRegion, Renderer},
};
use crate::activity::map_style;

/// Install a device-scoped map renderer for eframe callbacks.
///
/// `sample_count` must match the eframe render pass which invokes the callback.
/// Pipelines are owned by the returned handle, not eframe's callback resource map.
///
/// # Panics
/// Panics unless `sample_count` is 1 or 4, the configurations supported by this renderer.
#[must_use]
pub fn install(render_state: &egui_wgpu::RenderState, sample_count: u32) -> WgpuMapHandle {
    WgpuMapHandle::new(
        &render_state.device,
        render_state.target_format,
        sample_count,
    )
}

pub(super) fn paint_callback(rect: Rect, frame: Arc<Frame>, renderer: Arc<Renderer>) -> Shape {
    Shape::Callback(Callback::new_paint_callback(
        rect,
        Paint::new(rect, frame, renderer),
    ))
}

struct Paint {
    rect: Rect,
    // Keep the same immutable snapshot through prepare and paint, even if a newer scene arrives.
    frame: Arc<Frame>,
    renderer: Arc<Renderer>,
}

impl Paint {
    fn new(rect: Rect, frame: Arc<Frame>, renderer: Arc<Renderer>) -> Self {
        Self {
            rect,
            frame,
            renderer,
        }
    }
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(region) = draw_region(&PaintCallbackInfo {
            viewport: self.rect,
            clip_rect: self.rect,
            pixels_per_point: screen_descriptor.pixels_per_point,
            screen_size_px: screen_descriptor.size_in_pixels,
        }) {
            self.renderer.prepare(&self.frame, region, queue);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        if let Some(region) = draw_region(&info) {
            if info.viewport == self.rect {
                self.renderer.draw(&self.frame, region, render_pass);
            } else {
                self.renderer
                    .draw_relocated(&self.frame, region, render_pass);
            }
        }
    }
}

fn draw_region(info: &PaintCallbackInfo) -> Option<DrawRegion> {
    let viewport = pixels(&info.viewport_in_pixels())?;
    let clip = map_style::clip_rect(info.viewport, info.clip_rect);
    let scissor = pixels(
        &PaintCallbackInfo {
            viewport: clip,
            clip_rect: clip,
            pixels_per_point: info.pixels_per_point,
            screen_size_px: info.screen_size_px,
        }
        .viewport_in_pixels(),
    )?;
    let projection = [
        info.viewport.left(),
        info.viewport.top(),
        info.viewport.width(),
        info.viewport.height(),
    ]
    .map(|value| value * info.pixels_per_point);
    Some(DrawRegion {
        projection,
        viewport,
        scissor,
    })
}

fn pixels(rect: &egui::epaint::ViewportInPixels) -> Option<[u32; 4]> {
    Some([
        rect.left_px.try_into().ok()?,
        rect.top_px.try_into().ok()?,
        rect.width_px.try_into().ok()?,
        rect.height_px.try_into().ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn transformed_callbacks_match_final_placement_in_one_submission() {
        use super::super::platform;
        use crate::activity::map_runtime::MapMetrics;

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .unwrap();
        let (device, queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .unwrap();
        for samples in [1, 4] {
            let handle = WgpuMapHandle::new(&device, wgpu::TextureFormat::Rgba8Unorm, samples);
            let renderer = Arc::new(Renderer::new(
                &handle,
                platform::UploadController,
                MapMetrics::default(),
            ));
            let frame = relocation_frame(&handle);
            // The retained rectangle starts inside the target. Egui transforms only the outer shape.
            let original = Rect::from_min_size(pos2(160.0, 40.0), egui::vec2(320.0, 160.0));
            for dpr in [1.0, 1.5, 2.0] {
                let screen = ScreenDescriptor {
                    size_in_pixels: [768, 256],
                    pixels_per_point: dpr,
                };
                for scale in [0.5, 1.0, 1.5] {
                    let transforms = [
                        egui::emath::TSTransform::IDENTITY,
                        egui::emath::TSTransform {
                            scaling: scale,
                            translation: egui::vec2(-220.0, -100.0),
                        },
                        egui::emath::TSTransform {
                            scaling: scale,
                            translation: egui::vec2(480.0, 120.0),
                        },
                    ];
                    let transformed = transforms.map(|transform| {
                        let mut shape =
                            paint_callback(original, Arc::clone(&frame), Arc::clone(&renderer));
                        shape.transform(transform);
                        shape
                    });
                    let actual = capture_shapes(&transformed, &screen, samples, &device, &queue);
                    // Independent surfaces avoid shared uniform writes in this reference submission.
                    let reference = transforms.map(|transform| {
                        let renderer = Arc::new(Renderer::new(
                            &handle,
                            platform::UploadController,
                            MapMetrics::default(),
                        ));
                        paint_callback(transform * original, Arc::clone(&frame), renderer)
                    });
                    let expected = capture_shapes(&reference, &screen, samples, &device, &queue);
                    assert!(expected.as_chunks::<4>().0.contains(&[0, 0, 255, 255]));
                    assert!(
                        expected
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|pixel| pixel[0] > 200 && pixel[2] < 20),
                        "route/highlight must be visible"
                    );
                    assert_eq!(
                        actual.iter().zip(&expected).position(|(a, b)| a != b),
                        None,
                        "first differing byte: DPR={dpr}, MSAA={samples}, scale={scale}"
                    );
                }
                // An originally offscreen callback must also become drawable after a late transform.
                let outside = original.translate(egui::vec2(1000.0, 1000.0));
                let mut returning =
                    paint_callback(outside, Arc::clone(&frame), Arc::clone(&renderer));
                returning.translate(egui::vec2(-1000.0, -1000.0));
                let actual = capture_shapes(&[returning], &screen, samples, &device, &queue);
                let expected = capture_shapes(
                    &[paint_callback(
                        original,
                        Arc::clone(&frame),
                        Arc::clone(&renderer),
                    )],
                    &screen,
                    samples,
                    &device,
                    &queue,
                );
                assert_eq!(
                    actual.iter().zip(&expected).position(|(a, b)| a != b),
                    None,
                    "returning callback differs: DPR={dpr}, MSAA={samples}"
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn relocation_frame(handle: &WgpuMapHandle) -> Arc<Frame> {
        use super::super::{CameraUniform, GpuTile, TileId, VisibleTile, render_tests};
        let tile = render_tests::solid_tile(egui::Color32::BLUE);
        let id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };
        tile.gpu.store(Some(Arc::new(GpuTile::new(
            &handle.context,
            id,
            Arc::clone(&tile.mesh),
        ))));
        let mut frame = render_tests::highlighted_route_frame(&handle.context);
        frame.camera = CameraUniform::new([0.5, 0.640_625], [320.0, 160.0], 512.0, 0);
        frame.visible = vec![VisibleTile {
            id,
            tile,
            instances: 0..1,
        }];
        Arc::new(frame)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn capture_shapes(
        shapes: &[Shape],
        screen: &ScreenDescriptor,
        samples: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<u8> {
        let jobs: Vec<_> = shapes
            .iter()
            .map(|shape| {
                let Shape::Callback(callback) = shape else {
                    panic!("expected map callback");
                };
                egui::epaint::ClippedPrimitive {
                    clip_rect: Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(768.0, 256.0) / screen.pixels_per_point,
                    ),
                    primitive: egui::epaint::Primitive::Callback(callback.clone()),
                }
            })
            .collect();
        let mut renderer = egui_wgpu::Renderer::new(
            device,
            wgpu::TextureFormat::Rgba8Unorm,
            egui_wgpu::RendererOptions {
                msaa_samples: samples,
                ..egui_wgpu::RendererOptions::PREDICTABLE
            },
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let commands = renderer.update_buffers(device, queue, &mut encoder, &jobs, screen);
        queue.submit(commands.into_iter().chain([encoder.finish()]));
        super::super::render_tests::draw_image(device, queue, samples, |pass| {
            renderer.render(pass, &jobs, screen);
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn target_edge_clipping_matches_translated_unclipped_pixels() {
        use super::super::{CameraUniform, GpuTile, TileId, VisibleTile, platform, render_tests};
        use crate::activity::map_runtime::MapMetrics;

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .unwrap();
        let (device, queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .unwrap();
        for samples in [1, 4] {
            let handle = WgpuMapHandle::new(&device, wgpu::TextureFormat::Rgba8Unorm, samples);
            let renderer = Arc::new(Renderer::new(
                &handle,
                platform::UploadController,
                MapMetrics::default(),
            ));
            let tile = super::super::prepare_local_browser_tile(walkers::Tile::Vector {
                shapes: vec![
                    Shape::rect_filled(
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0)),
                        0.0,
                        egui::Color32::GREEN,
                    ),
                    Shape::rect_filled(
                        Rect::from_min_max(pos2(220.0, 210.0), pos2(280.0, 260.0)),
                        0.0,
                        egui::Color32::BLUE,
                    ),
                ],
                texts: Vec::new(),
            })
            .unwrap()
            .into_prepared()
            .unwrap();
            let id = TileId {
                zoom: 0,
                x: 0,
                y: 0,
            };
            tile.gpu.store(Some(Arc::new(GpuTile::new(
                &handle.context,
                id,
                Arc::clone(&tile.mesh),
            ))));
            for dpr in [1.0, 1.5, 2.0] {
                let mut frame = render_tests::highlighted_route_frame(&handle.context);
                frame.camera = CameraUniform::new([0.5, 0.5], [320.0 / dpr, 160.0 / dpr], 512.0, 0);
                frame.visible = vec![VisibleTile {
                    id,
                    tile: Arc::clone(&tile),
                    instances: 0..1,
                }];
                let frame = Arc::new(frame);
                let render = |left: i32, top: i32| {
                    let rect = Rect::from_min_size(
                        pos2(left as f32 / dpr, top as f32 / dpr),
                        egui::vec2(320.0 / dpr, 160.0 / dpr),
                    );
                    let callback = Paint::new(rect, Arc::clone(&frame), Arc::clone(&renderer));
                    let screen = ScreenDescriptor {
                        size_in_pixels: [768, 256],
                        pixels_per_point: dpr,
                    };
                    capture_callback(&callback, &screen, samples, &device, &queue)
                };
                let reference = render(200, 48);
                assert!(reference.as_chunks::<4>().0.contains(&[0, 0, 255, 255]));
                for (left, top) in [
                    (-120, 48),
                    (620, 48),
                    (200, -64),
                    (200, 180),
                    (-120, -64),
                    (620, 180),
                    (800, 300),
                ] {
                    let actual = render(left, top);
                    assert_translated_pixels(&actual, &reference, [left, top], dpr, samples);
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn capture_callback(
        callback: &Paint,
        screen: &ScreenDescriptor,
        samples: u32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<u8> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut resources = CallbackResources::default();
        let commands = callback.prepare(device, queue, screen, &mut encoder, &mut resources);
        queue.submit(commands);
        super::super::render_tests::draw_image(device, queue, samples, |pass| {
            callback.paint(
                PaintCallbackInfo {
                    viewport: callback.rect,
                    clip_rect: callback.rect,
                    pixels_per_point: screen.pixels_per_point,
                    screen_size_px: screen.size_in_pixels,
                },
                pass,
                &resources,
            );
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_translated_pixels(
        actual: &[u8],
        reference: &[u8],
        [left, top]: [i32; 2],
        dpr: f32,
        samples: u32,
    ) {
        for y in 0..256_i32 {
            for x in 0..768_i32 {
                let local_x = x - left;
                let local_y = y - top;
                let expected = if (0..320).contains(&local_x) && (0..160).contains(&local_y) {
                    let index = usize::try_from((local_y + 48) * 768 + local_x + 200).unwrap() * 4;
                    &reference[index..index + 4]
                } else {
                    &[0, 0, 0, 255]
                };
                let index = usize::try_from(y * 768 + x).unwrap() * 4;
                assert!(
                    actual[index..index + 4]
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 2),
                    "pixel ({x}, {y}), origin=({left}, {top}), DPR={dpr}, MSAA={samples}: {:?} != {expected:?}",
                    &actual[index..index + 4]
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn callback_keeps_prepared_frame_when_a_new_scene_arrives() {
        use super::super::{GpuTile, TileId, assemble_tile_frame, platform, render_tests};
        use crate::activity::{map::camera::MapCamera, map_runtime::MapMetrics};

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .expect("map callback snapshot regression requires a WGPU adapter");
        let (device, queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .unwrap();
        let handle = WgpuMapHandle::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1);
        let renderer = Arc::new(Renderer::new(
            &handle,
            platform::UploadController,
            MapMetrics::default(),
        ));
        let tile = render_tests::solid_tile(egui::Color32::BLUE);
        let id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };
        tile.gpu.store(Some(Arc::new(GpuTile::new(
            &handle.context,
            id,
            Arc::clone(&tile.mesh),
        ))));
        let viewport = Rect::from_min_max(pos2(0.0, 0.0), pos2(768.0, 256.0));
        let mut camera = MapCamera::default();
        camera.set_zoom(0.0);
        camera.center_at(walkers::lon_lat(0.0, 0.0));
        let assembly = assemble_tile_frame([(&id, &tile)], &camera, viewport);
        let mut latest = Arc::new(Frame {
            camera: assembly.camera,
            visible: assembly.visible,
            ..Frame::default()
        });
        let callback = Paint::new(viewport, Arc::clone(&latest), Arc::clone(&renderer));
        let mut resources = CallbackResources::default();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let screen = ScreenDescriptor {
            size_in_pixels: [768, 256],
            pixels_per_point: 1.0,
        };
        let commands = callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        queue.submit(commands);
        // Publishing a replacement between prepare and paint must not change this callback's scene.
        Arc::make_mut(&mut latest).visible.clear();
        let pixels = render_tests::draw_row(&device, &queue, 1, |pass| {
            callback.paint(
                PaintCallbackInfo {
                    viewport,
                    clip_rect: viewport,
                    pixels_per_point: 1.0,
                    screen_size_px: [768, 256],
                },
                pass,
                &resources,
            );
        });
        assert_eq!(&pixels[384 * 4..384 * 4 + 4], &[0, 0, 255, 255]);
        let next = Paint::new(viewport, latest, renderer);
        let commands = next.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        queue.submit(commands);
        let pixels = render_tests::draw_row(&device, &queue, 1, |pass| {
            next.paint(
                PaintCallbackInfo {
                    viewport,
                    clip_rect: viewport,
                    pixels_per_point: 1.0,
                    screen_size_px: [768, 256],
                },
                pass,
                &resources,
            );
        });
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0, 0, 0, 255])
        );
    }

    #[test]
    fn fractional_dpr_keeps_projection_separate_from_target_bounded_clip() {
        let info = PaintCallbackInfo {
            viewport: Rect::from_min_max(pos2(10.0, 20.0), pos2(210.0, 120.0)),
            clip_rect: Rect::from_min_max(pos2(30.0, 0.0), pos2(190.0, 100.0)),
            pixels_per_point: 1.5,
            screen_size_px: [300, 160],
        };
        assert_eq!(
            draw_region(&info),
            Some(DrawRegion {
                projection: [15.0, 30.0, 300.0, 150.0],
                viewport: [15, 30, 285, 130],
                scissor: [45, 30, 240, 120],
            })
        );
    }

    #[test]
    fn disjoint_clip_does_not_expand_to_visible_pixels() {
        let info = PaintCallbackInfo {
            viewport: Rect::from_min_max(pos2(10.0, 20.0), pos2(50.0, 60.0)),
            clip_rect: Rect::from_min_max(pos2(80.0, 20.0), pos2(100.0, 60.0)),
            pixels_per_point: 2.0,
            screen_size_px: [200, 200],
        };
        let region = draw_region(&info).unwrap();
        assert_eq!(region.scissor[2], 0);
    }
}
