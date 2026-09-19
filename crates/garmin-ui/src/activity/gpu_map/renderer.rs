//! Map GPU preparation and submission, independent of egui's callback lifecycle.
//!
//! The host owns the render target/pass and supplies physical-pixel placement. Frame assembly,
//! route/label preparation and platform execution remain outside this boundary for now.

use std::sync::Arc;

use web_time::Instant;
use wgpu::util::DeviceExt as _;

use super::{
    CameraUniform, Frame, Resources, SurfaceGpu, UploadContext, WgpuMapHandle, platform,
    single_uniform_bind_group,
};
use crate::activity::map_runtime::{MapMetrics, MapRenderPerformanceSample, MapRenderPhase};

impl WgpuMapHandle {
    pub(super) fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        assert!(
            matches!(sample_count, 1 | 4),
            "WGPU sample count must be either 1 or 4"
        );
        let context = Arc::new(UploadContext::new(device));
        let resources = Arc::new(Resources::new(
            device,
            target_format,
            sample_count,
            &context.camera_layout,
            &context.tile_layout,
            &context.route_source_layout,
            &context.route_style_layout,
        ));
        Self { context, resources }
    }
}

/// Full projection and target-bounded viewport/scissor in physical pixels.
/// Each rectangle is [left, top, width, height]. Clipping must not change the map projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DrawRegion {
    /// Full physical-pixel projection [left, top, width, height], possibly outside the target.
    pub projection: [f32; 4],
    pub viewport: [u32; 4],
    pub scissor: [u32; 4],
}

/// Device pipelines are shared; uniforms and upload demand belong to this map surface.
pub(super) struct Renderer {
    context: Arc<UploadContext>,
    resources: Arc<Resources>,
    surface: SurfaceGpu,
    uploads: platform::UploadController,
    metrics: MapMetrics,
}

impl Renderer {
    pub(super) fn new(
        handle: &WgpuMapHandle,
        uploads: platform::UploadController,
        metrics: MapMetrics,
    ) -> Self {
        Self {
            context: Arc::clone(&handle.context),
            resources: Arc::clone(&handle.resources),
            surface: SurfaceGpu::new(&handle.context),
            uploads,
            metrics,
        }
    }

    /// Prepare uniforms and advance the existing platform upload budget.
    /// Regular drawing uses this same frame, projection and viewport; late placement changes use
    /// `draw_relocated`. Submit before preparing another frame on this surface: queue writes to a
    /// uniform buffer are not per-command snapshots.
    pub(super) fn prepare(&self, frame: &Frame, region: DrawRegion, queue: &wgpu::Queue) {
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_render_prepare").entered();
        platform::prepare_uploads(&self.uploads, queue, &self.metrics);
        let surface = &self.surface;
        let camera = camera_for_region(frame.camera, region);
        queue.write_buffer(&surface.camera, 0, bytemuck::bytes_of(&camera));
        if let Some(route) = &frame.route {
            queue.write_buffer(
                &surface.outline_style,
                0,
                bytemuck::bytes_of(&route.outline),
            );
            queue.write_buffer(&surface.color_style, 0, bytemuck::bytes_of(&route.color));
            if let Some(highlight) = &route.highlight {
                queue.write_buffer(
                    &surface.highlight_outline_style,
                    0,
                    bytemuck::bytes_of(&highlight.outline),
                );
                queue.write_buffer(
                    &surface.highlight_color_style,
                    0,
                    bytemuck::bytes_of(&highlight.color),
                );
            }
        }
        self.metrics.record_render(MapRenderPerformanceSample {
            phase: MapRenderPhase::Prepare,
            milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
        });
    }

    pub(super) fn draw(
        &self,
        frame: &Frame,
        region: DrawRegion,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) {
        self.draw_inner(frame, region, false, render_pass);
    }

    /// A late host transform needs an immutable camera binding: writing the prepared buffer here
    /// would also change any earlier draw using it in this submission.
    pub(super) fn draw_relocated(
        &self,
        frame: &Frame,
        region: DrawRegion,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) {
        self.draw_inner(frame, region, true, render_pass);
    }

    fn draw_inner(
        &self,
        frame: &Frame,
        region: DrawRegion,
        relocated: bool,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) {
        let [left, top, width, height] = region.viewport;
        let [clip_left, clip_top, clip_width, clip_height] = region.scissor;
        if width == 0 || height == 0 || clip_width == 0 || clip_height == 0 {
            return;
        }
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_draw_submission").entered();
        let surface = &self.surface;
        let resources = &self.resources;
        let relocated_camera = relocated.then(|| {
            let camera = camera_for_region(frame.camera, region);
            let buffer =
                self.context
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("activity map relocated camera"),
                        contents: bytemuck::bytes_of(&camera),
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
            single_uniform_bind_group(
                &self.context.device,
                &self.context.camera_layout,
                &buffer,
                "activity map relocated camera binding",
            )
        });
        let camera = relocated_camera
            .as_ref()
            .unwrap_or(&surface.camera_bind_group);
        render_pass.set_viewport(
            left as f32,
            top as f32,
            width as f32,
            height as f32,
            0.0,
            1.0,
        );
        render_pass.set_scissor_rect(clip_left, clip_top, clip_width, clip_height);
        draw_tiles(frame, camera, resources, render_pass);
        if let Some(route) = &frame.route {
            let gpu = &route.resource;
            render_pass.set_pipeline(&resources.route_pipeline);
            render_pass.set_bind_group(0, camera, &[]);
            render_pass.set_bind_group(1, &gpu.origin_bind_group, &[]);
            render_pass.set_vertex_buffer(0, gpu.segments.slice(..));
            render_pass.set_bind_group(2, &surface.outline_bind_group, &[]);
            render_pass.draw(0..6, 0..gpu.segment_count);
            render_pass.set_bind_group(2, &surface.color_bind_group, &[]);
            render_pass.draw(0..6, 0..gpu.segment_count);
            if route.highlight.is_some() {
                render_pass.set_bind_group(2, &surface.highlight_outline_bind_group, &[]);
                render_pass.draw(0..6, 0..gpu.segment_count);
                render_pass.set_bind_group(2, &surface.highlight_color_bind_group, &[]);
                render_pass.draw(0..6, 0..gpu.segment_count);
            }
        }
        self.metrics.record_render(MapRenderPerformanceSample {
            phase: MapRenderPhase::Draw,
            milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
        });
    }
}

fn camera_for_region(mut camera: CameraUniform, region: DrawRegion) -> CameraUniform {
    let [x, y, width, height] = region.viewport.map(|value| value as f32);
    if width > 0.0 && height > 0.0 {
        let [full_x, full_y, full_width, full_height] = region.projection;
        camera.projection_scale_offset = [
            full_width / width,
            full_height / height,
            (2.0 * (full_x - x) + full_width - width) / width,
            -(2.0 * (full_y - y) + full_height - height) / height,
        ];
    }
    camera
}

fn draw_tiles(
    frame: &Frame,
    camera: &wgpu::BindGroup,
    resources: &Resources,
    render_pass: &mut wgpu::RenderPass<'_>,
) {
    render_pass.set_pipeline(&resources.pipeline);
    render_pass.set_bind_group(0, camera, &[]);
    for tile in &frame.visible {
        let Some(gpu) = tile.tile.gpu.load_full() else {
            continue;
        };
        render_pass.set_bind_group(1, &gpu.bind_group, &[]);
        render_pass.set_vertex_buffer(0, gpu.vertices.slice(..));
        render_pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..gpu.index_count, 0, tile.instances.clone());
        #[cfg(any(target_arch = "wasm32", test))]
        if let Some(first_draw) = &gpu.first_draw
            && let Some(trace) = first_draw
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        {
            trace.drawn();
        }
    }
}
