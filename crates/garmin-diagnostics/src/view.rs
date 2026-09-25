//! Rust owns presentation;
//! Askama escapes all text placed into HTML.

use super::{
    events::Event,
    response::{Batch, Reply},
};
use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse as _, Response},
};
use garmin_model::logging::{Level, Record};
use nu_ansi_term::Color;
use serde::Serialize;
use std::fmt::{Display, Write as _};

#[derive(Clone, Copy)]
enum Style {
    Dim,
    Heading,
    Cyan,
    Blue,
    Magenta,
    Green,
    Yellow,
    Red,
}

impl Style {
    fn paint(self, value: impl Display, ansi: bool) -> String {
        if !ansi {
            return value.to_string();
        }
        let style = match self {
            Self::Dim => nu_ansi_term::Style::new().dimmed(),
            Self::Heading => nu_ansi_term::Style::new().bold(),
            Self::Cyan => Color::Cyan.normal(),
            Self::Blue => Color::Blue.normal(),
            Self::Magenta => Color::Purple.normal(),
            Self::Green => Color::Green.normal(),
            Self::Yellow => Color::Yellow.normal(),
            Self::Red => Color::Red.normal(),
        };
        style.paint(value.to_string()).to_string()
    }
}

#[derive(Template)]
#[template(path = "page.html")]
struct Page<'a> {
    title: &'a str,
    live: bool,
    cursor: String,
    content: Content,
}

#[derive(Template)]
#[template(path = "records.html")]
struct Records<'a> {
    rows: &'a [String],
}

struct Content {
    state: Option<Vec<String>>,
    records: Vec<String>,
    status: String,
}

trait Line {
    fn line(&self, ansi: bool) -> String;
}

impl Line for Record {
    fn line(&self, ansi: bool) -> String {
        let severity = match self.level {
            Level::Trace => Style::Dim,
            Level::Debug => Style::Blue,
            Level::Info => Style::Green,
            Level::Warn => Style::Yellow,
            Level::Error => Style::Red,
        };
        format!(
            "{} {} {} [{}] {} {:?}",
            Style::Dim.paint(self.timestamp_ms, ansi),
            severity.paint(format_args!("{:?}", self.level), ansi),
            Style::Dim.paint(self.source.escape_debug(), ansi),
            Style::Cyan.paint(self.component.escape_debug(), ansi),
            self.message.escape_debug(),
            self.fields
        )
    }
}

impl Line for Event {
    fn line(&self, ansi: bool) -> String {
        let kind = match self.observation.kind.as_str() {
            "connection" => Style::Green,
            "window" => Style::Cyan,
            "automation" => Style::Magenta,
            "renderer" => Style::Blue,
            _ => Style::Heading,
        };
        format!(
            "{} {} {} [{}] {}{:?}",
            Style::Dim.paint(self.timestamp_ms, ansi),
            Style::Dim.paint(self.source.escape_debug(), ansi),
            kind.paint(self.observation.kind.escape_debug(), ansi),
            Style::Cyan.paint(self.observation.window.escape_debug(), ansi),
            if self.observation.removed {
                Style::Yellow.paint("removed ", ansi)
            } else {
                String::new()
            },
            self.observation.fields
        )
    }
}

impl Content {
    fn new<T: Line>(batch: &Batch<T>, ansi: bool) -> Self {
        let mut status = Style::Dim.paint(
            format_args!("cursor={} more={}", batch.cursor, batch.more),
            ansi,
        );
        if let Some(error) = &batch.error {
            let _ = write!(
                status,
                " · {}",
                Style::Red.paint(format_args!("Error: {}", error.escape_debug()), ansi)
            );
        }
        let mut records = Vec::new();
        if batch.gap {
            records.push(Style::Yellow.paint("GAP: retained history is incomplete.", ansi));
        }
        records.extend(batch.records.iter().map(|record| record.line(ansi)));
        Self {
            state: batch
                .state
                .as_ref()
                .map(|state| state.iter().map(|record| record.line(ansi)).collect()),
            records,
            status,
        }
    }

    fn from_reply(reply: &Reply, ansi: bool) -> Self {
        match reply {
            Reply::Logs(batch) => Self::new(batch, ansi),
            Reply::Events(batch) => Self::new(batch, ansi),
        }
    }
}

pub(crate) fn readable(reply: &Reply, ansi: bool) -> String {
    let content = Content::from_reply(reply, ansi);
    let mut text = String::new();
    if let Some(state) = content.state {
        let _ = writeln!(text, "{}", Style::Heading.paint("# Current state", ansi));
        for line in state {
            let _ = writeln!(text, "{line}");
        }
    }
    for line in content.records {
        let _ = writeln!(text, "{line}");
    }
    let _ = writeln!(text, "# {}", content.status);
    text
}

pub(crate) fn page(reply: &Reply, logs: bool, live: bool) -> Response {
    let template = Page {
        title: if logs {
            "Application logs"
        } else {
            "Application events"
        },
        live,
        cursor: reply.cursor().to_string(),
        content: Content::from_reply(reply, false),
    };
    match template.render() {
        Ok(html) if html.len() <= garmin_service_api::control::MAX_REPLY_BYTES => {
            Html(html).into_response()
        }
        Ok(_) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "diagnostic view exceeds reply limit; narrow the filters",
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not render diagnostics",
        )
            .into_response(),
    }
}

#[derive(Serialize)]
pub(crate) struct Update {
    state: Option<String>,
    records: String,
    status: String,
}

impl Update {
    pub fn render(reply: &Reply) -> Result<Self, std::io::Error> {
        let content = Content::from_reply(reply, false);
        let update = Self {
            state: content
                .state
                .map(|rows| Records { rows: &rows }.render())
                .transpose()
                .map_err(std::io::Error::other)?,
            records: Records {
                rows: &content.records,
            }
            .render()
            .map_err(std::io::Error::other)?,
            status: content.status,
        };
        if serde_json::to_vec(&update)
            .map_err(std::io::Error::other)?
            .len()
            > garmin_service_api::control::MAX_REPLY_BYTES
        {
            return Err(std::io::Error::other(
                "diagnostic view exceeds reply limit; narrow the filters",
            ));
        }
        Ok(update)
    }
}
