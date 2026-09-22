//! Built-in interaction workloads.

pub const SCENARIOS: &[&str] = &["stationary-arrival", "warm-interaction", "activity-smoke"];

#[derive(Clone, Copy)]
pub(super) enum Action {
    Wait,
    Ready,
    Click,
    Drag { x: f32, y: f32 },
    Wheel(f32),
    Scroll(f32),
    Observe(f64),
    Value(&'static str),
}

impl Action {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Ready => "readiness",
            Self::Click => "click",
            Self::Drag { .. } => "drag",
            Self::Wheel(_) => "zoom",
            Self::Scroll(_) => "scroll",
            Self::Observe(_) => "observe",
            Self::Value(_) => "assert",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub phase: &'static str,
    pub target: &'static str,
    pub action: Action,
    pub after: f64,
}

pub(super) fn return_to_chooser(menu_open: bool) -> Vec<Step> {
    [
        ("profile.toggle", Action::Wait),
        ("profile.toggle", Action::Click),
        ("profile.logout", Action::Wait),
        ("profile.logout", Action::Click),
        ("profile.0", Action::Wait),
    ]
    .into_iter()
    .skip(if menu_open { 2 } else { 0 })
    .map(|(target, action)| Step {
        phase: "reset",
        target,
        action,
        after: 0.2,
    })
    .collect()
}

pub(super) fn steps(name: &str) -> Option<Vec<Step>> {
    if !SCENARIOS.contains(&name) {
        return None;
    }
    let mut steps = Vec::new();
    let mut add = |phase, target, action, after| {
        steps.push(Step {
            phase,
            target,
            action,
            after,
        });
    };
    add("setup", "profile.0", Action::Wait, 0.1);
    add("setup", "profile.0", Action::Click, 0.2);
    add("setup", "activity.0", Action::Wait, 0.1);
    add("setup", "activity.0", Action::Click, 0.2);
    add("setup", "activity.0", Action::Value("selected"), 0.1);
    add("setup", "map.fit", Action::Wait, 0.1);
    add("setup", "map.fit", Action::Click, 0.2);
    add("arrival", "map", Action::Ready, 0.2);
    if name == "stationary-arrival" {
        add("stationary", "map", Action::Observe(8.0), 0.0);
    }
    if name != "stationary-arrival" {
        for _ in 0..4 {
            add("warm", "map", Action::Drag { x: 0.7, y: 0.6 }, 0.3);
            add("warm", "map", Action::Wheel(100.0), 0.5);
            add("warm", "map", Action::Wheel(-100.0), 0.5);
            add("warm", "map.fit", Action::Click, 0.5);
        }
        add("return", "map", Action::Ready, 0.2);
    }
    if name == "activity-smoke" {
        add("playback", "playback.toggle", Action::Click, 0.3);
        add("playback", "playback.toggle", Action::Value("playing"), 0.1);
        add("playback", "playback.speed.2", Action::Click, 0.3);
        add("playback", "playback.speed", Action::Value("2×"), 0.2);
        add("playback", "playback.toggle", Action::Click, 0.3);
        add("playback", "playback.toggle", Action::Value("stopped"), 0.1);
        add("lap", "lap.0", Action::Click, 0.3);
        add("lap", "map.full-activity", Action::Wait, 0.1);
        add("lap", "map.full-activity", Action::Click, 0.3);
        add("chart", "activity.viewer", Action::Scroll(-320.0), 0.5);
        add("chart", "chart.0", Action::Wait, 0.1);
        add("chart", "chart.0", Action::Drag { x: 0.75, y: 0.5 }, 0.3);
        add("chart", "activity.viewer", Action::Scroll(320.0), 0.5);
        add("replacement", "activity.1", Action::Click, 0.3);
        add("replacement", "activity.1", Action::Value("selected"), 0.1);
        // The second fixture has no GPS data.
        add("replacement", "map.empty", Action::Wait, 0.1);
        add("replacement", "map.empty", Action::Value("empty"), 0.1);
        add("restore", "activity.0", Action::Click, 0.3);
        add("restore", "activity.0", Action::Value("selected"), 0.1);
        add("restore", "map", Action::Ready, 0.2);
    }
    Some(steps)
}
