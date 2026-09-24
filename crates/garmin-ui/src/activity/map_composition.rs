//! Browser worker-map composition. No map scheduling or browser handles live here.

use std::sync::Arc;

use egui::{Rect, Shape, Ui, epaint::PaintCallbackInfo};
use egui_wgpu::{Callback, CallbackResources, CallbackTrait};

use super::map::gpu_map::egui_adapter::draw_region;

/// Final painted placement, in application-canvas physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// Unclipped map projection: left, top, width, height.
    pub projection: [f32; 4],
    /// Clipped visible rectangle: left, top, width, height.
    pub clip: [u32; 4],
    /// Application canvas backing dimensions, not window dimensions.
    pub screen: [u32; 2],
    /// Egui scale, including UI zoom.
    pub pixels_per_point: f32,
}

/// Browser integration callbacks. Implementations must not hold browser handles in this trait.
/// A browser host can call module-level JS functions from these main-thread callbacks instead.
pub trait Host: Send + Sync + 'static {
    /// Begin a UI pass. The host flushes after painting; discarded passes must not publish surfaces.
    fn begin_pass(&self);
    /// Begin actual painting, which may occur in a later event than UI logic.
    fn begin_paint(&self);
    /// Publish placement from the actual paint callback, after egui transforms and clipping.
    fn painted(&self, placement: Placement);
}

/// Coordinates the composition host without owning the egui context or any browser objects.
pub struct CompositionPlugin {
    pipeline: Arc<wgpu::RenderPipeline>,
    host: Arc<dyn Host>,
}

impl CompositionPlugin {
    /// Install using the UI's device/format/sample count. No changes to the default renderer.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        host: impl Host,
    ) -> Self {
        Self {
            pipeline: Arc::new(pipeline(device, format, samples, "transparent")),
            host: Arc::new(host),
        }
    }

    /// Erase only the map's clipped pixels; later egui shapes still paint above the opening.
    pub fn opening(&self, rect: Rect) -> Shape {
        Shape::Callback(Callback::new_paint_callback(
            rect,
            Opening {
                pipeline: Arc::clone(&self.pipeline),
                host: Arc::clone(&self.host),
            },
        ))
    }
}

impl egui::plugin::Plugin for CompositionPlugin {
    fn debug_name(&self) -> &'static str {
        "map composition"
    }
    fn on_begin_pass(&mut self, ui: &mut Ui) {
        self.host.begin_pass();
        // A full-root sentinel executes even when this frame contains no map opening.
        // Logic-only input events must never hide the previously presented underlay.
        ui.painter()
            .add(Shape::Callback(Callback::new_paint_callback(
                ui.clip_rect(),
                BeginPaint(Arc::clone(&self.host)),
            )));
    }
}

struct BeginPaint(Arc<dyn Host>);

impl CallbackTrait for BeginPaint {
    fn paint(
        &self,
        _info: PaintCallbackInfo,
        _pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        self.0.begin_paint();
    }
}

struct Opening {
    pipeline: Arc<wgpu::RenderPipeline>,
    host: Arc<dyn Host>,
}

impl CallbackTrait for Opening {
    fn paint(
        &self,
        info: PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        let Some(region) = draw_region(&info) else {
            return;
        };
        let [x, y, width, height] = region.scissor;
        if width == 0 || height == 0 {
            return;
        }
        pass.set_scissor_rect(x, y, width, height);
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..3, 0..1);
        self.host.painted(Placement {
            projection: region.projection,
            clip: region.scissor,
            screen: info.screen_size_px,
            pixels_per_point: info.pixels_per_point,
        });
    }
}

/// Deterministic worker pattern for the composition gate, not a map-rendering fallback.
#[must_use]
pub fn pattern_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    pipeline(device, format, 1, "pattern")
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    samples: u32,
    fragment: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("map composition"),
        source: wgpu::ShaderSource::Wgsl(include_str!("map_composition.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("map composition"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn composition_shader_validates() {
        let shader = naga::front::wgsl::parse_str(include_str!("map_composition.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&shader)
        .unwrap();
    }
}
