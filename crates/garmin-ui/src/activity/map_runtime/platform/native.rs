use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
};

use arc_swap::ArcSwap;

use super::super::{
    BrowserLabelTask, BrowserRouteTask, MapSceneData, MapTileResponse, SurfaceDriver, SurfaceView,
    TileCoordinates, TileData,
};
use crate::activity::native_latest::LatestWorker;

/// Native map backend contract.
pub trait Backend: Send + Sync {
    fn fetch(&self, coordinates: TileCoordinates, reply: TileReply);

    fn submit_labels(&self, task: BrowserLabelTask) {
        task.complete(Err("this map backend does not prepare labels".to_owned()));
    }

    fn submit_route(&self, task: BrowserRouteTask) {
        task.complete(Err("this map backend does not prepare routes".to_owned()));
    }
}

/// Return transport data on a worker thread; the runtime owns decoding and publication.
pub type TileReply = Box<dyn FnOnce(Result<TileData, String>) + Send>;

pub(in crate::activity::map_runtime) type Shared<T> = Arc<T>;

#[derive(Clone)]
pub(in crate::activity::map_runtime) struct SceneSlot(Arc<ArcSwap<MapSceneData>>);

impl Default for SceneSlot {
    fn default() -> Self {
        Self(Arc::new(ArcSwap::from_pointee(MapSceneData::default())))
    }
}

impl SceneSlot {
    pub(in crate::activity::map_runtime) fn publish(&self, scene: MapSceneData) {
        self.0.store(Arc::new(scene));
    }

    pub(in crate::activity::map_runtime) fn load(&self) -> Arc<MapSceneData> {
        self.0.load_full()
    }
}

pub(in crate::activity::map_runtime) struct SurfaceRuntime {
    worker: LatestWorker<SurfaceView>,
}

impl SurfaceRuntime {
    pub(in crate::activity::map_runtime) fn new(mut driver: SurfaceDriver) -> Self {
        Self {
            worker: LatestWorker::spawn("garmin-toolkit-map-surface", move |view| {
                driver.update(&view);
            }),
        }
    }

    pub(in crate::activity::map_runtime) fn submit(&self, view: SurfaceView) {
        let _ = self.worker.submit(view);
    }

    pub(in crate::activity::map_runtime) fn discarded(&self) -> u64 {
        self.worker.discarded()
    }

    #[cfg(test)]
    pub(in crate::activity::map_runtime) fn synchronize(&self) {
        self.worker.synchronize();
    }
}

#[derive(Clone, Default)]
pub(in crate::activity::map_runtime) struct ResponseQueue(Arc<Mutex<VecDeque<MapTileResponse>>>);

pub(in crate::activity::map_runtime) struct ResponseTarget(Weak<Mutex<VecDeque<MapTileResponse>>>);

impl ResponseTarget {
    pub(in crate::activity::map_runtime) fn upgrade(&self) -> Option<ResponseQueue> {
        self.0.upgrade().map(ResponseQueue)
    }
}

impl ResponseQueue {
    pub(in crate::activity::map_runtime) fn target(&self) -> ResponseTarget {
        ResponseTarget(Arc::downgrade(&self.0))
    }

    pub(in crate::activity::map_runtime) fn drain(&self) -> Vec<MapTileResponse> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    pub(in crate::activity::map_runtime) fn push(&self, response: MapTileResponse) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(response);
    }
}
