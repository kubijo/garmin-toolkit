//! Browser transport only; payloads and routing are owned by Rust.
use eframe::egui::Context;
use garmin_ui::window::protocol::{Message, valid_session};
use serde::{Serialize, de::DeserializeOwned};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast as _, prelude::*};

const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

pub struct Channel<C, S> {
    transport: web_sys::BroadcastChannel,
    incoming: Rc<RefCell<Vec<Message<C, S>>>>,
    _callback: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl<C: Serialize + DeserializeOwned + 'static, S: Serialize + DeserializeOwned + 'static>
    Channel<C, S>
{
    pub fn new(session: &str, context: Context) -> Result<Self, String> {
        if !valid_session(session) {
            return Err("Invalid application window session".into());
        }
        let channel = web_sys::BroadcastChannel::new(&format!("garmin-window-{session}"))
            .map_err(|error| super::js_reason(&error))?;
        let incoming = Rc::new(RefCell::new(Vec::new()));
        let queue = incoming.clone();
        let callback = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            if let Some(text) = event.data().as_string()
                && text.len() <= MAX_MESSAGE_BYTES
                && let Ok(message) = serde_json::from_str::<Message<C, S>>(&text)
            {
                let mut queue = queue.borrow_mut();
                if queue.len() < 64 {
                    queue.push(message);
                    context.request_repaint();
                }
            }
        }) as Box<dyn FnMut(_)>);
        channel.set_onmessage(Some(callback.as_ref().unchecked_ref()));
        Ok(Self {
            transport: channel,
            incoming,
            _callback: callback,
        })
    }

    pub fn send(&self, message: &Message<C, S>) -> Result<(), String> {
        self.send_serialized(message)
    }

    pub fn send_serialized(&self, message: &impl Serialize) -> Result<(), String> {
        let text = serde_json::to_string(message).map_err(|error| error.to_string())?;
        if text.len() > MAX_MESSAGE_BYTES {
            return Err("Window snapshot exceeds the 32 MiB message limit".into());
        }
        self.transport
            .post_message(&JsValue::from_str(&text))
            .map_err(|error| super::js_reason(&error))
    }

    pub fn drain(&self) -> Vec<Message<C, S>> {
        std::mem::take(&mut *self.incoming.borrow_mut())
    }
}

impl<C, S> Drop for Channel<C, S> {
    fn drop(&mut self) {
        self.transport.set_onmessage(None);
        self.transport.close();
    }
}
