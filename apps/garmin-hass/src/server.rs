use crate::BrowserOptions;
use crate::devices::Host;
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{
        DefaultBodyLimit, Path as AxumPath, State,
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
use tokio::io::AsyncReadExt as _;
use tower_http::services::ServeDir;

const ADDRESS_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_ADDRESS";
const WEB_ROOT_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_WEB_ROOT";
const CSP_REPORT_LIMIT: usize = 32 * 1024;
const CSP_NONCE_PLACEHOLDER: &str = "GARMIN_TOOLKIT_CSP_NONCE";
const UPLOAD_TELEMETRY_PLACEHOLDER: &str = "GARMIN_TOOLKIT_MAP_UPLOAD_TELEMETRY";
const MAP_EXPERIMENT_PLACEHOLDER: &str = "GARMIN_TOOLKIT_MAP_RENDER_EXPERIMENT";
const UI_AUTOMATION_PLACEHOLDER: &str = "GARMIN_TOOLKIT_UI_AUTOMATION";

pub(super) async fn serve(
    host: Arc<Host>,
    map_tiles: garmin_map_tiles::Service,
    browser: BrowserOptions,
) -> Result<(), std::io::Error> {
    let address = std::env::var(ADDRESS_ENVIRONMENT)
        .unwrap_or_else(|_| "127.0.0.1:8099".to_owned())
        .parse::<SocketAddr>()
        .map_err(std::io::Error::other)?;
    let app = router(host, map_tiles, browser)?;
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
    fn load(root: &Path, browser: BrowserOptions) -> io::Result<Self> {
        let html = std::fs::read_to_string(root.join("index.html"))?;
        if !html.contains(CSP_NONCE_PLACEHOLDER) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser index lacks the CSP nonce placeholder",
            ));
        }
        if !html.contains(UPLOAD_TELEMETRY_PLACEHOLDER) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser index lacks the map upload telemetry placeholder",
            ));
        }
        if !html.contains(MAP_EXPERIMENT_PLACEHOLDER) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser index lacks the map experiment placeholder",
            ));
        }
        let telemetry = serde_json::to_string(&browser.map_upload_telemetry)?;
        if !html.contains(UI_AUTOMATION_PLACEHOLDER) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser index lacks the UI automation placeholder",
            ));
        }
        let automation = serde_json::to_string(&browser.ui_automation)?;
        let experiment = serde_json::to_string(&browser.map_render_experiment)?;
        Ok(Self(
            html.replace(UPLOAD_TELEMETRY_PLACEHOLDER, &telemetry)
                .replace(MAP_EXPERIMENT_PLACEHOLDER, &experiment)
                .replace(UI_AUTOMATION_PLACEHOLDER, &automation)
                .into(),
        ))
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

fn router(
    host: Arc<Host>,
    map_tiles: garmin_map_tiles::Service,
    browser: BrowserOptions,
) -> io::Result<Router> {
    let app = Router::new()
        .route("/remoc", any(websocket))
        .route("/device-download/{token}", get(device_download))
        .route("/map/tiles/{zoom}/{x}/{file}", get(map_tile))
        .route("/health", get(|| async { "ok" }))
        .route("/csp-report", post(csp_report))
        .with_state(host)
        .layer(Extension(map_tiles));
    let app = match std::env::var_os(WEB_ROOT_ENVIRONMENT).map(PathBuf::from) {
        Some(root) => app
            .route("/", get(web_index))
            .route("/index.html", get(web_index))
            .fallback_service(ServeDir::new(&root).append_index_html_on_directories(true))
            .layer(Extension(WebIndex::load(&root, browser)?)),
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
    let entry_point = browser_entry_point(path);
    let map_tile = path.starts_with("/map/tiles/");
    let static_asset = fingerprinted_asset(path)
        && !entry_point
        && !path.starts_with("/device-download/")
        && !map_tile
        && !matches!(path, "/health" | "/remoc" | "/csp-report");
    let nonce = Nonce::random();
    let content_security_policy = content_security_policy(request.headers(), &nonce);
    request.extensions_mut().insert(nonce);
    if entry_point {
        request.headers_mut().remove(header::IF_MODIFIED_SINCE);
        request.headers_mut().remove(header::IF_NONE_MATCH);
    }
    let mut response = next.run(request).await;
    if !map_tile {
        let policy = browser_asset_policy(static_asset, response.status());
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    }
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, content_security_policy);
    response
}

fn browser_entry_point(path: &str) -> bool {
    matches!(
        path,
        "/" | "/index.html" | "/map-worker.js" | "/map-render-worker.js" | "/worker-codec.js"
    ) || path.ends_with("-initializer.js")
}

fn fingerprinted_asset(path: &str) -> bool {
    let Some((stem, extension)) = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
    else {
        return false;
    };
    if !matches!(
        extension,
        "js" | "wasm" | "svg" | "css" | "png" | "jpg" | "webp" | "woff" | "woff2"
    ) {
        return false;
    }
    let Some((_, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    // All published assets use esbuild's dependency-aware base32 hash.
    hash.len() == 8
        && hash
            .bytes()
            .all(|c| c.is_ascii_uppercase() || matches!(c, b'2'..=b'7'))
}

fn browser_asset_policy(static_asset: bool, status: StatusCode) -> &'static str {
    if static_asset && status.is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    }
}

async fn map_tile(
    Extension(service): Extension<garmin_map_tiles::Service>,
    AxumPath((zoom, x, file)): AxumPath<(u8, u32, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(y) = file
        .strip_suffix(".pbf")
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let tile_id = garmin_map_tiles::TileId { zoom, x, y };
    if tile_id.validate().is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let tile = match service.tile(tile_id).await {
        Ok(tile) => tile,
        Err(error) => {
            tracing::warn!(%error, zoom, x, y, "Could not serve activity map tile");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let etag = HeaderValue::from_str(&tile.etag).ok();
    let cache_control = HeaderValue::from_str(&format!("public, max-age={}", tile.max_age_seconds))
        .unwrap_or_else(|_| HeaderValue::from_static("no-cache"));
    if request_etag_matches(&headers, &tile.etag) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Some(etag) = etag {
            response.headers_mut().insert(header::ETAG, etag);
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, cache_control);
        return response;
    }
    let mut response = Response::new(Body::from(tile.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.mapbox-vector-tile"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, cache_control);
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(etag) = etag {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

fn request_etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    let etag = etag.strip_prefix("W/").unwrap_or(etag);
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        })
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
        .directive("worker-src", ["'self'"])
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

async fn device_download(
    State(host): State<Arc<Host>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    let Some(download) = host.take_browser_download(&token) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_length = download.size;
    let file = match tokio::fs::File::open(download.path()).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, "prepared browser download disappeared");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let body = Body::from_stream(futures_util::stream::try_unfold(
        (file, download),
        |(mut file, download)| async move {
            let mut bytes = vec![0_u8; 64 * 1024];
            let count = file.read(&mut bytes).await?;
            if count == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                bytes.truncate(count);
                Ok(Some((Bytes::from(bytes), (file, download))))
            }
        },
    ));
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    if let Ok(content_length) = HeaderValue::from_str(&content_length.to_string()) {
        headers.insert(header::CONTENT_LENGTH, content_length);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
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
        CSP_NONCE_PLACEHOLDER, MAP_EXPERIMENT_PLACEHOLDER, UPLOAD_TELEMETRY_PLACEHOLDER, WebIndex,
        browser_asset_policy, browser_entry_point, browser_origin_allowed, content_security_policy,
        csp_report, fingerprinted_asset, request_etag_matches, router, startup_banner, web_link,
    };
    use crate::BrowserOptions;
    use crate::devices::{Host, demo::DemoSource};
    use axum::body::Bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use futures_util::{SinkExt as _, StreamExt as _, future};
    use garmin_service_api::{
        ApplicationService as _, ApplicationServiceClient, DeviceBrowserTarget,
        DeviceCatalogEntryKind, InspectionState,
    };
    use garmin_services::Application;
    use http_security_headers::{ContentSecurityPolicy, Nonce};
    use remoc::prelude::*;
    use std::time::{SystemTime, UNIX_EPOCH};
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
        assert!(policy.contains("worker-src 'self'"));
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
    fn map_revalidation_accepts_lists_and_weak_etags() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static(r#""other", W/"current""#),
        );

        assert!(request_etag_matches(&headers, r#""current""#));
        assert!(!request_etag_matches(&headers, r#""missing""#));
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

    #[test]
    fn web_index_embeds_the_startup_telemetry_choice_without_changing_assets() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let source = include_str!("../web/index.html");
        let html = format!(
            "{source}<script nonce=\"{CSP_NONCE_PLACEHOLDER}\" src=\"app-hash.js\"></script>"
        );
        std::fs::write(root.path().join("index.html"), html)?;
        let nonce = Nonce::from_encoded("dGVzdA==")?;
        for enabled in [true, false] {
            let rendered = WebIndex::load(
                root.path(),
                BrowserOptions {
                    map_upload_telemetry: enabled,
                    ..Default::default()
                },
            )?
            .render(&nonce);
            let value = rendered
                .split("data-map-upload-telemetry=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            assert_eq!(serde_json::from_str::<bool>(value)?, enabled);
            assert!(rendered.contains("src=\"app-hash.js\""));
            assert!(rendered.contains("nonce=\"dGVzdA==\""));
            assert!(!rendered.contains(UPLOAD_TELEMETRY_PLACEHOLDER));
        }
        std::fs::write(
            root.path().join("index.html"),
            format!("<script nonce=\"{CSP_NONCE_PLACEHOLDER}\"></script>"),
        )?;
        assert!(
            WebIndex::load(root.path(), BrowserOptions::default()).is_err(),
            "a stale bundle cannot silently ignore the control"
        );
        Ok(())
    }

    #[test]
    fn stable_browser_worker_is_never_cached_as_an_immutable_asset() {
        for path in [
            "/worker-codec.js",
            "/snippets/crate-123456789/codec.js",
            "/app.js",
            "/icon.svg",
        ] {
            assert!(!fingerprinted_asset(path));
        }
        for path in [
            "/map-worker-ABCDEFG2.js",
            "/chunks/chunk-ABCD2345.js",
            "/app_bg-ABCD2345.wasm",
            "/icon-ABCD2345.svg",
        ] {
            assert!(fingerprinted_asset(path));
        }
        assert!(browser_entry_point("/map-worker.js"));
        assert!(browser_entry_point("/map-render-worker.js"));
        assert!(browser_entry_point("/worker-codec.js"));
        assert_eq!(browser_asset_policy(false, StatusCode::OK), "no-store");
        assert_eq!(
            browser_asset_policy(true, StatusCode::OK),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            browser_asset_policy(true, StatusCode::NOT_FOUND),
            "no-store"
        );
    }

    #[test]
    fn web_index_embeds_experiment_control_and_rejects_stale_bundles() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = format!(
            "{}<script nonce=\"{CSP_NONCE_PLACEHOLDER}\"></script>",
            include_str!("../web/index.html")
        );
        std::fs::write(root.path().join("index.html"), &source)?;
        let nonce = Nonce::from_encoded("dGVzdA==")?;
        for enabled in [false, true] {
            let rendered = WebIndex::load(
                root.path(),
                BrowserOptions {
                    map_render_experiment: enabled,
                    ..Default::default()
                },
            )?
            .render(&nonce);
            assert!(rendered.contains(&format!("data-map-render-experiment=\"{enabled}\"")));
            assert!(!rendered.contains(MAP_EXPERIMENT_PLACEHOLDER));
        }
        let defaults = WebIndex::load(root.path(), BrowserOptions::default())?.render(&nonce);
        assert!(defaults.contains("data-map-render-experiment=\"true\""));
        std::fs::write(
            root.path().join("index.html"),
            source.replace(MAP_EXPERIMENT_PLACEHOLDER, "false"),
        )?;
        assert!(WebIndex::load(root.path(), BrowserOptions::default()).is_err());
        Ok(())
    }

    #[test]
    fn web_index_embeds_automation_only_when_requested() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = format!(
            "{}<script nonce=\"{CSP_NONCE_PLACEHOLDER}\"></script>",
            include_str!("../web/index.html")
        );
        std::fs::write(root.path().join("index.html"), &source)?;
        let nonce = Nonce::from_encoded("dGVzdA==")?;
        for enabled in [false, true] {
            let rendered = WebIndex::load(
                root.path(),
                BrowserOptions {
                    ui_automation: enabled,
                    ..Default::default()
                },
            )?
            .render(&nonce);
            assert!(rendered.contains(&format!("data-ui-automation=\"{enabled}\"")));
            assert!(!rendered.contains(super::UI_AUTOMATION_PLACEHOLDER));
        }
        std::fs::write(
            root.path().join("index.html"),
            source.replace(super::UI_AUTOMATION_PLACEHOLDER, "false"),
        )?;
        assert!(WebIndex::load(root.path(), BrowserOptions::default()).is_err());
        Ok(())
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
        let _router = router(
            Host::new(
                Box::new(DemoSource::new(
                    directory.path().join("device"),
                    tokio::runtime::Handle::current(),
                )?),
                Application::new(storage),
            ),
            garmin_map_tiles::Service::new(directory.path().join("map-cache"))?,
            BrowserOptions::default(),
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn map_tiles_are_relative_cacheable_and_revalidated() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let cache = directory.path().join("map-cache/0/0");
        tokio::fs::create_dir_all(&cache).await?;
        let bytes = [0x1a, 0x05, 0x0a, 0x01, b'x', 0x78, 0x02];
        tokio::fs::write(cache.join("0.pbf"), bytes).await?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        tokio::fs::write(
            cache.join("0.json"),
            serde_json::to_vec(&serde_json::json!({
                "fetched_at": now,
                "accessed_at": now,
                "etag": null,
            }))?,
        )
        .await?;
        let storage = crate::prepare_storage(directory.path()).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = router(
            Host::new(
                Box::new(DemoSource::new(
                    directory.path().join("device"),
                    tokio::runtime::Handle::current(),
                )?),
                Application::new(storage),
            ),
            garmin_map_tiles::Service::new(directory.path().join("map-cache"))?,
            BrowserOptions::default(),
        )?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let client = reqwest::Client::new();
        let url = format!("http://{address}/map/tiles/0/0/0.pbf");

        let response = client.get(&url).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static(
                "application/vnd.mapbox-vector-tile"
            ))
        );
        assert!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("public, max-age=604"))
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_some()
        );
        let etag = response
            .headers()
            .get(header::ETAG)
            .expect("a cached tile has a response validator")
            .clone();
        assert_eq!(response.bytes().await?.as_ref(), bytes.as_slice());

        let response = client
            .get(&url)
            .header(header::IF_NONE_MATCH, etag)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            client
                .get(format!("http://{address}/map/tiles/23/0/0.pbf"))
                .send()
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn prepared_browser_download_is_same_origin_and_single_use() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = crate::prepare_storage(directory.path()).await?;
        let host = Host::new(
            Box::new(DemoSource::new(
                directory.path().join("device"),
                tokio::runtime::Handle::current(),
            )?),
            Application::new(storage),
        );
        let ticket = host
            .prepare_device_browser_download(
                "demo:watch-o-matic-9000".to_owned(),
                DeviceBrowserTarget {
                    storage_id: "internal".to_owned(),
                    path: "Garmin/Activity/History/2026/city-ride.fit".into(),
                    kind: DeviceCatalogEntryKind::File,
                },
            )
            .await?
            .map_err(anyhow::Error::msg)?;
        assert_eq!(ticket.file_name, "city-ride.fit");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = router(
            host,
            garmin_map_tiles::Service::new(directory.path().join("map-cache"))?,
            BrowserOptions::default(),
        )?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let url = format!("http://{address}/device-download/{}", ticket.token);
        let response = reqwest::get(&url).await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_DISPOSITION),
            Some(&HeaderValue::from_static("attachment"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );
        assert_eq!(
            response.bytes().await?.as_ref(),
            garmin_fit::fixture::ActivityCase::CityRide
                .encode()?
                .as_slice()
        );
        assert_eq!(reqwest::get(url).await?.status(), StatusCode::NOT_FOUND);

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn websocket_carries_automatically_inspected_device_snapshots() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = crate::prepare_storage(directory.path()).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = router(
            Host::new(
                Box::new(DemoSource::new(
                    directory.path().join("device"),
                    tokio::runtime::Handle::current(),
                )?),
                Application::new(storage),
            ),
            garmin_map_tiles::Service::new(directory.path().join("map-cache"))?,
            BrowserOptions::default(),
        )?;
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
