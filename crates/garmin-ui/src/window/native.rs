use super::{Event, Spec, WindowHost};
use egui::{Context, Ui, ViewportCommand, ViewportId};
use std::{
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct NativeWindow<C, S> {
    spec: Option<Spec>,
    inline: super::InlineWindow,
    live: Arc<AtomicBool>,
    #[cfg(any(feature = "automation", test))]
    control: Option<super::control::Registration>,
    types: PhantomData<fn() -> (C, S)>,
}

impl<C, S> Default for NativeWindow<C, S> {
    fn default() -> Self {
        Self {
            spec: None,
            inline: super::InlineWindow::default(),
            live: Arc::default(),
            #[cfg(any(feature = "automation", test))]
            control: None,
            types: PhantomData,
        }
    }
}

impl<C, S> NativeWindow<C, S> {
    /// Render shared controller state independently of the parent's repaint schedule.
    /// Actions and close events are delivered on the child pass, not queued for a parent pass.
    pub fn present_deferred(
        &mut self,
        context: &Context,
        intl: &garmin_i18n::Intl,
        render: impl Fn(&mut Ui) -> Option<C> + Send + Sync + 'static,
        event: impl Fn(&Context, Event<C>) + Send + Sync + 'static,
    ) {
        let Some(spec) = self.spec.clone() else {
            return;
        };
        if !self.live.load(Ordering::Acquire) {
            return;
        }
        #[cfg(any(feature = "automation", test))]
        if self.control.is_none() {
            self.control = super::control::register(context, &spec);
        }
        #[cfg(any(feature = "automation", test))]
        let control = self
            .control
            .as_ref()
            .map(super::control::Registration::liveness);
        let live = Arc::clone(&self.live);
        let labels = crate::shell::WindowLabels::new(intl);
        context.show_viewport_deferred(
            ViewportId::from_hash_of(&spec.id),
            builder(&spec),
            move |ui, class| {
                if !live.load(Ordering::Acquire) {
                    return;
                }
                #[cfg(any(feature = "automation", test))]
                let revoked = control
                    .as_ref()
                    .is_some_and(|live| !live.load(Ordering::Acquire));
                #[cfg(not(any(feature = "automation", test)))]
                let revoked = false;
                let (closed, command) = if revoked {
                    (true, None)
                } else {
                    content(ui, class, &spec.title, &labels, &render)
                };
                if closed {
                    live.store(false, Ordering::Release);
                    #[cfg(any(feature = "automation", test))]
                    if let Some(control) = &control {
                        control.store(false, Ordering::Release);
                    }
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    event(ui.ctx(), Event::Closed);
                    ui.ctx().request_repaint_of(ViewportId::ROOT);
                } else if let Some(command) = command {
                    event(ui.ctx(), Event::Command { id: 0, command });
                }
            },
        );
    }

    pub fn set_inline(&mut self, context: &Context, inline: bool) {
        if inline == self.inline.is_open() {
            return;
        }
        if let Some(spec) = self.spec.as_ref().or_else(|| self.inline.spec()).cloned() {
            if inline {
                self.open_inline(context, spec);
            } else {
                let _ = self.open(context, spec);
            }
        }
    }

    pub fn open_inline(&mut self, context: &Context, spec: Spec) {
        self.close(context);
        self.inline.open(context, spec);
    }
}

impl<C, S> WindowHost for NativeWindow<C, S> {
    type Command = C;
    type Snapshot = S;

    fn open(&mut self, context: &Context, spec: Spec) -> Result<(), String> {
        if self.live.load(Ordering::Acquire)
            && self
                .spec
                .as_ref()
                .is_some_and(|current| current.id == spec.id)
        {
            context
                .send_viewport_cmd_to(ViewportId::from_hash_of(&spec.id), ViewportCommand::Focus);
        } else {
            self.close(context);
            self.live = Arc::new(AtomicBool::new(true));
            #[cfg(any(feature = "automation", test))]
            {
                self.control = super::control::register(context, &spec);
            }
            self.spec = Some(spec);
        }
        context.request_repaint();
        Ok(())
    }

    fn close(&mut self, context: &Context) {
        let was_live = self.live.swap(false, Ordering::AcqRel);
        self.inline.close();
        #[cfg(any(feature = "automation", test))]
        {
            self.control = None;
        }
        if let Some(spec) = self.spec.take()
            && was_live
        {
            context.send_viewport_cmd_to(ViewportId::from_hash_of(spec.id), ViewportCommand::Close);
        }
    }

    fn is_open(&self) -> bool {
        (self.spec.is_some() && self.live.load(Ordering::Acquire)) || self.inline.is_open()
    }

    fn present(
        &mut self,
        context: &Context,
        intl: &garmin_i18n::Intl,
        _snapshot: impl FnOnce() -> S,
        mut render: impl FnMut(&mut Ui) -> Option<C>,
    ) -> Vec<Event<C>> {
        if self.inline.is_open() {
            return self.inline.present(context, intl, render);
        }
        #[cfg(any(feature = "automation", test))]
        if self.control.as_ref().is_some_and(|control| !control.live()) {
            self.close(context);
            return vec![Event::Closed];
        }
        let Some(spec) = &self.spec else {
            return Vec::new();
        };
        #[cfg(any(feature = "automation", test))]
        if self.control.is_none() {
            self.control = super::control::register(context, spec);
        }
        let mut events = Vec::new();
        let mut closed = false;
        let labels = crate::shell::WindowLabels::new(intl);
        context.show_viewport_immediate(
            ViewportId::from_hash_of(&spec.id),
            builder(spec),
            |ui, class| {
                let (close, command) = content(ui, class, &spec.title, &labels, &mut render);
                closed = close;
                if let Some(command) = command {
                    events.push(Event::Command { id: 0, command });
                }
            },
        );
        if closed {
            self.close(context);
            events.push(Event::Closed);
        }
        events
    }

    fn reply(&mut self, _id: u32, _error: Option<String>) {}
}

fn builder(spec: &Spec) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(&spec.title)
        .with_decorations(false)
        .with_transparent(true)
        .with_inner_size(spec.size)
}

fn content<C>(
    ui: &mut Ui,
    class: egui::ViewportClass,
    title: &str,
    labels: &crate::shell::WindowLabels,
    mut render: impl FnMut(&mut Ui) -> Option<C>,
) -> (bool, Option<C>) {
    if ui.input(|input| input.viewport().close_requested()) {
        return (true, None);
    }
    if class == egui::ViewportClass::EmbeddedWindow {
        return (false, render(ui));
    }
    let bounds = surface_bounds(ui);
    let result = surface(ui, |ui| {
        let closed = show_header(ui, title, labels);
        (closed, if closed { None } else { render(ui) })
    });
    super::resize::resize_at(ui, bounds);
    result
}

fn surface_bounds(ui: &Ui) -> egui::Rect {
    let expanded = ui.input(|input| {
        input.viewport().maximized.unwrap_or_default()
            || input.viewport().fullscreen.unwrap_or_default()
    });
    ui.available_rect_before_wrap()
        .shrink(if expanded { 0.0 } else { 12.0 })
}

/// Shared decoration for native and embedded windows.
pub fn frame(ui: &Ui) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    egui::Frame::NONE
        .fill(crate::theme::color32(palette.surfaces().background()))
        .stroke(egui::Stroke::new(
            1.0,
            crate::theme::color32(palette.borders().subtle()),
        ))
        .shadow(egui::Shadow {
            offset: [0, 3],
            blur: 16,
            spread: 0,
            color: egui::Color32::from_black_alpha(110),
        })
}

/// Paint the native child window frame, leaving transparent space for its shadow.
pub fn surface<R>(ui: &mut Ui, render: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = frame(ui);
    let contents = surface_bounds(ui).shrink(frame.stroke.width);
    ui.painter().add(frame.paint(contents));
    ui.scope_builder(egui::UiBuilder::new().max_rect(contents), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(contents));
        render(ui)
    })
    .inner
}

fn show_header(ui: &mut Ui, title: &str, labels: &crate::shell::WindowLabels) -> bool {
    use crate::shell;
    use crate::shell::WindowAction;
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or_default());
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
