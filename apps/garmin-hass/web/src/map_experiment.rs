//! First composition gate. The real activity renderer is not migrated by this fixture.

use eframe::{egui, wgpu};
use garmin_ui::activity::map_composition::{CompositionPlugin, Host, Placement};
use wasm_bindgen::prelude::*;

#[path = "map_experiment/renderer.rs"]
pub(super) mod renderer;

#[wasm_bindgen(module = "/map-experiment.js")]
extern "C" {
    #[wasm_bindgen(js_name = reportRendererStartup)]
    pub(super) fn report_startup(mode: &str, backend: &str, adapter: &str, telemetry: bool);
    #[wasm_bindgen(js_name = takeRendererDiagnostics)]
    fn renderer_diagnostics() -> String;
    #[wasm_bindgen(js_name = startMap, catch)]
    fn start_map(
        canvas: &web_sys::HtmlCanvasElement,
        mode: &str,
        repaint: &JsValue,
    ) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = setMapRoute)]
    fn map_route(revision: u32, bytes: &[u8]);
    #[wasm_bindgen(js_name = setMapView)]
    fn map_view(json: &str);
    #[wasm_bindgen(js_name = failMap)]
    fn map_fail(reason: &str);
    #[wasm_bindgen(js_name = mapFailure)]
    fn map_failure() -> String;
    #[wasm_bindgen(js_name = mapReadiness)]
    fn map_readiness() -> String;
    #[wasm_bindgen(js_name = compositionFixture)]
    fn composition_fixture() -> bool;
    #[wasm_bindgen(js_name = experimentMode, catch)]
    fn experiment_mode(enabled: bool, search: &str) -> Result<String, JsValue>;
    #[wasm_bindgen(js_name = startComposition, catch)]
    fn start_composition(
        canvas: &web_sys::HtmlCanvasElement,
        mode: &str,
        repaint: &JsValue,
    ) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = beginCompositionPass)]
    fn begin_pass();
    #[wasm_bindgen(js_name = beginCompositionPaint)]
    fn begin_paint();
    #[wasm_bindgen(js_name = paintedComposition)]
    fn painted(projection: &[f32], clip: &[u32], screen: &[u32]);
    #[wasm_bindgen(js_name = compositionStatusText)]
    fn status_text() -> String;
    #[wasm_bindgen(js_name = disposeComposition)]
    fn dispose();
}

pub(super) fn update_diagnostics(context: &egui::Context) {
    if let Ok(diagnostics) = serde_json::from_str::<
        garmin_ui::activity::map_diagnostics::RendererDiagnostics,
    >(&renderer_diagnostics())
    {
        diagnostics.install(context);
    }
}

impl garmin_ui::activity::map_remote::Host for BrowserHost {
    fn route(&self, revision: u32, bytes: &[u8]) {
        map_route(revision, bytes);
    }
    fn view(&self, json: &str) {
        map_view(json);
    }
    fn fail(&self, reason: &str) {
        map_fail(reason);
    }
    fn failure(&self) -> String {
        map_failure()
    }
    fn readiness(&self) -> String {
        map_readiness()
    }
}

pub(super) fn is_fixture() -> bool {
    composition_fixture()
}

pub(super) fn install_map(
    creation: &eframe::CreationContext<'_>,
    canvas: &web_sys::HtmlCanvasElement,
    mode: &str,
) -> Result<(), std::io::Error> {
    let render = creation
        .wgpu_render_state
        .as_ref()
        .ok_or_else(|| std::io::Error::other("remote map requires WGPU"))?;
    if render.adapter.get_info().backend != wgpu::Backend::Gl {
        return Err(std::io::Error::other(
            "remote map requires a WebGL2 UI canvas",
        ));
    }
    let context = creation.egui_ctx.clone();
    let repaint = Closure::<dyn FnMut()>::new(move || context.request_repaint()).into_js_value();
    start_map(canvas, mode, &repaint).map_err(|e| std::io::Error::other(super::js_reason(&e)))?;
    creation.egui_ctx.add_plugin(CompositionPlugin::new(
        &render.device,
        render.target_format,
        1,
        BrowserHost,
    ));
    creation
        .egui_ctx
        .add_plugin(garmin_ui::activity::map_remote::RemoteMapPlugin::new(
            BrowserHost,
        ));
    Ok(())
}

pub(super) fn mode(canvas: &web_sys::HtmlCanvasElement) -> Result<String, JsValue> {
    let enabled = canvas
        .get_attribute("data-map-render-experiment")
        .ok_or_else(|| super::js_error("the map experiment configuration is missing"))?;
    let enabled = serde_json::from_str::<bool>(&enabled)
        .map_err(|_| super::js_error("the map experiment configuration is invalid"))?;
    let search = web_sys::window()
        .ok_or_else(|| super::js_error("browser window is missing"))?
        .location()
        .search()?;
    experiment_mode(enabled, &search)
}

pub(super) fn options(mode: &str) -> eframe::WebOptions {
    let mut options = eframe::WebOptions::default();
    if !mode.is_empty() {
        // Force the SAME UI backend for baseline and experiment. No backend substitution.
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
        {
            setup.instance_descriptor.backends = wgpu::Backends::GL;
        }
    }
    options
}

struct BrowserHost;

impl Host for BrowserHost {
    fn begin_pass(&self) {
        begin_pass();
    }
    fn begin_paint(&self) {
        begin_paint();
    }
    fn painted(&self, placement: Placement) {
        painted(&placement.projection, &placement.clip, &placement.screen);
    }
}

pub(super) struct Fixture {
    width: f32,
    height: f32,
    show_map: bool,
    show_window: bool,
    show_modal: bool,
}

impl Fixture {
    pub(super) fn new(
        creation: &eframe::CreationContext<'_>,
        canvas: &web_sys::HtmlCanvasElement,
        mode: &str,
    ) -> Result<Self, std::io::Error> {
        let render = creation
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| std::io::Error::other("composition requires the WGPU renderer"))?;
        if render.adapter.get_info().backend != wgpu::Backend::Gl {
            return Err(std::io::Error::other(
                "composition requires a WebGL2 UI canvas",
            ));
        }
        let context = creation.egui_ctx.clone();
        let repaint =
            Closure::<dyn FnMut()>::new(move || context.request_repaint()).into_js_value();
        start_composition(canvas, mode, &repaint)
            .map_err(|error| std::io::Error::other(super::js_reason(&error)))?;
        creation.egui_ctx.add_plugin(CompositionPlugin::new(
            &render.device,
            render.target_format,
            1,
            BrowserHost,
        ));
        Ok(Self {
            width: 760.0,
            height: 400.0,
            show_map: true,
            show_window: false,
            show_modal: false,
        })
    }
}

impl eframe::App for Fixture {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        super::hide_loading_overlay();
        update_diagnostics(ui.ctx());
        ui.heading("Map composition proof — not the activity renderer");
        ui.label(status_text());
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_map, "Show surface");
            ui.checkbox(&mut self.show_window, "Overlap window");
            let mut dark = ui.visuals().dark_mode;
            if ui.checkbox(&mut dark, "Dark theme").changed() {
                ui.ctx().set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
            }
            ui.checkbox(&mut self.show_modal, "Modal");
        });
        ui.add(egui::Slider::new(&mut self.width, 160.0..=1600.0).text("Width"));
        ui.add(egui::Slider::new(&mut self.height, 120.0..=1200.0).text("Height"));
        let mut zoom = ui.ctx().zoom_factor();
        if ui
            .add(egui::Slider::new(&mut zoom, 0.5..=2.0).text("UI zoom"))
            .changed()
        {
            ui.ctx().set_zoom_factor(zoom);
        }
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add_space(80.0);
            if self.show_map {
                let (rect, _) = ui
                    .allocate_exact_size(egui::vec2(self.width, self.height), egui::Sense::hover());
                // Erase an opaque fill: transparent source-over painting would fail this proof.
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_rgb(180, 0, 90));
                let opening = ui.ctx().plugin::<CompositionPlugin>().lock().opening(rect);
                ui.painter().add(opening);
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(16.0)), |ui| {
                    if ui
                        .button("Overlay control")
                        .on_hover_text("Tooltip above the worker canvas")
                        .clicked()
                    {
                        self.show_window = true;
                    }
                });
            }
            ui.add_space(500.0);
            ui.label("Opaque content after the map");
        });
        egui::Window::new("Composition overlay")
            .open(&mut self.show_window)
            .show(ui.ctx(), |ui| {
                ui.label("This window and its shadow must stay above the worker canvas.");
            });
        if self.show_modal {
            let modal =
                egui::Modal::new(egui::Id::new("composition-proof-modal")).show(ui.ctx(), |ui| {
                    ui.label(
                        "The modal backdrop must dim the worker surface and block its controls.",
                    );
                    ui.button("Close modal").clicked()
                });
            if modal.inner || modal.should_close() {
                self.show_modal = false;
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        dispose();
    }
}
