use super::response::Cursor;
use garmin_model::logging::{Filter, Level};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Text,
    Ansi,
    Json,
    Html,
    Sse,
}

#[derive(Clone)]
pub(crate) struct Query {
    pub logs: Filter,
    pub kind: String,
    pub window: String,
    pub after: Option<Cursor>,
    pub limit: usize,
    pub format: Format,
}

impl Query {
    pub fn parse(raw: &str, logs: bool, stream: bool, accept: &str) -> Result<Self, String> {
        if raw.len() > 4096 {
            return Err("query exceeds 4096 bytes".into());
        }
        let mut values = BTreeMap::new();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            let allowed = matches!(
                key.as_ref(),
                "source" | "since_ms" | "until_ms" | "after" | "limit" | "format"
            ) || if logs {
                matches!(key.as_ref(), "minimum" | "component" | "text")
            } else {
                matches!(key.as_ref(), "kind" | "window")
            };
            if !allowed
                || values
                    .insert(key.into_owned(), value.into_owned())
                    .is_some()
            {
                return Err("unknown or duplicate filter".into());
            }
        }
        let get = |key: &str| values.get(key).cloned().unwrap_or_default();
        let number = |key: &str| {
            values
                .get(key)
                .map(|value| value.parse::<u64>().map_err(|_| format!("invalid {key}")))
                .transpose()
        };
        let minimum = match get("minimum").to_ascii_lowercase().as_str() {
            "" | "info" => Level::Info,
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => return Err("invalid minimum severity".into()),
        };
        let kind = get("kind");
        if !matches!(
            kind.as_str(),
            "" | "connection" | "window" | "automation" | "renderer"
        ) {
            return Err("invalid event kind".into());
        }
        let since_ms = number("since_ms")?;
        let until_ms = number("until_ms")?;
        if since_ms
            .zip(until_ms)
            .is_some_and(|(start, end)| start > end)
        {
            return Err("since_ms exceeds until_ms".into());
        }
        let limit =
            usize::try_from(number("limit")?.unwrap_or(100)).map_err(|_| "invalid limit")?;
        if !(1..=256).contains(&limit) {
            return Err("limit must be between 1 and 256".into());
        }
        let after = values.get("after").map(|value| value.parse()).transpose()?;
        let format = values.get("format").cloned().unwrap_or_else(|| {
            if accept.contains("text/html") {
                "html"
            } else if stream && accept.contains("text/event-stream") {
                "sse"
            } else if !stream && accept.contains("application/json") {
                "json"
            } else {
                "text"
            }
            .into()
        });
        let format = match format.as_str() {
            "html" => Format::Html,
            "text" => Format::Text,
            "ansi" => Format::Ansi,
            "sse" if stream => Format::Sse,
            "json" if !stream => Format::Json,
            _ => return Err("invalid format for this route".into()),
        };
        Ok(Self {
            logs: Filter {
                minimum,
                component: get("component"),
                source: get("source"),
                text: get("text"),
                since_ms,
                until_ms,
                ..Filter::default()
            },
            kind,
            window: get("window"),
            after,
            limit,
            format,
        })
    }

    pub fn matches(&self, event: &super::events::Event, history: bool) -> bool {
        (self.kind.is_empty() || self.kind == event.observation.kind)
            && (self.window.is_empty() || self.window == event.observation.window)
            && event.source.contains(&self.logs.source)
            && (!history
                || (self
                    .logs
                    .since_ms
                    .is_none_or(|start| event.timestamp_ms >= start)
                    && self
                        .logs
                        .until_ms
                        .is_none_or(|end| event.timestamp_ms <= end)))
    }
}
