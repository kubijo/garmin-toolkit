//! Native developer tools: local RPC logs and an explicitly started loopback server.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use eframe::egui;
use garmin_service_api::control::ControlCommand as Command;
use garmin_ui::developer::{Request, state};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, Default, clap::Parser)]
pub struct Options {
    /// Enable semantic automation (demo builds only).
    #[arg(long)]
    pub ui_automation: bool,
    /// Start the loopback control server on an OS-assigned port (demo builds only).
    #[arg(long)]
    pub control_server: bool,
}

impl Options {
    /// Validate startup controls before opening application storage.
    /// # Errors
    /// Production builds reject automation and the control server.
    pub fn validate(self) -> std::io::Result<()> {
        if (self.ui_automation || self.control_server) && !cfg!(feature = "demo") {
            return Err(std::io::Error::other(
                "automation and control server require a demo build",
            ));
        }
        Ok(())
    }
}

struct Pending {
    command: Command,
    response: oneshot::Sender<Result<Value, String>>,
}
#[derive(Clone)]
struct Bridge {
    sender: mpsc::Sender<Pending>,
    context: egui::Context,
}

struct NativeTools {
    runtime: Option<tokio::runtime::Runtime>,
    store: garmin_logging::Store,
    receiver: mpsc::Receiver<Pending>,
    sender: mpsc::Sender<Pending>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for NativeTools {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl NativeTools {
    fn shutdown(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

pub fn shutdown(context: &egui::Context) {
    if let Some(plugin) = context.plugin_opt::<NativeTools>() {
        plugin.lock().shutdown();
    }
}

pub fn install(
    context: &egui::Context,
    options: Options,
    store: garmin_logging::Store,
) -> std::io::Result<()> {
    let runtime = Some(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );
    let (sender, receiver) = mpsc::channel(16);
    let mut tools = NativeTools {
        runtime,
        store,
        receiver,
        sender,
        server: None,
    };
    #[cfg(feature = "demo")]
    if options.ui_automation || options.control_server {
        context.add_plugin(garmin_ui::automation::Driver::default());
    }
    if options.control_server {
        tools.start(context);
    }
    context.add_plugin(tools);
    garmin_ui::developer::reconnect(context);
    Ok(())
}

impl egui::plugin::Plugin for NativeTools {
    fn debug_name(&self) -> &'static str {
        "native developer tools"
    }
}

impl NativeTools {
    fn start(&mut self, context: &egui::Context) {
        if self.server.is_some() {
            return;
        }
        if !cfg!(feature = "demo") {
            state(context)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .server_error = Some("Control server requires a demo build".into());
            return;
        }
        #[cfg(feature = "demo")]
        if context
            .plugin_opt::<garmin_ui::automation::Driver>()
            .is_none()
        {
            context.add_plugin(garmin_ui::automation::Driver::default());
        }
        let result = (|| -> std::io::Result<_> {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            listener.set_nonblocking(true)?;
            let url = format!("http://{}", listener.local_addr()?);
            let _entered = self
                .runtime
                .as_ref()
                .expect("native runtime is active")
                .enter();
            let listener = tokio::net::TcpListener::from_std(listener)?;
            Ok((listener, url))
        })();
        match result {
            Ok((listener, url)) => {
                println!("Control server: {url}");
                let handle = state(context);
                {
                    let mut state = handle
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.server = Some(url);
                    state.server_error = None;
                }
                let router = Router::new()
                    .route("/api/control", post(control))
                    .route("/api/capabilities", get(capabilities))
                    .route("/api/debug", get(debug))
                    .layer(DefaultBodyLimit::max(16 * 1024))
                    .with_state(Bridge {
                        sender: self.sender.clone(),
                        context: context.clone(),
                    });
                let context = context.clone();
                self.server = Some(
                    self.runtime
                        .as_ref()
                        .expect("native runtime is active")
                        .spawn(async move {
                            if let Err(error) = axum::serve(listener, router).await {
                                let mut state = handle
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                state.server = None;
                                state.server_error = Some(error.to_string());
                                context.request_repaint();
                            }
                        }),
                );
            }
            Err(error) => {
                state(context)
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .server_error = Some(error.to_string());
            }
        }
    }

    fn update(&mut self, context: &egui::Context) {
        while let Ok(pending) = self.receiver.try_recv() {
            if pending.response.is_closed() {
                continue;
            }
            #[cfg(feature = "demo")]
            let result = garmin_ui::automation::command(
                context,
                &pending.command.operation,
                &pending.command.argument,
            );
            #[cfg(not(feature = "demo"))]
            let result = {
                let _ = pending.command;
                Err("automation requires a demo build".into())
            };
            let _ = pending.response.send(result);
        }
        for request in garmin_ui::developer::take_requests(context) {
            match request {
                Request::StartServer => self.start(context),
                Request::StopServer => {
                    if let Some(server) = self.server.take() {
                        server.abort();
                    }
                    state(context)
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .server = None;
                }
                Request::OpenFolder => {
                    let path = self.store.directory().to_path_buf();
                    self.runtime
                        .as_ref()
                        .expect("native runtime is active")
                        .spawn_blocking(move || {
                            let launcher = if cfg!(target_os = "macos") {
                                "open"
                            } else {
                                "xdg-open"
                            };
                            if let Err(error) =
                                std::process::Command::new(launcher).arg(path).spawn()
                            {
                                tracing::warn!(%error, "Could not open log folder");
                            }
                        });
                }
                request => {
                    let _entered = self
                        .runtime
                        .as_ref()
                        .expect("native runtime is active")
                        .enter();
                    let client = self.store.client();
                    self.runtime
                        .as_ref()
                        .expect("native runtime is active")
                        .spawn(garmin_ui::developer::logs(context.clone(), client, request));
                }
            }
        }
        let handle = state(context);
        let export = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .export
            .take();
        if let Some(export) = export {
            let context = context.clone();
            self.runtime
                .as_ref()
                .expect("native runtime is active")
                .spawn_blocking(move || {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("garmin-logs.jsonl")
                        .save_file()
                        && let Err(error) = std::fs::write(path, export)
                    {
                        state(&context)
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .log_error = Some(error.to_string());
                        context.request_repaint();
                    }
                });
        }
    }
}

pub fn update(context: &egui::Context, intl: &garmin_i18n::Intl) {
    if let Some(plugin) = context.plugin_opt::<NativeTools>() {
        plugin.lock().update(context);
    }
    #[cfg(feature = "demo")]
    if let Some(plugin) = context.plugin_opt::<garmin_ui::automation::Driver>() {
        let requested = plugin.lock().take_launch_request();
        if let Some(name) = requested
            && let Err(error) = garmin_ui::automation::command(context, "start", &json!(name))
        {
            plugin.lock().launch_error = Some(error);
        }
        garmin_ui::automation::show_status(context);
    }
    garmin_ui::developer::show(context, true, intl);
}

async fn control(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> (StatusCode, Json<Value>) {
    // Browser automation belongs to the browser bridge, not this native listener.
    if headers.contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"browser origins are not accepted"})),
        );
    }
    let (sender, receiver) = oneshot::channel();
    if bridge
        .sender
        .try_send(Pending {
            command,
            response: sender,
        })
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"control queue is full"})),
        );
    }
    bridge.context.request_repaint();
    match tokio::time::timeout(std::time::Duration::from_secs(5), receiver).await {
        Ok(Ok(Ok(value))) => (StatusCode::OK, Json(json!({"value":value}))),
        Ok(Ok(Err(error))) => (StatusCode::BAD_REQUEST, Json(json!({"error":error}))),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"UI command timed out or application closed"})),
        ),
    }
}
async fn capabilities() -> Json<Value> {
    Json(garmin_service_api::control::capabilities())
}
async fn debug(State(bridge): State<Bridge>) -> Json<Value> {
    Json(
        json!({"debug":state(&bridge.context).lock().unwrap_or_else(std::sync::PoisonError::into_inner).debug}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    #[test]
    fn production_rejects_explicit_automation_before_startup() {
        let options = Options {
            ui_automation: true,
            control_server: false,
        };
        assert_eq!(options.validate().is_ok(), cfg!(feature = "demo"));
        assert!(Options::default().validate().is_ok());
    }

    #[test]
    #[cfg(feature = "demo")]
    fn server_is_explicit_dynamic_and_stops_without_closing_the_application() {
        let directory = tempfile::tempdir().unwrap();
        let store = garmin_logging::Store::open(directory.path(), "test").unwrap();
        let context = egui::Context::default();
        install(&context, Options::default(), store).unwrap();
        assert!(state(&context).lock().unwrap().server.is_none());
        state(&context)
            .lock()
            .unwrap()
            .requests
            .push(Request::StartServer);
        context.plugin::<NativeTools>().lock().update(&context);
        let url = state(&context).lock().unwrap().server.clone().unwrap();
        let address = url.strip_prefix("http://").unwrap();
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(
                b"GET /api/capabilities HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("200 OK"));
        assert!(response.contains("targets"));
        state(&context)
            .lock()
            .unwrap()
            .requests
            .push(Request::StopServer);
        context.plugin::<NativeTools>().lock().update(&context);
        assert!(state(&context).lock().unwrap().server.is_none());
        assert!(
            context
                .plugin_opt::<garmin_ui::automation::Driver>()
                .is_some()
        );
    }
}
