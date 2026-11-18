//! Built-in interaction workloads.

use std::time::Duration;

pub const SCENARIOS: &[&str] = &[
    "stationary-arrival",
    "warm-interaction",
    "activity-smoke",
    "responsive-layout",
];

#[derive(Clone)]
pub(super) enum Action {
    Resize {
        width: f32,
        height: f32,
    },
    Available,
    Reveal,
    Wait,
    Ready,
    Click,
    ClickWith(egui::Modifiers),
    SecondaryClick,
    Drag {
        x: f32,
        y: f32,
    },
    DragTo {
        destination: String,
        modifiers: egui::Modifiers,
        dwell_frames: usize,
    },
    DragPath(Vec<DragStop>),
    Wheel(f32),
    Scroll(f32),
    Observe(Duration),
    Value(String),
    Key {
        key: egui::Key,
        modifiers: egui::Modifiers,
    },
    Text(String),
}

#[derive(Clone)]
pub(super) struct DragStop {
    pub destination: String,
    pub modifiers: egui::Modifiers,
    pub dwell_frames: usize,
}

impl Action {
    pub(super) const fn uses_pointer(&self) -> bool {
        matches!(
            self,
            Self::Click
                | Self::ClickWith(_)
                | Self::SecondaryClick
                | Self::Drag { .. }
                | Self::DragTo { .. }
                | Self::DragPath(_)
                | Self::Wheel(_)
                | Self::Scroll(_)
        )
    }

    pub(super) const fn name(&self) -> &'static str {
        match self {
            Self::Resize { .. } => "resize",
            Self::Available => "assert_available",
            Self::Reveal => "scroll_into_view",
            Self::Wait => "wait",
            Self::Ready => "readiness",
            Self::Click | Self::ClickWith(_) => "click",
            Self::SecondaryClick => "secondary_click",
            Self::Drag { .. } => "drag",
            Self::DragTo { .. } => "drag_to",
            Self::DragPath(_) => "drag_path",
            Self::Wheel(_) => "zoom",
            Self::Scroll(_) => "scroll",
            Self::Observe(_) => "observe",
            Self::Value(_) => "assert",
            Self::Key { .. } => "key",
            Self::Text(_) => "text",
        }
    }
}

#[derive(Clone)]
pub(super) struct Step {
    pub phase: &'static str,
    pub target: String,
    pub action: Action,
    pub after: Duration,
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
        after: Duration::from_millis(200),
    })
    .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "The scenario registry keeps each named sequence together"
)]
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
            Duration::from_millis(100),
        );
    }
    add(
        "setup",
        "profile.0",
        Action::Wait,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "profile.0",
        Action::Click,
        Duration::from_millis(200),
    );
    add(
        "setup",
        "activity.list.toggle",
        Action::Wait,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "activity.list.toggle",
        Action::Click,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "activity.0",
        Action::Wait,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "activity.0",
        Action::Click,
        Duration::from_millis(200),
    );
    add(
        "setup",
        "activity.selection",
        Action::Value("0".into()),
        Duration::from_millis(100),
    );
    add(
        "setup",
        "activity.viewer",
        Action::Wait,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "activity.viewer",
        Action::Scroll(100_000.0),
        Duration::from_millis(300),
    );
    add(
        "setup",
        "map.fit",
        Action::Reveal,
        Duration::from_millis(100),
    );
    add(
        "setup",
        "map.fit",
        Action::Click,
        Duration::from_millis(200),
    );
    add("arrival", "map", Action::Ready, Duration::from_millis(200));
    if name == "responsive-layout" {
        steps.extend(responsive_steps());
        return Some(steps);
    }
    if name == "stationary-arrival" {
        add(
            "stationary",
            "map",
            Action::Observe(Duration::from_secs(8)),
            Duration::from_millis(0),
        );
    }
    if name != "stationary-arrival" {
        for _ in 0..4 {
            add(
                "warm",
                "map",
                Action::Drag { x: 0.7, y: 0.6 },
                Duration::from_millis(300),
            );
            add(
                "warm",
                "map",
                Action::Wheel(100.0),
                Duration::from_millis(500),
            );
            add(
                "warm",
                "map",
                Action::Wheel(-100.0),
                Duration::from_millis(500),
            );
            add("warm", "map.fit", Action::Click, Duration::from_millis(500));
        }
        add("return", "map", Action::Ready, Duration::from_millis(200));
    }
    if name == "activity-smoke" {
        steps.extend(activity_smoke_steps());
    }
    Some(steps)
}

#[expect(
    clippy::too_many_lines,
    reason = "The steps describe a single activity scenario in order"
)]
fn activity_smoke_steps() -> Vec<Step> {
    [
        (
            "playback",
            "playback.toggle",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "playback",
            "playback.toggle",
            Action::Value("playing".into()),
            Duration::from_millis(100),
        ),
        (
            "playback",
            "playback.speed.2",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "playback",
            "playback.speed",
            Action::Value("2×".into()),
            Duration::from_millis(200),
        ),
        (
            "playback",
            "playback.toggle",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "playback",
            "playback.toggle",
            Action::Value("stopped".into()),
            Duration::from_millis(100),
        ),
        (
            "lap",
            "activity.viewer",
            Action::Scroll(-100_000.0),
            Duration::from_millis(500),
        ),
        ("lap", "lap.0", Action::Click, Duration::from_millis(300)),
        (
            "lap",
            "activity.viewer",
            Action::Scroll(100_000.0),
            Duration::from_millis(300),
        ),
        (
            "lap",
            "map.full-activity",
            Action::Wait,
            Duration::from_millis(100),
        ),
        (
            "lap",
            "map.full-activity",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "chart",
            "activity.viewer",
            Action::Scroll(-320.0),
            Duration::from_millis(500),
        ),
        ("chart", "chart.0", Action::Wait, Duration::from_millis(100)),
        (
            "chart",
            "chart.0",
            Action::Drag { x: 0.75, y: 0.5 },
            Duration::from_millis(300),
        ),
        (
            "chart",
            "activity.viewer",
            Action::Scroll(320.0),
            Duration::from_millis(500),
        ),
        (
            "replacement",
            "activity.viewer",
            Action::Scroll(100_000.0),
            Duration::from_millis(300),
        ),
        (
            "replacement",
            "activity.list.toggle",
            Action::Click,
            Duration::from_millis(200),
        ),
        (
            "replacement",
            "activity.1",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "replacement",
            "activity.selection",
            Action::Value("1".into()),
            Duration::from_millis(100),
        ),
        // The second fixture has no GPS data.
        (
            "replacement",
            "map.empty",
            Action::Wait,
            Duration::from_millis(100),
        ),
        (
            "replacement",
            "map.empty",
            Action::Value("empty".into()),
            Duration::from_millis(100),
        ),
        (
            "restore",
            "activity.viewer",
            Action::Scroll(100_000.0),
            Duration::from_millis(300),
        ),
        (
            "restore",
            "activity.list.toggle",
            Action::Click,
            Duration::from_millis(200),
        ),
        (
            "restore",
            "activity.0",
            Action::Click,
            Duration::from_millis(300),
        ),
        (
            "restore",
            "activity.selection",
            Action::Value("0".into()),
            Duration::from_millis(100),
        ),
        ("restore", "map", Action::Ready, Duration::from_millis(200)),
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
        after: Duration::from_millis(100),
    }];
    for (phase, width, height) in [("narrow", 720.0, 640.0), ("wide", 1100.0, 720.0)] {
        steps.push(Step {
            phase,
            target: "viewport".into(),
            action: Action::Resize { width, height },
            after: Duration::from_millis(100),
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
                after: Duration::from_millis(100),
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
            after: Duration::from_millis(100),
        })
        .collect()
}
