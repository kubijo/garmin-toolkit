//! Platform execution selected once at the renderer boundary.

#[cfg(not(target_arch = "wasm32"))]
#[path = "platform/native.rs"]
mod implementation;
#[cfg(target_arch = "wasm32")]
#[path = "platform/web.rs"]
mod implementation;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "compile the inactive web executor on native only to test its upload admission policy"
)]
#[path = "platform/web.rs"]
mod web_tests;

pub(in crate::activity) use implementation::{Executor, ResourceCreationGate, UploadController};

pub(super) fn upload_visible(
    executor: &Executor,
    visible: &[super::VisibleTile],
) -> super::UploadStats {
    executor.upload_visible(visible)
}

pub(super) fn prepare_uploads(
    uploads: &UploadController,
    queue: &wgpu::Queue,
    metrics: &crate::activity::map_runtime::MapMetrics,
) {
    uploads.prepare(queue, metrics);
}
