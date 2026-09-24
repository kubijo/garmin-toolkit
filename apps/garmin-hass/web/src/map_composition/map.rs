//! Worker-local egui tessellation for labels/markers and production map GPU callbacks.
//! No application UI, input events, paint lists or images cross the thread boundary.

use eframe::{egui, egui_wgpu, wgpu};
use garmin_ui::activity::{self, map_remote::MapSurface, map_runtime};
use wasm_bindgen::JsValue;

pub(super) struct WorkerMap {
    context: egui::Context,
    surface: MapSurface,
    painter: egui_wgpu::Renderer,
}

impl WorkerMap {
    pub(super) fn readiness(&self) -> String {
        self.surface.readiness().into()
    }
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        module_url: &str,
        wasm_url: &str,
        preparation_url: &str,
    ) -> Result<Self, JsValue> {
        let backend = crate::map_worker::BrowserMapBackend::for_render_worker(
            module_url,
            wasm_url,
            preparation_url,
        )?;
        let handle = activity::WgpuMapHandle::for_target(device, format, 1);
        let runtime =
            map_runtime::MapRuntimeHandle::new(backend, map_runtime::Renderer::wgpu(handle));
        let context = egui::Context::default();
        garmin_ui::install(&context);
        context.set_request_repaint_callback(|info| {
            super::request_draw(info.delay.as_secs_f64() * 1000.0);
        });
        Ok(Self {
            context,
            surface: MapSurface::new(&runtime),
            painter: egui_wgpu::Renderer::new(
                device,
                format,
                egui_wgpu::RendererOptions::default(),
            ),
        })
    }

    pub(super) fn update(&mut self, view: &str, route: Option<&[u8]>) -> Result<(), String> {
        self.surface.update(view, route)
    }

    pub(super) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        dimensions: [u32; 2],
    ) {
        let Some((size, scale, dark)) = self.surface.viewport() else {
            return;
        };
        self.context.set_theme(if dark {
            egui::ThemePreference::Dark
        } else {
            egui::ThemePreference::Light
        });
        self.context.set_pixels_per_point(scale);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            time: Some(crate::browser_timing::now() / 1000.0),
            ..Default::default()
        };
        let mut output = self.context.run_ui(input, |ui| self.surface.paint(ui));
        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: dimensions,
            pixels_per_point: output.pixels_per_point,
        };
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.painter.update_texture(device, queue, id, &delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let commands =
            self.painter
                .update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(if dark {
                            wgpu::Color::BLACK
                        } else {
                            wgpu::Color::WHITE
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.painter
                .render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        queue.submit(commands.into_iter().chain([encoder.finish()]));
        for id in output.textures_delta.free.drain() {
            self.painter.free_texture(&id);
        }
    }
}
