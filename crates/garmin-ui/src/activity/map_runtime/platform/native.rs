use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::super::{BrowserLabelTask, BrowserRouteTask, MapTileResponse, TileTask};

/// Native map backend contract.
pub trait Backend: Send + Sync {
    fn submit(&self, task: TileTask);

    fn submit_labels(&self, task: BrowserLabelTask) {
        task.complete(Err("this map backend does not prepare labels".to_owned()));
    }

    fn submit_route(&self, task: BrowserRouteTask) {
        task.complete(Err("this map backend does not prepare routes".to_owned()));
    }
}

pub(in crate::activity::map_runtime) type Shared<T> = Arc<T>;

#[derive(Clone, Default)]
pub(in crate::activity::map_runtime) struct ResponseQueue(Arc<Mutex<VecDeque<MapTileResponse>>>);

impl ResponseQueue {
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
