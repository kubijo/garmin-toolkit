use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

use garmin_progress::{
    OperationStage, ProgressEvent, ProgressEventKind, ProgressScope, ProgressState, ProgressUnit,
};

const MAX_HISTORY_ENTRIES: usize = 2_048;

#[derive(Debug, Clone)]
pub(super) struct StageView {
    pub state: Option<ProgressState>,
    pub unit: ProgressUnit,
    pub label: String,
    pub path: Option<String>,
    pub completed: u64,
    pub total: Option<u64>,
    pub started_at: Option<Instant>,
    pub started_recorded_at: Option<SystemTime>,
    pub updated_at: Option<Instant>,
    pub updated_recorded_at: Option<SystemTime>,
    pub elapsed: Option<Duration>,
    pub aggregate_owned: bool,
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
            started_recorded_at: None,
            updated_at: None,
            updated_recorded_at: None,
            elapsed: None,
            aggregate_owned: false,
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
        self.updated_recorded_at = Some(event.recorded_at);
        match event.state {
            ProgressState::Started => {
                self.started_at = Some(now);
                self.started_recorded_at = Some(event.recorded_at);
                self.elapsed = None;
            }
            ProgressState::Advanced if self.started_at.is_none() => {
                self.started_at = Some(now);
                self.started_recorded_at = Some(event.recorded_at);
                self.elapsed = None;
            }
            ProgressState::Completed | ProgressState::Failed => {
                self.elapsed = self
                    .started_recorded_at
                    .and_then(|started| event.recorded_at.duration_since(started).ok());
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
    pub kind: ProgressEventKind,
    pub label: String,
    pub path: Option<String>,
    pub recorded_at: Option<SystemTime>,
    pub duration: Option<Duration>,
}

impl OperationView {
    pub fn initial(label: impl Into<String>) -> Self {
        Self {
            stage: None,
            state: None,
            kind: ProgressEventKind::Generic,
            label: label.into(),
            path: None,
            recorded_at: None,
            duration: None,
        }
    }

    pub fn from_event(event: &ProgressEvent) -> Self {
        Self {
            stage: Some(event.stage),
            state: Some(event.state),
            kind: event.kind,
            label: event.label.clone(),
            path: event.path.clone(),
            recorded_at: Some(event.recorded_at),
            duration: None,
        }
    }

    fn recorded(mut self, duration: Option<Duration>) -> Self {
        self.duration = duration;
        self
    }

    pub fn completed(label: impl Into<String>) -> Self {
        Self {
            state: Some(ProgressState::Completed),
            ..Self::initial(label)
        }
    }

    pub fn is_cached_verification(&self) -> bool {
        self.kind == ProgressEventKind::CachedChecksumVerified
            && self.state == Some(ProgressState::Completed)
            && self.path.is_some()
    }

    pub fn is_cached_download(&self) -> bool {
        self.kind == ProgressEventKind::CachedDownload
            && self.state == Some(ProgressState::Completed)
            && self.path.is_some()
    }

    fn continues_history_row(&self, previous: &Self) -> bool {
        self.is_cached_verification() && previous.is_cached_download() && self.path == previous.path
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
    pub history: VecDeque<OperationView>,
    pub history_rows_added: usize,
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
            history: VecDeque::new(),
            history_rows_added: 0,
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
            view.aggregate_owned = true;
        }
    }

    #[cfg(feature = "gallery")]
    pub fn active_mut(&mut self) -> impl Iterator<Item = &mut ItemView> {
        self.items.iter_mut().filter(|item| item.view.is_active())
    }

    pub fn apply(&mut self, event: &ProgressEvent) {
        let Some(stage_index) = self
            .stages
            .iter()
            .position(|(stage, _)| *stage == event.stage)
        else {
            if terminal(event.state) {
                self.push_history(OperationView::from_event(event).recorded(None));
            }
            return;
        };
        match &event.scope {
            ProgressScope::Stage => {
                let stage = &mut self.stages[stage_index].1;
                stage.aggregate_owned = true;
                stage.apply(event);
                self.current = OperationView::from_event(event);
                if terminal(event.state) {
                    let history = self.current.clone().recorded(stage.elapsed);
                    self.push_history(history);
                    for item in &mut self.items {
                        if item.stage == event.stage && item.view.is_active() {
                            item.view.state = Some(event.state);
                        }
                    }
                }
            }
            ProgressScope::Item { id } => {
                if !self.stages[stage_index].1.aggregate_owned {
                    self.stages[stage_index].1.apply(event);
                }
                self.apply_item(id, event);
            }
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
            let duration = item.view.elapsed.or_else(|| {
                item.view
                    .started_recorded_at
                    .and_then(|started| event.recorded_at.duration_since(started).ok())
            });
            self.push_history(
                OperationView {
                    path,
                    ..OperationView::from_event(event)
                }
                .recorded(duration),
            );
        }
    }

    fn push_history(&mut self, operation: OperationView) {
        let continues_row = self
            .history
            .back()
            .is_some_and(|previous| operation.continues_history_row(previous));
        if self.history.len() >= MAX_HISTORY_ENTRIES {
            self.history.pop_front();
        }
        self.history.push_back(operation);
        if !continues_row {
            self.history_rows_added = self.history_rows_added.saturating_add(1);
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
    use std::time::UNIX_EPOCH;

    fn event_at(
        recorded_at: SystemTime,
        stage: OperationStage,
        progress_state: ProgressState,
        label: &str,
    ) -> ProgressEvent {
        ProgressEvent {
            recorded_at,
            scope: ProgressScope::Stage,
            stage,
            state: progress_state,
            kind: ProgressEventKind::Generic,
            unit: stage.progress_unit(),
            label: label.to_owned(),
            path: None,
            completed: u64::from(progress_state == ProgressState::Completed),
            total: Some(1),
        }
    }

    #[test]
    fn history_uses_producer_event_time_and_duration() {
        let started = UNIX_EPOCH + Duration::from_secs(1_000);
        let completed = started + Duration::from_secs(7);
        let mut model = ProgressModel::new(&[OperationStage::Commit], "Preparing");

        model.apply(&event_at(
            started,
            OperationStage::Commit,
            ProgressState::Started,
            "Writing",
        ));
        model.apply(&event_at(
            completed,
            OperationStage::Commit,
            ProgressState::Completed,
            "Written",
        ));

        assert_eq!(model.history.len(), 1);
        assert_eq!(model.history[0].recorded_at, Some(completed));
        assert_eq!(model.history[0].duration, Some(Duration::from_secs(7)));
    }

    #[test]
    fn history_discards_oldest_entries_at_the_retention_limit() {
        let mut model = ProgressModel::new(&[], "Preparing");
        for index in 0..=MAX_HISTORY_ENTRIES {
            model.apply(&ProgressEvent {
                label: index.to_string(),
                ..event_at(
                    UNIX_EPOCH + Duration::from_secs(index as u64),
                    OperationStage::Inspect,
                    ProgressState::Completed,
                    "unused",
                )
            });
        }

        assert_eq!(model.history.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(model.history.front().unwrap().label, "1");
        assert_eq!(
            model.history.back().unwrap().label,
            MAX_HISTORY_ENTRIES.to_string()
        );
    }

    #[test]
    fn cached_download_and_verification_count_as_one_history_row() {
        let (progress, receiver) = ProgressReporter::channel();
        let mut model = ProgressModel::new(
            &[OperationStage::Download, OperationStage::Verify],
            "Starting",
        );
        progress
            .clone()
            .with_event_kind(ProgressEventKind::CachedDownload)
            .for_item("cached-map")
            .completed_with_path(
                OperationStage::Download,
                "Already cached",
                "Garmin/map.img",
                10,
                Some(10),
            );
        progress
            .with_event_kind(ProgressEventKind::CachedChecksumVerified)
            .for_item("cached-map")
            .completed_with_path(
                OperationStage::Verify,
                "Cached MD5 verified",
                "Garmin/map.img",
                10,
                Some(10),
            );
        for event in receiver.try_iter() {
            model.apply(&event);
        }

        assert_eq!(model.history.len(), 2);
        assert_eq!(model.history_rows_added, 1);
    }

    #[test]
    fn terminal_untracked_events_are_kept_as_history_only_rows() {
        let mut model = ProgressModel::new(&[OperationStage::Commit], "Preparing");
        let snapshot = event_at(
            UNIX_EPOCH + Duration::from_secs(2_000),
            OperationStage::Inspect,
            ProgressState::Completed,
            "Storage snapshot — Internal: 875 MB free of 31 GB",
        );

        model.apply(&snapshot);

        assert_eq!(model.stages.len(), 1);
        assert_eq!(model.history.len(), 1);
        assert_eq!(model.history[0].stage, Some(OperationStage::Inspect));
        assert!(model.history[0].label.starts_with("Storage snapshot"));
    }

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
            model.history.back().unwrap().path.as_deref(),
            Some("Garmin/first.img")
        );
    }

    #[test]
    fn terminal_reclamation_is_retained_with_path_time_and_duration() {
        let (progress, receiver) = ProgressReporter::channel();
        let removal = progress.for_item("recovery-remove:Garmin/obsolete.img");
        let mut model = ProgressModel::new(&[OperationStage::Commit], "Preparing");
        removal.started_with_path(
            OperationStage::Commit,
            "Removing obsolete file",
            "Garmin/obsolete.img",
            Some(8_000_000),
        );
        removal.completed_with_path(
            OperationStage::Commit,
            "Removed obsolete file; reclaimed 8.00 MB; free 875.36 MB → 883.36 MB",
            "Garmin/obsolete.img",
            8_000_000,
            Some(8_000_000),
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }

        assert_eq!(model.history.len(), 1);
        let entry = &model.history[0];
        assert_eq!(entry.state, Some(ProgressState::Completed));
        assert_eq!(entry.path.as_deref(), Some("Garmin/obsolete.img"));
        assert!(entry.label.contains("free 875.36 MB → 883.36 MB"));
        assert!(entry.recorded_at.is_some());
        assert!(entry.duration.is_some());
    }
}
