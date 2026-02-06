//! RGB Lib WebAssembly bindings
//!
//! This crate provides WebAssembly bindings for RGB Lib, allowing it to be used
//! in browsers and Node.js environments.
//!
//! ## WASM Considerations
//!
//! ### No access to the file system
//!
//! With no direct access to the file system, persistence cannot be handled by
//! RGB Lib directly. Instead, an in-memory wallet must be used in the WASM
//! environment, and the data must be exported using wallet methods. The persisted
//! data can be passed to wallet methods to recover the wallet state.
//!
//! ### Network access is limited to http(s)
//!
//! This essentially means the library only supports Esplora as blockchain client.
//! Both RPC and Electrum clients require sockets and will not work for RGB Lib
//! in a WASM environment out of the box.

use std::panic;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    #[wasm_bindgen(js_name = error)]
    fn console_error(s: &str);
}

/// Initialize WASM module. Call once after loading (e.g. after default()).
/// Sets a panic hook so Rust panics are printed to the browser console (instead of just "unreachable").
#[wasm_bindgen(start)]
pub fn init() {
    panic::set_hook(Box::new(move |info| {
        let msg = format!("[rgb-lib WASM panic] {}", info);
        console_error(&msg);
    }));
}

// Re-export main types and functions
mod wallet;
mod keys;
mod utils;
mod error;

pub use wallet::*;
pub use keys::*;
pub use utils::*;
pub use error::*;
