//! Shared popup-side polling, command admission, and acknowledgements.
use crate::window_channel::Channel;
use eframe::egui::Context;
use garmin_ui::window::protocol::{Link, Message};
use serde::{Serialize, de::DeserializeOwned};

pub struct Client<C, S> {
    channel: Channel<C, S>,
    pub link: Link,
    next_poll: f64,
}

impl<C: Serialize + DeserializeOwned + 'static, S: Serialize + DeserializeOwned + 'static>
    Client<C, S>
{
    pub fn new(context: Context, session: &str) -> Result<Self, String> {
        Ok(Self {
            channel: Channel::new(session, context)?,
            link: Link::default(),
            next_poll: 0.0,
        })
    }

    pub fn receive(&mut self, now: f64) -> Vec<S> {
        let mut snapshots = Vec::new();
        for message in self.channel.drain() {
            match message {
                Message::Snapshot(snapshot) => {
                    self.link.observe(now);
                    snapshots.push(*snapshot);
                }
                Message::Reply { id, error } => self.link.reply(id, error, now),
                Message::Poll | Message::Command { .. } => {}
            }
        }
        if now >= self.next_poll {
            if let Err(error) = self.channel.send(&Message::Poll) {
                self.link.error = Some(error);
            }
            self.next_poll = now + 0.5;
        }
        self.link.expire(now);
        snapshots
    }

    pub fn send(&mut self, request: C, now: f64) {
        if let Some(id) = self.link.begin(now)
            && let Err(error) = self.channel.send(&Message::Command { id, request })
        {
            self.link.reply(id, Some(error), now);
        }
    }
}
