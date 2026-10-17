use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/local.js")]
extern "C" {
    fn value() -> String;
}

#[wasm_bindgen]
pub fn imported_value() -> String {
    value()
}
