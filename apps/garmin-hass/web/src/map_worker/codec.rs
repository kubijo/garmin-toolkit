use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/worker-codec.js")]
extern "C" {
    #[wasm_bindgen(js_name = initMessage, catch)]
    pub fn init_message(version: u8, module: &str, wasm: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = tileTask, catch)]
    pub fn tile_task(
        version: u8,
        id: u32,
        zoom: u8,
        x: u32,
        y: u32,
        dark: bool,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = dataTask, catch)]
    pub fn data_task(
        version: u8,
        id: u32,
        kind: &str,
        bytes: &js_sys::ArrayBuffer,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub fn decode(value: JsValue) -> Result<Message, JsValue>;

    pub type Message;
    #[wasm_bindgen(method, getter)]
    pub fn version(this: &Message) -> u8;
    #[wasm_bindgen(method, getter, js_name = type)]
    pub fn message_type(this: &Message) -> String;
    #[wasm_bindgen(method, getter)]
    pub fn id(this: &Message) -> u32;
    #[wasm_bindgen(method, getter)]
    pub fn kind(this: &Message) -> String;
    #[wasm_bindgen(method, getter)]
    pub fn reason(this: &Message) -> String;
    #[wasm_bindgen(method, getter)]
    pub fn buffers(this: &Message) -> js_sys::Array;
}
