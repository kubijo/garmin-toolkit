//! Owner-published compact state, independent of rendering and HTTP transports.

use egui::{Context, Id};
use garmin_model::diagnostics::{Observation, Update};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

#[cfg(test)]
mod tests;

type Sink = Arc<dyn Fn(Observation) + Send + Sync>;

/// A collection handle whose sink can be released without dropping the UI context.
#[derive(Clone)]
pub struct Observer(Arc<Mutex<Option<Sink>>>);

impl Observer {
    pub fn stop(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// Install a host-owned sink.
/// Disabled hosts do not collect diagnostic state.
pub fn install(context: &Context, sink: impl Fn(Observation) + Send + Sync + 'static) -> Observer {
    let observer = Observer(Arc::new(Mutex::new(Some(Arc::new(sink)))));
    context.data_mut(|data| {
        data.insert_temp(Id::new("diagnostic-observer"), observer.clone());
    });
    observer
}

pub fn publish(context: &Context, observation: Observation) {
    let observer = context.data(|data| data.get_temp::<Observer>(Id::new("diagnostic-observer")));
    let sink = observer.and_then(|observer| {
        observer
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    });
    if let Some(sink) = sink {
        sink(observation);
    }
}

#[derive(Clone, Default)]
pub struct Journal(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    revision: u64,
    lost_through: u64,
    incomplete: bool,
    current: BTreeMap<(String, String), Observation>,
    changes: VecDeque<(u64, Observation)>,
}

impl Journal {
    pub fn publish(&self, observation: Observation) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if observation.kind == "window" && observation.removed {
            let removed: Vec<_> = state
                .current
                .values()
                .filter(|item| item.window == observation.window && item.kind != "window")
                .map(|item| {
                    let mut item = item.clone();
                    item.removed = true;
                    item
                })
                .collect();
            for item in removed {
                state.push(item);
            }
        }
        state.push(observation);
    }

    #[must_use]
    pub fn since(&self, after: u64) -> Update {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        Update {
            revision: state.revision,
            state: state.current.values().cloned().collect(),
            changes: state
                .changes
                .iter()
                .filter(|(revision, _)| *revision > after)
                .map(|(_, item)| item.clone())
                .collect(),
            gap: after < state.lost_through || after > state.revision,
            incomplete: state.incomplete,
        }
    }
}

impl State {
    fn push(&mut self, observation: Observation) {
        let key = (observation.kind.clone(), observation.window.clone());
        if self.current.get(&key) == Some(&observation)
            || (observation.removed && !self.current.contains_key(&key))
        {
            return;
        }
        if !observation.valid() || (!self.current.contains_key(&key) && self.current.len() >= 128) {
            if !self.incomplete {
                self.incomplete = true;
                self.revision += 1;
            }
            return;
        }
        self.revision += 1;
        if observation.removed {
            self.current.remove(&key);
        } else {
            self.current.insert(key, observation.clone());
        }
        // Coalesce only adjacent progress from the same run phase;
        // terminal and lifecycle changes remain ordered until the explicit retention boundary.
        if observation.kind == "automation"
            && observation
                .fields
                .get("state")
                .is_some_and(|value| value == "running")
            && self.changes.back().is_some_and(|(_, previous)| {
                previous.kind == observation.kind
                    && previous.window == observation.window
                    && previous.fields.get("state") == observation.fields.get("state")
                    && previous.fields.get("scenario") == observation.fields.get("scenario")
                    && previous.fields.get("phase") == observation.fields.get("phase")
            })
        {
            self.changes.pop_back();
        }
        self.changes.push_back((self.revision, observation));
        while self.changes.len() > 128 {
            if let Some((revision, _)) = self.changes.pop_front() {
                self.lost_through = revision;
            }
        }
    }
}

#[cfg(feature = "automation")]
pub(crate) fn automation(
    context: &Context,
    window: &str,
    report: Option<&crate::automation::Report>,
) {
    let Some(report) = report else {
        return;
    };
    let mut fields = BTreeMap::from([
        ("scenario".into(), report.scenario.clone()),
        ("state".into(), report.state.clone()),
        ("phase".into(), report.phase.clone()),
        ("completed".into(), report.completed.to_string()),
        ("total".into(), report.total.to_string()),
    ]);
    if let Some(failure) = &report.failure {
        fields.insert("failure".into(), failure.chars().take(512).collect());
    }
    publish(
        context,
        Observation {
            kind: "automation".into(),
            window: window.into(),
            fields,
            removed: false,
        },
    );
}
