//! Bounded, correlated root-viewport readback shared by native and browser control.
use egui::{ColorImage, Context, Event, RawInput, ViewportCommand, ViewportId};
use garmin_service_api::control::{Capture, CaptureInfo, MAX_CAPTURE_BYTES, MAX_CAPTURE_PIXELS};
use image::ImageEncoder as _;
use std::{io, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity(u64);

struct Pending {
    id: Identity,
    size: [usize; 2],
    scale: f32,
    rect: egui::Rect,
    frame: u64,
    result: Option<Result<Pixels, String>>,
}

#[derive(Default)]
pub struct CapturePlugin {
    next: u64,
    pending: Option<Pending>,
}

pub struct Pixels {
    pub info: CaptureInfo,
    pub image: Arc<ColorImage>,
}

impl Pixels {
    #[must_use]
    pub fn rgba(&self) -> Vec<u8> {
        self.image
            .pixels
            .iter()
            .flat_map(egui::Color32::to_srgba_unmultiplied)
            .collect()
    }

    /// Encode with a bounded output buffer.
    /// # Errors
    /// Returns an error if PNG encoding fails or exceeds the byte limit.
    pub fn png(self) -> Result<Capture, String> {
        let mut output = LimitedPng(Vec::new());
        image::codecs::png::PngEncoder::new(&mut output)
            .write_image(
                &self.rgba(),
                self.info.width,
                self.info.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| error.to_string())?;
        Ok(Capture {
            info: self.info,
            png: output.0,
        })
    }
}

struct LimitedPng(Vec<u8>);

impl io::Write for LimitedPng {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_CAPTURE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("screenshot exceeds PNG byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Dropping the ticket releases admission and ignores any late screenshot event.
pub struct Ticket {
    plugin: egui::plugin::TypedPluginHandle<CapturePlugin>,
    id: Identity,
}

impl Ticket {
    #[must_use]
    pub fn take(&self) -> Option<Result<Pixels, String>> {
        let mut plugin = self.plugin.lock();
        let pending = plugin.pending.as_mut()?;
        if pending.id != self.id {
            return Some(Err("capture was replaced".into()));
        }
        pending.result.take()
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut plugin = self.plugin.lock();
        if plugin
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == self.id)
        {
            plugin.pending = None;
        }
    }
}

/// Request the next rendered root frame. Only one capture can be pending.
/// # Errors
/// Rejects overlapping captures and invalid or oversized viewports.
pub fn request(context: &Context) -> Result<Ticket, String> {
    let plugin = context.plugin::<CapturePlugin>();
    let mut plugin = plugin.lock();
    if plugin.pending.is_some() {
        return Err("a screenshot is already pending".into());
    }
    let scale = context.pixels_per_point();
    let rect = context.input_for(ViewportId::ROOT, egui::InputState::viewport_rect);
    let size = rect.size();
    let width = (size.x * scale).round();
    let height = (size.y * scale).round();
    if !scale.is_finite()
        || scale <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || width < 1.0
        || height < 1.0
        || f64::from(width) * f64::from(height) > f64::from(MAX_CAPTURE_PIXELS)
    {
        return Err("screenshot exceeds pixel limit or has invalid dimensions".into());
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Validated positive bounded pixel dimensions"
    )]
    let size = [width as usize, height as usize];
    plugin.next = plugin
        .next
        .checked_add(1)
        .ok_or("capture identity exhausted")?;
    let id = Identity(plugin.next);
    plugin.pending = Some(Pending {
        id,
        size,
        scale,
        rect,
        frame: context.cumulative_frame_nr_for(ViewportId::ROOT),
        result: None,
    });
    context.send_viewport_cmd_to(
        ViewportId::ROOT,
        ViewportCommand::Screenshot(egui::UserData::new(id)),
    );
    context.request_repaint_of(ViewportId::ROOT);
    drop(plugin);
    Ok(Ticket {
        plugin: context.plugin::<CapturePlugin>(),
        id,
    })
}

impl egui::plugin::Plugin for CapturePlugin {
    fn debug_name(&self) -> &'static str {
        "root screenshot capture"
    }

    fn input_hook(&mut self, context: &Context, input: &mut RawInput) {
        if input.viewport_id != ViewportId::ROOT {
            return;
        }
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.result.is_some() {
            return;
        }
        let scale = input
            .viewports
            .get(&ViewportId::ROOT)
            .and_then(|viewport| viewport.native_pixels_per_point)
            .map_or_else(
                || context.pixels_per_point(),
                |native| native * context.zoom_factor(),
            );
        if input.screen_rect.is_some_and(|rect| rect != pending.rect)
            || scale.to_bits() != pending.scale.to_bits()
        {
            pending.result = Some(Err("viewport changed during screenshot capture".into()));
            return;
        }
        for event in &input.events {
            let Event::Screenshot {
                viewport_id,
                user_data,
                image,
            } = event
            else {
                continue;
            };
            if *viewport_id != ViewportId::ROOT
                || user_data
                    .data
                    .as_ref()
                    .and_then(|data| data.downcast_ref::<Identity>())
                    != Some(&pending.id)
            {
                continue;
            }
            let result = if image.size == pending.size {
                Ok(Pixels {
                    info: CaptureInfo {
                        width: u32::try_from(image.size[0]).unwrap_or_default(),
                        height: u32::try_from(image.size[1]).unwrap_or_default(),
                        pixels_per_point: pending.scale,
                        requested_frame: pending.frame,
                        received_frame: context.cumulative_frame_nr_for(ViewportId::ROOT),
                    },
                    image: Arc::clone(image),
                })
            } else {
                Err("viewport changed during screenshot capture".into())
            };
            pending.result = Some(result);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::plugin::Plugin as _;

    fn context() -> Context {
        let context = Context::default();
        context.add_plugin(CapturePlugin::default());
        let mut output = context.run_ui(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(4.0, 3.0),
                )),
                ..RawInput::default()
            },
            |_| {},
        );
        output.textures_delta.clear();
        context
    }

    fn event(id: Identity) -> Event {
        Event::Screenshot {
            viewport_id: ViewportId::ROOT,
            user_data: egui::UserData::new(id),
            image: Arc::new(ColorImage::filled([4, 3], egui::Color32::RED)),
        }
    }

    #[test]
    fn cancellation_and_child_events_cannot_complete_a_new_capture() {
        let context = context();
        let first = request(&context).expect("first capture");
        assert!(request(&context).is_err());
        let stale = first.id;
        drop(first);
        let second = request(&context).expect("capture after cancellation");
        let mut input = RawInput {
            events: vec![event(stale)],
            ..RawInput::default()
        };
        let handle = context.plugin::<CapturePlugin>();
        handle.lock().input_hook(&context, &mut input);
        assert!(second.take().is_none());
        input.events = vec![event(second.id)];
        input.viewport_id = ViewportId::from_hash_of("child");
        handle.lock().input_hook(&context, &mut input);
        assert!(second.take().is_none());
        input.viewport_id = ViewportId::ROOT;
        handle.lock().input_hook(&context, &mut input);
        let capture = second
            .take()
            .expect("correlated frame")
            .expect("pixels")
            .png()
            .expect("PNG");
        capture.validate().expect("valid capture");
        let decoded = image::load_from_memory(&capture.png)
            .expect("decodable PNG")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 3));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn resize_invalidates_capture_even_before_the_screenshot_arrives() {
        let context = context();
        let ticket = request(&context).expect("capture");
        let mut input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(8.0, 3.0),
            )),
            ..RawInput::default()
        };
        context
            .plugin::<CapturePlugin>()
            .lock()
            .input_hook(&context, &mut input);
        assert!(ticket.take().expect("resize rejection").is_err());
    }

    #[test]
    fn encoder_unpremultiplies_alpha_and_bounds_output() {
        use std::io::Write as _;
        let pixels = Pixels {
            info: CaptureInfo {
                width: 1,
                height: 1,
                pixels_per_point: 1.0,
                requested_frame: 0,
                received_frame: 1,
            },
            image: Arc::new(ColorImage::filled(
                [1, 1],
                egui::Color32::from_rgba_unmultiplied(255, 0, 0, 128),
            )),
        };
        let rgba = pixels.rgba();
        assert_eq!(rgba, [255, 0, 0, 128]);
        let mut output = LimitedPng(vec![0; MAX_CAPTURE_BYTES - 1]);
        assert!(output.write_all(&[1, 2]).is_err());
        assert_eq!(output.0.len(), MAX_CAPTURE_BYTES - 1);
    }
}
