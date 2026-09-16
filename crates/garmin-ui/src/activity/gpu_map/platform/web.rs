use std::sync::{Arc, Mutex};

use super::super::{
    BrowserLabelTask, BrowserRouteTask, GpuTile, LabelResult, LabelTask, PreparedGpuTile,
    RouteResult, RouteTask, UploadContext, UploadStats, Vertex, VisibleTile,
};
use crate::activity::map_runtime::Backend;

const BROWSER_UPLOAD_BUDGET_BYTES: usize = 8 * 1024 * 1024;

pub(in crate::activity) struct Executor {
    context: Arc<UploadContext>,
    label_results: Arc<Mutex<std::collections::VecDeque<LabelResult>>>,
    route_results: Arc<Mutex<std::collections::VecDeque<RouteResult>>>,
}

impl Executor {
    pub(in crate::activity) fn new(context: Arc<UploadContext>) -> Self {
        Self {
            context,
            label_results: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            route_results: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub(in crate::activity) fn upload_visible(&self, visible: &[VisibleTile]) -> UploadStats {
        let pending = visible
            .iter()
            .enumerate()
            .filter(|(_index, tile)| needs_upload(&tile.tile))
            .map(|(index, tile)| (index, upload_bytes(&tile.tile)))
            .collect::<Vec<_>>();
        let queued_bytes = pending.iter().map(|(_index, bytes)| bytes).sum();
        let costs = pending
            .iter()
            .map(|(_index, bytes)| *bytes)
            .collect::<Vec<_>>();
        let selected = select_uploads(&costs, BROWSER_UPLOAD_BUDGET_BYTES);
        let mut uploaded_bytes = 0;
        for pending_index in selected {
            let index = pending[pending_index].0;
            let tile = &visible[index].tile;
            let bytes = upload_bytes(tile);
            let gpu = Arc::new(GpuTile::new(
                &self.context,
                visible[index].id,
                Arc::clone(&tile.mesh),
            ));
            tile.gpu.store(Some(gpu));
            uploaded_bytes += bytes;
        }
        UploadStats {
            queued_bytes,
            uploaded_bytes,
        }
    }

    pub(in crate::activity) fn poll_label(&self) -> Option<LabelResult> {
        self.label_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    pub(in crate::activity) fn schedule_label(
        &self,
        task: LabelTask,
        backend: &dyn Backend,
    ) -> (Option<LabelResult>, bool) {
        backend.submit_labels(BrowserLabelTask {
            task,
            results: Arc::clone(&self.label_results),
        });
        (None, false)
    }

    pub(in crate::activity) fn poll_route(&self) -> Option<RouteResult> {
        self.route_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    pub(in crate::activity) fn schedule_route(&self, task: RouteTask, backend: &dyn Backend) {
        backend.submit_route(BrowserRouteTask {
            task,
            results: Arc::clone(&self.route_results),
            upload: Arc::clone(&self.context),
        });
    }
}

fn upload_bytes(tile: &PreparedGpuTile) -> usize {
    tile.mesh
        .vertices
        .len()
        .saturating_mul(std::mem::size_of::<Vertex>())
        .saturating_add(
            tile.mesh
                .indices
                .len()
                .saturating_mul(std::mem::size_of::<u32>()),
        )
}

fn needs_upload(tile: &PreparedGpuTile) -> bool {
    !tile.mesh.indices.is_empty() && tile.gpu.load().is_none()
}

fn select_uploads(costs: &[usize], budget: usize) -> Vec<usize> {
    let mut remaining = budget;
    costs
        .iter()
        .enumerate()
        .filter_map(|(index, cost)| {
            if *cost > remaining {
                return None;
            }
            remaining -= *cost;
            Some(index)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::select_uploads;

    #[test]
    fn upload_selection_never_exceeds_the_frame_budget() {
        let costs = [3, 5, 2, 7];
        let selected = select_uploads(&costs, 7);
        let uploaded = selected.iter().map(|index| costs[*index]).sum::<usize>();

        assert_eq!(selected, [0, 2]);
        assert!(uploaded <= 7);
    }
}
