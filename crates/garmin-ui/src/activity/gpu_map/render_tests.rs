//! Pixel regressions for the production map draw path.

use super::*;
use crate::activity::map::camera::MapCamera;

const WIDTH: u32 = 768;
const HEIGHT: u32 = 256;

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
    let context = UploadContext::new(&device);
    let resources = Resources::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
        &context.camera_layout,
        &context.tile_layout,
        &context.route_source_layout,
        &context.route_style_layout,
    );
    let mut camera = MapCamera::default();
    camera.set_zoom(0.0);
    for longitude in [-179.9, -0.1, 0.0, 0.1, 179.9] {
        camera.center_at(walkers::lon_lat(longitude, 0.0));
        let pixels = render_row(
            &context,
            &queue,
            &resources,
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
        &context,
        &queue,
        &resources,
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

fn render_row(
    context: &UploadContext,
    queue: &wgpu::Queue,
    resources: &Resources,
    camera: &MapCamera,
    id: TileId,
) -> Vec<u8> {
    let device = &context.device;
    let tile = prepare_local_browser_tile(Tile::Vector {
        shapes: vec![Shape::rect_filled(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0)),
            0.0,
            Color32::BLUE,
        )],
        texts: Vec::new(),
    })
    .unwrap()
    .into_prepared()
    .unwrap();
    tile.gpu.store(Some(Arc::new(GpuTile::new(
        context,
        id,
        Arc::clone(&tile.mesh),
    ))));
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(WIDTH as f32, HEIGHT as f32));
    let assembly = assemble_tile_frame([(&id, &tile)], camera, viewport);
    let frame = Frame {
        camera: assembly.camera,
        visible: assembly.visible,
        ..Frame::default()
    };
    let surface = SurfaceGpu::new(context);
    queue.write_buffer(&surface.camera, 0, bytemuck::bytes_of(&frame.camera));
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
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        draw_tiles(&frame, &surface, resources, &mut pass);
    }
    read_row(device, queue, encoder, &texture)
}

fn read_row(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("map pixel readback"),
        size: u64::from(WIDTH * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 200, z: 0 },
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
            height: 1,
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
