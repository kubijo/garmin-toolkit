//! Pixel regressions for the production map draw path.

use super::*;
use crate::activity::map::camera::MapCamera;

const WIDTH: u32 = 768;
const HEIGHT: u32 = 256;

#[test]
fn standalone_renderer_preserves_route_highlight_clipping_and_msaa() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = futures_lite::future::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
    )
    .expect("standalone map renderer regression requires a WGPU adapter");
    let (device, queue) =
        futures_lite::future::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .unwrap();
    for sample_count in [1, 4] {
        let handle = WgpuMapHandle::new(&device, wgpu::TextureFormat::Rgba8Unorm, sample_count);
        let renderer = renderer::Renderer::new(
            &handle,
            platform::UploadController,
            crate::activity::map_runtime::MapMetrics::default(),
        );
        let mut frame = highlighted_route_frame(&handle.context);
        // Nonzero placement and partial clipping must not re-center or stretch the route.
        let region = renderer::DrawRegion {
            projection: [64.0, 0.0, 640.0, HEIGHT as f32],
            viewport: [64, 0, 640, HEIGHT],
            scissor: [200, 0, 368, HEIGHT],
        };
        let pixels = render_frame_row(&handle, &queue, &renderer, &frame, region, sample_count);
        for (x, expected) in [
            (180, [0, 0, 0, 255]),
            (250, [255, 0, 0, 255]),
            (384, [255, 255, 0, 255]),
            (520, [255, 0, 0, 255]),
            (590, [0, 0, 0, 255]),
        ] {
            assert_eq!(
                &pixels[x * 4..x * 4 + 4],
                &expected,
                "route pixel x={x}, samples={sample_count}"
            );
        }
        // A new frame updates uniforms and removes the old highlight without replacing pipelines.
        frame.route.as_mut().unwrap().highlight = None;
        frame.route.as_mut().unwrap().color.fallback = color(Color32::GREEN);
        let pixels = render_frame_row(&handle, &queue, &renderer, &frame, region, sample_count);
        assert_eq!(&pixels[384 * 4..384 * 4 + 4], &[0, 255, 0, 255]);
        let empty = renderer::DrawRegion {
            scissor: [200, 0, 0, HEIGHT],
            ..region
        };
        let pixels = render_frame_row(&handle, &queue, &renderer, &frame, empty, sample_count);
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0, 0, 0, 255])
        );
    }
}

#[test]
fn world_copies_render_across_the_viewport_and_honor_nonzero_first_instances() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = futures_lite::future::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
    ) else {
        eprintln!("skipping map pixel regression because no WGPU adapter is available");
        return;
    };
    let (device, queue) =
        futures_lite::future::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .unwrap();
    let handle = WgpuMapHandle::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1);
    let mut camera = MapCamera::default();
    camera.set_zoom(0.0);
    for longitude in [-179.9, -0.1, 0.0, 0.1, 179.9] {
        camera.center_at(walkers::lon_lat(longitude, 0.0));
        let pixels = render_row(
            &handle,
            &queue,
            &camera,
            TileId {
                zoom: 0,
                x: 0,
                y: 0,
            },
        );
        for x in (32..WIDTH as usize).step_by(64) {
            assert_eq!(
                &pixels[x * 4..x * 4 + 4],
                &[0, 0, 255, 255],
                "uncovered world copy at x={x}, longitude={longitude}"
            );
        }
    }

    camera.set_zoom(2.0);
    camera.center_at(walkers::lon_lat(176.4, 0.0));
    let pixels = render_row(
        &handle,
        &queue,
        &camera,
        TileId {
            zoom: 2,
            x: 0,
            y: 2,
        },
    );
    assert_eq!(&pixels[128 * 4..128 * 4 + 4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[512 * 4..512 * 4 + 4], &[0, 0, 255, 255]);
}

pub(super) fn solid_tile(fill: Color32) -> Arc<PreparedGpuTile> {
    prepare_local_browser_tile(Tile::Vector {
        shapes: vec![Shape::rect_filled(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0)),
            0.0,
            fill,
        )],
        texts: Vec::new(),
    })
    .unwrap()
    .into_prepared()
    .unwrap()
}

fn render_row(
    handle: &WgpuMapHandle,
    queue: &wgpu::Queue,
    camera: &MapCamera,
    id: TileId,
) -> Vec<u8> {
    let context = &handle.context;
    let tile = solid_tile(Color32::BLUE);
    let capture = upload_trace::tests::Capture::default();
    let trace = Arc::new(upload_trace::UploadTrace::new(
        id,
        crate::activity::map_runtime::MapMetrics::new(capture.clone()),
    ));
    trace.begin_work();
    trace.published();
    let mut gpu = GpuTile::new(context, id, Arc::clone(&tile.mesh));
    gpu.first_draw = Some(std::sync::Mutex::new(Some(trace)));
    tile.gpu.store(Some(Arc::new(gpu)));
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(WIDTH as f32, HEIGHT as f32));
    let assembly = assemble_tile_frame([(&id, &tile)], camera, viewport);
    let frame = Frame {
        camera: assembly.camera,
        visible: assembly.visible,
        ..Frame::default()
    };
    let renderer = renderer::Renderer::new(
        handle,
        platform::UploadController,
        crate::activity::map_runtime::MapMetrics::default(),
    );
    let pixels = render_frame_row(
        handle,
        queue,
        &renderer,
        &frame,
        renderer::DrawRegion {
            projection: [0.0, 0.0, WIDTH as f32, HEIGHT as f32],
            viewport: [0, 0, WIDTH, HEIGHT],
            scissor: [0, 0, WIDTH, HEIGHT],
        },
        1,
    );
    assert_eq!(
        capture
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.event == crate::activity::map_runtime::MapUploadPhase::FirstDraw)
            .count(),
        1,
        "wrapped instances and repeated draws must report first submission only once"
    );
    pixels
}

pub(super) fn highlighted_route_frame(context: &UploadContext) -> Frame {
    let route = Arc::new(GpuRoute::new(
        context,
        [0.5, 0.5],
        Arc::new(CpuRoute {
            segments: vec![RouteSegment {
                start: [-0.5, 0.140_625],
                end: [0.5, 0.140_625],
                speed: [-1.0, -1.0],
                sample_indices: [0.0, 100.0],
                start_join: [0.0, 1.0],
                end_join: [0.0, 1.0],
                caps: [1.0, 1.0],
            }],
        }),
    ));
    Frame {
        camera: CameraUniform::new([0.5, 0.5], [640.0, 256.0], 512.0, 0),
        route: Some(VisibleRoute {
            resource: route,
            outline: RouteStyleUniform::new(8.0, 1.0, 0.0, color(Color32::WHITE), [0.0; 4]),
            color: RouteStyleUniform::new(4.0, 1.0, 1.0, color(Color32::RED), [0.0; 4]),
            highlight: Some(HighlightStyles {
                outline: RouteStyleUniform::new(
                    10.0,
                    1.0,
                    2.0,
                    color(Color32::WHITE),
                    [40.0, 60.0, 0.0, 0.0],
                ),
                color: RouteStyleUniform::new(
                    6.0,
                    1.0,
                    3.0,
                    color(Color32::YELLOW),
                    [40.0, 60.0, 0.0, 0.0],
                ),
            }),
        }),
        ..Frame::default()
    }
}

fn render_frame_row(
    handle: &WgpuMapHandle,
    queue: &wgpu::Queue,
    renderer: &renderer::Renderer,
    frame: &Frame,
    region: renderer::DrawRegion,
    sample_count: u32,
) -> Vec<u8> {
    let device = &handle.context.device;
    renderer.prepare(frame, region, queue);
    draw_row(device, queue, sample_count, |pass| {
        renderer.draw(frame, region, pass);
        renderer.draw(frame, region, pass);
    })
}

pub(super) fn draw_row(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sample_count: u32,
    paint: impl FnOnce(&mut wgpu::RenderPass<'static>),
) -> Vec<u8> {
    let pixels = draw_image(device, queue, sample_count, paint);
    pixels[200 * WIDTH as usize * 4..201 * WIDTH as usize * 4].to_vec()
}

pub(super) fn draw_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sample_count: u32,
    paint: impl FnOnce(&mut wgpu::RenderPass<'static>),
) -> Vec<u8> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("map world-copy pixel regression"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let msaa = (sample_count > 1).then(|| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("map multisample regression"),
                sample_count,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                size: texture.size(),
                mip_level_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: msaa.as_ref().unwrap_or(&view),
                depth_slice: None,
                resolve_target: msaa.as_ref().map(|_| &view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        paint(&mut pass.forget_lifetime());
    }
    read_image(device, queue, encoder, &texture)
}

fn read_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("map pixel readback"),
        size: u64::from(WIDTH * HEIGHT * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let pixels = output.slice(..).get_mapped_range().unwrap().to_vec();
    output.unmap();
    pixels
}
