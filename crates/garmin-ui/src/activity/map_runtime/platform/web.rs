use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};

use super::super::{
    BrowserLabelTask, BrowserRouteTask, MapSceneData, MapTileResponse, SurfaceDriver, SurfaceView,
    TileTask,
};

/// Browser map backend contract.
pub trait Backend {
    fn submit(&self, task: TileTask);

    fn submit_labels(&self, task: BrowserLabelTask) {
        task.complete(Err("this map backend does not prepare labels".to_owned()));
    }

    fn submit_route(&self, task: BrowserRouteTask) {
        task.complete(Err("this map backend does not prepare routes".to_owned()));
    }
}

pub(in crate::activity::map_runtime) type Shared<T> = Rc<T>;

#[derive(Clone)]
pub(in crate::activity::map_runtime) struct SceneSlot(Rc<RefCell<Arc<MapSceneData>>>);

impl Default for SceneSlot {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Arc::new(MapSceneData::default()))))
    }
}

impl SceneSlot {
    pub(in crate::activity::map_runtime) fn publish(&self, scene: MapSceneData) {
        *self.0.borrow_mut() = Arc::new(scene);
    }

    pub(in crate::activity::map_runtime) fn load(&self) -> Arc<MapSceneData> {
        Arc::clone(&self.0.borrow())
    }
}

pub(in crate::activity::map_runtime) struct SurfaceRuntime(RefCell<SurfaceDriver>);

impl SurfaceRuntime {
    pub(in crate::activity::map_runtime) fn new(driver: SurfaceDriver) -> Self {
        Self(RefCell::new(driver))
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "native runtime takes ownership of queued views"
    )]
    pub(in crate::activity::map_runtime) fn submit(&self, view: SurfaceView) {
        self.0.borrow_mut().update(&view);
    }

    #[expect(
        clippy::unused_self,
        reason = "matches the native runtime's per-instance discard counter"
    )]
    pub(in crate::activity::map_runtime) const fn discarded(&self) -> u64 {
        0
    }

    #[cfg(test)]
    pub(in crate::activity::map_runtime) fn synchronize(&self) {}
}

#[derive(Clone, Default)]
pub(in crate::activity::map_runtime) struct ResponseQueue(Rc<RefCell<VecDeque<MapTileResponse>>>);

impl ResponseQueue {
    pub(in crate::activity::map_runtime) fn drain(&self) -> Vec<MapTileResponse> {
        self.0.borrow_mut().drain(..).collect()
    }

    pub(in crate::activity::map_runtime) fn push(&self, response: MapTileResponse) {
        self.0.borrow_mut().push_back(response);
    }
}
