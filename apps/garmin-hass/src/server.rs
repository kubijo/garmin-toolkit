use crate::devices::Host;
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{
        DefaultBodyLimit, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, Request, StatusCode, header, uri::Authority},
    middleware::{self, Next},
    response::{Html, IntoResponse as _, Response},
    routing::{any, get, post},
};
use futures_util::{SinkExt as _, StreamExt as _, future};
use garmin_service_api::ApplicationServiceServerShared;
use http_security_headers::{ContentSecurityPolicy, Nonce};
use nu_ansi_term::{AnsiString, Color, Style};
use remoc::{codec, prelude::*};
use serde::Deserialize;
use std::{
    io::{self, IsTerminal as _, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower_http::services::ServeDir;

const ADDRESS_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_ADDRESS";
const WEB_ROOT_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_WEB_ROOT";
const CSP_REPORT_LIMIT: usize = 32 * 1024;
const CSP_NONCE_PLACEHOLDER: &str = "GARMIN_TOOLKIT_CSP_NONCE";

pub(super) async fn serve(host: Arc<Host>) -> Result<(), std::io::Error> {
    let address = std::env::var(ADDRESS_ENVIRONMENT)
        .unwrap_or_else(|_| "127.0.0.1:8099".to_owned())
        .parse::<SocketAddr>()
        .map_err(std::io::Error::other)?;
    let app = router(host)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    announce_server(address, crate::mode::PRODUCT_NAME)?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    #[cfg(unix)]
    let signal = {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => tokio::select! {
                result = tokio::signal::ctrl_c() => result.map(|()| "SIGINT"),
                _ = terminate.recv() => Ok("SIGTERM"),
            },
            Err(error) => Err(error),
        }
    };

    #[cfg(not(unix))]
    let signal = tokio::signal::ctrl_c().await.map(|()| "interrupt");

    match signal {
        Ok(signal) => tracing::info!(signal, "Shutdown requested"),
        Err(error) => tracing::error!(%error, "Could not install the shutdown signal handler"),
    }
}

fn announce_server(address: SocketAddr, product_name: &str) -> io::Result<()> {
    let web_url = format!("http://{address}/");
    let stdout = io::stdout();
    if !stdout.is_terminal() {
        tracing::info!(%address, %web_url, "Home Assistant host listening");
        return Ok(());
    }

    tracing::debug!(%address, %web_url, "Home Assistant host listening");
    let mut stdout = stdout.lock();
    stdout.write_all(startup_banner(product_name, address, &web_url).as_bytes())?;
    stdout.flush()
}

fn startup_banner(product_name: &str, address: SocketAddr, web_url: &str) -> String {
    let title = Style::new().bold().paint(product_name);
    let ready = Color::Green.paint("●");
    let web = Style::new().dimmed().paint("Web");
    let listening = Style::new().dimmed().paint("Listening");
    let stop = Style::new().dimmed().paint("Press Ctrl+C to stop");
    let link = web_link(web_url);
    format!(
        "\n  {title}\n  {ready} Server ready\n\n  {web}        {link}\n  {listening}  {address}\n\n  {stop}\n\n"
    )
}

fn web_link(web_url: &str) -> AnsiString<'_> {
    Color::Cyan.underline().paint(web_url).hyperlink(web_url)
}

#[derive(Clone)]
struct WebIndex(Arc<str>);

impl WebIndex {
    fn load(root: &Path) -> io::Result<Self> {
        let html = std::fs::read_to_string(root.join("index.html"))?;
        if !html.contains(CSP_NONCE_PLACEHOLDER) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser index lacks the CSP nonce placeholder",
            ));
        }
        Ok(Self(html.into()))
    }

    fn render(&self, nonce: &Nonce) -> String {
        self.0.replace(CSP_NONCE_PLACEHOLDER, nonce.as_str())
    }
}

async fn web_index(
    Extension(index): Extension<WebIndex>,
    Extension(nonce): Extension<Nonce>,
) -> Html<String> {
    Html(index.render(&nonce))
}

fn router(host: Arc<Host>) -> io::Result<Router> {
    let app = Router::new()
        .route("/remoc", any(websocket))
        .route("/health", get(|| async { "ok" }))
        .route("/csp-report", post(csp_report))
        .with_state(host);
    let app = match std::env::var_os(WEB_ROOT_ENVIRONMENT).map(PathBuf::from) {
        Some(root) => app
            .route("/", get(web_index))
            .route("/index.html", get(web_index))
            .fallback_service(ServeDir::new(&root).append_index_html_on_directories(true))
            .layer(Extension(WebIndex::load(&root)?)),
        None => app.route(
            "/",
            get(|| async {
                Html("Garmin Toolkit host is running; no browser bundle was configured.")
            }),
        ),
    };
    Ok(app
        .layer(DefaultBodyLimit::max(CSP_REPORT_LIMIT))
        .layer(middleware::from_fn(browser_cache_policy)))
}

async fn browser_cache_policy(mut request: Request<Body>, next: Next) -> Response {
    let path = request.uri().path();
    let entry_point = matches!(path, "/" | "/index.html");
    let initializer = path.ends_with("-initializer.js");
    let static_asset =
        !entry_point && !initializer && !matches!(path, "/health" | "/remoc" | "/csp-report");
    let nonce = Nonce::random();
    let content_security_policy = content_security_policy(request.headers(), &nonce);
    request.extensions_mut().insert(nonce);
    if entry_point || initializer {
        request.headers_mut().remove(header::IF_MODIFIED_SINCE);
        request.headers_mut().remove(header::IF_NONE_MATCH);
    }
    let mut response = next.run(request).await;
    let policy = if static_asset && response.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, content_security_policy);
    response
}

fn content_security_policy(headers: &HeaderMap, nonce: &Nonce) -> HeaderValue {
    let authority = forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, header::HOST.as_str()))
        .and_then(|value| value.parse::<Authority>().ok());
    let mut connections = vec!["'self'".to_owned()];
    if let Some(authority) = authority {
        connections.push(format!("ws://{authority}"));
        connections.push(format!("wss://{authority}"));
    }
    let policy = ContentSecurityPolicy::new()
        .default_src(["'none'"])
        .base_uri(["'none'"])
        .connect_src(connections)
        .font_src(["'self'", "data:"])
        .form_action(["'none'"])
        .frame_ancestors(["'self'"])
        .img_src(["'self'", "data:", "blob:"])
        .object_src(["'none'"])
        .script_src(["'self'", "'wasm-unsafe-eval'"])
        .nonce_for(["script-src"])
        .style_src(["'self'", "'unsafe-inline'"])
        .report_uri(["csp-report"])
        .to_header_value_with_nonce(nonce)
        .expect("the typed CSP is valid");
    HeaderValue::from_str(&policy).expect("the CSP is a valid header value")
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CspReportPayload {
    Legacy {
        #[serde(rename = "csp-report")]
        report: CspViolation,
    },
    ReportingApi(Vec<ReportingApiReport>),
}

#[derive(Deserialize)]
struct ReportingApiReport {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
    body: CspViolation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CspViolation {
    #[serde(default, alias = "document-uri", alias = "documentURL")]
    document_url: Option<String>,
    #[serde(default, alias = "blocked-uri", alias = "blockedURL")]
    blocked_url: Option<String>,
    #[serde(default, alias = "effective-directive")]
    effective_directive: Option<String>,
    #[serde(default, alias = "violated-directive")]
    violated_directive: Option<String>,
    #[serde(default, alias = "source-file")]
    source_file: Option<String>,
    #[serde(default, alias = "line-number")]
    line_number: Option<u64>,
    #[serde(default, alias = "column-number")]
    column_number: Option<u64>,
    #[serde(default, alias = "status-code")]
    status_code: Option<u16>,
    #[serde(default)]
    disposition: Option<String>,
}

async fn csp_report(body: Bytes) -> StatusCode {
    let payload = match serde_json::from_slice::<CspReportPayload>(&body) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(%error, bytes = body.len(), "Rejected malformed browser CSP report");
            return StatusCode::BAD_REQUEST;
        }
    };
    match payload {
        CspReportPayload::Legacy { report } => log_csp_violation("csp-report", None, &report),
        CspReportPayload::ReportingApi(reports) => {
            for report in reports {
                log_csp_violation(&report.kind, report.url.as_deref(), &report.body);
            }
        }
    }
    StatusCode::NO_CONTENT
}

fn log_csp_violation(kind: &str, context_url: Option<&str>, report: &CspViolation) {
    tracing::error!(
        target: "garmin_hass::csp",
        report_type = kind,
        document_url = report.document_url.as_deref().or(context_url).unwrap_or("-"),
        blocked_url = report.blocked_url.as_deref().unwrap_or("-"),
        directive = report
            .effective_directive
            .as_deref()
            .or(report.violated_directive.as_deref())
            .unwrap_or("-"),
        source_file = report.source_file.as_deref().unwrap_or("-"),
        line = report.line_number.unwrap_or_default(),
        column = report.column_number.unwrap_or_default(),
        status = report.status_code.unwrap_or_default(),
        disposition = report.disposition.as_deref().unwrap_or("-"),
        "Browser CSP violation"
    );
}

async fn websocket(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !browser_origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = serve_client(socket, host).await {
            tracing::warn!(%error, "Remoc client connection failed");
        }
    })
}

fn browser_origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(origin_authority) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .filter(|authority| !authority.is_empty() && !authority.contains('/'))
    else {
        return false;
    };
    forwarded_header(headers, "x-forwarded-host")
        .or_else(|| forwarded_header(headers, header::HOST.as_str()))
        .is_some_and(|host| origin_authority.eq_ignore_ascii_case(host))
}

fn forwarded_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .split(',')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn serve_client(socket: WebSocket, host: Arc<Host>) -> anyhow::Result<()> {
    let (websocket_tx, websocket_rx) = socket.split();
    let transport_tx = websocket_tx
        .with(|packet: Bytes| future::ready(Ok::<_, axum::Error>(Message::Binary(packet))));
    let transport_rx = websocket_rx.filter_map(|message| {
        future::ready(match message {
            Ok(Message::Binary(packet)) => Some(Ok(packet)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
    });
    let (server, client) = ApplicationServiceServerShared::<_, codec::Default>::new(host);
    remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
        .provide(client)
        .await
        .map_err(|error| anyhow::anyhow!("could not establish Remoc connection: {error}"))?;
    server
        .serve()
        .await
        .map_err(|error| anyhow::anyhow!("could not serve device service: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        CSP_NONCE_PLACEHOLDER, WebIndex, browser_origin_allowed, content_security_policy,
        csp_report, router, startup_banner, web_link,
    };
    use crate::devices::{DemoSource, Host};
    use axum::body::Bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use futures_util::{SinkExt as _, StreamExt as _, future};
    use garmin_service_api::{ApplicationService as _, ApplicationServiceClient, InspectionState};
    use garmin_services::Application;
    use http_security_headers::{ContentSecurityPolicy, Nonce};
    use remoc::prelude::*;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    #[test]
    fn browser_websockets_must_come_from_the_served_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8099"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:8099"),
        );
        assert!(browser_origin_allowed(&headers));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(!browser_origin_allowed(&headers));

        headers.insert("x-forwarded-host", HeaderValue::from_static("home.example"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://home.example"),
        );
        assert!(browser_origin_allowed(&headers));
    }

    #[test]
    fn browser_policy_allows_wasm_websockets_and_same_origin_framing() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8099"));
        let nonce = Nonce::from_encoded("dGVzdA==").expect("the test nonce is valid");
        let policy = content_security_policy(&headers, &nonce);
        let policy = policy.to_str().expect("the policy is text");
        let parsed = ContentSecurityPolicy::parse(policy).expect("the generated policy parses");

        assert!(policy.contains("connect-src 'self' ws://127.0.0.1:8099 wss://127.0.0.1:8099"));
        assert!(policy.contains("frame-ancestors 'self'"));
        assert!(policy.contains("script-src 'self' 'wasm-unsafe-eval'"));
        assert!(policy.contains("'nonce-dGVzdA=='"));
        assert!(policy.contains("report-uri csp-report"));
        assert!(!policy.contains("report-to"));
        assert!(!policy.contains("'unsafe-eval'"));
        assert_eq!(
            parsed.get("script-src"),
            Some(
                [
                    "'self'".to_owned(),
                    "'wasm-unsafe-eval'".to_owned(),
                    "'nonce-dGVzdA=='".to_owned(),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            parsed.get("object-src"),
            Some(["'none'".to_owned()].as_slice())
        );
    }

    #[test]
    fn web_index_replaces_the_trunk_nonce_placeholder() {
        let index =
            WebIndex(format!(r#"<script nonce="{CSP_NONCE_PLACEHOLDER}"></script>"#).into());
        let nonce = Nonce::from_encoded("dGVzdA==").expect("the test nonce is valid");

        assert_eq!(
            index.render(&nonce),
            r#"<script nonce="dGVzdA=="></script>"#
        );
    }

    #[tokio::test]
    async fn accepts_legacy_and_reporting_api_csp_reports() {
        let legacy = Bytes::from_static(
            br#"{"csp-report":{"document-uri":"http://localhost/","blocked-uri":"inline","effective-directive":"script-src-elem"}}"#,
        );
        let modern = Bytes::from_static(
            br#"[{"type":"csp-violation","url":"http://localhost/","body":{"blockedURL":"inline","effectiveDirective":"script-src-elem"}}]"#,
        );

        assert_eq!(csp_report(legacy).await, StatusCode::NO_CONTENT);
        assert_eq!(csp_report(modern).await, StatusCode::NO_CONTENT);
    }

    #[test]
    fn startup_banner_exposes_a_clickable_web_address() {
        let address = "127.0.0.1:8099".parse().expect("the address is valid");
        let banner = startup_banner("Garmin Toolkit Demo", address, "http://127.0.0.1:8099/");

        assert!(banner.contains("Server ready"));
        assert!(banner.contains("Garmin Toolkit Demo"));
        assert!(banner.contains("127.0.0.1:8099"));
        assert!(banner.contains("http://127.0.0.1:8099/"));
        assert_eq!(
            web_link("http://127.0.0.1:8099/").url_string(),
            Some("http://127.0.0.1:8099/")
        );
    }

    #[tokio::test]
    async fn host_routes_are_constructible() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = crate::prepare_storage(directory.path()).await?;
        let _router = router(Host::new(
            Box::new(DemoSource::new()),
            Application::new(storage),
        ))?;
        Ok(())
    }

    #[tokio::test]
    async fn websocket_carries_automatically_inspected_device_snapshots() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = crate::prepare_storage(directory.path()).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = router(Host::new(
            Box::new(DemoSource::new()),
            Application::new(storage),
        ))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let (socket, response) = connect_async(format!("ws://{address}/remoc")).await?;
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_some_and(|value| value
                    .to_str()
                    .is_ok_and(|value| value.contains("frame-ancestors 'self'")))
        );
        let (socket_tx, socket_rx) = socket.split();
        let transport_tx = socket_tx.with(|packet: Bytes| {
            future::ready(Ok::<_, tokio_tungstenite::tungstenite::Error>(
                Message::Binary(packet),
            ))
        });
        let transport_rx = socket_rx.filter_map(|message| {
            future::ready(match message {
                Ok(Message::Binary(packet)) => Some(Ok(packet)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
        let client: ApplicationServiceClient =
            remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
                .consume()
                .await?;
        assert_eq!(
            client.deployment_mode().await?,
            crate::mode::DEPLOYMENT_MODE
        );
        client.heartbeat().await?;
        let snapshots = client.watch_devices().await?;
        let update = snapshots.borrow()?;
        let device = update.first().expect("the demo device remains attached");

        assert_eq!(device.inspection, InspectionState::Ready);
        assert_eq!(
            device
                .storages
                .first()
                .and_then(|storage| storage.capacity.bytes()),
            Some((32_000_000_000, 8_600_000_000))
        );
        let user = client
            .create_profile("Alex Rider".to_owned())
            .await?
            .map_err(anyhow::Error::msg)?;
        assert_eq!(user.profile().display_name().as_str(), "Alex Rider");

        server.abort();
        Ok(())
    }
}
