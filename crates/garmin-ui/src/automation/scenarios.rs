//! Built-in interaction workloads.

pub const SCENARIOS: &[&str] = &[
    "stationary-arrival",
    "warm-interaction",
    "activity-smoke",
    "responsive-layout",
];

#[derive(Clone)]
pub(super) enum Action {
    Resize { width: f32, height: f32 },
    Available,
    Wait,
    Ready,
    Click,
    Drag { x: f32, y: f32 },
    Wheel(f32),
    Scroll(f32),
    Observe(f64),
    Value(String),
    Key(egui::Key),
    Text(String),
}

impl Action {
    pub(super) const fn name(&self) -> &'static str {
        match self {
            Self::Resize { .. } => "resize",
            Self::Available => "assert_available",
            Self::Wait => "wait",
            Self::Ready => "readiness",
            Self::Click => "click",
            Self::Drag { .. } => "drag",
            Self::Wheel(_) => "zoom",
            Self::Scroll(_) => "scroll",
            Self::Observe(_) => "observe",
            Self::Value(_) => "assert",
            Self::Key(_) => "key",
            Self::Text(_) => "text",
        }
    }
}

#[derive(Clone)]
pub(super) struct Step {
    pub phase: &'static str,
    pub target: String,
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
        target: target.into(),
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
    let mut add = |phase, target: &str, action, after| {
        steps.push(Step {
            phase,
            target: target.into(),
            action,
            after,
        });
    };
    if name == "responsive-layout" {
        add(
            "setup",
            "viewport",
            Action::Resize {
                width: 1100.0,
                height: 720.0,
            },
            0.1,
        );
    }
    add("setup", "profile.0", Action::Wait, 0.1);
    add("setup", "profile.0", Action::Click, 0.2);
    add("setup", "activity.0", Action::Wait, 0.1);
    add("setup", "activity.0", Action::Click, 0.2);
    add("setup", "activity.0", Action::Value("selected".into()), 0.1);
    add("setup", "map.fit", Action::Wait, 0.1);
    add("setup", "map.fit", Action::Click, 0.2);
    add("arrival", "map", Action::Ready, 0.2);
    if name == "responsive-layout" {
        steps.extend(responsive_steps());
        return Some(steps);
    }
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
        add(
            "playback",
            "playback.toggle",
            Action::Value("playing".into()),
            0.1,
        );
        add("playback", "playback.speed.2", Action::Click, 0.3);
        add(
            "playback",
            "playback.speed",
            Action::Value("2×".into()),
            0.2,
        );
        add("playback", "playback.toggle", Action::Click, 0.3);
        add(
            "playback",
            "playback.toggle",
            Action::Value("stopped".into()),
            0.1,
        );
        add("lap", "lap.0", Action::Click, 0.3);
        add("lap", "map.full-activity", Action::Wait, 0.1);
        add("lap", "map.full-activity", Action::Click, 0.3);
        add("chart", "activity.viewer", Action::Scroll(-320.0), 0.5);
        add("chart", "chart.0", Action::Wait, 0.1);
        add("chart", "chart.0", Action::Drag { x: 0.75, y: 0.5 }, 0.3);
        add("chart", "activity.viewer", Action::Scroll(320.0), 0.5);
        add("replacement", "activity.1", Action::Click, 0.3);
        add(
            "replacement",
            "activity.1",
            Action::Value("selected".into()),
            0.1,
        );
        // The second fixture has no GPS data.
        add("replacement", "map.empty", Action::Wait, 0.1);
        add(
            "replacement",
            "map.empty",
            Action::Value("empty".into()),
            0.1,
        );
        add("restore", "activity.0", Action::Click, 0.3);
        add(
            "restore",
            "activity.0",
            Action::Value("selected".into()),
            0.1,
        );
        add("restore", "map", Action::Ready, 0.2);
    }
    Some(steps)
}

fn responsive_steps() -> Vec<Step> {
    let mut steps = vec![Step {
        phase: "setup",
        target: "playback.speed.2".into(),
        action: Action::Click,
        after: 0.1,
    }];
    for (phase, width, height) in [("narrow", 720.0, 640.0), ("wide", 1100.0, 720.0)] {
        steps.push(Step {
            phase,
            target: "viewport".into(),
            action: Action::Resize { width, height },
            after: 0.1,
        });
        steps.push(Step {
            phase,
            target: "map".into(),
            action: Action::Ready,
            after: 0.1,
        });
        steps.extend(responsive_selection_steps(phase));
        for (target, action) in [
            ("playback.speed", Action::Value("2×".into())),
            ("profile.toggle", Action::Available),
            ("map.fit", Action::Available),
            ("playback.toggle", Action::Available),
            ("playback.toggle", Action::Value("stopped".into())),
            ("playback.toggle", Action::Click),
            ("playback.toggle", Action::Value("playing".into())),
            ("playback.toggle", Action::Click),
            ("playback.toggle", Action::Value("stopped".into())),
        ] {
            steps.push(Step {
                phase,
                target: target.into(),
                action,
                after: 0.1,
            });
        }
    }
    steps
}

pub(super) fn responsive_selection_steps(phase: &'static str) -> Vec<Step> {
    let mut actions = Vec::new();
    if phase == "narrow" {
        actions.extend([
            ("activity.list.toggle", Action::Click),
            ("activity.list.toggle", Action::Value("open".into())),
            ("activity.0", Action::Wait),
        ]);
    }
    actions.push(("activity.0", Action::Value("selected".into())));
    if phase == "narrow" {
        actions.extend([
            ("activity.list.toggle", Action::Click),
            ("activity.list.toggle", Action::Value("closed".into())),
            ("activity.details.toggle", Action::Click),
            ("activity.details.toggle", Action::Value("open".into())),
            ("activity.details.toggle", Action::Click),
            ("activity.details.toggle", Action::Value("closed".into())),
        ]);
    }
    actions
        .into_iter()
        .map(|(target, action)| Step {
            phase,
            target: target.into(),
            action,
            after: 0.1,
        })
        .collect()
}
