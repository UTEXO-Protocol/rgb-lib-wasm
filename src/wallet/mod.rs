//! RGB wallet
//!
//! This module defines the [`Wallet`] related modules.

pub(crate) mod backup;
pub mod idb_store;
pub(crate) mod memory_store;
pub(crate) mod offline;
#[cfg(feature = "esplora")]
pub(crate) mod online;
pub mod rust_only;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_bfa;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_burn;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_link;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_mint;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_send;
#[cfg(all(test, target_arch = "wasm32"))]
mod test_vanilla;
pub mod vss;

pub use offline::*;
#[cfg(feature = "esplora")]
pub use online::*;

use super::*;
