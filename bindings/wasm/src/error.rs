use wasm_bindgen::prelude::*;
use rgb_lib::Error as RgbLibErrorInner;

#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct RgbLibError {
    message: String,
}

#[wasm_bindgen]
impl RgbLibError {
    #[wasm_bindgen(constructor)]
    pub fn new(message: String) -> RgbLibError {
        RgbLibError { message }
    }

    #[wasm_bindgen(getter)]
    pub fn message(&self) -> String {
        self.message.clone()
    }
}

impl From<RgbLibErrorInner> for RgbLibError {
    fn from(err: RgbLibErrorInner) -> Self {
        RgbLibError {
            message: err.to_string(),
        }
    }
}
