//! Browser implementation of the shared secondary-window contract.
use crate::window_channel::Channel;
use eframe::egui::{self, Context, Ui};
use garmin_ui::window::{
    Event, Spec, WindowHost,
    protocol::{KIND_PARAMETER, Message, SESSION_PARAMETER, valid_session},
};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;

pub struct BrowserWindow<C, S> {
    session: Option<Session<C, S>>,
    error: Option<String>,
}

struct Session<C, S> {
    spec: Spec,
    channel: Channel<C, S>,
    url: String,
    name: String,
    window: Option<web_sys::Window>,
}

impl<C, S> Default for BrowserWindow<C, S> {
    fn default() -> Self {
        Self {
            session: None,
            error: None,
        }
    }
}

impl<C: Serialize + DeserializeOwned + 'static, S: Serialize + DeserializeOwned + 'static>
    BrowserWindow<C, S>
{
    fn launch(&mut self, tab: bool) -> Result<(), String> {
        let session = self.session.as_mut().ok_or("No window session")?;
        if let Some(window) = &session.window
            && !window.closed().unwrap_or(true)
        {
            return window.focus().map_err(|error| crate::js_reason(&error));
        }
        let browser = web_sys::window().ok_or("Browser window unavailable")?;
        let features = if tab {
            String::new()
        } else {
            format!(
                "popup,width={},height={},resizable=yes,scrollbars=yes",
                session.spec.size[0], session.spec.size[1]
            )
        };
        session.window = browser
            .open_with_url_and_target_and_features(&session.url, &session.name, &features)
            .map_err(|error| crate::js_reason(&error))?;
        if session.window.is_none() {
            return Err(
                "The browser blocked this window. Allow popups for this site, or open it in a tab."
                    .into(),
            );
        }
        Ok(())
    }

    fn show_error(&mut self, context: &Context) {
        let Some(error) = self.error.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Open application window")
            .id(egui::Id::new((
                "window-error",
                self.session.as_ref().map(|s| &s.spec.id),
            )))
            .open(&mut open)
            .show(context, |ui| {
                ui.label(error);
                if ui.button("Retry popup").clicked() {
                    self.error = self.launch(false).err();
                }
                if ui.button("Open in a tab").clicked() {
                    self.error = self.launch(true).err();
                }
            });
        if !open {
            self.close(context);
        }
    }
}

impl<C: Serialize + DeserializeOwned + 'static, S: Serialize + DeserializeOwned + 'static>
    WindowHost for BrowserWindow<C, S>
{
    type Command = C;
    type Snapshot = S;

    fn open(&mut self, context: &Context, spec: Spec) -> Result<(), String> {
        if self.session.as_ref().is_none_or(|session| {
            session.spec.id != spec.id
                || session
                    .window
                    .as_ref()
                    .is_some_and(|window| window.closed().unwrap_or(true))
        }) {
            self.close(context);
            match setup(context.clone(), spec) {
                Ok(session) => self.session = Some(session),
                Err(error) => {
                    self.error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        let result = self.launch(false);
        self.error = result.as_ref().err().cloned();
        result
    }

    fn close(&mut self, _context: &Context) {
        if let Some(session) = self.session.take()
            && let Some(window) = session.window
        {
            let _ = window.close();
        }
        self.error = None;
    }

    fn is_open(&self) -> bool {
        self.session.is_some()
    }

    fn present(
        &mut self,
        context: &Context,
        _intl: &garmin_i18n::Intl,
        snapshot: impl FnOnce() -> S,
        _render: impl FnMut(&mut Ui) -> Option<C>,
    ) -> Vec<Event<C>> {
        self.show_error(context);
        let Some(session) = &self.session else {
            return Vec::new();
        };
        if session
            .window
            .as_ref()
            .is_some_and(|window| window.closed().unwrap_or(true))
        {
            self.session = None;
            return vec![Event::Closed];
        }
        let mut events = Vec::new();
        let mut snapshot = Some(snapshot);
        let mut published = false;
        for message in session.channel.drain() {
            match message {
                Message::Poll => {
                    if published {
                        continue;
                    }
                    published = true;
                    let snapshot = snapshot.take().expect("one publication per frame")();
                    let response: Message<(), S> = Message::Snapshot(Box::new(snapshot));
                    if let Err(error) = session.channel.send_serialized(&response) {
                        self.error = Some(error);
                    }
                }
                Message::Command { id, request } => events.push(Event::Command {
                    id,
                    command: request,
                }),
                Message::Snapshot(_) | Message::Reply { .. } => {}
            }
        }
        events
    }

    fn reply(&mut self, id: u32, error: Option<String>) {
        if let Some(session) = &self.session
            && let Err(error) = session.channel.send(&Message::Reply { id, error })
        {
            self.error = Some(error);
        }
    }
}

fn setup<C: Serialize + DeserializeOwned + 'static, S: Serialize + DeserializeOwned + 'static>(
    context: Context,
    spec: Spec,
) -> Result<Session<C, S>, String> {
    use std::fmt::Write as _;
    let window = web_sys::window().ok_or("Browser window unavailable")?;
    let mut bytes = [0u8; 16];
    window
        .crypto()
        .map_err(|error| crate::js_reason(&error))?
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|error| crate::js_reason(&error))?;
    let mut token = String::new();
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to String");
    }
    let url = web_sys::Url::new(
        &window
            .location()
            .href()
            .map_err(|error| crate::js_reason(&error))?,
    )
    .map_err(|error| crate::js_reason(&error))?;
    url.set_search("");
    url.set_hash("");
    url.search_params().set(SESSION_PARAMETER, &token);
    url.search_params().set(KIND_PARAMETER, &spec.kind);
    Ok(Session {
        spec,
        channel: Channel::new(&token, context)?,
        url: url.href(),
        name: format!("garmin-window-{token}"),
        window: None,
    })
}

pub fn start_if_requested(canvas: &web_sys::HtmlCanvasElement) -> Result<bool, JsValue> {
    let window =
        web_sys::window().ok_or_else(|| JsValue::from_str("Browser window unavailable"))?;
    let params = web_sys::UrlSearchParams::new_with_str(&window.location().search()?)?;
    let Some(session) = params.get(SESSION_PARAMETER) else {
        return Ok(false);
    };
    if !valid_session(&session) {
        return Err(JsValue::from_str("Invalid window session"));
    }
    let kind = params
        .get(KIND_PARAMETER)
        .ok_or_else(|| JsValue::from_str("Missing window kind"))?;
    let title = match kind.as_str() {
        "developer-tools" => "Developer tools · Garmin Toolkit",
        "device-files" => "Device files · Garmin Toolkit",
        _ => return Err(JsValue::from_str("Unknown window kind")),
    };
    if let Some(document) = window.document() {
        document.set_title(title);
    }
    let canvas = canvas.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let text_canvas = canvas.clone();
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                crate::map_composition::options("main-gl"),
                Box::new(move |creation| {
                    garmin_ui::install(&creation.egui_ctx);
                    let app = match kind.as_str() {
                        "developer-tools" => {
                            crate::developer::popup::create(creation.egui_ctx.clone(), &session)
                        }
                        _ => crate::files::popup::create(creation.egui_ctx.clone(), &session),
                    }
                    .map_err(std::io::Error::other)?;
                    Ok(app)
                }),
            )
            .await;
        match result {
            Ok(()) => crate::identify_text_agent(&text_canvas),
            Err(error) => crate::show_browser_failure(&crate::js_reason(&error)),
        }
    });
    Ok(true)
}
