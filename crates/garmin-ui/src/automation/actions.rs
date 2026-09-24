//! Parsing for individual actions and ordered functional checks.
use super::{Action, Step};
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
        "click" => Action::Click,
        "drag" => Action::Drag {
            x: coordinate(argument, "x")?,
            y: coordinate(argument, "y")?,
        },
        "scroll" | "wheel" => Action::Scroll(number(argument, "delta")?),
        "key" => Action::Key(
            egui::Key::from_name(argument["key"].as_str().ok_or("key name required")?)
                .ok_or("unknown key")?,
        ),
        "text" => Action::Text(text(argument, "text")?),
        _ => return Err("unknown action kind".into()),
    };
    let mut steps = Vec::new();
    if matches!(action, Action::Key(_) | Action::Text(_)) {
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
