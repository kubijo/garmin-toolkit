//! Viewport-scoped, explicit automation requests to reveal rendered controls.

#[derive(Clone)]
struct Request {
    target: String,
    matches: usize,
    visible: bool,
}

fn key(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::new(("automation-scroll-into-view", viewport))
}

pub(crate) fn clear_reveal(context: &egui::Context, viewport: egui::ViewportId) {
    context.data_mut(|data| data.remove::<Request>(key(viewport)));
}

pub(crate) fn request_reveal(
    context: &egui::Context,
    viewport: egui::ViewportId,
    target: &str,
) -> Result<bool, String> {
    context.data_mut(|data| {
        let previous = data.get_temp::<Request>(key(viewport));
        let previous = previous.filter(|request| request.target == target);
        if previous.as_ref().is_some_and(|request| request.matches > 1) {
            return Err(format!("ambiguous semantic target: {target}"));
        }
        let visible = previous.is_some_and(|request| request.matches == 1 && request.visible);
        data.insert_temp(
            key(viewport),
            Request {
                target: target.into(),
                matches: 0,
                visible: false,
            },
        );
        Ok(visible)
    })
}

pub(super) fn target(ui: &egui::Ui, response: &egui::Response, name: &str) {
    let key = key(ui.ctx().viewport_id());
    let enabled = ui.is_visible() && response.enabled();
    let visible =
        enabled && response.rect.is_positive() && ui.clip_rect().contains_rect(response.rect);
    let scroll = ui.ctx().data_mut(|data| {
        let Some(mut request) = data.get_temp::<Request>(key) else {
            return false;
        };
        if request.target != name {
            return false;
        }
        request.matches += 1;
        request.visible = visible;
        let scroll = enabled && !visible;
        data.insert_temp(key, request);
        scroll
    });
    if scroll {
        response.scroll_to_me(None);
    }
}
