//! Connection-scoped reverse RPC for the root application's existing automation hooks.
use eframe::egui::{Context, Id};
use futures_util::FutureExt as _;
use futures_util::future::{AbortHandle, Abortable};
use garmin_service_api::control::{BrowserControl, BrowserControlServerShared, ControlDispatch};
use garmin_service_api::{ApplicationService as _, ApplicationServiceClient};
use remoc::{codec, rtc, rtc::ServerShared as _};
use std::{
    fmt::Write as _,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/ui-automation.js")]
extern "C" {
    #[wasm_bindgen(js_name = executeAutomation, catch)]
    fn execute(command: &str) -> Result<String, JsValue>;
}

#[wasm_bindgen(module = "/window-control.js")]
extern "C" {
    #[wasm_bindgen(js_name = installWindowControl)]
    fn install_window_control(capture: &JsValue);
    #[wasm_bindgen(js_name = captureWindow, catch)]
    fn capture_window(window: &str, milliseconds: f64) -> Result<js_sys::Promise, JsValue>;
}

pub(super) fn install_window_capture(context: &Context) {
    let context = context.clone();
    let callback = Closure::<dyn Fn(f64) -> js_sys::Promise>::new(move |milliseconds: f64| {
        let context = context.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            if !milliseconds.is_finite() || !(0.0..=5_000.0).contains(&milliseconds) {
                return Err(JsValue::from_str("invalid screenshot deadline"));
            }
            let deadline = monotonic_ms().unwrap_or(f64::INFINITY) + milliseconds;
            let result = capture(context, Arc::new(AtomicBool::new(true)), 0.0, deadline)
                .await
                .map_err(|error| JsValue::from_str(&error))?;
            let info = serde_json::to_string(&result.info)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            Ok(js_sys::Array::of2(
                &JsValue::from_str(&info),
                &js_sys::Uint8Array::from(result.png.as_slice()),
            )
            .into())
        })
    });
    install_window_control(callback.as_ref());
    callback.forget();
}

struct Handler {
    active: Arc<AtomicBool>,
    clock_offset_ms: Mutex<f64>,
    context: Context,
}

#[wasm_bindgen(module = "/screenshot.js")]
extern "C" {
    #[wasm_bindgen(js_name = beginScreenshot, catch)]
    fn begin_screenshot() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = cancelScreenshot)]
    fn cancel_screenshot(ticket: &JsValue);
    #[wasm_bindgen(js_name = finishScreenshot, catch)]
    fn finish_screenshot(
        ticket: &JsValue,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Result<js_sys::Promise, JsValue>;
}

struct CompositionCapture(JsValue);

impl Drop for CompositionCapture {
    fn drop(&mut self) {
        cancel_screenshot(&self.0);
    }
}

fn monotonic_ms() -> Option<f64> {
    Some(web_sys::window()?.performance()?.now())
}

impl BrowserControl for Handler {
    fn capture(
        &self,
        expires_at_ms: u64,
        window: Option<String>,
    ) -> impl Future<Output = Result<Result<garmin_service_api::control::Capture, String>, rtc::CallError>>
    {
        let context = self.context.clone();
        let active = Arc::clone(&self.active);
        let offset = *self
            .clock_offset_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // The RPC future stays Send; DOM/WASM futures run only on the browser executor.
        let (task, result) =
            capture_selected(context, active, offset, expires_at_ms, window).remote_handle();
        wasm_bindgen_futures::spawn_local(task);
        async move { Ok(result.await) }
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Monotonic milliseconds fit exactly in f64 for practical session lifetimes"
    )]
    fn execute(
        &self,
        request: ControlDispatch,
    ) -> impl Future<Output = Result<Result<String, String>, rtc::CallError>> {
        let server_now = monotonic_ms().unwrap_or(f64::INFINITY)
            + *self
                .clock_offset_ms
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
        let result = if !self.active.load(Ordering::Acquire) {
            Err("control connection is inactive".into())
        } else if server_now >= request.expires_at_ms as f64 {
            Err("command expired before browser dispatch".into())
        } else if request.command_json.len() > garmin_service_api::control::MAX_COMMAND_BYTES {
            Err("command exceeds size limit".into())
        } else {
            execute(&request.command_json).map_err(|error| crate::js_reason(&error))
        };
        std::future::ready(Ok(result))
    }
}

pub(super) struct Registration {
    abort: AbortHandle,
    handler: Arc<Handler>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.handler.active.store(false, Ordering::Release);
        self.abort.abort();
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Monotonic milliseconds fit exactly in f64 for practical session lifetimes"
)]
pub(super) async fn register(
    client: &ApplicationServiceClient,
    context: &Context,
) -> Option<Registration> {
    context.plugin_opt::<garmin_ui::automation::Driver>()?;
    let key = Id::new("automation-browser-session");
    let browser = if let Some(browser) = context.data(|data| data.get_temp::<String>(key)) {
        browser
    } else {
        let mut bytes = [0_u8; 16];
        web_sys::window()?
            .crypto()
            .ok()?
            .get_random_values_with_u8_array(&mut bytes)
            .ok()?;
        let mut browser = String::with_capacity(32);
        for byte in bytes {
            let _ = write!(browser, "{byte:02x}");
        }
        context.data_mut(|data| data.insert_temp(key, browser.clone()));
        browser
    };
    let handler = Arc::new(Handler {
        active: Arc::new(AtomicBool::new(false)),
        clock_offset_ms: Mutex::new(0.0),
        context: context.clone(),
    });
    let (server, reverse_client) =
        BrowserControlServerShared::<_, codec::Default>::new(handler.clone());
    let (abort, aborted) = AbortHandle::new_pair();
    let registration = Registration { abort, handler };
    wasm_bindgen_futures::spawn_local(async move {
        let _ = Abortable::new(server.serve(), aborted).await;
    });
    let started = monotonic_ms()?;
    let request = client.register_control(browser, reverse_client);
    let timeout = crate::wait_milliseconds(5_000);
    futures_util::pin_mut!(request, timeout);
    let reply = match futures_util::future::select(request, timeout).await {
        futures_util::future::Either::Left((reply, _)) => reply,
        futures_util::future::Either::Right(_) => {
            tracing::warn!("Control registration timed out");
            return None;
        }
    };
    let info = match reply {
        Ok(Ok(Some(info))) => info,
        Ok(Ok(None)) => return None,
        Ok(Err(error)) => {
            tracing::warn!(%error, "Control registration failed");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "Control registration failed");
            return None;
        }
    };
    // Registration request latency biases the clock estimate forward, so
    // transport latency cannot extend a command's dispatch deadline.
    *registration
        .handler
        .clock_offset_ms
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = info.server_time_ms as f64 - started;
    registration.handler.active.store(true, Ordering::Release);
    let endpoint = web_sys::window()
        .and_then(|window| {
            web_sys::Url::new_with_base("api/control", &window.location().href().ok()?).ok()
        })
        .map_or_else(|| "api/control".into(), |url| url.href());
    let _ = write!(
        garmin_ui::developer::state(context)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .debug,
        "\nControl: {endpoint} (localhost only)"
    );
    context.request_repaint();
    Some(registration)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Monotonic milliseconds fit exactly in f64 for practical session lifetimes"
)]
async fn capture_selected(
    context: Context,
    active: Arc<AtomicBool>,
    offset: f64,
    deadline: u64,
    window: Option<String>,
) -> Result<garmin_service_api::control::Capture, String> {
    let deadline = deadline as f64;
    let Some(window) = window.filter(|window| window != "root") else {
        return capture(context, active, offset, deadline).await;
    };
    validate_capture(&active, offset, deadline)?;
    let remaining =
        (deadline - monotonic_ms().unwrap_or(f64::INFINITY) - offset).clamp(0.0, 5_000.0);
    let promise = capture_window(&window, remaining).map_err(|error| crate::js_reason(&error))?;
    let value = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|error| crate::js_reason(&error))?;
    validate_capture(&active, offset, deadline)?;
    let values = value
        .dyn_into::<js_sys::Array>()
        .map_err(|_| "invalid window screenshot reply")?;
    let info = values
        .get(0)
        .as_string()
        .ok_or("missing window screenshot metadata")?;
    let png = values
        .get(1)
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(|_| "missing window screenshot pixels")?;
    if png.length() as usize > garmin_service_api::control::MAX_CAPTURE_BYTES {
        return Err("screenshot exceeds PNG byte limit".into());
    }
    let capture = garmin_service_api::control::Capture {
        info: serde_json::from_str(&info).map_err(|error| error.to_string())?,
        png: png.to_vec(),
    };
    capture.validate()?;
    Ok(capture)
}

fn validate_capture(active: &AtomicBool, offset: f64, deadline: f64) -> Result<(), String> {
    if !active.load(Ordering::Acquire) {
        return Err("control connection is inactive".into());
    }
    if monotonic_ms().unwrap_or(f64::INFINITY) + offset >= deadline {
        return Err("screenshot deadline expired".into());
    }
    Ok(())
}

async fn capture(
    context: Context,
    active: Arc<AtomicBool>,
    offset: f64,
    deadline: f64,
) -> Result<garmin_service_api::control::Capture, String> {
    let valid = || validate_capture(&active, offset, deadline);
    valid()?;
    let composition =
        CompositionCapture(begin_screenshot().map_err(|error| crate::js_reason(&error))?);
    let ticket = garmin_ui::capture::request(&context)?;
    let pixels = loop {
        valid()?;
        if let Some(result) = ticket.take() {
            break result?;
        }
        context.request_repaint();
        crate::wait_milliseconds(16).await;
    };
    let promise = finish_screenshot(
        &composition.0,
        &pixels.rgba(),
        pixels.info.width,
        pixels.info.height,
    )
    .map_err(|error| crate::js_reason(&error))?;
    let timeout = crate::wait_milliseconds(3_000);
    let complete = wasm_bindgen_futures::JsFuture::from(promise);
    futures_util::pin_mut!(complete, timeout);
    let value = match futures_util::future::select(complete, timeout).await {
        futures_util::future::Either::Left((result, _)) => {
            result.map_err(|error| crate::js_reason(&error))?
        }
        futures_util::future::Either::Right(_) => {
            return Err("screenshot composition timed out".into());
        }
    };
    valid()?;
    let png = js_sys::Uint8Array::new(&value);
    if png.length() as usize > garmin_service_api::control::MAX_CAPTURE_BYTES {
        return Err("screenshot exceeds PNG byte limit".into());
    }
    let capture = garmin_service_api::control::Capture {
        info: pixels.info,
        png: png.to_vec(),
    };
    capture.validate()?;
    Ok(capture)
}
