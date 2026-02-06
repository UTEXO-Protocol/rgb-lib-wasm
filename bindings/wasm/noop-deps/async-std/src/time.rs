//! Instant polyfill for wasm32: use `instant` crate instead of `std::time::Instant`
//! (std::time::Instant panics on wasm32 with "time not implemented on this platform").

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
pub use instant::Instant;
