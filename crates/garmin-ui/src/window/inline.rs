//! In-canvas presentation using the same content contract as platform windows.
use super::{Event, Spec};
use egui::{Context, Ui};

#[derive(Default)]
pub struct InlineWindow {
    spec: Option<Spec>,
    position: Option<egui::Pos2>,
}

impl InlineWindow {
    pub(super) fn spec(&self) -> Option<&Spec> {
        self.spec.as_ref()
    }
    pub fn open(&mut self, context: &Context, spec: Spec) {
        if self
            .spec
            .as_ref()
            .is_none_or(|current| current.id != spec.id)
        {
            self.position = None;
        }
        self.spec = Some(spec);
        context.request_repaint();
    }

    pub fn close(&mut self) {
        self.spec = None;
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.spec.is_some()
    }

    pub fn present<C>(
        &mut self,
        context: &Context,
        intl: &garmin_i18n::Intl,
        mut render: impl FnMut(&mut Ui) -> Option<C>,
    ) -> Vec<Event<C>> {
        let Some(spec) = &self.spec else {
            return Vec::new();
        };
        let mut open = true;
        let mut events = Vec::new();
        let bounds = context.content_rect();
        let mut drag = egui::Vec2::ZERO;
        let mut window = egui::Window::new(&spec.title)
            .id(egui::Id::new(("inline-window", &spec.id)))
            .default_size(spec.size)
            .default_pos(bounds.center() - egui::Vec2::from(spec.size).min(bounds.size()) / 2.0)
            .max_size(bounds.size())
            .constrain_to(bounds)
            .collapsible(false)
            .title_bar(false)
            .drag_area(egui::WindowDrag::Off)
            .frame(
                egui::Frame::window(&context.global_style())
                    .fill(context.global_style().visuals.panel_fill)
                    .inner_margin(0),
            );
        if let Some(position) = self.position {
            window = window.current_pos(position);
        }
        if let Some(response) = window.show(context, |ui| {
            let close = garmin_i18n::format_message!(intl, default_message: "Close window");
            match crate::shell::dialog_header(ui, &spec.title, &close) {
                Some(crate::shell::WindowAction::Close) => open = false,
                Some(crate::shell::WindowAction::Drag) => {
                    drag = ui.input(|input| input.pointer.delta());
                }
                _ => {}
            }
            if open && let Some(command) = render(ui) {
                events.push(Event::Command { id: 0, command });
            }
        }) {
            self.position = Some((response.response.rect.min + drag).clamp(
                bounds.min,
                bounds.max - response.response.rect.size().min(bounds.size()),
            ));
        }
        if !open {
            self.close();
            events.push(Event::Closed);
        }
        events
    }
}
