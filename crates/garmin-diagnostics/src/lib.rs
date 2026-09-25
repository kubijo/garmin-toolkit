//! Read-only, loopback diagnostic routes shared by desktop and HASS.
mod access;
mod events;
mod http;
mod query;
mod response;
mod stream;
mod view;

pub use events::Events;
pub use http::{Diagnostics, is_route};

#[cfg(test)]
mod tests;
