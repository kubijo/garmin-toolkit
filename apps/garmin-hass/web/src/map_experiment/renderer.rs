//! Real wgpu surface owned entirely by a dedicated worker, used by the composition proof.

use eframe::wgpu;
use wasm_bindgen::prelude::*;

#[path = "map.rs"]
mod map;

#[wasm_bindgen(module = "/worker-codec.js")]
extern "C" {
    #[wasm_bindgen(js_name = compositionWorkerFailure)]
    pub(crate) fn report_failure(reason: &str);
    #[wasm_bindgen(js_name = requestMapDraw)]
    fn request_draw(milliseconds: f64);
}

/// A worker-owned GPU surface. JS only sends bounded dimensions, never frame pixels.
#[wasm_bindgen]
pub struct CompositionRenderer {
    canvas: web_sys::OffscreenCanvas,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    configuration: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    backend: String,
    map: Option<map::WorkerMap>,
}

#[wasm_bindgen]
impl CompositionRenderer {
    /// Initialize the explicitly requested backend. No silent fallback.
    /// # Errors
    /// Returns the original adapter/device/surface initialization failure.
    pub async fn create(canvas: web_sys::OffscreenCanvas, mode: &str) -> Result<Self, JsValue> {
        // Worker instances do not run the window's startup/panic surface. Preserve the
        // actual Rust failure before WASM turns it into an uninformative `unreachable`.
        std::panic::set_hook(Box::new(|panic| report_failure(&panic.to_string())));
        let backends = match mode {
            "worker-gl" => wgpu::Backends::GL,
            "worker-webgpu" => wgpu::Backends::BROWSER_WEBGPU,
            _ => return Err(error("unsupported composition backend")),
        };
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        descriptor.backends = backends;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas.clone()))
            .map_err(error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(error)?;
        let backend = format!("{:?}", adapter.get_info().backend);
        // This proof uses only raster rendering. Default WebGPU limits request compute support,
        // which WebGL2 cannot provide, even when adapter/surface creation succeeds.
        let required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(error)?;
        device.set_device_lost_callback(|reason, message| {
            report_failure(&format!("worker device lost: {reason:?}: {message}"));
        });
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            report_failure(&format!("worker GPU error: {error}"));
        }));
        let configuration = surface
            .get_default_config(&adapter, 1, 1)
            .ok_or_else(|| error("worker surface has no supported configuration"))?;
        let pipeline =
            garmin_ui::activity::map_composition::pattern_pipeline(&device, configuration.format);
        Ok(Self {
            canvas,
            surface,
            device,
            queue,
            configuration,
            pipeline,
            backend,
            map: None,
        })
    }

    /// Effective backend for comparison validation.
    #[must_use]
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// Maximum accepted backing dimension.
    #[must_use]
    pub fn maximum_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Switch the proven surface to production map rendering, with its own preparation worker.
    /// # Errors
    /// Returns preparation-worker initialization failures without a local fallback.
    pub fn enable_map(
        &mut self,
        module_url: &str,
        wasm_url: &str,
        preparation_url: &str,
    ) -> Result<(), JsValue> {
        self.map = Some(map::WorkerMap::new(
            &self.device,
            self.configuration.format,
            module_url,
            wasm_url,
            preparation_url,
        )?);
        Ok(())
    }

    /// Apply one bounded view and optional immutable route replacement.
    /// # Errors
    /// Returns shared-codec validation errors.
    pub fn update_map(&mut self, view: &str, route: &[u8]) -> Result<(), JsValue> {
        self.map
            .as_mut()
            .ok_or_else(|| error("map is not initialized"))?
            .update(view, (!route.is_empty()).then_some(route))
            .map_err(error)
    }

    /// Submit one fixture frame; completion does not mean compositor presentation.
    /// # Errors
    /// Rejects invalid dimensions and reports surface acquisition failure.
    pub fn draw(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        if width == 0 || height == 0 || width > self.maximum_size() || height > self.maximum_size()
        {
            return Err(error("invalid worker surface dimensions"));
        }
        if self.canvas.width() != width
            || self.canvas.height() != height
            || self.configuration.width != width
            || self.configuration.height != height
        {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.configuration.width = width;
            self.configuration.height = height;
            self.surface.configure(&self.device, &self.configuration);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            status => {
                return Err(error(format!(
                    "worker surface acquisition failed: {status:?}"
                )));
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(map) = &mut self.map {
            map.draw(&self.device, &self.queue, &target, [width, height]);
            self.queue.present(frame);
            return Ok(());
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        Ok(())
    }
}

fn error(reason: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&reason.to_string()).into()
}
