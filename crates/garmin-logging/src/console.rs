//! One human-readable log renderer for application stderr and diagnostics.

use garmin_model::logging::{Level, Record};
use nu_ansi_term::{Color, Style};
use std::fmt;
use std::{collections::BTreeMap, fmt::Write as _, io::IsTerminal as _};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    field::RecordFields,
    fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields, MakeWriter, format::Writer},
    registry::LookupSpan,
};

/// Terminal policy for application stderr.
#[must_use]
pub fn stderr_ansi() -> bool {
    std::env::var_os("FORCE_COLOR").map_or_else(
        || std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        |value| value != "0",
    )
}

/// Install the shared formatter with the caller's writer and color policy.
pub fn layer<S, W>(source: &'static str, writer: W, ansi: bool) -> impl tracing_subscriber::Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(ansi)
        .event_format(Formatter(source))
        .fmt_fields(Formatter(source))
        .with_writer(writer)
}

/// Infer scalar styling from retained text; quoted scalars remain strings.
#[must_use]
pub fn value_style(value: &str) -> Style {
    match value {
        "true" | "false" => Color::Fixed(208).normal(),
        "null" | "None" => Style::new().dimmed(),
        _ if value.parse::<f64>().is_ok() => Color::Fixed(141).normal(),
        _ => Color::Fixed(71).normal(),
    }
}

fn paint(value: impl fmt::Display, style: Style, ansi: bool) -> String {
    let value = value.to_string();
    if ansi {
        style.paint(value).to_string()
    } else {
        value
    }
}

/// Preserve readable quotes and paths while escaping terminal controls.
pub struct Text<'a>(pub &'a str);

impl fmt::Display for Text<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.0.chars() {
            match character {
                '\\' | '\'' | '"' => output.write_char(character)?,
                _ => write!(output, "{}", character.escape_debug())?,
            }
        }
        Ok(())
    }
}

/// Render retained or live fields using the same escaping and palette.
#[must_use]
pub fn fields(values: &BTreeMap<String, String>, ansi: bool) -> String {
    values
        .iter()
        .map(|(key, value)| {
            format!(
                "{}{}{}",
                paint(Text(key), Color::Cyan.normal(), ansi),
                paint("=", Style::new().dimmed(), ansi),
                paint(Text(value), value_style(value), ansi),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render a log record identically for live terminals, snapshots, and streams.
#[must_use]
pub fn line(record: &Record, ansi: bool) -> String {
    let (level, color) = match record.level {
        Level::Trace => ("TRACE", Color::Purple),
        Level::Debug => ("DEBUG", Color::Blue),
        Level::Info => ("INFO", Color::Green),
        Level::Warn => ("WARN", Color::Yellow),
        Level::Error => ("ERROR", Color::Red),
    };
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(record.timestamp_ms) * 1_000_000,
    )
    .ok()
    .and_then(|time| {
        time.format(&time::format_description::well_known::Rfc3339)
            .ok()
    })
    .unwrap_or_else(|| record.timestamp_ms.to_string());
    let mut output = format!(
        "{} {} {} [{}]",
        paint(timestamp, Style::new().dimmed(), ansi),
        paint(level, color.bold(), ansi),
        paint(Text(&record.source), Style::new().dimmed(), ansi),
        paint(Text(&record.component), Color::Fixed(71).normal(), ansi),
    );
    if !record.message.is_empty() {
        output.push(' ');
        output.push_str(&paint(
            Text(&record.message),
            Color::Fixed(71).normal(),
            ansi,
        ));
    }
    if !record.fields.is_empty() {
        output.push(' ');
        output.push_str(&fields(&record.fields, ansi));
    }
    output
}

struct Formatter(&'static str);

impl<'writer> FormatFields<'writer> for Formatter {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        values: R,
    ) -> fmt::Result {
        let mut visitor = Values::default();
        values.record(&mut visitor);
        writer.write_str(&fields(&visitor.0, writer.has_ansi_escapes()))
    }
}

impl<S, N> FormatEvent<S, N> for Formatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut values = Values::default();
        event.record(&mut values);
        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing::Level::TRACE => Level::Trace,
            tracing::Level::DEBUG => Level::Debug,
            tracing::Level::INFO => Level::Info,
            tracing::Level::WARN => Level::Warn,
            tracing::Level::ERROR => Level::Error,
        };
        if let Some(scope) = ctx.event_scope() {
            values.0.insert(
                "span".into(),
                scope
                    .from_root()
                    .map(|span| span.name())
                    .collect::<Vec<_>>()
                    .join(">"),
            );
        }
        let record = Record {
            sequence: 0,
            timestamp_ms: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            level,
            component: metadata.target().into(),
            source: self.0.into(),
            session: String::new(),
            source_sequence: 0,
            message: values.0.remove("message").unwrap_or_default(),
            fields: values.0,
        };
        writer.write_str(&line(&record, writer.has_ansi_escapes()))?;
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let extensions = span.extensions();
                if let Some(fields) = extensions.get::<FormattedFields<N>>()
                    && !fields.is_empty()
                {
                    write!(writer, " {fields}")?;
                }
            }
        }
        writeln!(writer)
    }
}

#[derive(Default)]
struct Values(BTreeMap<String, String>);

impl Visit for Values {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.insert(field.name().into(), value.into());
        } else {
            self.record_debug(field, &value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if field.name().starts_with("log.") {
            return;
        }
        self.0.insert(field.name().into(), value.to_string());
        let mut sources = Vec::new();
        let mut source = value.source();
        while let Some(error) = source {
            sources.push(error.to_string());
            source = error.source();
        }
        if !sources.is_empty() {
            self.0
                .insert(format!("{}.sources", field.name()), sources.join("; "));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !field.name().starts_with("log.") {
            self.0.insert(field.name().into(), format!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Seek as _};
    use tracing_subscriber::prelude::*;

    fn capture(ansi: bool, emit: impl FnOnce()) -> String {
        let mut output = tempfile::tempfile().unwrap();
        let subscriber =
            tracing_subscriber::registry().with(layer("test", output.try_clone().unwrap(), ansi));
        tracing::subscriber::with_default(subscriber, emit);
        output.rewind().unwrap();
        let mut text = String::new();
        output.read_to_string(&mut text).unwrap();
        text.trim_end_matches('\n').to_owned()
    }

    #[test]
    fn messages_escape_terminal_controls_without_escaping_readable_quotes() {
        for ansi in [false, true] {
            let output = capture(ansi, || {
                tracing::info!("bad \"quote\" C:\\maps\x1b[2J\x07\u{9b}31m");
            });
            if !ansi {
                assert!(!output.chars().any(char::is_control));
            }
            assert!(!output.contains("\x1b[2J"));
            assert!(!output.contains(['\x07', '\u{9b}']));
            assert!(output.contains("bad \"quote\" C:\\maps"));
            assert!(output.contains("\\u{1b}[2J"));
            assert!(output.contains("\\u{7}"));
            assert!(output.contains("\\u{9b}31m"));
        }
    }

    #[derive(Debug)]
    struct ErrorChain {
        message: &'static str,
        source: Option<Box<Self>>,
    }

    impl fmt::Display for ErrorChain {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            output.write_str(self.message)
        }
    }

    impl std::error::Error for ErrorChain {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &dyn std::error::Error)
        }
    }

    #[test]
    fn typed_errors_sanitize_the_entire_source_chain() {
        for ansi in [false, true] {
            for nested in [false, true] {
                let error = ErrorChain {
                    message: "outer \"quote\" C:\\maps\x1b[2J",
                    source: nested.then(|| {
                        Box::new(ErrorChain {
                            message: "inner\x1b]52;clipboard\x07",
                            source: Some(Box::new(ErrorChain {
                                message: "root\u{9b}31m",
                                source: None,
                            })),
                        })
                    }),
                };
                let output = capture(ansi, || {
                    tracing::warn!(
                        before = true,
                        error = &error as &dyn std::error::Error,
                        after = 42
                    );
                });
                assert!(!output.contains("\x1b[2J"));
                assert!(!output.contains("\x1b]52"));
                assert!(!output.contains(['\x07', '\u{9b}']));
                assert!(output.contains("outer \"quote\" C:\\maps\\u{1b}[2J"));
                assert_eq!(output.contains(".sources"), nested);
                if nested {
                    assert!(output.contains("inner\\u{1b}]52;clipboard\\u{7}"));
                    assert!(output.contains("root\\u{9b}31m"));
                }
                if !ansi {
                    assert!(!output.contains('\x1b'));
                    assert!(output.contains(" before=true error="));
                    assert!(output.contains(" after=42 "));
                }
            }
        }
    }

    #[test]
    fn bridge_metadata_is_omitted_without_extra_separators_or_losing_value_styles() {
        for ansi in [false, true] {
            let output = capture(ansi, || {
                tracing::info!(
                    log.target = "dependency",
                    name = "ride",
                    log.file = "/private/source.rs",
                    enabled = true,
                    count = 42,
                    log.line = 123
                );
            });
            assert!(!output.contains("log."));
            assert!(!output.contains("dependency"));
            assert!(!output.contains("private"));
            assert!(!output.starts_with(' '));
            assert!(!output.ends_with(' '));
            assert!(!output.contains("  "));
            if ansi {
                assert!(output.contains(&Color::Fixed(71).paint("\"ride\"").to_string()));
                assert!(output.contains(&Color::Fixed(208).paint("true").to_string()));
                assert!(output.contains(&Color::Fixed(141).paint("42").to_string()));
            } else {
                assert!(output.ends_with("count=42 enabled=true name=\"ride\""));
            }
        }
    }

    #[test]
    fn server_values_and_span_fields_use_semantic_colors_only_when_enabled() {
        for ansi in [false, true] {
            let output = capture(ansi, || {
                let _span = tracing::info_span!("request", path = "a b").entered();
                tracing::info!(target: "garmin_hass::server",
                    signal = "SIGINT", enabled = true, count = 42, text = "true",
                    "Shutdown requested");
            });
            for value in [
                "garmin_hass::server",
                "Shutdown requested",
                "\"SIGINT\"",
                "\"true\"",
                "\"a b\"",
            ] {
                if ansi {
                    assert!(output.contains(&Color::Fixed(71).paint(value).to_string()));
                } else {
                    assert!(output.contains(value));
                }
            }
            if ansi {
                assert!(output.contains(&Color::Fixed(208).paint("true").to_string()));
                assert!(output.contains(&Color::Fixed(141).paint("42").to_string()));
            } else {
                assert!(!output.contains('\x1b'));
                assert!(output.contains(" INFO test [garmin_hass::server] Shutdown requested "));
                assert!(output.contains("span=request"));
            }
        }
    }

    #[test]
    fn retained_records_use_the_live_renderer() {
        let record = Record {
            sequence: 1,
            timestamp_ms: 0,
            source_sequence: 1,
            level: Level::Info,
            source: "test".into(),
            component: "example".into(),
            session: String::new(),
            message: "Ready \"now\"".into(),
            fields: BTreeMap::from([
                ("count".into(), "42".into()),
                ("name".into(), "\"ride\"".into()),
            ]),
        };
        for ansi in [false, true] {
            let live = capture(ansi, || {
                tracing::info!(target: "example", count = 42, name = "ride", "Ready \"now\"");
            });
            let retained = line(&record, ansi);
            // Only the timestamp differs between a live event and a retained record.
            assert_eq!(
                live.split_once(' ').unwrap().1,
                retained.split_once(' ').unwrap().1
            );
        }
        assert!(line(&record, false).starts_with("1970-01-01T00:00:00Z INFO"));
    }
}
