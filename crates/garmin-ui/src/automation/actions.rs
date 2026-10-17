//! Parsing for individual actions and ordered functional checks.
use super::{Action, Step, scenarios::DragStop};
use serde_json::Value;

pub(super) fn parse(argument: &Value) -> Result<Vec<Step>, String> {
    let kind = argument["kind"].as_str().ok_or("action requires kind")?;
    let target = if kind == "resize" {
        "viewport"
    } else {
        argument["target"]
            .as_str()
            .filter(|target| !target.is_empty() && target.len() <= 256)
            .ok_or("action requires a semantic target of 1–256 bytes")?
    };
    let action = match kind {
        "resize" => Action::Resize {
            width: dimension(argument, "width")?,
            height: dimension(argument, "height")?,
        },
        "assert_available" => Action::Available,
        "assert_value" => Action::Value(text(argument, "value")?),
        "wait" => Action::Wait,
        "click" => {
            let modifiers = modifier(argument)?;
            if modifiers == egui::Modifiers::NONE {
                Action::Click
            } else {
                Action::ClickWith(modifiers)
            }
        }
        "secondary_click" => Action::SecondaryClick,
        "drag" => Action::Drag {
            x: coordinate(argument, "x")?,
            y: coordinate(argument, "y")?,
        },
        "drag_to" => Action::DragTo {
            destination: argument["destination"]
                .as_str()
                .filter(|destination| !destination.is_empty() && destination.len() <= 256)
                .ok_or("drag_to requires a semantic destination of 1–256 bytes")?
                .to_owned(),
            modifiers: modifier(argument)?,
            dwell_frames: dwell_frames(argument)?,
        },
        "drag_path" => {
            let stops = argument["stops"]
                .as_array()
                .filter(|stops| (1..=8).contains(&stops.len()))
                .ok_or("drag_path requires 1–8 stops")?;
            let stops = stops
                .iter()
                .map(|stop| {
                    Ok(DragStop {
                        destination: stop["destination"]
                            .as_str()
                            .filter(|value| !value.is_empty() && value.len() <= 256)
                            .ok_or("drag_path stop requires a semantic destination")?
                            .to_owned(),
                        modifiers: modifier(stop)?,
                        dwell_frames: dwell_frames(stop)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            if stops.iter().map(|stop| stop.dwell_frames).sum::<usize>() > 240 {
                return Err("drag_path dwell time must be at most 240 frames".into());
            }
            Action::DragPath(stops)
        }
        "scroll" | "wheel" => Action::Scroll(number(argument, "delta")?),
        "key" => Action::Key {
            key: egui::Key::from_name(argument["key"].as_str().ok_or("key name required")?)
                .ok_or("unknown key")?,
            modifiers: modifier(argument)?,
        },
        "text" => Action::Text(text(argument, "text")?),
        _ => return Err("unknown action kind".into()),
    };
    let mut steps = Vec::new();
    let click_first = argument["click_first"].as_bool().unwrap_or(true);
    if argument.get("click_first").is_some() && argument["click_first"].as_bool().is_none() {
        return Err("click_first must be a boolean".into());
    }
    if click_first && matches!(action, Action::Key { .. } | Action::Text(_)) {
        steps.push(Step {
            phase: "action",
            target: target.into(),
            action: Action::Click,
            after: 0.05,
        });
    }
    steps.push(Step {
        phase: "action",
        target: target.into(),
        action,
        after: 0.0,
    });
    Ok(steps)
}

pub(super) fn sequence(argument: &Value) -> Result<Vec<Step>, String> {
    let actions = argument
        .as_array()
        .filter(|actions| (1..=64).contains(&actions.len()))
        .ok_or("sequence requires 1–64 actions")?;
    let mut steps = Vec::new();
    for action in actions {
        steps.extend(parse(action)?);
    }
    Ok(steps)
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .filter(|text| text.len() <= 4096)
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} requires text of at most 4096 bytes"))
}

#[expect(clippy::cast_possible_truncation, reason = "bounded finite egui input")]
fn number(value: &Value, key: &str) -> Result<f32, String> {
    let number = value[key]
        .as_f64()
        .ok_or_else(|| format!("{key} must be numeric"))?;
    if !number.is_finite() || number.abs() > 100_000.0 {
        return Err(format!("{key} is out of range"));
    }
    Ok(number as f32)
}

fn coordinate(value: &Value, key: &str) -> Result<f32, String> {
    let value = number(value, key)?;
    if !(0.0..=1.0).contains(&value) {
        return Err("drag coordinates must be within 0..1".into());
    }
    Ok(value)
}

fn dimension(value: &Value, key: &str) -> Result<f32, String> {
    let value = number(value, key)?;
    if !(1.0..=8192.0).contains(&value) {
        return Err(format!("{key} must be within 1..8192 logical points"));
    }
    Ok(value)
}

fn modifier(argument: &Value) -> Result<egui::Modifiers, String> {
    match argument["modifier"].as_str().unwrap_or("none") {
        "none" => Ok(egui::Modifiers::NONE),
        "ctrl" => Ok(egui::Modifiers::CTRL | egui::Modifiers::COMMAND),
        "shift" => Ok(egui::Modifiers::SHIFT),
        "alt" => Ok(egui::Modifiers::ALT),
        _ => Err("modifier must be none, ctrl, shift, or alt".into()),
    }
}

fn dwell_frames(argument: &Value) -> Result<usize, String> {
    match argument["dwell_frames"].as_u64() {
        Some(frames @ 0..=120) => Ok(usize::try_from(frames).expect("bounded frame count")),
        Some(_) => Err("dwell_frames must be at most 120".into()),
        None if argument.get("dwell_frames").is_some() => {
            Err("dwell_frames must be a nonnegative integer".into())
        }
        None => Ok(0),
    }
}
