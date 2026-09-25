//! Read-only client for the localhost diagnostic routes;
//! server-owned formatting.

use anyhow::{Context as _, Result, bail};
use clap::Subcommand;
use serde::Serialize;
use std::{io::IsTerminal as _, io::Write as _, path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug, clap::Args)]
pub(super) struct Args {
    /// Base URL of the running desktop or HASS control server.
    #[arg(long, global = true, default_value = "http://127.0.0.1:8099/", value_parser = local_url)]
    url: Url,
    #[command(subcommand)]
    resource: Resource,
}

#[derive(Debug, Subcommand)]
enum Resource {
    /// Read structured application logs.
    Logs(Logs),
    /// Read diagnostic state and lifecycle events.
    Events(Events),
}

#[derive(Debug, clap::Args, Serialize)]
struct Common {
    /// Keep reading live updates until interrupted.
    #[arg(long)]
    #[serde(skip)]
    follow: bool,
    /// Write to a file instead of stdout.
    #[arg(long)]
    #[serde(skip)]
    output: Option<PathBuf>,
    /// Continue after a previously returned cursor.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
    /// Maximum records per batch.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=256))]
    limit: u16,
    /// Match a source name substring.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// Include history at or after this Unix millisecond timestamp.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    since_ms: Option<u64>,
    /// Include history at or before this Unix millisecond timestamp.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    until_ms: Option<u64>,
}

#[derive(Debug, clap::Args, Serialize)]
struct Logs {
    #[command(flatten)]
    #[serde(flatten)]
    common: Common,
    /// Minimum log severity.
    #[arg(long, value_parser = ["trace", "debug", "info", "warn", "error"])]
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<String>,
    /// Match a component name substring.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<String>,
    /// Case-insensitive search in messages and fields.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Debug, clap::Args, Serialize)]
struct Events {
    #[command(flatten)]
    #[serde(flatten)]
    common: Common,
    /// Restrict the event kind.
    #[arg(long, value_parser = ["connection", "window", "automation", "renderer"])]
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    /// Restrict the window handle.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<String>,
}

impl Args {
    pub(super) async fn run(self, json: bool) -> Result<()> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5));
        let follow = match &self.resource {
            Resource::Logs(args) => args.common.follow,
            Resource::Events(args) => args.common.follow,
        };
        if !follow {
            builder = builder.read_timeout(Duration::from_secs(10));
        }
        if self.url.host_str() == Some("localhost") {
            builder = builder.resolve(
                "localhost",
                (
                    [127, 0, 0, 1],
                    self.url.port_or_known_default().unwrap_or(80),
                )
                    .into(),
            );
        }
        let client = builder.build()?;
        let (common, request) = match &self.resource {
            Resource::Logs(args) => (
                &args.common,
                client
                    .get(endpoint(&self.url, "logs", args.common.follow)?)
                    .query(args),
            ),
            Resource::Events(args) => (
                &args.common,
                client
                    .get(endpoint(&self.url, "events", args.common.follow)?)
                    .query(args),
            ),
        };
        let color = super::color_enabled(
            super::OUTPUT_COLOR.get().copied().unwrap_or_default(),
            common.output.is_none() && std::io::stdout().is_terminal(),
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var_os("FORCE_COLOR").as_deref(),
        );
        let format = output_format(json, common.follow, color);
        let mut response = tokio::time::timeout(
            Duration::from_secs(10),
            request.query(&[("format", format)]).send(),
        )
        .await
        .context("diagnostic server did not respond")??
        .error_for_status()?;
        let mut output: Box<dyn std::io::Write> = if let Some(path) = &common.output {
            if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            Box::new(std::fs::File::create(path).context("could not create diagnostic output")?)
        } else {
            Box::new(std::io::stdout())
        };
        loop {
            tokio::select! {
                chunk = response.chunk() => match chunk? {
                    Some(bytes) => { output.write_all(&bytes)?; output.flush()?; }
                    None => return Ok(()),
                },
                signal = tokio::signal::ctrl_c() => { signal?; return Ok(()); }
            }
        }
    }
}

fn output_format(json: bool, follow: bool, color: bool) -> &'static str {
    match (json, follow, color) {
        (true, false, _) => "json",
        (true, true, _) => "sse",
        (false, _, true) => "ansi",
        (false, _, false) => "text",
    }
}

fn endpoint(base: &Url, resource: &str, follow: bool) -> Result<Url> {
    Ok(base.join(&format!(
        "api/{resource}-{}",
        if follow { "stream" } else { "get" }
    ))?)
}

fn local_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value)?;
    let local = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.to_canonical().is_loopback(),
        _ => false,
    };
    if !local
        || !matches!(url.scheme(), "http" | "https")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("expected a localhost HTTP(S) base URL without credentials, query, or fragment");
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn ansi_is_only_requested_for_colored_human_output() {
        for follow in [false, true] {
            assert_eq!(output_format(false, follow, true), "ansi");
            assert_eq!(output_format(false, follow, false), "text");
            for color in [false, true] {
                assert_eq!(
                    output_format(true, follow, color),
                    if follow { "sse" } else { "json" }
                );
            }
        }
    }

    #[tokio::test]
    async fn follow_survives_more_than_ten_seconds_without_records() -> Result<()> {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut reader = tokio::io::BufReader::new(socket);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await? == 0 || line == "\r\n" {
                    break;
                }
            }
            let mut socket = reader.into_inner();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n")
                .await?;
            tokio::time::sleep(Duration::from_secs(11)).await;
            socket.write_all(b"hello\n").await?;
            anyhow::Ok(())
        });
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("follow.log");
        let cli = crate::Cli::try_parse_from([
            "garmin-cli",
            "diagnostics",
            "logs",
            "--follow",
            "--url",
            &url,
            "--output",
            output.to_str().expect("temporary output path"),
        ])?;
        let Some(crate::Command::Diagnostics(args)) = cli.command else {
            panic!("diagnostics command");
        };
        args.run(false).await?;
        server.await??;
        assert_eq!(std::fs::read_to_string(output)?, "hello\n");
        Ok(())
    }

    #[test]
    fn clap_validates_resource_filters_and_preserves_query_text() {
        let cli = crate::Cli::try_parse_from([
            "garmin-cli",
            "diagnostics",
            "logs",
            "--url",
            "http://localhost:1234",
            "--follow",
            "--minimum",
            "warn",
            "--text",
            "a & b",
            "--json",
        ])
        .unwrap();
        let Some(crate::Command::Diagnostics(args)) = cli.command else {
            panic!("diagnostics command");
        };
        let Resource::Logs(logs) = args.resource else {
            panic!("logs command");
        };
        let request = reqwest::Client::new()
            .get(endpoint(&args.url, "logs", true).unwrap())
            .query(&logs)
            .build()
            .unwrap();
        assert_eq!(request.url().path(), "/api/logs-stream");
        assert!(
            request
                .url()
                .query_pairs()
                .any(|(key, value)| key == "text" && value == "a & b")
        );
        assert!(
            !request
                .url()
                .query_pairs()
                .any(|(key, _)| key == "follow" || key == "output")
        );
        assert!(
            crate::Cli::try_parse_from([
                "garmin-cli",
                "diagnostics",
                "events",
                "--minimum",
                "warn"
            ])
            .is_err()
        );
        assert!(
            crate::Cli::try_parse_from(["garmin-cli", "diagnostics", "logs", "--limit", "257"])
                .is_err()
        );
        assert!(local_url("https://example.com").is_err());
        assert!(local_url("http://localhost/?x=y").is_err());
    }
}
