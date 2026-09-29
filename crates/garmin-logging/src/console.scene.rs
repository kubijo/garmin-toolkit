use ansi_to_tui::IntoText as _;
use gallery::prelude::*;
use garmin_model::logging::{Level, Record};
use std::collections::BTreeMap;
use std::fmt::Write as _;

scene_meta! { title: "Application / Logging" }

#[scene(default)]
fn all_variants(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    let font = match ctx.buttons("font", &["Departure Mono", "JetBrains Mono Nerd Font"], 1) {
        0 => crate::screens::PreviewFont::DepartureMono,
        _ => crate::screens::PreviewFont::JetBrainsMonoNerd,
    };
    let columns = match ctx.buttons("columns", &["80", "100"], 0) {
        0 => 80,
        _ => 100,
    };
    let revision = ctx.scene_revision();
    ui.columns(2, |panels| {
        for (panel, ansi) in panels.iter_mut().zip([true, false]) {
            panel.heading(if ansi { "ANSI colors" } else { "Plain text" });
            ctx.stage(panel, Stage::Fill, |ui| {
                let text = samples(ansi)
                    .into_text()
                    .expect("the log renderer emits valid ANSI");
                ui.push_id(ansi, |ui| {
                    crate::screens::show_text(ui, text, font, columns, revision);
                });
            });
        }
    });
}

fn samples(ansi: bool) -> String {
    let mut output = String::new();
    for level in [
        Level::Trace,
        Level::Debug,
        Level::Info,
        Level::Warn,
        Level::Error,
    ] {
        append(&mut output, ansi, level, "Log severity", &[]);
    }
    for (label, message, fields) in examples() {
        let _ = writeln!(output, "\n# {label}");
        append(&mut output, ansi, Level::Info, message, &fields);
    }
    output
}

fn append(output: &mut String, ansi: bool, level: Level, message: &str, fields: &[(&str, &str)]) {
    let record = Record {
        sequence: 1,
        timestamp_ms: 1_790_417_762_456,
        level,
        component: "garmin::server".into(),
        source: "hass".into(),
        session: "gallery".into(),
        source_sequence: 1,
        message: message.into(),
        fields: fields
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect::<BTreeMap<_, _>>(),
    };
    let _ = writeln!(output, "{}", garmin_logging::console::line(&record, ansi));
}

type Example = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

fn examples() -> [Example; 7] {
    [
        (
            "Strings and booleans",
            "Shutdown requested",
            vec![
                ("signal", "\"SIGINT\""),
                ("enabled", "true"),
                ("cached", "false"),
                ("text", "\"true\""),
                ("numeric_text", "\"42\""),
            ],
        ),
        (
            "Numbers and absent values",
            "Transfer progress",
            vec![
                ("count", "42"),
                ("delta", "-7"),
                ("ratio", "0.75"),
                ("rate", "1.2e3"),
                ("missing", "None"),
                ("optional", "null"),
            ],
        ),
        (
            "Quotes, paths and Unicode",
            "Opened \"Morning ride\" · Příliš žluťoučký",
            vec![("path", "\"C:\\maps\\ride.fit\""), ("empty", "\"\"")],
        ),
        (
            "Span context",
            "Tile request completed",
            vec![
                ("span", "request>download"),
                ("url", "\"https://maps.example/tiles/12/2048/1362.pbf\""),
            ],
        ),
        (
            "Error chain",
            "Request failed",
            vec![
                ("error", "cannot fetch tile"),
                ("error.sources", "connection refused; network unavailable"),
            ],
        ),
        (
            "Escaped controls",
            "Untrusted text\nnext line\r\t\x1b[2J\u{9b}31m\x07",
            vec![("detail", "clipboard\x1b]52;payload\x07")],
        ),
        (
            "Fields without a message",
            "",
            vec![("ready", "true"), ("attempt", "2")],
        ),
    ]
}
