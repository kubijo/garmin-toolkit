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
    pub(super) const fn uses_pointer(&self) -> bool {
        matches!(
            self,
            Self::Click | Self::Drag { .. } | Self::Wheel(_) | Self::Scroll(_)
        )
    }

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
    add("setup", "activity.list.toggle", Action::Wait, 0.1);
    add("setup", "activity.list.toggle", Action::Click, 0.1);
    add("setup", "activity.0", Action::Wait, 0.1);
    add("setup", "activity.0", Action::Click, 0.2);
    add(
        "setup",
        "activity.selection",
        Action::Value("0".into()),
        0.1,
    );
    add("setup", "activity.viewer", Action::Wait, 0.1);
    add("setup", "activity.viewer", Action::Scroll(100_000.0), 0.3);
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
        steps.extend(activity_smoke_steps());
    }
    Some(steps)
}

fn activity_smoke_steps() -> Vec<Step> {
    [
        ("playback", "playback.toggle", Action::Click, 0.3),
        (
            "playback",
            "playback.toggle",
            Action::Value("playing".into()),
            0.1,
        ),
        ("playback", "playback.speed.2", Action::Click, 0.3),
        (
            "playback",
            "playback.speed",
            Action::Value("2×".into()),
            0.2,
        ),
        ("playback", "playback.toggle", Action::Click, 0.3),
        (
            "playback",
            "playback.toggle",
            Action::Value("stopped".into()),
            0.1,
        ),
        ("lap", "activity.viewer", Action::Scroll(-100_000.0), 0.5),
        ("lap", "lap.0", Action::Click, 0.3),
        ("lap", "activity.viewer", Action::Scroll(100_000.0), 0.3),
        ("lap", "map.full-activity", Action::Wait, 0.1),
        ("lap", "map.full-activity", Action::Click, 0.3),
        ("chart", "activity.viewer", Action::Scroll(-320.0), 0.5),
        ("chart", "chart.0", Action::Wait, 0.1),
        ("chart", "chart.0", Action::Drag { x: 0.75, y: 0.5 }, 0.3),
        ("chart", "activity.viewer", Action::Scroll(320.0), 0.5),
        (
            "replacement",
            "activity.viewer",
            Action::Scroll(100_000.0),
            0.3,
        ),
        ("replacement", "activity.list.toggle", Action::Click, 0.2),
        ("replacement", "activity.1", Action::Click, 0.3),
        (
            "replacement",
            "activity.selection",
            Action::Value("1".into()),
            0.1,
        ),
        // The second fixture has no GPS data.
        ("replacement", "map.empty", Action::Wait, 0.1),
        (
            "replacement",
            "map.empty",
            Action::Value("empty".into()),
            0.1,
        ),
        ("restore", "activity.viewer", Action::Scroll(100_000.0), 0.3),
        ("restore", "activity.list.toggle", Action::Click, 0.2),
        ("restore", "activity.0", Action::Click, 0.3),
        (
            "restore",
            "activity.selection",
            Action::Value("0".into()),
            0.1,
        ),
        ("restore", "map", Action::Ready, 0.2),
    ]
    .into_iter()
    .map(|(phase, target, action, after)| Step {
        phase,
        target: target.into(),
        action,
        after,
    })
    .collect()
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
        steps.extend(responsive_selection_steps(phase));
        for (target, action) in [
            (
                if phase == "narrow" {
                    "activity.selection"
                } else {
                    "activity.viewer"
                },
                Action::Scroll(if phase == "narrow" { -460.0 } else { 100_000.0 }),
            ),
            ("map", Action::Ready),
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
    let actions = [
        ("activity.viewer", Action::Scroll(100_000.0)),
        ("activity.list.toggle", Action::Click),
        ("activity.0", Action::Wait),
        ("activity.0", Action::Value("selected".into())),
        ("activity.list.close", Action::Click),
        ("activity.list.toggle", Action::Value("closed".into())),
        ("activity.selection", Action::Value("0".into())),
    ];
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
