//! Platform-owned sharing and completion queue primitives.

#[cfg(not(target_arch = "wasm32"))]
#[path = "platform/native.rs"]
mod implementation;
#[cfg(target_arch = "wasm32")]
#[path = "platform/web.rs"]
mod implementation;

pub use implementation::Backend;
pub(super) use implementation::{ResponseQueue, Shared};
