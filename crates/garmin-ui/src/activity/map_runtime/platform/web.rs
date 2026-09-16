use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use super::super::{BrowserLabelTask, BrowserRouteTask, MapTileResponse, TileTask};

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
