//! Production egui and map pipelines targeting an offscreen texture, with optional readback.

use std::{io, sync::Arc, time::Duration};

use eframe::{egui, egui_wgpu, wgpu};
use futures_lite::future::block_on;

use super::Result;

pub(super) struct Renderer {
    pub state: egui_wgpu::RenderState,
    targets: Option<Targets>,
}

struct Targets {
    color: wgpu::Texture,
    multisample: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_row: u32,
}

impl Renderer {
    pub fn new(name: Option<&str>) -> Result<Self> {
        let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
        setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
        let name = name.map(str::to_lowercase);
        setup.native_adapter_selector = Some(Arc::new(move |adapters, _| {
            let candidates: Vec<_> = adapters
                .iter()
                .filter(|adapter| {
                    let info = adapter.get_info();
                    hardware_matches(&info, name.as_deref())
                })
                .collect();
            match candidates.as_slice() {
                [adapter] => Ok((*adapter).clone()),
                _ => Err(format!(
                    "expected exactly one matching hardware adapter, found {}; available: {}",
                    candidates.len(),
                    adapters
                        .iter()
                        .map(|adapter| {
                            let info = adapter.get_info();
                            format!(
                                "{} ({:?}, vendor {:#06x})",
                                info.name, info.device_type, info.vendor
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }));
        let setup = egui_wgpu::WgpuSetup::CreateNew(setup);
        let instance = block_on(setup.new_instance());
        let state = block_on(egui_wgpu::RenderState::create(
            &egui_wgpu::WgpuConfiguration {
                wgpu_setup: setup,
                ..Default::default()
            },
            &instance,
            None,
            egui_wgpu::RendererOptions {
                msaa_samples: u32::from(crate::MULTISAMPLING),
                ..Default::default()
            },
        ))?;
        if state.target_format != wgpu::TextureFormat::Rgba8Unorm {
            return Err(io::Error::other("probe readback requires RGBA8 output").into());
        }
        Ok(Self {
            state,
            targets: None,
        })
    }

    pub fn draw(
        &mut self,
        context: &egui::Context,
        output: &mut egui::FullOutput,
        capture: bool,
        clear_color: [f32; 4],
    ) -> Result<Option<egui::ColorImage>> {
        let size = context.content_rect().size() * context.pixels_per_point();
        if !size.is_finite() || size.min_elem() < 1.0 || size.max_elem() > 2048.0 {
            return Err(
                io::Error::other("probe render target exceeds 2048 pixels per side").into(),
            );
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite positive dimensions bounded above before conversion"
        )]
        let dimensions = [size.x.round() as u32, size.y.round() as u32];
        let device = &self.state.device;
        let queue = &self.state.queue;
        if self
            .targets
            .as_ref()
            .is_none_or(|targets| [targets.color.width(), targets.color.height()] != dimensions)
        {
            self.targets = Some(Targets::new(device, dimensions));
        }
        let targets = self.targets.as_ref().expect("targets initialized above");
        let mut renderer = self.state.renderer.write();
        for (id, changes) in output.textures_delta.set.drain() {
            for change in changes {
                renderer.update_texture(device, queue, id, &change);
            }
        }
        let primitives = context.tessellate(output.shapes.clone(), context.pixels_per_point());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: dimensions,
            pixels_per_point: context.pixels_per_point(),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let callbacks = renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        let view = targets
            .color
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("headless desktop, production MSAA"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets.multisample,
                        depth_slice: None,
                        resolve_target: Some(&view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: f64::from(clear_color[0]),
                                g: f64::from(clear_color[1]),
                                b: f64::from(clear_color[2]),
                                a: f64::from(clear_color[3]),
                            }),
                            store: wgpu::StoreOp::Discard,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        if capture {
            encoder.copy_texture_to_buffer(
                targets.color.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &targets.readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(targets.padded_row),
                        rows_per_image: None,
                    },
                },
                targets.color.size(),
            );
        }
        queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        // Bound work in flight without reading pixels on ordinary frames.
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })?;
        for id in output.textures_delta.free.drain() {
            renderer.free_texture(&id);
        }
        drop(renderer);
        capture.then(|| targets.read(device)).transpose()
    }
}

fn hardware_matches(info: &wgpu::AdapterInfo, name: Option<&str>) -> bool {
    matches!(
        info.device_type,
        wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu
    ) && name.map_or(info.vendor == 0x1002, |name| {
        info.name.to_lowercase().contains(name)
    })
}

impl Targets {
    fn new(device: &wgpu::Device, [width, height]: [u32; 2]) -> Self {
        let descriptor = wgpu::TextureDescriptor {
            label: Some("headless desktop target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        let color = device.create_texture(&descriptor);
        let multisample = device
            .create_texture(&wgpu::TextureDescriptor {
                sample_count: u32::from(crate::MULTISAMPLING),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                ..descriptor
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe capture staging"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            color,
            multisample,
            readback,
            padded_row,
        }
    }

    fn read(&self, device: &wgpu::Device) -> Result<egui::ColorImage> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })?;
        receiver.recv_timeout(Duration::from_secs(5))??;
        let size = [self.color.width() as usize, self.color.height() as usize];
        let mapped = self.readback.slice(..).get_mapped_range()?;
        let image = unpack(&mapped, size, self.padded_row as usize);
        drop(mapped);
        self.readback.unmap();
        Ok(image)
    }
}

fn unpack(mapped: &[u8], size: [usize; 2], padded_row: usize) -> egui::ColorImage {
    let pixels = mapped
        .chunks_exact(padded_row)
        .flat_map(|row| {
            row[..size[0] * 4].as_chunks::<4>().0.iter().map(|pixel| {
                egui::Color32::from_rgba_premultiplied(pixel[0], pixel[1], pixel[2], pixel[3])
            })
        })
        .collect();
    egui::ColorImage::new(size, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_rows_keep_channel_order_and_skip_alignment_bytes() {
        let mut mapped = vec![99_u8; 512];
        mapped[..8].copy_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]);
        mapped[256..264].copy_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]);
        let image = unpack(&mapped, [2, 2], 256);
        assert_eq!(
            image.pixels,
            [
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
                egui::Color32::TRANSPARENT
            ]
        );
    }

    #[test]
    fn a_matching_name_cannot_admit_a_software_adapter() {
        let mut info = wgpu::AdapterInfo {
            name: "AMD test adapter".into(),
            vendor: 0x1002,
            device: 0,
            device_type: wgpu::DeviceType::Cpu,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 32,
            subgroup_max_size: 64,
            transient_saves_memory: None,
            limit_bucket: None,
        };
        assert!(!hardware_matches(&info, None));
        assert!(!hardware_matches(&info, Some("amd")));
        info.device_type = wgpu::DeviceType::IntegratedGpu;
        assert!(hardware_matches(&info, None));
        assert!(!hardware_matches(&info, Some("nvidia")));
        info.vendor = 0x10de;
        assert!(!hardware_matches(&info, None));
    }
}
