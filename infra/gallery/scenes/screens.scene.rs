use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::terminal_input::TerminalInput;
use gallery::prelude::*;
use garmin_cli_tui::preview::{PreviewScreen, PreviewState, render_preview};
use garmin_cli_tui::{LOADING_SPINNER_INTERVAL, RunProfile};
use nerd_font::NerdFont;
use num_traits::ToPrimitive as _;
use parley_ratatui::vello::AaConfig;
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    BundledFont, FontOptions, FontSource, FontStack, GpuRenderer, GpuRendererOptions,
    ParleyBackend, Rgba, TerminalRenderer, TexturePresentation, TextureReadback, TextureTarget,
    Theme,
};
use ratatui::Terminal;

scene_meta! { title: "TUI / Setup / Update" }

const SIZES: &[&str] = &["80 × 24", "100 × 30", "140 × 40"];
const RESIZE_SETTLE_TIME: Duration = Duration::from_millis(120);
// Static storage avoids wgpu's plug-in teardown ordering bug and preserves
// font, layout, render-target, and readback caches across scene changes.
static TERMINAL_RENDERER: OnceLock<Mutex<TerminalPreviewRenderer>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum PreviewFont {
    DepartureMono,
    JetBrainsMonoNerd,
}

#[derive(Clone, Copy, Debug)]
struct FontFit {
    font: PreviewFont,
    columns: u16,
    rows: u16,
    max_width: u32,
    max_height: u32,
}

#[derive(Clone, Copy, Debug)]
struct TerminalPreviewSpec {
    profile: RunProfile,
    screen: PreviewScreen,
    columns: u16,
    rows: u16,
    font: PreviewFont,
    animation_frame: usize,
}

#[derive(Clone, Copy)]
struct FontScale {
    cell_width: f32,
    cell_height: f32,
}

#[derive(Clone)]
struct PreviewTexture {
    texture: egui::TextureHandle,
    scene_revision: SceneRevision,
    animation_frame: usize,
    input_revision: u64,
    rendered_font_size: u16,
    target_font_size: u16,
    viewport_pixels: [u32; 2],
    target_since: Instant,
}

#[scene(order = 10)]
fn contact_garmin(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ContactGarmin,
    );
}

#[scene(default, order = 20)]
fn loading_components(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::LoadingComponents,
    );
}

#[scene(order = 25)]
fn reading_storage(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::ReadingStorage,
    );
}

#[scene(order = 30)]
fn map_selection(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(ctx, ui, RunProfile::PRODUCTION, PreviewScreen::MapSelection);
}

#[scene(order = 40)]
fn storage_capacity(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::StorageCapacity,
    );
}

#[scene(order = 41)]
fn storage_unavailable(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::StorageUnavailable,
    );
}

#[scene(order = 42)]
fn storage_refresh_failed(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::StorageRefreshFailed,
    );
}

#[scene(order = 43)]
fn long_storage_metadata(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::StorageStress,
    );
}

#[scene(order = 50)]
fn update_confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let backup_enabled = ctx.toggle("verified backup", true);
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::UpdateConfirmation(backup_enabled),
    );
}

#[scene(order = 50)]
fn pipeline_probe_confirmation(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    show_terminal(
        ctx,
        ui,
        RunProfile::PRODUCTION,
        PreviewScreen::PipelineProbeConfirmation,
    );
}

pub fn show_terminal(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    profile: RunProfile,
    screen: PreviewScreen,
) {
    let scene_revision = ctx.scene_revision();
    let animation_frame = animation_frame(ui, screen);
    let size = ctx.buttons("terminal size", SIZES, 1);
    let (columns, rows) = match size {
        0 => (80, 24),
        2 => (140, 40),
        _ => (100, 30),
    };
    ui.columns(2, |previews| {
        ctx.stage(&mut previews[0], Stage::Fill, |ui| {
            show_font_preview(
                ui,
                TerminalPreviewSpec {
                    profile,
                    screen,
                    columns,
                    rows,
                    font: PreviewFont::DepartureMono,
                    animation_frame,
                },
                scene_revision,
            );
        });
        ctx.stage(&mut previews[1], Stage::Fill, |ui| {
            show_font_preview(
                ui,
                TerminalPreviewSpec {
                    profile,
                    screen,
                    columns,
                    rows,
                    font: PreviewFont::JetBrainsMonoNerd,
                    animation_frame,
                },
                scene_revision,
            );
        });
    });
}

fn show_font_preview(ui: &mut Ui, spec: TerminalPreviewSpec, scene_revision: SceneRevision) {
    let label = match spec.font {
        PreviewFont::DepartureMono => "Departure Mono (pixel)",
        PreviewFont::JetBrainsMonoNerd => "JetBrains Mono Nerd Font",
    };
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(label).strong());
        ui.add_space(6.0);
    });

    let pixels_per_point = ui.ctx().pixels_per_point().max(1.0);
    let available = ui.available_size();
    let max_pixels = [
        even_floor(available.x * pixels_per_point)
            .saturating_sub(2)
            .max(2),
        even_floor(available.y * pixels_per_point)
            .saturating_sub(2)
            .max(2),
    ];
    let fit = FontFit {
        font: spec.font,
        columns: spec.columns,
        rows: spec.rows,
        max_width: max_pixels[0],
        max_height: max_pixels[1],
    };
    let font_size = shared_renderer().fit_font_size(fit);
    let texture_id = ui.id().with((
        spec.profile,
        spec.screen,
        spec.columns,
        spec.rows,
        spec.font,
    ));
    let input_id = ui
        .id()
        .with(("terminal-input", spec.profile, spec.screen, spec.font));
    let mut input = ui
        .data(|data| data.get_temp::<(SceneRevision, TerminalInput)>(input_id))
        .filter(|(revision, _)| *revision == scene_revision)
        .map(|(_, input)| input)
        .unwrap_or_default();
    input.set_grid([spec.columns, spec.rows]);
    let preview = terminal_preview(
        ui,
        spec,
        scene_revision,
        texture_id,
        font_size,
        max_pixels,
        &mut input,
    );
    let (image, response) = paint_terminal_texture(
        ui,
        &preview.texture,
        available,
        pixels_per_point,
        input_id,
        input.state.is_interactive(),
    );
    input.interact(ui, &response, image, [spec.columns, spec.rows]);
    ui.data_mut(|data| {
        data.insert_temp(texture_id, preview);
        data.insert_temp(input_id, (scene_revision, input));
    });
}

fn terminal_preview(
    ui: &Ui,
    spec: TerminalPreviewSpec,
    scene_revision: SceneRevision,
    texture_id: egui::Id,
    font_size: u16,
    max_pixels: [u32; 2],
    input: &mut TerminalInput,
) -> PreviewTexture {
    let now = Instant::now();
    let mut preview = ui
        .data(|data| data.get_temp::<PreviewTexture>(texture_id))
        .filter(|preview| preview.scene_revision == scene_revision)
        .unwrap_or_else(|| PreviewTexture {
            texture: load_terminal_texture(ui, spec, font_size, &mut input.state),
            scene_revision,
            animation_frame: spec.animation_frame,
            input_revision: input.revision,
            rendered_font_size: font_size,
            target_font_size: font_size,
            viewport_pixels: max_pixels,
            target_since: now,
        });
    if preview.viewport_pixels != max_pixels || preview.target_font_size != font_size {
        preview.viewport_pixels = max_pixels;
        preview.target_font_size = font_size;
        preview.target_since = now;
    }
    if preview.rendered_font_size != preview.target_font_size {
        let settled_for = now.saturating_duration_since(preview.target_since);
        if settled_for >= RESIZE_SETTLE_TIME {
            preview.texture =
                load_terminal_texture(ui, spec, preview.target_font_size, &mut input.state);
            preview.rendered_font_size = preview.target_font_size;
            preview.animation_frame = spec.animation_frame;
            preview.input_revision = input.revision;
        } else {
            ui.ctx()
                .request_repaint_after(RESIZE_SETTLE_TIME.saturating_sub(settled_for));
        }
    }
    let input_changed = preview.input_revision != input.revision;
    if preview.animation_frame != spec.animation_frame || input_changed {
        preview.texture.set(
            terminal_image(spec, preview.rendered_font_size, &mut input.state),
            egui::TextureOptions::LINEAR,
        );
        preview.animation_frame = spec.animation_frame;
        preview.input_revision = input.revision;
    }
    preview
}

fn paint_terminal_texture(
    ui: &mut Ui,
    texture: &egui::TextureHandle,
    available: egui::Vec2,
    pixels_per_point: f32,
    input_id: egui::Id,
    interactive: bool,
) -> (egui::Rect, egui::Response) {
    let [width, height] = texture.size();
    let texture_size = [
        u32::try_from(width).expect("preview width fits in u32"),
        u32::try_from(height).expect("preview height fits in u32"),
    ];
    let [logical_width, logical_height] =
        TexturePresentation::new(texture_size, pixels_per_point).logical_size();
    let logical_size = egui::vec2(logical_width, logical_height);
    let (allocated, _) = ui.allocate_exact_size(available, egui::Sense::hover());
    let image_origin = allocated.center() - logical_size / 2.0;
    let image_rect = egui::Rect::from_min_size(
        snap_inward_to_even_physical_pixel(image_origin, pixels_per_point),
        logical_size,
    );
    ui.painter_at(allocated).image(
        texture.id(),
        image_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    let response = ui.interact(
        image_rect.intersect(allocated),
        input_id,
        if interactive {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let response = if interactive {
        response.on_hover_text("Scroll either panel. Click for keyboard control; Tab switches panels. Esc releases focus.")
    } else {
        response
    };
    (image_rect, response)
}

fn load_terminal_texture(
    ui: &Ui,
    spec: TerminalPreviewSpec,
    font_size: u16,
    state: &mut PreviewState,
) -> egui::TextureHandle {
    ui.ctx().load_texture(
        format!(
            "garmin-cli-{:?}-{:?}-{}x{}-{:?}-{font_size}px",
            spec.profile, spec.screen, spec.columns, spec.rows, spec.font
        ),
        terminal_image(spec, font_size, state),
        egui::TextureOptions::LINEAR,
    )
}

fn terminal_image(
    spec: TerminalPreviewSpec,
    font_size: u16,
    state: &mut PreviewState,
) -> egui::ColorImage {
    let backend = ParleyBackend::new(spec.columns, spec.rows);
    let mut terminal = Terminal::new(backend).expect("the in-memory backend is infallible");
    terminal
        .draw(|frame| {
            render_preview(
                frame,
                spec.profile,
                spec.screen,
                spec.animation_frame,
                state,
            );
        })
        .expect("the in-memory backend is infallible");

    shared_renderer().render(terminal.backend().buffer(), spec.font, font_size)
}

fn animation_frame(ui: &Ui, screen: PreviewScreen) -> usize {
    if !matches!(
        screen,
        PreviewScreen::LoadingComponents
            | PreviewScreen::ReadingStorage
            | PreviewScreen::ConcurrentProgress
            | PreviewScreen::ProgressOverflow
    ) {
        return 0;
    }

    ui.ctx().request_repaint_after(LOADING_SPINNER_INTERVAL);
    let elapsed = ui.input(|input| input.time.max(0.0));
    (elapsed / LOADING_SPINNER_INTERVAL.as_secs_f64())
        .floor()
        .to_usize()
        .unwrap_or(usize::MAX)
}

fn shared_renderer() -> std::sync::MutexGuard<'static, TerminalPreviewRenderer> {
    TERMINAL_RENDERER
        .get_or_init(|| Mutex::new(TerminalPreviewRenderer::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

struct TerminalPreviewRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    gpu: GpuRenderer,
    terminals: [TerminalRenderer; 2],
    font_sizes: [u16; 2],
    font_scales: [FontScale; 2],
    targets: [Option<TextureTarget>; 2],
    readbacks: [TextureReadback; 2],
    pixels: [Vec<u8>; 2],
}

impl TerminalPreviewRenderer {
    fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("a graphics adapter is required to render modern font previews");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("the graphics adapter must provide a compatible device");
        let gpu = GpuRenderer::new_with_options(
            &device,
            GpuRendererOptions {
                antialiasing_method: AaConfig::Area,
            },
        )
        .expect("the Vello renderer must initialize");
        let font_sizes = [11, 16];
        let terminals = [
            TerminalRenderer::new(
                font_options(PreviewFont::DepartureMono, font_sizes[0]),
                terminal_theme(),
            ),
            TerminalRenderer::new(
                font_options(PreviewFont::JetBrainsMonoNerd, font_sizes[1]),
                terminal_theme(),
            ),
        ];
        let font_scales = [
            measure_font_scale(PreviewFont::DepartureMono),
            measure_font_scale(PreviewFont::JetBrainsMonoNerd),
        ];

        Self {
            device,
            queue,
            gpu,
            terminals,
            font_sizes,
            font_scales,
            targets: [None, None],
            readbacks: [TextureReadback::new(), TextureReadback::new()],
            pixels: [Vec::new(), Vec::new()],
        }
    }

    fn fit_font_size(&self, fit: FontFit) -> u16 {
        let mut smallest = 1;
        let mut largest = u16::MAX;
        let mut best = 1;
        while smallest <= largest {
            let candidate = smallest + (largest - smallest) / 2;
            let cell_size = self.cell_size(fit.font, candidate);
            let [width, height] = pixel_size(fit.columns, fit.rows, cell_size);
            if width <= fit.max_width && height <= fit.max_height {
                best = candidate;
                smallest = candidate.saturating_add(1);
            } else {
                largest = candidate.saturating_sub(1);
            }
        }
        best
    }

    fn render(
        &mut self,
        buffer: &ratatui::buffer::Buffer,
        font: PreviewFont,
        font_size: u16,
    ) -> egui::ColorImage {
        self.configure(font, font_size);
        let index = font.index();
        let metrics = self.terminals[index].metrics();
        let cell_size = [
            metrics
                .cell_width
                .to_u32()
                .expect("rounded cell width fits in u32"),
            metrics
                .cell_height
                .to_u32()
                .expect("rounded cell height fits in u32"),
        ];
        debug_assert_eq!(cell_size, self.cell_size(font, font_size));
        let [width, height] = pixel_size(buffer.area.width, buffer.area.height, cell_size);
        let target = self.targets[index].get_or_insert_with(|| {
            TextureTarget::new(
                &self.device,
                width,
                height,
                wgpu::TextureFormat::Rgba8Unorm,
                Some("garmin-cli.gallery.terminal-preview"),
            )
        });
        if target.width != width || target.height != height {
            *target = TextureTarget::new(
                &self.device,
                width,
                height,
                wgpu::TextureFormat::Rgba8Unorm,
                Some("garmin-cli.gallery.terminal-preview"),
            );
        }
        self.gpu
            .render_to_rgba8_into(
                &mut self.terminals[index],
                &mut self.readbacks[index],
                &self.device,
                &self.queue,
                target,
                buffer,
                None,
                false,
                &mut self.pixels[index],
            )
            .expect("the terminal preview must render");

        egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &self.pixels[index],
        )
    }

    fn configure(&mut self, font: PreviewFont, font_size: u16) {
        let index = font.index();
        if self.font_sizes[index] == font_size {
            return;
        }
        self.terminals[index] =
            TerminalRenderer::new(font_options(font, font_size), terminal_theme());
        self.font_sizes[index] = font_size;
    }

    fn cell_size(&self, font: PreviewFont, font_size: u16) -> [u32; 2] {
        let scale = self.font_scales[font.index()];
        [
            (scale.cell_width * f32::from(font_size))
                .round()
                .max(1.0)
                .to_u32()
                .expect("cell width fits in u32"),
            (scale.cell_height * f32::from(font_size))
                .round()
                .max(1.0)
                .to_u32()
                .expect("cell height fits in u32"),
        ]
    }
}

impl PreviewFont {
    const fn index(self) -> usize {
        match self {
            Self::DepartureMono => 0,
            Self::JetBrainsMonoNerd => 1,
        }
    }
}

fn measure_font_scale(font: PreviewFont) -> FontScale {
    const REFERENCE_SIZE: u16 = 4096;

    let renderer = TerminalRenderer::new(font_options(font, REFERENCE_SIZE), terminal_theme());
    let metrics = renderer.metrics();
    FontScale {
        cell_width: metrics.cell_width / f32::from(REFERENCE_SIZE),
        cell_height: metrics.cell_height / f32::from(REFERENCE_SIZE),
    }
}

fn pixel_size(columns: u16, rows: u16, cell_size: [u32; 2]) -> [u32; 2] {
    [
        even_ceil(u32::from(columns).saturating_mul(cell_size[0])),
        even_ceil(u32::from(rows).saturating_mul(cell_size[1])),
    ]
}

fn font_options(font: PreviewFont, size: u16) -> FontOptions {
    let mut options = FontOptions::default().with_font_stack(font_stack(font));
    options.size = f32::from(size);
    options
}

fn even_floor(value: f32) -> u32 {
    value
        .max(2.0)
        .floor()
        .to_u32()
        .expect("viewport extent fits in u32")
        & !1
}

const fn even_ceil(value: u32) -> u32 {
    value.saturating_add(1) & !1
}

fn snap_inward_to_even_physical_pixel(position: egui::Pos2, pixels_per_point: f32) -> egui::Pos2 {
    let snap = |value: f32| (value * pixels_per_point / 2.0).ceil() * 2.0 / pixels_per_point;
    egui::pos2(snap(position.x), snap(position.y))
}

fn font_stack(font: PreviewFont) -> FontStack {
    let (family, bytes, fallback_family, fallback_bytes) = match font {
        PreviewFont::DepartureMono => (
            "Departure Mono",
            include_bytes!("../fonts/DepartureMono-Regular.otf").as_slice(),
            NerdFont::FONT_FAMILY,
            NerdFont::FONT_BYTES,
        ),
        PreviewFont::JetBrainsMonoNerd => (
            NerdFont::FONT_FAMILY,
            NerdFont::FONT_BYTES,
            "Departure Mono",
            include_bytes!("../fonts/DepartureMono-Regular.otf").as_slice(),
        ),
    };
    FontStack {
        regular: FontSource::Bundled(BundledFont::from_static(bytes).with_family_name(family)),
        bold: None,
        italic: None,
        bold_italic: None,
        fallbacks: vec![FontSource::Bundled(
            BundledFont::from_static(fallback_bytes).with_family_name(fallback_family),
        )],
    }
}

const fn terminal_theme() -> Theme {
    Theme {
        foreground: Rgba::rgb(216, 202, 184),
        background: Rgba::rgb(10, 13, 19),
        cursor: Rgba::rgb(255, 174, 87),
        palette: [
            Rgba::rgb(10, 13, 19),
            Rgba::rgb(205, 49, 49),
            Rgba::rgb(166, 227, 65),
            Rgba::rgb(255, 174, 87),
            Rgba::rgb(70, 130, 180),
            Rgba::rgb(188, 63, 188),
            Rgba::rgb(141, 232, 210),
            Rgba::rgb(216, 202, 184),
            Rgba::rgb(92, 92, 92),
            Rgba::rgb(241, 76, 76),
            Rgba::rgb(166, 227, 65),
            Rgba::rgb(255, 174, 87),
            Rgba::rgb(110, 170, 220),
            Rgba::rgb(214, 112, 214),
            Rgba::rgb(141, 232, 210),
            Rgba::rgb(255, 255, 255),
        ],
    }
}
