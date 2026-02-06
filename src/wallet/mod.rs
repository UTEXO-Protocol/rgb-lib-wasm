//! RGB wallet
//!
//! This module defines the [`Wallet`] related modules.

pub(crate) mod backup;
#[cfg(target_arch = "wasm32")]
pub(crate) mod memory_store;
pub(crate) mod offline;
#[cfg(any(feature = "electrum", feature = "esplora", feature = "esplora-wasm"))]
pub(crate) mod online;
pub mod rust_only;

#[cfg(test)]
pub(crate) mod test;

pub use offline::*;
#[cfg(any(feature = "electrum", feature = "esplora", feature = "esplora-wasm"))]
pub use online::*;

use super::*;
