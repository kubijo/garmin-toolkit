//! Central browser User Timing bridge for map work visible in Chrome traces.

use wasm_bindgen::{JsCast, JsValue};

pub(super) fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

/// A stable mark name with JSON detail keeps the browser's retained timeline bounded.
pub(super) fn mark_detail(name: &str, detail: &str) {
    let Some(performance) = web_sys::window().and_then(|window| window.performance()) else {
        return;
    };
    let options = js_sys::Object::new();
    if js_sys::Reflect::set(
        &options,
        &JsValue::from_str("detail"),
        &JsValue::from_str(detail),
    )
    .is_err()
    {
        return;
    }
    let Ok(mark) = js_sys::Reflect::get(performance.as_ref(), &JsValue::from_str("mark"))
        .and_then(JsCast::dyn_into::<js_sys::Function>)
    else {
        return;
    };
    performance.clear_marks_with_mark_name(name);
    let _ignored = mark.apply(
        performance.as_ref(),
        &js_sys::Array::of2(&JsValue::from_str(name), &options),
    );
}

pub(super) fn measure_duration(name: &str, milliseconds: f64) {
    let Some(performance) = web_sys::window().and_then(|window| window.performance()) else {
        return;
    };
    let options = js_sys::Object::new();
    let start = (performance.now() - milliseconds).max(0.0);
    if js_sys::Reflect::set(
        &options,
        &JsValue::from_str("start"),
        &JsValue::from_f64(start),
    )
    .is_err()
        || js_sys::Reflect::set(
            &options,
            &JsValue::from_str("duration"),
            &JsValue::from_f64(milliseconds),
        )
        .is_err()
    {
        return;
    }
    let Ok(measure) = js_sys::Reflect::get(performance.as_ref(), &JsValue::from_str("measure"))
        .and_then(JsCast::dyn_into::<js_sys::Function>)
    else {
        return;
    };
    performance.clear_measures_with_measure_name(name);
    let arguments = js_sys::Array::of2(&JsValue::from_str(name), &options);
    let _ignored = measure.apply(performance.as_ref(), &arguments);
}
