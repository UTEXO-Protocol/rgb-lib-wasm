//! WASM-compatible no-op replacement for home crate
//! Returns None for home directory in WASM environment.
//! Always provides home_dir() so dependents (e.g. sqlx-postgres) compile without enabling "std" feature.

use std::path::PathBuf;

/// Get the home directory (returns None for WASM / no-op)
pub fn home_dir() -> Option<PathBuf> {
    None
}

/// For compatibility with home crate API (called from env.rs)
pub(crate) fn home_dir_inner() -> Option<PathBuf> {
    None
}
