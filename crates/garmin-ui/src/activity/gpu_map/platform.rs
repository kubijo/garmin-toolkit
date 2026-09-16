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

pub(in crate::activity) use implementation::Executor;

pub(super) fn upload_visible(
    executor: &Executor,
    visible: &[super::VisibleTile],
) -> super::UploadStats {
    #[cfg(target_arch = "wasm32")]
    {
        executor.upload_visible(visible)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = executor;
        Executor::upload_visible(visible)
    }
}
