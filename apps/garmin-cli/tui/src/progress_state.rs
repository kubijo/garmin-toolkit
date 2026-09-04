use std::time::{Duration, Instant};

use garmin_progress::{OperationStage, ProgressEvent, ProgressScope, ProgressState, ProgressUnit};

#[derive(Debug, Clone)]
pub(super) struct StageView {
    pub state: Option<ProgressState>,
    pub unit: ProgressUnit,
    pub label: String,
    pub path: Option<String>,
    pub completed: u64,
    pub total: Option<u64>,
    pub started_at: Option<Instant>,
    pub updated_at: Option<Instant>,
    pub elapsed: Option<Duration>,
}

impl Default for StageView {
    fn default() -> Self {
        Self {
            state: None,
            unit: ProgressUnit::default(),
            label: "Waiting".to_owned(),
            path: None,
            completed: 0,
            total: None,
            started_at: None,
            updated_at: None,
            elapsed: None,
        }
    }
}

impl StageView {
    fn apply(&mut self, event: &ProgressEvent) {
        self.state = Some(event.state);
        self.unit = event.unit;
        event.label.clone_into(&mut self.label);
        self.path.clone_from(&event.path);
        self.completed = event.completed;
        self.total = event.total;
        let now = Instant::now();
        self.updated_at = Some(now);
        match event.state {
            ProgressState::Started => {
                self.started_at = Some(now);
                self.elapsed = None;
            }
            ProgressState::Advanced if self.started_at.is_none() => {
                self.started_at = Some(now);
                self.elapsed = None;
            }
            ProgressState::Completed | ProgressState::Failed => {
                self.elapsed = self.started_at.map(|started| started.elapsed());
            }
            ProgressState::Advanced => {}
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            Some(ProgressState::Started | ProgressState::Advanced)
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct OperationView {
    pub stage: Option<OperationStage>,
    pub state: Option<ProgressState>,
    pub label: String,
    pub path: Option<String>,
}

impl OperationView {
    pub fn initial(label: impl Into<String>) -> Self {
        Self {
            stage: None,
            state: None,
            label: label.into(),
            path: None,
        }
    }

    pub fn from_event(event: &ProgressEvent) -> Self {
        Self {
            stage: Some(event.stage),
            state: Some(event.state),
            label: event.label.clone(),
            path: event.path.clone(),
        }
    }

    pub fn completed(label: impl Into<String>) -> Self {
        Self {
            state: Some(ProgressState::Completed),
            ..Self::initial(label)
        }
    }
}

#[derive(Debug)]
pub(super) struct ItemView {
    pub id: String,
    pub stage: OperationStage,
    pub view: StageView,
}

pub(super) struct ProgressModel {
    pub stages: Vec<(OperationStage, StageView)>,
    pub current: OperationView,
    pub history: Vec<OperationView>,
    items: Vec<ItemView>,
}

impl ProgressModel {
    pub fn new(stages: &[OperationStage], initial: &str) -> Self {
        Self {
            stages: stages
                .iter()
                .map(|stage| (*stage, StageView::default()))
                .collect(),
            current: OperationView::initial(initial),
            history: Vec::new(),
            items: Vec::new(),
        }
    }

    pub fn active(&self) -> impl Iterator<Item = &ItemView> {
        self.items.iter().filter(|item| item.view.is_active())
    }

    pub fn stage_is_active(&self, stage: OperationStage) -> bool {
        self.stages
            .iter()
            .any(|(candidate, view)| *candidate == stage && view.is_active())
    }

    pub fn item_completion(&self, stage: OperationStage) -> (usize, usize) {
        let completed = self
            .items
            .iter()
            .filter(|item| item.stage == stage && item.view.state == Some(ProgressState::Completed))
            .count();
        (completed, self.items.len())
    }

    pub fn set_initial_completion(&mut self, stage: OperationStage, label: &str) {
        if let Some((_, view)) = self
            .stages
            .iter_mut()
            .find(|(candidate, _)| *candidate == stage)
        {
            view.state = Some(ProgressState::Completed);
            view.unit = stage.progress_unit();
            label.clone_into(&mut view.label);
            view.completed = 0;
            view.total = None;
        }
    }

    #[cfg(feature = "gallery")]
    pub fn active_mut(&mut self) -> impl Iterator<Item = &mut ItemView> {
        self.items.iter_mut().filter(|item| item.view.is_active())
    }

    pub fn apply(&mut self, event: &ProgressEvent) {
        let Some((_, stage)) = self
            .stages
            .iter_mut()
            .find(|(stage, _)| *stage == event.stage)
        else {
            return;
        };
        match &event.scope {
            ProgressScope::Stage => {
                stage.apply(event);
                self.current = OperationView::from_event(event);
                if terminal(event.state) {
                    self.history.push(self.current.clone());
                    for item in &mut self.items {
                        if item.stage == event.stage && item.view.is_active() {
                            item.view.state = Some(event.state);
                        }
                    }
                }
            }
            ProgressScope::Item { id } => self.apply_item(id, event),
        }
    }

    fn apply_item(&mut self, id: &str, event: &ProgressEvent) {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .unwrap_or_else(|| {
                self.items.push(ItemView {
                    id: id.to_owned(),
                    stage: event.stage,
                    view: StageView::default(),
                });
                self.items.len() - 1
            });
        let item = &mut self.items[index];
        let diagnostic_changed = event.state == ProgressState::Advanced
            && item.stage == event.stage
            && item.view.state.is_some()
            && item.view.label != event.label;
        let path = event.path.clone().or_else(|| item.view.path.clone());
        if item.stage != event.stage {
            item.stage = event.stage;
            item.view = StageView::default();
        }
        item.view.apply(event);
        item.view.path.clone_from(&path);
        if terminal(event.state) || diagnostic_changed {
            self.history.push(OperationView {
                path,
                ..OperationView::from_event(event)
            });
        }
    }
}

const fn terminal(state: ProgressState) -> bool {
    matches!(state, ProgressState::Completed | ProgressState::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_progress::ProgressReporter;

    #[test]
    fn initial_completion_resolves_a_known_skipped_stage_without_timing_noise() {
        let mut model = ProgressModel::new(&[OperationStage::Backup], "Preparing");

        model.set_initial_completion(
            OperationStage::Backup,
            "Skipped by user; automatic rollback is unavailable",
        );

        let backup = &model.stages[0].1;
        assert_eq!(backup.state, Some(ProgressState::Completed));
        assert_eq!(
            backup.label,
            "Skipped by user; automatic rollback is unavailable"
        );
        assert_eq!(backup.completed, 0);
        assert_eq!(backup.total, None);
        assert_eq!(backup.started_at, None);
        assert_eq!(backup.elapsed, None);

        let (progress, receiver) = ProgressReporter::channel();
        progress.completed(
            OperationStage::Backup,
            "Skipped by user; automatic rollback is unavailable",
            0,
            None,
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        let backup = &model.stages[0].1;
        assert_eq!(backup.started_at, None);
        assert_eq!(backup.elapsed, None);
    }

    #[test]
    fn interleaved_items_keep_identity_without_overwriting_stage_totals() {
        let (progress, receiver) = ProgressReporter::channel();
        let first = progress.for_item("first");
        let second = progress.for_item("second");
        let mut model = ProgressModel::new(
            &[OperationStage::Download, OperationStage::Verify],
            "Preparing",
        );
        progress.started(OperationStage::Download, "Downloading two files", Some(300));
        first.started_with_path(
            OperationStage::Download,
            "Downloading",
            "Garmin/first.img",
            Some(100),
        );
        second.started_with_path(
            OperationStage::Download,
            "Downloading",
            "Garmin/second.img",
            Some(200),
        );
        for amount in [10, 20, 40] {
            second.advanced(
                OperationStage::Download,
                "Downloading",
                amount * 2,
                Some(200),
            );
            first.advanced(OperationStage::Download, "Downloading", amount, Some(100));
        }
        progress.advanced(
            OperationStage::Download,
            "Downloading two files",
            120,
            Some(300),
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        let rows = model.active().collect::<Vec<_>>();
        assert_eq!(
            rows.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(
            rows.iter()
                .map(|item| item.view.completed)
                .collect::<Vec<_>>(),
            [40, 80]
        );
        assert_eq!(model.stages[0].1.completed, 120);
        assert_eq!(model.stages[0].1.total, Some(300));
        assert!(model.stages[0].1.path.is_none());
        assert!(model.history.is_empty());

        second.advanced(
            OperationStage::Download,
            "Trying fallback host",
            80,
            Some(200),
        );
        second.advanced(
            OperationStage::Download,
            "Trying fallback host",
            80,
            Some(200),
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        assert_eq!(model.history.len(), 1);
        assert_eq!(model.history[0].path.as_deref(), Some("Garmin/second.img"));

        first.completed(OperationStage::Download, "Downloaded", 100, Some(100));
        first.started(OperationStage::Verify, "Checking checksum", Some(100));
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        let rows = model.active().collect::<Vec<_>>();
        assert_eq!(rows[0].id, "first");
        assert_eq!(rows[0].stage, OperationStage::Verify);
        assert_eq!(rows[0].view.path.as_deref(), Some("Garmin/first.img"));
        assert_eq!(rows[1].id, "second");
        assert_eq!(model.stages[0].1.state, Some(ProgressState::Advanced));

        first.failed(OperationStage::Verify, "Checksum mismatch");
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        assert_eq!(
            model
                .active()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
        assert_eq!(
            model.history.last().unwrap().path.as_deref(),
            Some("Garmin/first.img")
        );
    }
}
