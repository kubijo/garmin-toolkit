use super::{Event, Spec, WindowHost};
use egui::{Context, Ui, ViewportCommand, ViewportId};
use std::marker::PhantomData;

pub struct NativeWindow<C, S> {
    spec: Option<Spec>,
    types: PhantomData<fn() -> (C, S)>,
}

impl<C, S> Default for NativeWindow<C, S> {
    fn default() -> Self {
        Self {
            spec: None,
            types: PhantomData,
        }
    }
}

impl<C, S> WindowHost for NativeWindow<C, S> {
    type Command = C;
    type Snapshot = S;

    fn open(&mut self, context: &Context, spec: Spec) -> Result<(), String> {
        if self
            .spec
            .as_ref()
            .is_some_and(|current| current.id == spec.id)
        {
            context
                .send_viewport_cmd_to(ViewportId::from_hash_of(&spec.id), ViewportCommand::Focus);
        } else {
            self.close(context);
            self.spec = Some(spec);
        }
        context.request_repaint();
        Ok(())
    }

    fn close(&mut self, context: &Context) {
        if let Some(spec) = self.spec.take() {
            context.send_viewport_cmd_to(ViewportId::from_hash_of(spec.id), ViewportCommand::Close);
        }
    }

    fn is_open(&self) -> bool {
        self.spec.is_some()
    }

    fn present(
        &mut self,
        context: &Context,
        intl: &garmin_i18n::Intl,
        _snapshot: impl FnOnce() -> S,
        mut render: impl FnMut(&mut Ui) -> Option<C>,
    ) -> Vec<Event<C>> {
        let Some(spec) = &self.spec else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut closed = false;
        context.show_viewport_immediate(
            ViewportId::from_hash_of(&spec.id),
            egui::ViewportBuilder::default()
                .with_title(&spec.title)
                .with_decorations(false)
                .with_transparent(true)
                .with_inner_size(spec.size),
            |ui, class| {
                if ui.input(|input| input.viewport().close_requested()) {
                    closed = true;
                    return;
                }
                if class == egui::ViewportClass::EmbeddedWindow {
                    if let Some(command) = render(ui) {
                        events.push(Event::Command { id: 0, command });
                    }
                } else {
                    let bounds = surface_bounds(ui);
                    surface(ui, |ui| {
                        closed = show_header(ui, &spec.title, intl);
                        if !closed && let Some(command) = render(ui) {
                            events.push(Event::Command { id: 0, command });
                        }
                    });
                    super::resize::resize_at(ui, bounds);
                }
            },
        );
        if closed {
            self.spec = None;
            events.push(Event::Closed);
        }
        events
    }

    fn reply(&mut self, _id: u32, _error: Option<String>) {}
}

fn surface_bounds(ui: &Ui) -> egui::Rect {
    let expanded = ui.input(|input| {
        input.viewport().maximized.unwrap_or_default()
            || input.viewport().fullscreen.unwrap_or_default()
    });
    ui.available_rect_before_wrap()
        .shrink(if expanded { 0.0 } else { 12.0 })
}

/// Paint the native child window frame, leaving transparent space for its shadow.
pub fn surface<R>(ui: &mut Ui, render: impl FnOnce(&mut Ui) -> R) -> R {
    let bounds = surface_bounds(ui);
    let palette = crate::theme::palette(ui);
    let border = palette.borders().subtle();
    ui.painter().add(
        egui::Shadow {
            offset: [0, 3],
            blur: 16,
            spread: 0,
            color: egui::Color32::from_black_alpha(110),
        }
        .as_shape(bounds, 0),
    );
    ui.painter().rect(
        bounds,
        0.0,
        crate::theme::color32(palette.surfaces().background()),
        egui::Stroke::new(1.0, crate::theme::color32(border)),
        egui::StrokeKind::Inside,
    );
    let contents = bounds.shrink(1.0);
    ui.scope_builder(egui::UiBuilder::new().max_rect(contents), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(contents));
        render(ui)
    })
    .inner
}

fn show_header(ui: &mut Ui, title: &str, intl: &garmin_i18n::Intl) -> bool {
    use crate::shell;
    use crate::shell::WindowAction;
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or_default());
    let labels = shell::WindowLabels::new(intl);
    let controls = labels.props(ui.ctx());
    let mut action = shell::window_header(ui, title, &controls);
    let menu_id = ui.id().with("native-window-menu");
    if let Some(WindowAction::ShowMenu(_)) = action {
        egui::Popup::open_id(ui.ctx(), menu_id);
    }
    egui::Popup::new(
        menu_id,
        ui.ctx().clone(),
        egui::PopupAnchor::PointerFixed,
        ui.layer_id(),
    )
    .open_memory(None)
    .kind(egui::PopupKind::Menu)
    .show(|ui| {
        for (label, requested) in [
            (controls.minimize_label, WindowAction::Minimize),
            (
                if maximized {
                    controls.restore_label
                } else {
                    controls.maximize_label
                },
                WindowAction::ToggleMaximize,
            ),
            (controls.close_label, WindowAction::Close),
        ] {
            if ui.button(label).clicked() {
                action = Some(requested);
                ui.close();
            }
        }
    });
    match action {
        Some(WindowAction::Drag) => ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag),
        Some(WindowAction::Minimize) => {
            ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        Some(WindowAction::ToggleMaximize) => ui
            .ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
        Some(WindowAction::Close) => {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            return true;
        }
        Some(WindowAction::ShowMenu(_)) | None => {}
    }
    false
}
